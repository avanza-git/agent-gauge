//! Configuration: built-in defaults merged with an optional user JSON file.
//!
//! The file lives at `$XDG_CONFIG_HOME/agent-gauge/config.json` (falling back
//! to `~/.config/agent-gauge/config.json`). Legacy settings are copied on startup.
//! Unknown keys are preserved verbatim on save.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Claude Code data directory. `~` is expanded on use.
    pub claude_dir: String,
    /// Base refresh interval for the GUI worker.
    pub refresh_seconds: u64,
    /// Upper bound for the adaptive back-off when the endpoint rate-limits.
    pub refresh_max_seconds: u64,
    /// Optional statusLine-dumped rate-limit file (zero-cost data source).
    pub statusline_cache_path: String,
    /// While the statusline dump is fresh, hit the endpoint at most this often.
    pub usage_endpoint_min_seconds: u64,

    pub osd_opacity: f32,
    pub osd_scale: f32,
    pub osd_view_mode: String,
    /// "top-left" | "top-right" | "bottom-left" | "bottom-right" | "custom"
    pub osd_position: String,
    pub osd_x: Option<f32>,
    pub osd_y: Option<f32>,
    pub osd_minimized: bool,
    pub osd_always_on_top: bool,
    /// Show the widget as a regular window in the taskbar / dock.
    pub show_in_taskbar: bool,
    /// Linux: run under XWayland when available so the OSD can position
    /// itself, stay on top and hide from the taskbar (native Wayland can't).
    pub force_x11: bool,
    pub theme: String,

    pub notifications_enabled: bool,
    pub notify_thresholds: Vec<f64>,
    /// Project when each limit runs out from the recent usage rate.
    pub forecast_enabled: bool,
    /// Notify once per window when a limit is on pace to run out before it resets.
    pub forecast_notify: bool,

    /// Agents to track: "claude" and/or "codex".
    pub providers: Vec<String>,
    /// OSD rows to hide: "session", "weekly", "scoped", "codex_session",
    /// "codex_weekly", "cost". New rows default to visible.
    pub osd_hidden_rows: Vec<String>,
    /// How often to spawn the `codex app-server` RPC (a disk cache is served in between).
    pub codex_poll_seconds: u64,
    /// Explicit path to the `codex` CLI; empty = search PATH and known locations.
    pub codex_bin: String,

    /// Unknown keys (e.g. Python-only features) — kept so saves don't drop them.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            claude_dir: "~/.claude".into(),
            refresh_seconds: 60,
            refresh_max_seconds: 300,
            statusline_cache_path: String::new(),
            usage_endpoint_min_seconds: 300,
            osd_opacity: 0.75,
            osd_scale: 1.0,
            osd_view_mode: "bars".into(),
            osd_position: "top-right".into(),
            osd_x: None,
            osd_y: None,
            osd_minimized: false,
            osd_always_on_top: true,
            show_in_taskbar: false,
            force_x11: true,
            theme: "default".into(),
            notifications_enabled: true,
            notify_thresholds: vec![0.75, 0.90],
            forecast_enabled: true,
            forecast_notify: true,
            providers: vec!["claude".into()],
            osd_hidden_rows: Vec::new(),
            codex_poll_seconds: 300,
            codex_bin: String::new(),
            extra: Map::new(),
        }
    }
}

/// Expand a leading `~` to the user's home directory.
pub fn expand_tilde(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(p)
}

impl Config {
    pub fn codex_enabled(&self) -> bool {
        self.providers.iter().any(|p| p == "codex")
    }

    pub fn claude_enabled(&self) -> bool {
        self.providers.iter().any(|p| p == "claude")
    }

    pub fn row_visible(&self, id: &str) -> bool {
        !self.osd_hidden_rows.iter().any(|r| r == id)
    }

    /// Add or remove *item* from a string list setting.
    pub fn set_in_list(list: &mut Vec<String>, item: &str, present: bool) {
        list.retain(|x| x != item);
        if present {
            list.push(item.to_string());
        }
    }

    pub fn claude_dir(&self) -> PathBuf {
        expand_tilde(&self.claude_dir)
    }

    pub fn statusline_cache_path(&self) -> Option<PathBuf> {
        let p = self.statusline_cache_path.trim();
        (!p.is_empty()).then(|| expand_tilde(p))
    }

