//! Non-GUI entry points: JSON dump, single field, statusline, history export.

use std::io::{self, Write};
use std::path::Path;

use chrono::{TimeZone, Utc};
use clap::{Parser, ValueEnum};
use serde_json::Value;

use crate::collector::Collector;
use crate::config::{Config, user_config_path};
use crate::history;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ExportFormat {
    Csv,
    Json,
}

/// AgentGauge: Claude and Codex usage limits — desktop widget and CLI.
#[derive(Debug, Parser)]
#[command(name = "agent-gauge", version, about)]
pub struct Args {
    /// Collect in the background for the KDE panel, without opening a window.
    #[arg(long, conflicts_with_all = ["panel_snapshot", "json", "once", "field", "statusline", "export"])]
    pub panel_service: bool,
    /// Print the cached panel snapshot without collecting or accessing the network.
    #[arg(long, conflicts_with_all = ["json", "once", "field", "statusline", "export", "detach"])]
    pub panel_snapshot: bool,
    /// Emit full stats as JSON and exit.
    #[arg(long)]
    pub json: bool,
    /// Collect once, print JSON and exit (alias of --json).
    #[arg(long)]
    pub once: bool,
    /// Print a single stats field (e.g. session_utilization) and exit.
    #[arg(long, value_name = "NAME")]
    pub field: Option<String>,
    /// One-line summary for Claude Code's statusLine, e.g. "S 42% · W 18% · $3.21".
    #[arg(long)]
    pub statusline: bool,
    /// Export utilisation history to stdout.
    #[arg(long, value_enum, value_name = "FORMAT")]
    pub export: Option<ExportFormat>,
    /// Look-back window for --export, in days.
    #[arg(long, default_value_t = 30)]
    pub days: u32,
    /// Launch the GUI in the background and return immediately.
    #[arg(short, long)]
    pub detach: bool,
    /// Use this config file instead of ~/.config/agent-gauge/config.json.
    #[arg(long, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,
}

impl Args {
    pub fn is_cli(&self) -> bool {
        self.json || self.once || self.field.is_some() || self.statusline || self.export.is_some()
    }

    pub fn config_path(&self) -> std::path::PathBuf {
        self.config.clone().unwrap_or_else(user_config_path)
    }
}

/// `S 42% · W 18% · $3.21 · Fable 55%`. Truncates like the OSD does.
/// Shows `--` when rate-limited with nothing known yet. Never panics.
pub fn format_statusline(data: &Value, sep: &str) -> String {
    let f = |k: &str| data.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    let pct = |v: f64| ((v * 100.0).floor() as i64).to_string();
    let errored = data
        .get("rate_limit_error")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let (session, weekly) = (f("session_utilization"), f("weekly_utilization"));
    let (s_txt, w_txt) = if errored && session == 0.0 && weekly == 0.0 {
        ("--".to_string(), "--".to_string())
    } else {
        (pct(session), pct(weekly))
    };
    // ` (full 1h20m)` when a window is on pace to run out before it resets.
    let now = data
        .get("collected_at")
        .and_then(Value::as_i64)
        .filter(|t| *t > 0)
        .unwrap_or_else(|| Utc::now().timestamp());
    let full = |id: &str| {
        data.pointer(&format!("/forecasts/{id}/exhausts_at"))
            .and_then(Value::as_i64)
            .map(|t| crate::format::until_compact(t, now))
            .filter(|t| !t.is_empty())
            .map(|t| format!(" (full {t})"))
            .unwrap_or_default()
    };
    let mut line = format!(
        "S {s_txt}%{}{sep}W {w_txt}%{}{sep}${:.2}",
        full("session"),
        full("weekly"),
        f("today_cost")
    );
    if let Some(label) = data
        .get("scoped_label")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        line.push_str(&format!("{sep}{label} {}%", pct(f("scoped_utilization"))));
    }
    line
}

fn statusline_sep() -> &'static str {
    // Windows consoles on non-UTF-8 code pages can't render the middle dot.
    if cfg!(windows) { " | " } else { " · " }
}

