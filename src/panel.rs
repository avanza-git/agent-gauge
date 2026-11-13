//! Cached, credential-free snapshots consumed by the KDE Plasma widget.
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{
    config::{self, Config},
    lock::InstanceLock,
    stats::UsageStats,
    worker::{self, WorkerMsg},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub utilization: f64,
    pub reset: i64,
    /// Unix seconds when the limit is projected to run out before `reset`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exhausts_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub error: String,
    pub session: Option<Window>,
    pub weekly: Option<Window>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub updated_at: i64,
    pub stale_after: u64,
    pub stale: bool,
    pub thresholds: Vec<f64>,
    pub providers: Vec<Provider>,
}

impl Snapshot {
    pub fn new(stats: &UsageStats, cfg: &Config, interval: u64, previous: Option<&Self>) -> Self {
        let window = |known: bool, utilization: f64, reset: i64, id: &str| {
            (known && utilization.is_finite()).then(|| Window {
                utilization,
                reset,
                exhausts_at: stats
                    .forecasts
                    .get(id)
                    .filter(|f| f.exhausts())
                    .map(|f| f.exhausts_at),
            })
        };
        let claude = cfg.claude_enabled();
        let codex = cfg.codex_enabled();
        let known = claude && stats.rate_limit_error.is_empty();
        let mut result = Self {
            version: 1,
            updated_at: stats.collected_at,
            stale_after: interval.saturating_add(90),
            stale: false,
            thresholds: cfg
                .notify_thresholds
                .iter()
                .copied()
                .filter(|t| t.is_finite() && *t > 0.0 && *t <= 1.0)
                .collect(),
            providers: vec![
                Provider {
                    id: "claude".into(),
                    label: "Claude".into(),
                    enabled: claude,
                    error: if claude {
                        stats.rate_limit_error.clone()
                    } else {
                        String::new()
                    },
                    session: window(
                        claude && (known || stats.session_reset > 0),
                        stats.session_utilization,
                        stats.session_reset,
                        "session",
                    ),
                    weekly: window(
                        claude && (known || stats.weekly_reset > 0),
                        stats.weekly_utilization,
                        stats.weekly_reset,
                        "weekly",
                    ),
                },
                Provider {
                    id: "codex".into(),
                    label: "Codex".into(),
                    enabled: codex,
                    error: if codex {
                        stats.codex_error.clone()
                    } else {
                        String::new()
                    },
                    session: window(
                        codex && stats.codex_available && stats.codex_has_session,
                        stats.codex_session_utilization,
                        stats.codex_session_reset,
                        "codex_session",
                    ),
                    weekly: window(
                        codex && stats.codex_available && stats.codex_has_weekly,
                        stats.codex_weekly_utilization,
                        stats.codex_weekly_reset,
                        "codex_weekly",
                    ),
                },
            ],
        };
        // Keep the last known reading on provider failures, but never carry it past a reset.
        for provider in &mut result.providers {
            if provider.enabled
                && !provider.error.is_empty()
                && let Some(old) =
                    previous.and_then(|s| s.providers.iter().find(|p| p.id == provider.id))
            {
                for (current, cached) in [
                    (&mut provider.session, &old.session),
                    (&mut provider.weekly, &old.weekly),
                ] {
                    if current.is_none() {
                        *current =
                            cached
                                .clone()
                                .filter(|w| w.reset > stats.collected_at)
                                .map(|mut w| {
                                    // A carried-over projection is only shown until it comes due.
                                    w.exhausts_at =
                                        w.exhausts_at.filter(|t| *t > stats.collected_at);
                                    w
                                });
                    }
                }
            }
        }
        result
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || self.updated_at <= 0
            || self.stale_after == 0
            || self.providers.len() != 2
            || self.providers[0].id != "claude"
            || self.providers[1].id != "codex"
            || self.providers.iter().any(|p| {
                [&p.session, &p.weekly].iter().any(|w| {
                    w.as_ref()
                        .is_some_and(|w| !w.utilization.is_finite() || w.utilization < 0.0)
                })
            })
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid panel snapshot",
            ));
        }
        Ok(())
    }
}

pub fn snapshot_path() -> PathBuf {
    config::cache_dir().join("panel-snapshot.json")
}

