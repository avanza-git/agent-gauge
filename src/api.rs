//! Plan utilisation from Claude Code's own `/api/oauth/usage` endpoint.
//!
//! Retry policy (kept tight because status bars may poll the CLI often):
//! - 401 → credentials expired, no retry.
//! - 429 → only wait out an explicit positive `Retry-After` (capped at 5 s);
//!   otherwise report a calm "rate limited" state. The endpoint's quota is
//!   budget-based, so hammering it just keeps us throttled.
//! - 5xx / network / bad JSON → up to 2 retries, 0.2 s → 0.4 s plus jitter.

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use serde_json::Value;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const MAX_RETRIES: u32 = 2;
const BASE_DELAY: f64 = 0.2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RateLimits {
    pub session_utilization: f64,
    pub session_reset: i64,
    pub weekly_utilization: f64,
    pub weekly_reset: i64,
    pub scoped_utilization: f64,
    pub scoped_reset: i64,
    pub scoped_label: String,
    pub overage_status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    NoCredentials,
    CredentialsExpired,
    RateLimited,
    Other(String),
}

impl FetchError {
    pub fn message(&self) -> String {
        match self {
            FetchError::NoCredentials => crate::credentials::no_credentials_message().into(),
            FetchError::CredentialsExpired => {
                "Credentials expired -- re-authenticate with 'claude'".into()
            }
            FetchError::RateLimited => "Rate limited -- using last known values".into(),
            FetchError::Other(m) => m.clone(),
        }
    }
}

fn pct_to_frac(v: Option<&Value>) -> f64 {
    let f = match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    } / 100.0;
    if f.is_finite() {
        f.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// ISO-8601 → unix seconds; naive timestamps are treated as local time.
pub fn iso_to_epoch(v: Option<&Value>) -> i64 {
    let Some(s) = v
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return 0;
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return dt.timestamp();
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt)
            && let Some(dt) = Local.from_local_datetime(&naive).earliest()
        {
            return dt.timestamp();
        }
    }
    0
}

/// Translate an `/api/oauth/usage` JSON payload. Pure, for testing.
pub fn parse_usage_payload(payload: &Value) -> Result<RateLimits, FetchError> {
    let Some(obj) = payload.as_object() else {
        return Err(FetchError::Other(
            "Unexpected /api/oauth/usage payload".into(),
        ));
    };
    let five = obj.get("five_hour").filter(|v| v.is_object());
    let seven = obj.get("seven_day").filter(|v| v.is_object());
    let extra = obj.get("extra_usage").filter(|v| v.is_object());

    // Model-scoped weekly cap (e.g. Fable): the highest-utilised
    // `weekly_scoped` entry in `limits`, labelled by the model display name.
    let (mut scoped_util, mut scoped_reset, mut scoped_label) = (0.0, 0, String::new());
    if let Some(limits) = obj.get("limits").and_then(Value::as_array) {
        let mut best = -1.0;
        for lim in limits {
            if lim.get("kind").and_then(Value::as_str) != Some("weekly_scoped") {
                continue;
            }
            let label = lim
                .pointer("/scope/model/display_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if label.is_empty() {
                continue;
            }
            let frac = pct_to_frac(lim.get("percent"));
            if frac > best {
                best = frac;
                scoped_util = frac;
                scoped_reset = iso_to_epoch(lim.get("resets_at"));
                scoped_label = label.to_string();
            }
        }
    }

    let enabled = extra
        .and_then(|e| e.get("is_enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    Ok(RateLimits {
        session_utilization: pct_to_frac(five.and_then(|f| f.get("utilization"))),
        session_reset: iso_to_epoch(five.and_then(|f| f.get("resets_at"))),
        weekly_utilization: pct_to_frac(seven.and_then(|f| f.get("utilization"))),
        weekly_reset: iso_to_epoch(seven.and_then(|f| f.get("resets_at"))),
        scoped_utilization: scoped_util,
        scoped_reset,
        scoped_label,
        overage_status: if enabled { "allowed" } else { "rejected" }.into(),
    })
}

fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 100_000) as f64 / 1_000_000.0 // 0 – 0.1 s
}

fn parse_retry_after(v: Option<&str>) -> Option<f64> {
    let secs: f64 = v?.trim().parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then_some(secs)
}

