//! One collection pass: local transcripts + plan utilisation → [`UsageStats`].

use std::collections::VecDeque;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Duration, Local, Utc};

use crate::api::{self, FetchError, RateLimits};
use crate::config::Config;
use crate::history::{self, Sample};
use crate::scanner::{self, Scanner};
use crate::stats::{LiveActivity, UsageStats};
use crate::{codex, credentials, forecast, pricing, sessions, statusline_cache};

const TOP_PROJECTS: usize = 10;
/// History window loaded for forecasts (the longest lookback).
const FORECAST_HISTORY_SECONDS: f64 = 24.0 * 3600.0;

/// Holds state worth keeping between refreshes (the incremental transcript cache).
#[derive(Debug, Default)]
pub struct Collector {
    scanner: Scanner,
    /// In-memory Codex readings for forecasts (Codex has no on-disk history).
    codex_samples: VecDeque<CodexSample>,
}

#[derive(Debug, Clone, Copy)]
struct CodexSample {
    ts: f64,
    session: f64,
    session_reset: i64,
    weekly: f64,
    weekly_reset: i64,
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn mtime_ts(p: &Path) -> f64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn fetch_rate_limits(claude_dir: &Path) -> Result<RateLimits, FetchError> {
    let token = credentials::load_token(claude_dir).ok_or(FetchError::NoCredentials)?;
    api::fetch_oauth_usage(&token)
}

impl Collector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collect(&mut self, cfg: &Config) -> UsageStats {
        self.collect_with(cfg, fetch_rate_limits)
    }