/// Replace, never truncate, the visible file so concurrent readers see complete JSON.
pub fn write_snapshot(path: &Path, snapshot: &Snapshot) -> io::Result<()> {
    snapshot.validate()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        serde_json::to_writer(&mut file, snapshot)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn read_snapshot(path: &Path, now: i64) -> io::Result<Snapshot> {
    let mut snapshot: Snapshot = serde_json::from_slice(&fs::read(path)?)?;
    snapshot.validate()?;
    snapshot.stale = now.saturating_sub(snapshot.updated_at) > snapshot.stale_after as i64;
    Ok(snapshot)
}

/// This path only reads the snapshot: no credentials, collector, or network access.
pub fn print_snapshot() -> i32 {
    match read_snapshot(&snapshot_path(), chrono::Utc::now().timestamp()) {
        Ok(snapshot) => match serde_json::to_writer(io::stdout().lock(), &snapshot) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("agent-gauge: {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("agent-gauge: panel snapshot unavailable: {e}");
            1
        }
    }
}

pub fn run(config_path: PathBuf) -> i32 {
    let _lock = match InstanceLock::acquire() {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            eprintln!("agent-gauge is already running; stop it before starting panel mode.");
            return 1;
        }
        Err(e) => {
            eprintln!("agent-gauge: cannot acquire instance lock: {e}");
            return 1;
        }
    };
    let mut cfg = Config::load(&config_path);
    let stamp = || fs::metadata(&config_path).and_then(|m| m.modified()).ok();
    let mut modified = stamp();
    let path = snapshot_path();
    let mut previous = read_snapshot(&path, chrono::Utc::now().timestamp()).ok();
    let (tx, rx) = mpsc::channel();
    let wake = worker::spawn(cfg.clone(), move |stats, interval| {
        let _ = tx.send((stats, interval));
    });
    loop {
        match rx.recv_timeout(Duration::from_secs(1)) {
            Ok((stats, interval)) => {
                let snapshot = Snapshot::new(&stats, &cfg, interval, previous.as_ref());
                if let Err(e) = write_snapshot(&path, &snapshot) {
                    eprintln!("agent-gauge: cannot publish panel snapshot: {e}");
                }
                previous = Some(snapshot);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!("agent-gauge: collection worker stopped");
                return 1;
            }
        }
        let current = stamp();
        if current != modified {
            modified = current;
            cfg = Config::load(&config_path);
            let _ = wake.send(WorkerMsg::Reconfigure(Box::new(cfg.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (UsageStats, Config) {
        let cfg = Config {
            providers: vec!["claude".into(), "codex".into()],
            ..Config::default()
        };
        let stats = UsageStats {
            collected_at: 1000,
            session_utilization: 0.42,
            session_reset: 2000,
            codex_available: true,
            codex_has_session: true,
            codex_session_utilization: 0.25,
            codex_session_reset: 2000,
            ..UsageStats::default()
        };
        (stats, cfg)
    }

    #[test]
    fn atomic_snapshot_round_trip_and_staleness() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel.json");
        let (stats, cfg) = sample();
        let snapshot = Snapshot::new(&stats, &cfg, 60, None);
        write_snapshot(&path, &snapshot).unwrap();
        let read = read_snapshot(&path, 1100).unwrap();
        assert!(!read.stale);
        assert_eq!(
            read.providers[0].session.as_ref().unwrap().utilization,
            0.42
        );
        assert!(read_snapshot(&path, 1151).unwrap().stale);
        write_snapshot(&path, &Snapshot::new(&stats, &cfg, 300, None)).unwrap();
        assert!(!read_snapshot(&path, 1300).unwrap().stale);
    }

    #[test]
    fn missing_malformed_and_incompatible_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("panel.json");
        assert!(read_snapshot(&path, 1000).is_err());
        fs::write(&path, "{").unwrap();
        assert!(read_snapshot(&path, 1000).is_err());
        let (stats, cfg) = sample();
        let mut snapshot = Snapshot::new(&stats, &cfg, 60, None);
        snapshot.version = 2;
        assert!(write_snapshot(&path, &snapshot).is_err());
    }

    #[test]
    fn unknown_disabled_and_failed_providers() {
        let (mut stats, mut cfg) = sample();
        let old = Snapshot::new(&stats, &cfg, 60, None);
        stats.codex_available = false;
        stats.codex_error = "offline".into();
        stats.rate_limit_error = "No credentials".into();
        stats.session_reset = 0;
        let failed = Snapshot::new(&stats, &cfg, 60, Some(&old));
        assert_eq!(
            failed.providers[0].session.as_ref().unwrap().utilization,
            0.42
        );
        assert!(failed.providers[1].session.is_some());
        assert!(
            Snapshot::new(&stats, &cfg, 60, None).providers[0]
                .session
                .is_none()
        );
        stats.collected_at = 2001;
        assert!(
            Snapshot::new(&stats, &cfg, 60, Some(&old)).providers[1]
                .session
                .is_none()
        );
        cfg.providers.clear();
        let disabled = Snapshot::new(&stats, &cfg, 60, Some(&old));
        assert!(
            disabled
                .providers
                .iter()
                .all(|p| !p.enabled && p.session.is_none() && p.error.is_empty())
        );
    }

    #[test]
    fn forecast_in_snapshot_and_old_json_parses() {
        let (mut stats, cfg) = sample();
        stats.forecasts.insert(
            "session".into(),
            crate::forecast::Forecast {
                rate_per_hour: 0.6,
                exhausts_at: 1500,
            },
        );
        stats.forecasts.insert(
            "codex_session".into(),
            crate::forecast::Forecast {
                rate_per_hour: 0.01,
                exhausts_at: 0,
            },
        );
        let snap = Snapshot::new(&stats, &cfg, 60, None);
        assert_eq!(
            snap.providers[0].session.as_ref().unwrap().exhausts_at,
            Some(1500)
        );
        assert_eq!(
            snap.providers[1].session.as_ref().unwrap().exhausts_at,
            None
        );
        let json = serde_json::to_string(&snap).unwrap();
        assert_eq!(json.matches("exhausts_at").count(), 1);

        // Snapshots written before this field existed still parse.
        let w: Window = serde_json::from_str(r#"{"utilization":0.5,"reset":10}"#).unwrap();
        assert_eq!(w.exhausts_at, None);

        // Carried over on failure only until the projected time passes.
        stats.rate_limit_error = "offline".into();
        stats.forecasts.clear();
        stats.session_reset = 0;
        stats.collected_at = 1400;
        let carried = Snapshot::new(&stats, &cfg, 60, Some(&snap));
        assert_eq!(
            carried.providers[0].session.as_ref().unwrap().exhausts_at,
            Some(1500)
        );
        stats.collected_at = 1600;
        let carried = Snapshot::new(&stats, &cfg, 60, Some(&snap));
        assert_eq!(
            carried.providers[0].session.as_ref().unwrap().exhausts_at,
            None
        );
    }
}