    /// Load *path*, falling back to defaults for anything missing or invalid.
    pub fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(v @ Value::Object(_)) => Self::from_value(v),
            Ok(_) => {
                eprintln!(
                    "WARNING: config {} must be a JSON object; using defaults.",
                    path.display()
                );
                Self::default()
            }
            Err(e) => {
                eprintln!("WARNING: failed to load config {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Deserialize leniently: a key with the wrong type falls back to its
    /// default instead of discarding the whole file.
    fn from_value(v: Value) -> Self {
        if let Ok(cfg) = serde_json::from_value::<Config>(v.clone()) {
            return cfg;
        }
        let Value::Object(user) = v else {
            return Self::default();
        };
        let defaults = serde_json::to_value(Self::default()).expect("default config serializes");
        let Value::Object(mut merged) = defaults else {
            unreachable!()
        };
        for (k, val) in user {
            let mut candidate = merged.clone();
            candidate.insert(k.clone(), val.clone());
            if serde_json::from_value::<Config>(Value::Object(candidate)).is_ok() {
                merged.insert(k, val);
            } else {
                eprintln!("WARNING: ignoring invalid config value for {k:?}");
            }
        }
        serde_json::from_value(Value::Object(merged)).unwrap_or_default()
    }

    /// Atomically write the config (tmp file + rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            f.write_all(text.as_bytes())?;
            f.write_all(b"\n")?;
            f.sync_all()?;
        }
        fs::rename(&tmp, path)
    }
}

/// `$XDG_CONFIG_HOME/agent-gauge/config.json`, else `~/.config/agent-gauge/config.json`.
pub fn user_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_tilde("~/.config"));
    base.join("agent-gauge").join("config.json")
}

/// `$XDG_CACHE_HOME/agent-gauge`, else `~/.cache/agent-gauge`.
pub fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| expand_tilde("~/.cache"))
        .join("agent-gauge")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let cfg = Config::load(Path::new("/nonexistent/config.json"));
        assert_eq!(cfg.refresh_seconds, 60);
        assert_eq!(cfg.theme, "default");
        assert!(!cfg.show_in_taskbar);
        assert!(cfg.forecast_enabled && cfg.forecast_notify);
    }

    #[test]
    fn forecast_keys_lenient() {
        let cfg = Config::from_value(
            serde_json::json!({"forecast_enabled": false, "forecast_notify": "yes"}),
        );
        assert!(!cfg.forecast_enabled);
        assert!(cfg.forecast_notify);
    }

    #[test]
    fn partial_override_and_unknown_keys_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(
            &path,
            r#"{"theme":"nord","show_news":true,"refresh_seconds":30}"#,
        )
        .unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.theme, "nord");
        assert_eq!(cfg.refresh_seconds, 30);
        assert_eq!(cfg.osd_position, "top-right");
        assert_eq!(cfg.extra.get("show_news"), Some(&Value::Bool(true)));

        cfg.save(&path).unwrap();
        let again: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(again["show_news"], Value::Bool(true));
        assert_eq!(again["theme"], Value::String("nord".into()));
    }

    #[test]
    fn bad_value_falls_back_per_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"theme":"dracula","refresh_seconds":"fast"}"#).unwrap();
        let cfg = Config::load(&path);
        assert_eq!(cfg.theme, "dracula");
        assert_eq!(cfg.refresh_seconds, 60);
    }

    #[test]
    fn row_and_agent_settings() {
        let mut cfg = Config::default();
        assert!(cfg.claude_enabled() && !cfg.codex_enabled());
        assert!(cfg.row_visible("cost"));
        Config::set_in_list(&mut cfg.osd_hidden_rows, "cost", true);
        Config::set_in_list(&mut cfg.osd_hidden_rows, "cost", true); // no duplicates
        assert_eq!(cfg.osd_hidden_rows, vec!["cost".to_string()]);
        assert!(!cfg.row_visible("cost"));
        Config::set_in_list(&mut cfg.providers, "codex", true);
        Config::set_in_list(&mut cfg.providers, "claude", false);
        assert!(cfg.codex_enabled() && !cfg.claude_enabled());
    }

    #[test]
    fn python_written_null_coords_ok() {
        let cfg = Config::from_value(serde_json::json!({"osd_x": null, "osd_y": 12.0}));
        assert_eq!(cfg.osd_x, None);
        assert_eq!(cfg.osd_y, Some(12.0));
    }
}
