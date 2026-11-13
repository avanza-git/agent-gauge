//! The snapshot produced by one collection pass. Field names match the
//! Python widget's JSON output so `--field NAME` scripts keep working.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::forecast::Forecast;
use crate::pricing::Cost;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenBucket {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    /// Subset of `cache_creation` written with the 1-hour TTL.
    pub cache_creation_1h: u64,
}

impl TokenBucket {
    pub fn add(&mut self, o: &TokenBucket) {
        self.input += o.input;
        self.output += o.output;
        self.cache_read += o.cache_read;
        self.cache_creation += o.cache_creation;
        self.cache_creation_1h += o.cache_creation_1h;
    }

    pub fn is_zero(&self) -> bool {
        *self == TokenBucket::default()
    }

    /// Prompt length as Anthropic counts it for prompt-length pricing.
    pub fn prompt_tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_creation
    }
}

/// A running Claude Code session (subset of `~/.claude/sessions/<pid>.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActiveSession {
    #[serde(default)]
    pub pid: i64,
    #[serde(default, rename = "sessionId")]
    pub session_id: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default, rename = "startedAt")]
    pub started_at: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub status: String,
}

/// "Is a Claude Code session writing right now?" — drives the OSD's
/// `● LIVE 5.3k tok/min` badge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct LiveActivity {
    pub is_live: bool,
    pub tokens_per_minute: f64,
    pub output_tokens_last_window: u64,
    /// Unix seconds; 0 = no recent activity.
    pub last_activity_ts: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageStats {
    // From ~/.claude/history.jsonl
    pub today_messages: u64,
    pub today_sessions: u64,
    pub week_messages: u64,
    pub week_sessions: u64,

    // From conversation transcripts (output tokens)
    pub today_tokens: u64,
    pub week_tokens: u64,
    pub today_model_tokens: BTreeMap<String, u64>,
    pub today_by_model_detailed: BTreeMap<String, TokenBucket>,
    /// Top 10 projects by today's output tokens, largest first.
    #[serde(serialize_with = "ordered_map")]
    pub today_by_project: Vec<(String, u64)>,

    // Plan utilisation (0.0 – 1.0) and reset epochs (unix seconds)
    pub session_utilization: f64,
    pub session_reset: i64,
    pub weekly_utilization: f64,
    pub weekly_reset: i64,
    pub scoped_utilization: f64,
    pub scoped_reset: i64,
    /// Empty = the API reported no model-scoped weekly cap.
    pub scoped_label: String,
    pub overage_status: String,
    pub rate_limit_error: String,
    pub rate_limited: bool,
    pub credentials_expired: bool,
    /// True when session/weekly came from the statusline dump instead of the API.
    pub from_statusline_cache: bool,

    pub subscription_type: String,

    // Cost (USD; pay-as-you-go equivalent for subscribers)
    pub today_cost: f64,
    pub week_cost: f64,
    /// $ saved this week via prompt caching.
    pub cache_savings: f64,
    pub today_cost_by_model: BTreeMap<String, Cost>,

    /// False when the "claude" agent is switched off (no API call, no scan).
    pub claude_enabled: bool,

    // Optional OpenAI Codex provider (only when "codex" is in `providers`).
    /// False hides all Codex rows.
    pub codex_available: bool,
    pub codex_has_session: bool,
    pub codex_session_utilization: f64,
    pub codex_session_reset: i64,
    pub codex_has_weekly: bool,
    pub codex_weekly_utilization: f64,
    pub codex_weekly_reset: i64,
    pub codex_error: String,

    pub active_sessions: Vec<ActiveSession>,
    pub active_subagent_count: u64,
    pub live_activity: LiveActivity,

    /// Usage-rate projections keyed by OSD row id ("session", "weekly",
    /// "scoped", "codex_session", "codex_weekly"). Missing = not enough data.
    pub forecasts: BTreeMap<String, Forecast>,

    /// Unix seconds when this snapshot was collected.
    pub collected_at: i64,
}

/// Serialize a `Vec<(K, V)>` as a JSON object, keeping the vec's order.
fn ordered_map<S: serde::Serializer>(v: &[(String, u64)], s: S) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut m = s.serialize_map(Some(v.len()))?;
    for (k, val) in v {
        m.serialize_entry(k, val)?;
    }
    m.end()
}

impl UsageStats {
    pub fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Subscribers pay a flat fee, so "cost" is really API-equivalent value.
    pub fn is_subscriber(&self) -> bool {
        matches!(
            self.subscription_type.as_str(),
            "max" | "pro" | "team" | "enterprise"
        )
    }
}
