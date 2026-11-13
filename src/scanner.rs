//! Token accounting from Claude Code conversation transcripts.
//!
//! Files scanned:
//! - top-level sessions: `projects/<proj>/<session>.jsonl`
//! - Task subagents: `projects/<proj>/<session>/subagents/**/agent-*.jsonl`
//!
//! Rules (ported from the reference widget):
//! - only `type == "assistant"` lines carry billable `message.usage`;
//! - Claude Code writes one line per content block, each repeating the full
//!   usage, so each `message.id` is counted once;
//! - a resumed/forked session replays turns with all-zero usage — those lines
//!   never claim the id, so the real figures win regardless of file order;
//! - dates are the timestamp's UTC `YYYY-MM-DD` prefix.
//!
//! [`Scanner`] keeps per-file parse results and a byte offset, so repeated
//! scans (the GUI refresh loop) only parse newly appended lines.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::pricing;
use crate::stats::TokenBucket;

/// A subagent transcript touched this recently counts as "active".
pub const SUBAGENT_ACTIVE_SECONDS: u64 = 60;
/// Live tok/min is averaged over assistant turns in this window.
pub const LIVE_WINDOW_SECONDS: f64 = 5.0 * 60.0;
/// The LIVE badge shows only if a turn landed this recently.
pub const LIVE_ACTIVE_SECONDS: f64 = 90.0;
/// Files untouched for this long can't hold live turns (cheap early filter).
const LIVE_FILE_MTIME_SECONDS: u64 = 10 * 60;

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    /// `YYYY-MM-DD` (UTC).
    pub date: String,
    /// Unix seconds of the turn (0 if the timestamp didn't parse).
    pub ts: f64,
    pub msg_id: Option<String>,
    pub counts: TokenBucket,
    /// Billing model key (may carry a long-prompt tier suffix).
    pub model: String,
}

fn u64_field(v: Option<&Value>) -> u64 {
    v.and_then(Value::as_u64).unwrap_or(0)
}

/// Extract the billable record from one transcript line, if any.
pub fn parse_line(line: &str) -> Option<Record> {
    let line = line.trim();
    // Cheap pre-filter: most lines are user/tool/system entries.
    if line.is_empty() || !line.contains("\"assistant\"") {
        return None;
    }
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let msg = entry.get("message")?.as_object()?;
    let usage = msg.get("usage").and_then(Value::as_object);
    let get = |k: &str| u64_field(usage.and_then(|u| u.get(k)));
    let counts = TokenBucket {
        input: get("input_tokens"),
        output: get("output_tokens"),
        cache_read: get("cache_read_input_tokens"),
        cache_creation: get("cache_creation_input_tokens"),
        cache_creation_1h: u64_field(
            usage
                .and_then(|u| u.get("cache_creation"))
                .and_then(|c| c.get("ephemeral_1h_input_tokens")),
        ),
    };
    let ts = entry.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let date: String = ts.chars().take(10).collect();
    let raw_model = msg
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let ts_secs = chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| d.timestamp_millis() as f64 / 1000.0)
        .unwrap_or(0.0);
    Some(Record {
        date,
        ts: ts_secs,
        msg_id: msg
            .get("id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        model: pricing::billing_model(raw_model, counts.prompt_tokens()),
        counts,
    })
}

/// True if this record's message id was already counted in this pass.
/// Zero-usage replays never claim an id.
pub fn is_duplicate(rec: &Record, seen: &mut HashSet<String>) -> bool {
    let Some(id) = rec.msg_id.as_ref() else {
        return false;
    };
    if seen.contains(id) {
        return true;
    }
    if !rec.counts.is_zero() {
        seen.insert(id.clone());
    }
    false
}

#[derive(Debug, Default)]
struct FileEntry {
    offset: u64,
    records: Vec<Record>,
    touched: bool,
}

#[derive(Debug, Default, Clone)]
pub struct TokenTotals {
    pub today_output: u64,
    pub week_output: u64,
    pub today_by_model: BTreeMap<String, u64>,
    pub today_by_model_detailed: BTreeMap<String, TokenBucket>,
    pub week_by_model_detailed: BTreeMap<String, TokenBucket>,
    pub today_by_project: HashMap<String, u64>,
    pub active_subagents: u64,
    /// Output tokens of assistant turns in the last [`LIVE_WINDOW_SECONDS`].
    pub live_output_tokens: u64,
    /// Unix seconds of the newest assistant turn in that window (0 = none).
    pub live_last_activity_ts: f64,
}

#[derive(Debug, Default)]
pub struct Scanner {
    files: HashMap<PathBuf, FileEntry>,
}

/// A transcript file plus the project (first dir under `projects/`) it belongs to.
#[derive(Debug, Clone)]
pub struct TranscriptFile {
    pub path: PathBuf,
    pub project: String,
    pub mtime: SystemTime,
    pub is_subagent: bool,
}