    /// `fetch` is injectable so tests never touch the network.
    pub fn collect_with<F>(&mut self, cfg: &Config, fetch: F) -> UsageStats
    where
        F: FnOnce(&Path) -> Result<RateLimits, FetchError>,
    {
        let claude_dir = cfg.claude_dir();
        let samples_path = claude_dir.join(history::FILENAME);
        let now = now_ts();
        let mut stats = UsageStats {
            collected_at: now as i64,
            subscription_type: credentials::load_subscription_type(&claude_dir),
            ..Default::default()
        };

        if !cfg.claude_enabled() {
            self.collect_codex(cfg, &mut stats);
            return stats;
        }
        stats.claude_enabled = true;

        // --- local message counts (history.jsonl, local calendar)
        let mc = sessions::message_counts(&claude_dir, Local::now());
        stats.today_messages = mc.today_messages;
        stats.today_sessions = mc.today_sessions;
        stats.week_messages = mc.week_messages;
        stats.week_sessions = mc.week_sessions;

        // --- tokens + cost (transcript timestamps are UTC)
        let utc_now = Utc::now();
        let today = utc_now.format("%Y-%m-%d").to_string();
        let week: Vec<String> = (0..7)
            .map(|i| (utc_now - Duration::days(i)).format("%Y-%m-%d").to_string())
            .collect();
        let t = self
            .scanner
            .scan(&claude_dir, &today, &week, SystemTime::now());
        stats.today_tokens = t.today_output;
        stats.week_tokens = t.week_output;
        stats.today_model_tokens = t.today_by_model;
        stats.active_subagent_count = t.active_subagents;
        stats.live_activity = LiveActivity {
            is_live: t.live_last_activity_ts > 0.0
                && t.live_last_activity_ts >= now - scanner::LIVE_ACTIVE_SECONDS,
            tokens_per_minute: t.live_output_tokens as f64 / (scanner::LIVE_WINDOW_SECONDS / 60.0),
            output_tokens_last_window: t.live_output_tokens,
            last_activity_ts: t.live_last_activity_ts,
        };
        let mut projects: Vec<(String, u64)> = t.today_by_project.into_iter().collect();
        projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        projects.truncate(TOP_PROJECTS);
        stats.today_by_project = projects;

        let (today_cost, today_per_model) =
            pricing::calculate_stats_cost(&t.today_by_model_detailed);
        let (week_cost, _) = pricing::calculate_stats_cost(&t.week_by_model_detailed);
        stats.today_cost = today_cost.total;
        stats.today_cost_by_model = today_per_model;
        stats.week_cost = week_cost.total;
        stats.cache_savings = week_cost.cache_savings;
        stats.today_by_model_detailed = t.today_by_model_detailed;

        stats.active_sessions = sessions::active_sessions(&claude_dir);

        self.collect_codex(cfg, &mut stats);

        // --- plan utilisation
        let sl_path = cfg.statusline_cache_path();
        // Endpoint relief: a seconds-fresh statusline dump has the same
        // session/weekly numbers; only force the endpoint every
        // `usage_endpoint_min_seconds` (history mtime = last successful fetch).
        let sl_live = sl_path
            .as_deref()
            .and_then(|p| statusline_cache::load(p, now, 2.0 * cfg.refresh_seconds.max(1) as f64));
        let endpoint_due =
            now - mtime_ts(&samples_path) >= cfg.usage_endpoint_min_seconds.max(1) as f64;

        match sl_live {
            Some(sl) if !endpoint_due => {
                (stats.session_utilization, stats.session_reset) = sl.session;
                (stats.weekly_utilization, stats.weekly_reset) = sl.weekly;
                stats.from_statusline_cache = true;
                // The statusline payload has no scoped cap: carry the last one forward.
                if let Some(lk) = history::last_known(&history::load(&samples_path, 0.0), now) {
                    stats.scoped_label = lk.scoped_label;
                    stats.scoped_utilization = lk.scoped;
                    stats.scoped_reset = lk.scoped_reset;
                }
            }
            _ => match fetch(&claude_dir) {
                Ok(rl) => {
                    stats.session_utilization = rl.session_utilization;
                    stats.session_reset = rl.session_reset;
                    stats.weekly_utilization = rl.weekly_utilization;
                    stats.weekly_reset = rl.weekly_reset;
                    stats.scoped_utilization = rl.scoped_utilization;
                    stats.scoped_reset = rl.scoped_reset;
                    stats.scoped_label = rl.scoped_label;
                    stats.overage_status = rl.overage_status;
                    record_sample(&samples_path, now, &stats);
                }
                Err(e) => {
                    stats.rate_limit_error = e.message();
                    stats.rate_limited = e == FetchError::RateLimited;
                    stats.credentials_expired = e == FetchError::CredentialsExpired;
                    if let Some(lk) = history::last_known(&history::load(&samples_path, 0.0), now) {
                        stats.session_utilization = lk.session;
                        stats.session_reset = lk.session_reset;
                        stats.weekly_utilization = lk.weekly;
                        stats.weekly_reset = lk.weekly_reset;
                        stats.scoped_utilization = lk.scoped;
                        stats.scoped_reset = lk.scoped_reset;
                        stats.scoped_label = lk.scoped_label;
                    }
                    // A fresher zero-cost source beats the last on-disk sample.
                    if let Some(sl) = sl_path.as_deref().and_then(|p| {
                        statusline_cache::load(p, now, statusline_cache::MAX_AGE_SECONDS)
                    }) {
                        (stats.session_utilization, stats.session_reset) = sl.session;
                        (stats.weekly_utilization, stats.weekly_reset) = sl.weekly;
                        stats.from_statusline_cache = true;
                    }
                }
            },
        }
        if cfg.forecast_enabled {
            claude_forecasts(&samples_path, now, &mut stats);
        }
        stats
    }
}

