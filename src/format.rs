//! Small text helpers shared by the OSD, menu and details window.

use chrono::{Local, TimeZone};

/// Compact reset label: `2h 31m` (< 24 h), `Mon 16:00` (>= 24 h), `soon`, or "".
pub fn reset_short(reset_ts: i64, now: i64) -> String {
    if reset_ts <= 0 {
        return String::new();
    }
    let remaining = reset_ts - now;
    if remaining <= 0 {
        return "soon".into();
    }
    let (hours, minutes) = (remaining / 3600, (remaining % 3600) / 60);
    if hours < 24 {
        return if hours > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{minutes}m")
        };
    }
    Local
        .timestamp_opt(reset_ts, 0)
        .single()
        .map(|d| d.format("%a %H:%M").to_string())
        .unwrap_or_default()
}

/// `Resets in 3 hr 28 min` / `Resets Mon 04:00 PM` / "".
pub fn reset_long(reset_ts: i64, now: i64) -> String {
    if reset_ts <= 0 {
        return String::new();
    }
    let remaining = reset_ts - now;
    if remaining <= 0 {
        return "Resets soon".into();
    }
    let (hours, minutes) = (remaining / 3600, (remaining % 3600) / 60);
    if hours >= 24 {
        return Local
            .timestamp_opt(reset_ts, 0)
            .single()
            .map(|d| d.format("Resets %a %I:%M %p").to_string())
            .unwrap_or_default();
    }
    if hours > 0 {
        format!("Resets in {hours} hr {minutes} min")
    } else {
        format!("Resets in {minutes} min")
    }
}

/// Compact duration until *ts*: `45m`, `1h20m`, `2d3h`; "" if not in the future.
pub fn until_compact(ts: i64, now: i64) -> String {
    let remaining = ts - now;
    if ts <= 0 || remaining <= 0 {
        return String::new();
    }
    let minutes = (remaining + 59) / 60;
    let (d, h, m) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h{m:02}m"),
        _ => format!("{d}d{h}h"),
    }
}

/// `1.2M`, `58.1k`, `950`.
pub fn tokens(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

pub fn pct(v: f64) -> String {
    format!("{}%", (v * 100.0).floor() as i64)
}

/// `-home-user-my-project` → `my-project` (best effort; the dir name is lossy).
pub fn project_name(dir: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().replace(['/', '.', '_'], "-"))
        .unwrap_or_default();
    let trimmed = dir.strip_prefix(&home).unwrap_or(dir);
    let trimmed = trimmed.trim_start_matches('-');
    if trimmed.is_empty() {
        dir.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn ago(secs: i64) -> String {
    match secs {
        s if s < 5 => "just now".into(),
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s => format!("{}h ago", s / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_labels() {
        assert_eq!(reset_short(0, 100), "");
        assert_eq!(reset_short(100, 200), "soon");
        assert_eq!(reset_short(100 + 2 * 3600 + 31 * 60, 100), "2h 31m");
        assert_eq!(reset_short(100 + 45 * 60, 100), "45m");
        assert_eq!(
            reset_long(100 + 3 * 3600 + 28 * 60, 100),
            "Resets in 3 hr 28 min"
        );
    }

    #[test]
    fn token_and_pct_format() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(58_091), "58.1k");
        assert_eq!(tokens(2_930_671), "2.9M");
        assert_eq!(pct(0.999), "99%");
    }
}