fn walk_subagents(dir: &Path, project: &str, out: &mut Vec<TranscriptFile>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(md) = fs::metadata(&p) else { continue };
        if md.is_dir() {
            walk_subagents(&p, project, out);
        } else if md.is_file() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("agent-") && name.ends_with(".jsonl") {
                out.push(TranscriptFile {
                    path: p,
                    project: project.to_string(),
                    mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    is_subagent: true,
                });
            }
        }
    }
}

/// Enumerate every transcript under `projects/`.
pub fn list_transcripts(projects_dir: &Path) -> Vec<TranscriptFile> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else {
        return out;
    };
    for proj in projects.flatten() {
        let proj_path = proj.path();
        if !fs::metadata(&proj_path)
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let project = proj.file_name().to_string_lossy().into_owned();
        let Ok(entries) = fs::read_dir(&proj_path) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(md) = fs::metadata(&p) else { continue };
            if md.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
                out.push(TranscriptFile {
                    path: p,
                    project: project.clone(),
                    mtime: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    is_subagent: false,
                });
            } else if md.is_dir() {
                let sub = p.join("subagents");
                if sub.is_dir() {
                    walk_subagents(&sub, &project, &mut out);
                }
            }
        }
    }
    out
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse any bytes appended since the last call and return all records.
    fn records(&mut self, path: &Path) -> &[Record] {
        let entry = self.files.entry(path.to_path_buf()).or_default();
        entry.touched = true;
        let Ok(mut f) = fs::File::open(path) else {
            entry.records.clear();
            entry.offset = 0;
            return &entry.records;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < entry.offset {
            // Truncated / rewritten: start over.
            entry.records.clear();
            entry.offset = 0;
        }
        if len > entry.offset && f.seek(SeekFrom::Start(entry.offset)).is_ok() {
            let mut buf = Vec::with_capacity((len - entry.offset) as usize);
            if f.read_to_end(&mut buf).is_ok() {
                // Only consume complete lines; a half-flushed tail waits for next time.
                if let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') {
                    let complete = &buf[..=last_nl];
                    for line in complete.split(|&b| b == b'\n') {
                        if let Some(rec) = std::str::from_utf8(line).ok().and_then(parse_line) {
                            entry.records.push(rec);
                        }
                    }
                    entry.offset += complete.len() as u64;
                }
            }
        }
        &entry.records
    }

    /// Drop cached files that weren't read during the last scan.
    fn forget_untouched(&mut self) {
        self.files.retain(|_, e| std::mem::take(&mut e.touched));
    }

    /// One pass over transcripts modified in the last 8 days, aggregating
    /// today (`today` = UTC `YYYY-MM-DD`) and the 7-day window (`week`).
    pub fn scan(
        &mut self,
        claude_dir: &Path,
        today: &str,
        week: &[String],
        now: SystemTime,
    ) -> TokenTotals {
        let mut t = TokenTotals::default();
        let projects_dir = claude_dir.join("projects");
        let projects_dir = fs::canonicalize(&projects_dir).unwrap_or(projects_dir);
        let cutoff = now - Duration::from_secs(8 * 86400);
        let active_cutoff = now - Duration::from_secs(SUBAGENT_ACTIVE_SECONDS);
        let week: HashSet<&str> = week.iter().map(String::as_str).collect();
        let live_file_cutoff = now - Duration::from_secs(LIVE_FILE_MTIME_SECONDS);
        let now_ts = now
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let live_from = now_ts - LIVE_WINDOW_SECONDS;

        let mut seen = HashSet::new();
        let mut live_seen: HashSet<String> = HashSet::new();
        for tf in list_transcripts(&projects_dir) {
            if tf.is_subagent && tf.mtime >= active_cutoff {
                t.active_subagents += 1;
            }
            if tf.mtime < cutoff {
                continue;
            }
            let live_file = tf.mtime >= live_file_cutoff;
            for rec in self.records(&tf.path) {
                // Live rate: billed turns in the window, each message once.
                // Future-dated turns (clock skew) are ignored.
                if live_file
                    && rec.counts.output > 0
                    && rec.ts >= live_from
                    && rec.ts <= now_ts + 60.0
                    && let Some(id) = rec.msg_id.as_ref()
                    && live_seen.insert(id.clone())
                {
                    t.live_output_tokens += rec.counts.output;
                    t.live_last_activity_ts = t.live_last_activity_ts.max(rec.ts);
                }
                let is_today = rec.date == today;
                if !is_today && !week.contains(rec.date.as_str()) {
                    continue;
                }
                if is_duplicate(rec, &mut seen) {
                    continue;
                }
                let out = rec.counts.output;
                t.week_output += out;
                t.week_by_model_detailed
                    .entry(rec.model.clone())
                    .or_default()
                    .add(&rec.counts);
                if is_today {
                    t.today_output += out;
                    *t.today_by_model.entry(rec.model.clone()).or_default() += out;
                    t.today_by_model_detailed
                        .entry(rec.model.clone())
                        .or_default()
                        .add(&rec.counts);
                    *t.today_by_project.entry(tf.project.clone()).or_default() += out;
                }
            }
        }
        self.forget_untouched();
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(id: &str, ts: &str, model: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":10,"cache_creation":{{"ephemeral_1h_input_tokens":4}}}}}}}}"#
        )
    }

    fn write(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn parse_line_extracts_usage() {
        let r = parse_line(&line("m1", "2026-11-13T10:00:00Z", "claude-opus-5-5", 5, 7)).unwrap();
        assert_eq!(r.date, "2026-11-13");
        assert_eq!(r.counts.output, 7);
        assert_eq!(r.counts.cache_creation_1h, 4);
        assert!(parse_line(r#"{"type":"user","message":{"content":"assistant"}}"#).is_none());
        assert!(parse_line("garbage \"assistant\"").is_none());
    }

    #[test]
    fn dedupes_content_blocks_and_zero_replays() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path();
        let p1 = claude.join("projects/-home-a/s1.jsonl");
        let p2 = claude.join("projects/-home-b/s2.jsonl");
        let today = "2026-11-13";
        // Same message id twice in one file (two content blocks).
        write(
            &p1,
            &[
                line("m1", "2026-11-13T10:00:00Z", "claude-sonnet-4-6", 5, 100),
                line("m1", "2026-11-13T10:00:00Z", "claude-sonnet-4-6", 5, 100),
            ],
        );
        // A zero-usage replay of m2 in one file and the real one in another.
        write(&p2, &[r#"{"type":"assistant","timestamp":"2026-11-13T11:00:00Z","message":{"id":"m2","model":"claude-sonnet-4-6","usage":{"input_tokens":0,"output_tokens":0}}}"#.to_string()]);
        write(
            &p1,
            &[line(
                "m2",
                "2026-11-13T11:00:00Z",
                "claude-sonnet-4-6",
                1,
                50,
            )],
        );
        // Older day in the week window.
        write(
            &p2,
            &[line("m3", "2026-11-10T11:00:00Z", "claude-opus-5-5", 1, 9)],
        );

        let week: Vec<String> = (7..=13).map(|d| format!("2026-11-{d:02}")).collect();
        let mut sc = Scanner::new();
        let t = sc.scan(claude, today, &week, SystemTime::now());
        assert_eq!(t.today_output, 150);
        assert_eq!(t.week_output, 159);
        assert_eq!(t.today_by_project.get("-home-a"), Some(&150));
        assert_eq!(
            t.today_by_model_detailed["claude-sonnet-4-6"].cache_creation,
            20
        );
    }

    #[test]
    fn live_window_rate() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path();
        let p = claude.join("projects/-p/live.jsonl");
        let now = chrono::Utc::now();
        let iso = |secs_ago: i64| (now - chrono::Duration::seconds(secs_ago)).to_rfc3339();
        write(
            &p,
            &[
                line("l1", &iso(30), "claude-opus-5-5", 1, 400),
                line("l1", &iso(30), "claude-opus-5-5", 1, 400), // same message, counted once
                line("l2", &iso(200), "claude-opus-5-5", 1, 100),
                line("old", &iso(600), "claude-opus-5-5", 1, 9999), // outside 5-min window
                line(
                    "fut",
                    &(now + chrono::Duration::seconds(3600)).to_rfc3339(),
                    "claude-opus-5-5",
                    1,
                    7777,
                ),
            ],
        );
        let today = now.format("%Y-%m-%d").to_string();
        let t = Scanner::new().scan(
            claude,
            &today,
            std::slice::from_ref(&today),
            SystemTime::now(),
        );
        assert_eq!(t.live_output_tokens, 500);
        let age = now.timestamp() as f64 - t.live_last_activity_ts;
        assert!((29.0..32.0).contains(&age), "age {age}");
    }

    #[test]
    fn includes_nested_subagents_and_is_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let claude = dir.path();
        let sub = claude.join("projects/-p/sess/subagents/workflows/wf1/agent-1.jsonl");
        let journal = claude.join("projects/-p/sess/subagents/journal.jsonl");
        write(
            &sub,
            &[line(
                "a1",
                "2026-11-13T01:00:00Z",
                "claude-haiku-4-5",
                1,
                10,
            )],
        );
        write(
            &journal,
            &[line(
                "j1",
                "2026-11-13T01:00:00Z",
                "claude-haiku-4-5",
                1,
                999,
            )],
        );
        let week = vec!["2026-11-13".to_string()];
        let mut sc = Scanner::new();
        let t = sc.scan(claude, "2026-11-13", &week, SystemTime::now());
        assert_eq!(t.today_output, 10);
        assert_eq!(t.active_subagents, 1);

        // Append a complete line and a half-written one.
        write(
            &sub,
            &[line("a2", "2026-11-13T02:00:00Z", "claude-haiku-4-5", 1, 5)],
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&sub)
            .unwrap()
            .write_all(br#"{"type":"assistant","#)
            .unwrap();
        let t = sc.scan(claude, "2026-11-13", &week, SystemTime::now());
        assert_eq!(t.today_output, 15);
    }
}