/// Project each Claude window. Every reading is anchored at the time it was
/// actually taken: a value carried forward from the history file is paired
/// with that sample's timestamp, not "now", so it can't flatten the rate.
fn claude_forecasts(samples_path: &Path, now: f64, stats: &mut UsageStats) {
    let samples = history::load(samples_path, now - FORECAST_HISTORY_SECONDS);
    // The newest sample (optionally one with a scoped cap), if recent enough to trust.
    let last_ts = |scoped: bool| {
        samples
            .iter()
            .rev()
            .find(|s| !scoped || s.scoped_label.as_deref().is_some_and(|l| !l.is_empty()))
            .map(|s| s.ts)
            .filter(|ts| now - ts <= statusline_cache::MAX_AGE_SECONDS)
    };
    let fresh_fetch = stats.rate_limit_error.is_empty() && !stats.from_statusline_cache;
    // Session/weekly come from the endpoint or a statusline dump (both "now"),
    // or else from the last on-disk sample.
    let main_at = if stats.rate_limit_error.is_empty() || stats.from_statusline_cache {
        Some(now)
    } else {
        last_ts(false)
    };
    // The statusline has no scoped cap, so unless the endpoint just answered
    // it was carried forward from history.
    let scoped_at = if fresh_fetch {
        Some(now)
    } else {
        last_ts(true)
    };

    let mut add = |id: &str,
                   at: Option<f64>,
                   util: f64,
                   reset: i64,
                   params: &forecast::Params,
                   pick: fn(&Sample) -> Option<(f64, i64)>| {
        let Some(at) = at else { return };
        let pts = forecast::window_points(&samples, pick, reset);
        if let Some(f) = forecast::project(&pts, at, util, reset, params) {
            stats.forecasts.insert(id.to_string(), f);
        }
    };
    let (su, sr) = (stats.session_utilization, stats.session_reset);
    let (wu, wr) = (stats.weekly_utilization, stats.weekly_reset);
    add("session", main_at, su, sr, &forecast::SESSION, |s| {
        Some((s.session, s.session_reset))
    });
    add("weekly", main_at, wu, wr, &forecast::WEEKLY, |s| {
        Some((s.weekly, s.weekly_reset))
    });
    if !stats.scoped_label.is_empty() {
        let (cu, cr) = (stats.scoped_utilization, stats.scoped_reset);
        add("scoped", scoped_at, cu, cr, &forecast::WEEKLY, |s| {
            Some((s.scoped?, s.scoped_reset?))
        });
    }
}

impl Collector {
    fn collect_codex(&mut self, cfg: &Config, stats: &mut UsageStats) {
        if !cfg.codex_enabled() {
            return;
        }
        let cx = codex::collect(cfg);
        stats.codex_available = cx.available;
        stats.codex_error = cx.error.clone();
        stats.codex_has_session = cx.limits.has_session;
        stats.codex_session_utilization = cx.limits.session_pct;
        stats.codex_session_reset = cx.limits.session_reset;
        stats.codex_has_weekly = cx.limits.has_weekly;
        stats.codex_weekly_utilization = cx.limits.weekly_pct;
        stats.codex_weekly_reset = cx.limits.weekly_reset;
        if cfg.forecast_enabled && cx.available {
            self.codex_forecasts(&cx, stats);
        }
    }

    fn codex_forecasts(&mut self, cx: &codex::CodexStatus, stats: &mut UsageStats) {
        let l = &cx.limits;
        let ts = cx.fetched_at;
        // A cached reading is the same point again: only record new ones.
        if ts > 0.0 && self.codex_samples.back().is_none_or(|b| ts > b.ts) {
            self.codex_samples.push_back(CodexSample {
                ts,
                session: l.session_pct,
                session_reset: l.session_reset,
                weekly: l.weekly_pct,
                weekly_reset: l.weekly_reset,
            });
        }
        while self
            .codex_samples
            .front()
            .is_some_and(|f| f.ts < ts - FORECAST_HISTORY_SECONDS)
        {
            self.codex_samples.pop_front();
        }
        // Project from the reading's own time so a cached value isn't "now".
        let older: Vec<&CodexSample> = self.codex_samples.iter().filter(|c| c.ts < ts).collect();
        let pts = |pick: fn(&CodexSample) -> (f64, i64), reset: i64| -> Vec<(f64, f64)> {
            older
                .iter()
                .filter(|c| forecast::same_window(pick(c).1, reset))
                .map(|c| (c.ts, pick(c).0))
                .collect()
        };
        if l.has_session
            && let Some(f) = forecast::project(
                &pts(|c| (c.session, c.session_reset), l.session_reset),
                ts,
                l.session_pct,
                l.session_reset,
                &forecast::SESSION,
            )
        {
            stats.forecasts.insert("codex_session".into(), f);
        }
        if l.has_weekly
            && let Some(f) = forecast::project(
                &pts(|c| (c.weekly, c.weekly_reset), l.weekly_reset),
                ts,
                l.weekly_pct,
                l.weekly_reset,
                &forecast::WEEKLY,
            )
        {
            stats.forecasts.insert("codex_weekly".into(), f);
        }
    }
}

