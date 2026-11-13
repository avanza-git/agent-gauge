//! Optional second provider: OpenAI Codex rate limits.
//!
//! Talks to the local `codex` CLI's `app-server` over stdio JSON-RPC:
//! `initialize` → `initialized` → `account/rateLimits/read`. The result has up
//! to two windows (`primary`, `secondary`) with `usedPercent`, `resetsAt` and
//! `windowDurationMins`; windows are classified by duration because plans
//! without a 5 h limit send their weekly window as `primary`.
//!
//! Spawning the app-server takes a couple of seconds, so results are cached in
//! `~/.cache/agent-gauge/codex_limits.json` (compatible format with the Python
//! widget) and refreshed at most every `codex_poll_seconds`. On RPC failure the
//! cache is served, with expired windows clamped to zero.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::config::{Config, cache_dir, expand_tilde};

const RPC_TIMEOUT: Duration = Duration::from_secs(12);
/// Codex windows are 5 h (300 min) and weekly (10080 min); a day splits them.
const SESSION_WINDOW_MAX_MINS: f64 = 24.0 * 60.0;
const BIN_CANDIDATES: &[&str] = &[
    "/usr/lib/chatgpt/resources/codex", // ChatGPT desktop (Linux) bundles it here
    "/opt/homebrew/bin/codex",
    "/usr/local/bin/codex",
    "~/.local/bin/codex", // macOS apps and LaunchAgents start with a bare PATH
    "/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexLimits {
    pub has_session: bool,
    pub session_pct: f64,
    pub session_reset: i64,
    pub has_weekly: bool,
    pub weekly_pct: f64,
    pub weekly_reset: i64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexStatus {
    pub available: bool,
    pub limits: CodexLimits,
    pub error: String,
    /// Unix seconds when `limits` were read from Codex (cache time if cached).
    pub fetched_at: f64,
}

fn cache_path() -> PathBuf {
    cache_dir().join("codex_limits.json")
}

fn now_ts() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// `codex_bin` from config → `PATH` → well-known install locations → (macOS)
/// the CLI bundled in the ChatGPT desktop app, wherever it is installed.
pub fn find_codex_bin(cfg: &Config) -> Option<PathBuf> {
    let configured = cfg.codex_bin.trim();
    if !configured.is_empty() {
        let p = expand_tilde(configured);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { "codex.exe" } else { "codex" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    BIN_CANDIDATES
        .iter()
        .map(|p| expand_tilde(p))
        .find(|p| p.is_file())
        .or_else(macos_app_codex_bin)
}

/// The CLI the ChatGPT/Codex desktop app (bundle id `com.openai.codex`) ships in
/// `Contents/Resources/codex-cli/bin/codex`. Launch Services knows where the app
/// is, so this works for `/Applications`, `~/Applications` or another volume.
#[cfg(target_os = "macos")]
fn macos_app_codex_bin() -> Option<PathBuf> {
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::NSString;

    const BUNDLE_IDS: &[&str] = &["com.openai.codex", "com.openai.chat"];
    let workspace = NSWorkspace::sharedWorkspace(); // thread-safe
    BUNDLE_IDS.iter().find_map(|id| {
        let app = workspace
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(id))?
            .to_file_path()?;
        let bin = app.join("Contents/Resources/codex-cli/bin/codex");
        bin.is_file().then_some(bin)
    })
}

#[cfg(not(target_os = "macos"))]
fn macos_app_codex_bin() -> Option<PathBuf> {
    None
}

/// Extract the two windows from an `account/rateLimits/read` result.
pub fn parse_rate_limits(payload: &Value) -> Option<CodexLimits> {
    let limits = payload.get("rateLimits")?.as_object()?;
    let window = |block: &Value| -> Option<(f64, i64)> {
        let pct = block.get("usedPercent")?.as_f64()?;
        let mut reset = block.get("resetsAt").and_then(Value::as_f64).unwrap_or(0.0) as i64;
        if reset > 1_000_000_000_000 {
            reset /= 1000; // milliseconds → seconds
        }
        Some(((pct / 100.0).clamp(0.0, 1.0), reset))
    };
    let mut out = CodexLimits::default();
    for slot in ["primary", "secondary"] {
        let Some(block) = limits.get(slot).filter(|b| b.is_object()) else {
            continue;
        };
        let Some((pct, reset)) = window(block) else {
            continue;
        };
        let is_session = match block.get("windowDurationMins").and_then(Value::as_f64) {
            Some(mins) => mins <= SESSION_WINDOW_MAX_MINS,
            None => slot == "primary", // older app-servers had fixed slots
        };
        if is_session {
            (out.has_session, out.session_pct, out.session_reset) = (true, pct, reset);
        } else {
            (out.has_weekly, out.weekly_pct, out.weekly_reset) = (true, pct, reset);
        }
    }
    (out.has_session || out.has_weekly).then_some(out)
}

/// A window whose reset has passed has rolled over — show 0, not a stale %.
fn clamp_expired(mut l: CodexLimits, now: f64) -> CodexLimits {
    if l.session_reset != 0 && now >= l.session_reset as f64 {
        (l.session_pct, l.session_reset) = (0.0, 0);
    }
    if l.weekly_reset != 0 && now >= l.weekly_reset as f64 {
        (l.weekly_pct, l.weekly_reset) = (0.0, 0);
    }
    l
}

/// One `account/rateLimits/read` round-trip, hard-bounded by [`RPC_TIMEOUT`].
/// Lines are read on a helper thread so a stalled app-server can never hang
/// the caller; the child is killed on every exit path.
fn rate_limits_rpc(bin: &Path) -> Option<Value> {
    let mut child = Command::new(bin)
        .arg("app-server")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut send = |v: Value| -> bool {
        let mut line = v.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .is_ok()
    };

    let deadline = Instant::now() + RPC_TIMEOUT;
    let mut result = None;
    if send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": {"name": "agent-gauge", "title": "AgentGauge Widget", "version": env!("CARGO_PKG_VERSION")}}
    })) {
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            let Ok(line) = rx.recv_timeout(remaining) else {
                break;
            };
            let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else {
                continue;
            };
            match msg.get("id").and_then(Value::as_i64) {
                Some(1) => {
                    send(json!({"jsonrpc": "2.0", "method": "initialized"}));
                    // The app-server drops a request sent right after the handshake.
                    std::thread::sleep(Duration::from_millis(600));
                    send(
                        json!({"jsonrpc": "2.0", "id": 2, "method": "account/rateLimits/read", "params": {}}),
                    );
                }
                Some(2) => {
                    result = msg.get("result").cloned();
                    break;
                }
                _ => {}
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn load_cache() -> Option<(f64, Value)> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()?;
    let fetched = v.get("fetched_at").and_then(Value::as_f64).unwrap_or(0.0);
    Some((fetched, v.get("payload")?.clone()))
}

fn save_cache(payload: &Value) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    let body = json!({"fetched_at": now_ts(), "payload": payload}).to_string();
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(tmp, path);
    }
}

