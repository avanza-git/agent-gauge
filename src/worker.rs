//! Shared collection loop for desktop and headless panel modes.
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::collector::Collector;
use crate::config::Config;
use crate::format;
use crate::notifier::{self, CrossingDetector};
use crate::stats::UsageStats;

pub enum WorkerMsg {
    Refresh,
    Reconfigure(Box<Config>),
}

pub fn next_interval(cfg: &Config, previous: u64, rate_limited: bool) -> u64 {
    let base = cfg.refresh_seconds.max(10);
    let max = cfg.refresh_max_seconds.max(base);
    if rate_limited {
        previous.saturating_mul(2).clamp(base, max)
    } else {
        base
    }
}

/// Keep one collector alive so scans remain incremental. Dropping the sender stops it.
pub fn spawn(
    mut cfg: Config,
    mut publish: impl FnMut(UsageStats, u64) + Send + 'static,
) -> Sender<WorkerMsg> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("agent-gauge-refresh".into())
        .spawn(move || {
            let mut collector = Collector::new();
            let mut detector = CrossingDetector::new(&cfg.notify_thresholds);
            let mut forecast_seen = ForecastNotified::default();
            let mut interval = cfg.refresh_seconds.max(10);
            loop {
                let stats = collector.collect(&cfg);
                interval = next_interval(&cfg, interval, stats.rate_limited);
                if cfg.notifications_enabled {
                    let rows = notify_rows(&stats);
                    notify_crossings(&mut detector, &rows);
                    if cfg.forecast_enabled && cfg.forecast_notify {
                        notify_forecasts(&mut forecast_seen, &rows, &stats);
                    }
                }
                publish(stats, interval);
                match rx.recv_timeout(Duration::from_secs(interval)) {
                    Ok(WorkerMsg::Reconfigure(new_cfg)) => {
                        cfg = *new_cfg;
                        detector = CrossingDetector::new(&cfg.notify_thresholds);
                        forecast_seen = ForecastNotified::default();
                        interval = cfg.refresh_seconds.max(10);
                    }
                    Ok(WorkerMsg::Refresh) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .expect("spawn refresh thread");
    tx
}

/// One notifiable window: (crossing scope, forecast key, label, util, reset).
type NotifyRow = (&'static str, &'static str, String, f64, i64);

fn notify_rows(s: &UsageStats) -> Vec<NotifyRow> {
    let mut rows = vec![
        (
            "session",
            "session",
            "Claude Session".to_string(),
            s.session_utilization,
            s.session_reset,
        ),
        (
            "weekly",
            "weekly",
            "Claude Weekly".to_string(),
            s.weekly_utilization,
            s.weekly_reset,
        ),
    ];
    if !s.scoped_label.is_empty() {
        rows.push((
            "scoped",
            "scoped",
            format!("Claude {} weekly", s.scoped_label),
            s.scoped_utilization,
            s.scoped_reset,
        ));
    }
    if s.codex_available && s.codex_has_session {
        rows.push((
            "codex5h",
            "codex_session",
            "Codex 5h".to_string(),
            s.codex_session_utilization,
            s.codex_session_reset,
        ));
    }
    if s.codex_available && s.codex_has_weekly {
        rows.push((
            "codex7d",
            "codex_weekly",
            "Codex weekly".to_string(),
            s.codex_weekly_utilization,
            s.codex_weekly_reset,
        ));
    }
    rows
}

fn notify_crossings(detector: &mut CrossingDetector, rows: &[NotifyRow]) {
    let now = chrono::Utc::now().timestamp();
    for (scope, _, label, util, reset) in rows {
        if let Some(t) = detector.check(scope, *util).last() {
            let when = format::reset_long(*reset, now);
            notifier::send(
                format!("{label} usage at {}", format::pct(*t)),
                format!("Now {}. {when}", format::pct(*util)),
            );
        }
    }
}

/// Don't warn about projections that land only just before the reset.
const FORECAST_MIN_LEAD_SECONDS: i64 = 15 * 60;

/// Reset epoch each window was last warned about, so a warning fires once per window.
#[derive(Debug, Default)]
pub struct ForecastNotified(HashMap<&'static str, i64>);

impl ForecastNotified {
    /// True the first time *key*'s window (identified by *reset*) should warn.
    pub fn should_notify(&mut self, key: &'static str, exhausts_at: i64, reset: i64) -> bool {
        if exhausts_at <= 0 || reset - exhausts_at < FORECAST_MIN_LEAD_SECONDS {
            return false;
        }
        match self.0.get(key) {
            Some(r) if crate::forecast::same_window(*r, reset) => false,
            _ => {
                self.0.insert(key, reset);
                true
            }
        }
    }
}

fn notify_forecasts(seen: &mut ForecastNotified, rows: &[NotifyRow], s: &UsageStats) {
    let now = chrono::Utc::now().timestamp();
    for (_, key, label, _, reset) in rows {
        let Some(f) = s.forecasts.get(*key) else {
            continue;
        };
        if seen.should_notify(key, f.exhausts_at, *reset) {
            notifier::send(
                format!("{label} on pace to hit 100%"),
                format!(
                    "Full in ~{} at the current rate. {}",
                    format::until_compact(f.exhausts_at, now),
                    format::reset_long(*reset, now)
                ),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adaptive_interval_and_recovery() {
        let cfg = Config::default();
        assert_eq!(next_interval(&cfg, 60, true), 120);
        assert_eq!(next_interval(&cfg, 240, true), 300);
        assert_eq!(next_interval(&cfg, 300, false), 60);
        assert_eq!(next_interval(&cfg, u64::MAX, true), 300);
    }

    #[test]
    fn forecast_notifies_once_per_window() {
        let mut seen = ForecastNotified::default();
        let reset = 10_000;
        assert!(!seen.should_notify("session", 0, reset)); // not exhausting
        assert!(!seen.should_notify("session", reset - 60, reset)); // too close to reset
        assert!(seen.should_notify("session", 5_000, reset));
        assert!(!seen.should_notify("session", 4_000, reset + 30)); // same window, jitter
        assert!(seen.should_notify("weekly", 5_000, reset)); // independent keys
        assert!(seen.should_notify("session", 25_000, 30_000)); // next window
    }
}