/// Fetch with the retry policy described in the module docs.
pub fn fetch_oauth_usage(token: &str) -> Result<RateLimits, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();

    for attempt in 0..=MAX_RETRIES {
        let result = agent
            .get(USAGE_URL)
            .header("Authorization", &format!("Bearer {token}"))
            .header("anthropic-beta", "oauth-2025-04-20")
            .header("User-Agent", "agent-gauge")
            .call();

        let last = attempt >= MAX_RETRIES;
        match result {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                match status {
                    200..=299 => {
                        let body = resp.body_mut().with_config().limit(65_536).read_to_string();
                        match body
                            .ok()
                            .and_then(|b| serde_json::from_str::<Value>(&b).ok())
                        {
                            Some(v) => return parse_usage_payload(&v),
                            None if last => {
                                return Err(FetchError::Other("OAuth usage request failed".into()));
                            }
                            None => {}
                        }
                    }
                    401 => return Err(FetchError::CredentialsExpired),
                    429 => {
                        let ra = parse_retry_after(
                            resp.headers()
                                .get("retry-after")
                                .and_then(|h| h.to_str().ok()),
                        );
                        match ra {
                            Some(secs) if secs > 0.0 && !last => {
                                std::thread::sleep(Duration::from_secs_f64(secs.min(5.0)));
                                continue;
                            }
                            _ => return Err(FetchError::RateLimited),
                        }
                    }
                    500..=599 if !last => {}
                    _ => return Err(FetchError::Other(format!("OAuth usage error {status}"))),
                }
            }
            Err(_) if last => {
                return Err(FetchError::Other(
                    "OAuth usage request failed -- check network".into(),
                ));
            }
            Err(_) => {}
        }
        let delay = BASE_DELAY * f64::from(1u32 << attempt) + jitter();
        std::thread::sleep(Duration::from_secs_f64(delay));
    }
    Err(FetchError::Other("OAuth usage request failed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_full_payload() {
        let p = json!({
            "five_hour": {"utilization": 42.5, "resets_at": "2026-11-13T15:00:00+00:00"},
            "seven_day": {"utilization": 18, "resets_at": "2026-11-17T00:00:00Z"},
            "extra_usage": {"is_enabled": true},
            "limits": [
                {"kind": "weekly_scoped", "percent": 30,
                 "resets_at": "2026-11-17T00:00:00Z",
                 "scope": {"model": {"display_name": "Sonnet"}}},
                {"kind": "weekly_scoped", "percent": 55,
                 "resets_at": "2026-11-18T00:00:00Z",
                 "scope": {"model": {"display_name": "Fable"}}},
                {"kind": "weekly_scoped", "percent": 99, "scope": {}},
                {"kind": "five_hour", "percent": 99}
            ]
        });
        let r = parse_usage_payload(&p).unwrap();
        assert!((r.session_utilization - 0.425).abs() < 1e-9);
        assert_eq!(r.session_reset, 1_794_582_000);
        assert!((r.weekly_utilization - 0.18).abs() < 1e-9);
        assert_eq!(r.scoped_label, "Fable");
        assert!((r.scoped_utilization - 0.55).abs() < 1e-9);
        assert_eq!(r.overage_status, "allowed");
    }

    #[test]
    fn tolerates_nulls_and_garbage() {
        let p = json!({"five_hour": null, "seven_day": {"utilization": "nan"}, "limits": "x"});
        let r = parse_usage_payload(&p).unwrap();
        assert_eq!(r.session_utilization, 0.0);
        assert_eq!(r.weekly_utilization, 0.0);
        assert_eq!(r.scoped_label, "");
        assert_eq!(r.overage_status, "rejected");
        assert!(parse_usage_payload(&json!([1, 2])).is_err());
    }

    #[test]
    fn clamps_percentages() {
        let p = json!({"five_hour": {"utilization": 250}, "seven_day": {"utilization": -5}});
        let r = parse_usage_payload(&p).unwrap();
        assert_eq!(r.session_utilization, 1.0);
        assert_eq!(r.weekly_utilization, 0.0);
    }

    #[test]
    fn retry_after_parsing() {
        assert_eq!(parse_retry_after(Some("3")), Some(3.0));
        assert_eq!(parse_retry_after(Some("-1")), None);
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(None), None);
    }
}