/// Codex utilisation for the overlay; never panics.
pub fn collect(cfg: &Config) -> CodexStatus {
    let unavailable = |e: &str| CodexStatus {
        error: e.into(),
        ..Default::default()
    };
    let Some(bin) = find_codex_bin(cfg) else {
        return unavailable("codex binary not found (set codex_bin in config)");
    };
    let now = now_ts();
    let cache = load_cache();

    if let Some((fetched, payload)) = &cache {
        let age = now - fetched;
        if (0.0..cfg.codex_poll_seconds.max(30) as f64).contains(&age)
            && let Some(l) = parse_rate_limits(payload)
        {
            return CodexStatus {
                available: true,
                limits: clamp_expired(l, now),
                error: String::new(),
                fetched_at: *fetched,
            };
        }
    }

    if let Some(payload) = rate_limits_rpc(&bin)
        && let Some(l) = parse_rate_limits(&payload)
    {
        save_cache(&payload);
        return CodexStatus {
            available: true,
            limits: clamp_expired(l, now),
            error: String::new(),
            fetched_at: now,
        };
    }

    // RPC failed — any cache, however old, beats nothing.
    if let Some((fetched, l)) = cache
        .as_ref()
        .and_then(|(f, p)| parse_rate_limits(p).map(|l| (*f, l)))
    {
        return CodexStatus {
            available: true,
            limits: clamp_expired(l, now),
            error: "rpc failed; serving cache".into(),
            fetched_at: fetched,
        };
    }
    unavailable("rateLimits/read returned no window data (try `codex login`)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_by_duration() {
        let p = json!({"rateLimits": {
            "primary": {"usedPercent": 4, "windowDurationMins": 300, "resetsAt": 1794555643},
            "secondary": {"usedPercent": 67, "windowDurationMins": 10080, "resetsAt": 1794808656000i64}
        }});
        let l = parse_rate_limits(&p).unwrap();
        assert!(l.has_session && l.has_weekly);
        assert!((l.session_pct - 0.04).abs() < 1e-9);
        assert_eq!(l.weekly_reset, 1_794_808_656); // ms normalised
    }

    #[test]
    fn weekly_only_plan_in_primary_slot() {
        let p =
            json!({"rateLimits": {"primary": {"usedPercent": 10, "windowDurationMins": 10080}}});
        let l = parse_rate_limits(&p).unwrap();
        assert!(!l.has_session && l.has_weekly);
        assert!(parse_rate_limits(&json!({"rateLimits": {}})).is_none());
        assert!(parse_rate_limits(&json!(null)).is_none());
    }

    #[test]
    fn expired_windows_clamped() {
        let l = CodexLimits {
            has_session: true,
            session_pct: 0.5,
            session_reset: 100,
            ..Default::default()
        };
        let c = clamp_expired(l, 200.0);
        assert_eq!((c.session_pct, c.session_reset), (0.0, 0));
    }
}
