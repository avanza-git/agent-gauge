//! Optional zero-cost data source: a file your Claude Code `statusLine`
//! script dumps, shaped like
//! `{"captured_at": "...", "rate_limits": {"five_hour": {"used_percentage": 54,
//! "resets_at": 1783795800}, "seven_day": {...}}}`.

use std::fs;
use std::path::Path;

use chrono::DateTime;
use serde_json::Value;

/// Default freshness limit for the rate-limit fallback.
pub const MAX_AGE_SECONDS: f64 = 20.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StatuslineLimits {
    pub session: (f64, i64),
    pub weekly: (f64, i64),
}

fn window(block: Option<&Value>, now_ts: f64) -> Option<(f64, i64)> {
    let block = block?.as_object()?;
    let used = block.get("used_percentage")?.as_f64()?;
    let pct = (used / 100.0).clamp(0.0, 1.0);
    let reset = block
        .get("resets_at")
        .and_then(Value::as_f64)
        .map(|r| r as i64)
        .unwrap_or(0);
    if reset > 0 && now_ts >= reset as f64 {
        return Some((0.0, 0)); // window rolled over since capture
    }
    Some((pct, reset))
}

pub fn parse(text: &str, now_ts: f64, max_age_seconds: f64) -> Option<StatuslineLimits> {
    let data: Value = serde_json::from_str(text).ok()?;
    let captured = DateTime::parse_from_rfc3339(data.get("captured_at")?.as_str()?.trim()).ok()?;
    let age = now_ts - captured.timestamp() as f64;
    if age > max_age_seconds || age < -300.0 {
        return None;
    }
    let limits = data.get("rate_limits")?;
    Some(StatuslineLimits {
        session: window(limits.get("five_hour"), now_ts)?,
        weekly: window(limits.get("seven_day"), now_ts)?,
    })
}

pub fn load(path: &Path, now_ts: f64, max_age_seconds: f64) -> Option<StatuslineLimits> {
    parse(&fs::read_to_string(path).ok()?, now_ts, max_age_seconds)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: f64 = 1_783_795_000.0; // 2026-07-11T18:36:40Z

    fn sample(captured: &str) -> String {
        format!(
            r#"{{"captured_at":"{captured}","rate_limits":{{
                "five_hour":{{"used_percentage":54,"resets_at":1783795800}},
                "seven_day":{{"used_percentage":46,"resets_at":1783790000}}}}}}"#
        )
    }

    #[test]
    fn fresh_dump_parses_and_clamps_expired_window() {
        let l = parse(&sample("2026-07-11T18:35:00Z"), NOW, MAX_AGE_SECONDS).unwrap();
        assert_eq!(l.session, (0.54, 1_783_795_800));
        assert_eq!(l.weekly, (0.0, 0)); // reset already passed
    }

    #[test]
    fn stale_or_future_dump_rejected() {
        assert!(parse(&sample("2026-07-11T18:00:00Z"), NOW, MAX_AGE_SECONDS).is_none());
        assert!(parse(&sample("2026-07-11T19:00:00Z"), NOW, MAX_AGE_SECONDS).is_none());
        assert!(parse("garbage", NOW, MAX_AGE_SECONDS).is_none());
    }
}