pub fn export_history(
    path: &Path,
    fmt: ExportFormat,
    days: u32,
    out: &mut dyn Write,
) -> io::Result<usize> {
    let now = Utc::now().timestamp() as f64;
    let samples = history::load(path, now - f64::from(days) * 86400.0);
    match fmt {
        ExportFormat::Csv => {
            writeln!(out, "ts,iso,session,weekly")?;
            for s in &samples {
                let iso = Utc
                    .timestamp_opt(s.ts as i64, 0)
                    .single()
                    .map(|d| d.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
                    .unwrap_or_default();
                writeln!(out, "{},{iso},{},{}", s.ts, s.session, s.weekly)?;
            }
        }
        ExportFormat::Json => {
            serde_json::to_writer_pretty(&mut *out, &samples).map_err(io::Error::other)?;
            writeln!(out)?;
        }
    }
    Ok(samples.len())
}

/// Run a CLI invocation; returns the process exit code.
pub fn run(args: &Args) -> i32 {
    let mut cfg = Config::load(&args.config_path());
    // The CLI is Claude-first (statusline etc.) regardless of the OSD's agent picks.
    Config::set_in_list(&mut cfg.providers, "claude", true);
    let mut stdout = io::stdout().lock();

    if let Some(fmt) = args.export {
        let path = cfg.claude_dir().join(history::FILENAME);
        return match export_history(&path, fmt, args.days, &mut stdout) {
            Ok(n) => {
                eprintln!("# exported {n} samples");
                0
            }
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => 0,
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        };
    }

    let stats = Collector::new().collect(&cfg);

    if args.statusline {
        // Never fail: a statusLine command that errors shows up inside Claude Code.
        let _ = writeln!(
            stdout,
            "{}",
            format_statusline(&stats.to_json_value(), statusline_sep())
        );
        return 0;
    }

    if let Some(name) = &args.field {
        let data = stats.to_json_value();
        return match data.get(name) {
            None => {
                eprintln!("error: unknown field {name:?}");
                2
            }
            Some(Value::String(s)) => {
                let _ = writeln!(stdout, "{s}");
                0
            }
            Some(v) => {
                let _ = writeln!(stdout, "{v}");
                0
            }
        };
    }

    match serde_json::to_string_pretty(&stats) {
        Ok(s) => {
            let _ = writeln!(stdout, "{s}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statusline_basic() {
        let d = json!({"session_utilization": 0.429, "weekly_utilization": 0.18,
                       "today_cost": 3.214, "scoped_label": "Fable", "scoped_utilization": 0.55});
        assert_eq!(
            format_statusline(&d, " · "),
            "S 42% · W 18% · $3.21 · Fable 55%"
        );
    }

    #[test]
    fn statusline_unknown_when_errored_without_data() {
        let d = json!({"rate_limit_error": "Rate limited", "today_cost": 1.0});
        assert_eq!(format_statusline(&d, " | "), "S --% | W --% | $1.00");
        assert_eq!(
            format_statusline(&json!(null), " | "),
            "S 0% | W 0% | $0.00"
        );
    }

    #[test]
    fn statusline_with_forecast() {
        let d = json!({"session_utilization": 0.62, "weekly_utilization": 0.18, "today_cost": 3.21,
                       "collected_at": 1000,
                       "forecasts": {"session": {"rate_per_hour": 0.3, "exhausts_at": 1000 + 80 * 60},
                                     "weekly": {"rate_per_hour": 0.01, "exhausts_at": 0}}});
        assert_eq!(
            format_statusline(&d, " · "),
            "S 62% (full 1h20m) · W 18% · $3.21"
        );
    }

    #[test]
    fn until_compact_formats() {
        use crate::format::until_compact;
        assert_eq!(until_compact(0, 100), "");
        assert_eq!(until_compact(50, 100), "");
        assert_eq!(until_compact(100 + 45 * 60, 100), "45m");
        assert_eq!(until_compact(100 + 30, 100), "1m");
        assert_eq!(until_compact(100 + 65 * 60, 100), "1h05m");
        assert_eq!(until_compact(100 + 51 * 3600, 100), "2d3h");
    }

    #[test]
    fn export_csv() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.jsonl");
        let now = Utc::now().timestamp() as f64;
        history::append(
            &p,
            &history::Sample {
                ts: now,
                session: 0.5,
                weekly: 0.25,
                ..Default::default()
            },
        )
        .unwrap();
        history::append(
            &p,
            &history::Sample {
                ts: now - 40.0 * 86400.0,
                ..Default::default()
            },
        )
        .unwrap();
        let mut buf = Vec::new();
        let n = export_history(&p, ExportFormat::Csv, 30, &mut buf).unwrap();
        assert_eq!(n, 1);
        let text = String::from_utf8(buf).unwrap();
        assert!(text.starts_with("ts,iso,session,weekly\n"));
        assert!(text.contains(",0.5,0.25"));
    }
}