fn record_sample(path: &Path, now: f64, s: &UsageStats) {
    let has_scoped = !s.scoped_label.is_empty();
    let sample = Sample {
        ts: now,
        session: s.session_utilization,
        weekly: s.weekly_utilization,
        session_reset: s.session_reset,
        weekly_reset: s.weekly_reset,
        scoped: has_scoped.then_some(s.scoped_utilization),
        scoped_reset: has_scoped.then_some(s.scoped_reset),
        scoped_label: has_scoped.then(|| s.scoped_label.clone()),
    };
    if history::append(path, &sample).is_ok() && history::prune_due(path, now) {
        let _ = history::prune(path, now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg_for(dir: &Path) -> Config {
        Config {
            claude_dir: dir.to_string_lossy().into_owned(),
            ..Default::default()
        }
    }

    #[test]
    fn success_records_sample() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Collector::new();
        let s = c.collect_with(&cfg_for(dir.path()), |_| {
            Ok(RateLimits {
                session_utilization: 0.4,
                session_reset: 4_000_000_000,
                weekly_utilization: 0.2,
                scoped_label: "Fable".into(),
                scoped_utilization: 0.5,
                ..Default::default()
            })
        });
        assert_eq!(s.session_utilization, 0.4);
        assert!(s.rate_limit_error.is_empty());
        let samples = history::load(&dir.path().join(history::FILENAME), 0.0);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].scoped_label.as_deref(), Some("Fable"));
    }

    #[test]
    fn claude_disabled_skips_api() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            providers: vec![],
            ..cfg_for(dir.path())
        };
        let s = Collector::new().collect_with(&cfg, |_| panic!("API must not be called"));
        assert!(!s.claude_enabled);
        assert!(!s.codex_available);
    }

    #[test]
    fn failure_falls_back_to_last_known() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(history::FILENAME);
        history::append(
            &path,
            &Sample {
                ts: 1.0,
                session: 0.7,
                weekly: 0.3,
                session_reset: 4_000_000_000,
                ..Default::default()
            },
        )
        .unwrap();
        let mut c = Collector::new();
        let s = c.collect_with(&cfg_for(dir.path()), |_| Err(FetchError::RateLimited));
        assert!(s.rate_limited);
        assert!(!s.credentials_expired);
        assert_eq!(s.session_utilization, 0.7);
        assert_eq!(s.session_reset, 4_000_000_000);

        let s = c.collect_with(&cfg_for(dir.path()), |_| {
            Err(FetchError::CredentialsExpired)
        });
        assert!(s.credentials_expired);
        assert!(
            s.forecasts.is_empty(),
            "the only sample is far too old to anchor on"
        );
    }

    #[test]
    fn rising_history_produces_forecast() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(history::FILENAME);
        let now = now_ts();
        let reset = now as i64 + 4 * 3600;
        // +10% every 10 min in the current window; one point from an old window.
        for (ago, session, sr) in [
            (3000.0, 0.95, reset - 5 * 3600),
            (1200.0, 0.30, reset),
            (600.0, 0.40, reset),
        ] {
            history::append(
                &path,
                &Sample {
                    ts: now - ago,
                    session,
                    weekly: 0.2,
                    session_reset: sr,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let ok = |_: &Path| {
            Ok(RateLimits {
                session_utilization: 0.50,
                session_reset: reset,
                weekly_utilization: 0.2,
                ..Default::default()
            })
        };
        // History mtime is fresh, but no statusline → the endpoint is fetched.
        let s = Collector::new().collect_with(&cfg_for(dir.path()), ok);
        let f = s.forecasts["session"];
        assert!(f.exhausts(), "{f:?}");
        assert!((f.rate_per_hour - 0.6).abs() < 0.05, "{f:?}");
        assert!(!s.forecasts.contains_key("weekly"), "weekly span too short");

        let cfg = Config {
            forecast_enabled: false,
            ..cfg_for(dir.path())
        };
        assert!(Collector::new().collect_with(&cfg, ok).forecasts.is_empty());

        // Rate-limited: projects from the last sample at *its* timestamp, so the
        // gap since then doesn't dilute the rate.
        let s =
            Collector::new().collect_with(&cfg_for(dir.path()), |_| Err(FetchError::RateLimited));
        assert!(s.rate_limited);
        let f = s.forecasts["session"];
        assert!((f.rate_per_hour - 0.6).abs() < 0.05, "{f:?}");
        assert!(f.exhausts(), "{f:?}");
    }
}
