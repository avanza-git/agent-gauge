//! Utilisation samples in `~/.claude/usage-history.jsonl` — the same file and
//! line format the Python widget writes, so the two can share history.
//!
//! Line: `{ts, session, weekly, session_reset?, weekly_reset?, scoped?, scoped_reset?, scoped_label?}`.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const FILENAME: &str = "usage-history.jsonl";
pub const KEEP_SECONDS: f64 = 90.0 * 86400.0;
const PRUNE_SLACK_SECONDS: f64 = 86400.0;

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    #[serde(default)]
    pub ts: f64,
    #[serde(default)]
    pub session: f64,
    #[serde(default)]
    pub weekly: f64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub session_reset: i64,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub weekly_reset: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_reset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped_label: Option<String>,
}

pub fn append(path: &Path, sample: &Sample) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(sample).map_err(std::io::Error::other)?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

/// All samples with `ts >= since_ts`; malformed lines are skipped.
pub fn load(path: &Path, since_ts: f64) -> Vec<Sample> {
    let Ok(f) = fs::File::open(path) else {
        return Vec::new();
    };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Sample>(l.trim()).ok())
        .filter(|s| s.ts >= since_ts)
        .collect()
}

/// True when the oldest sample is more than a day past the keep cutoff —
/// batches the full-file rewrite to roughly once a day.
pub fn prune_due(path: &Path, now: f64) -> bool {
    let Ok(f) = fs::File::open(path) else {
        return false;
    };
    let first = BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .find(|l| !l.trim().is_empty());
    match first.and_then(|l| serde_json::from_str::<Sample>(&l).ok()) {
        Some(s) => s.ts < now - KEEP_SECONDS - PRUNE_SLACK_SECONDS,
        None => true,
    }
}

pub fn prune(path: &Path, now: f64) -> std::io::Result<()> {
    let keep = load(path, now - KEEP_SECONDS);
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        for s in &keep {
            let mut line = serde_json::to_string(s).map_err(std::io::Error::other)?;
            line.push('\n');
            f.write_all(line.as_bytes())?;
        }
    }
    fs::rename(tmp, path)
}

/// The last-known utilisation to show when the API can't be reached.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastKnown {
    pub session: f64,
    pub session_reset: i64,
    pub weekly: f64,
    pub weekly_reset: i64,
    pub scoped: f64,
    pub scoped_reset: i64,
    pub scoped_label: String,
}

/// Restore from the newest samples, searching each reset key independently
/// (a sample may carry one without the other) and clamping windows whose
/// reset time has already passed back to zero.
pub fn last_known(samples: &[Sample], now: f64) -> Option<LastKnown> {
    let last = samples.last()?;
    let mut lk = LastKnown {
        session: last.session,
        weekly: last.weekly,
        ..Default::default()
    };
    for prev in samples.iter().rev() {
        if lk.session_reset == 0 && prev.session_reset != 0 {
            lk.session_reset = prev.session_reset;
        }
        if lk.weekly_reset == 0 && prev.weekly_reset != 0 {
            lk.weekly_reset = prev.weekly_reset;
        }
        if lk.scoped_label.is_empty()
            && let Some(label) = prev.scoped_label.as_deref().filter(|l| !l.is_empty())
        {
            lk.scoped_label = label.to_string();
            lk.scoped = prev.scoped.unwrap_or(0.0);
            lk.scoped_reset = prev.scoped_reset.unwrap_or(0);
        }
        if lk.session_reset != 0 && lk.weekly_reset != 0 && !lk.scoped_label.is_empty() {
            break;
        }
    }
    if lk.session_reset != 0 && now >= lk.session_reset as f64 {
        lk.session = 0.0;
        lk.session_reset = 0;
    }
    if lk.weekly_reset != 0 && now >= lk.weekly_reset as f64 {
        lk.weekly = 0.0;
        lk.weekly_reset = 0;
    }
    if lk.scoped_reset != 0 && now >= lk.scoped_reset as f64 {
        lk.scoped = 0.0;
        lk.scoped_reset = 0;
        lk.scoped_label.clear();
    }
    Some(lk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(ts: f64, session: f64, sr: i64, wr: i64) -> Sample {
        Sample {
            ts,
            session,
            weekly: 0.3,
            session_reset: sr,
            weekly_reset: wr,
            ..Default::default()
        }
    }

    #[test]
    fn append_load_roundtrip_and_python_compat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILENAME);
        append(&path, &s(100.0, 0.5, 200, 0)).unwrap();
        // A Python-written line with an int ts and no optional keys.
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"ts\": 150, \"session\": 0.6, \"weekly\": 0.4}\nnot json\n")
            .unwrap();
        let all = load(&path, 0.0);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].ts, 150.0);
        assert_eq!(load(&path, 120.0).len(), 1);
        let line = fs::read_to_string(&path).unwrap();
        assert!(!line.lines().next().unwrap().contains("weekly_reset"));
    }

    #[test]
    fn last_known_searches_keys_independently_and_clamps() {
        let samples = vec![
            s(1.0, 0.1, 0, 5000),
            s(2.0, 0.2, 3000, 0),
            s(3.0, 0.9, 0, 0),
        ];
        let lk = last_known(&samples, 1000.0).unwrap();
        assert_eq!(lk.session, 0.9);
        assert_eq!(lk.session_reset, 3000);
        assert_eq!(lk.weekly_reset, 5000);
        let lk = last_known(&samples, 4000.0).unwrap();
        assert_eq!((lk.session, lk.session_reset), (0.0, 0));
        assert_eq!(lk.weekly_reset, 5000);
        assert!(last_known(&[], 0.0).is_none());
    }

    #[test]
    fn prune_drops_old_samples() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILENAME);
        let now = 200.0 * 86400.0;
        append(&path, &s(1.0, 0.1, 0, 0)).unwrap();
        append(&path, &s(now - 10.0, 0.2, 0, 0)).unwrap();
        assert!(prune_due(&path, now));
        prune(&path, now).unwrap();
        assert_eq!(load(&path, 0.0).len(), 1);
        assert!(!prune_due(&path, now));
    }
}
