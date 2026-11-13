//! Local Claude Code activity: running sessions and the prompt history log.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use chrono::{Local, TimeZone};
use serde_json::Value;

use crate::stats::ActiveSession;

/// Is *pid* alive? Uses `kill(pid, 0)` on Unix (EPERM still means alive).
#[cfg(unix)]
pub fn process_alive(pid: i64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence/permission check.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Non-Unix: we can't probe cheaply without extra deps, so trust the file.
#[cfg(not(unix))]
pub fn process_alive(pid: i64) -> bool {
    pid > 0
}

/// Sessions in `~/.claude/sessions/*.json` whose recorded PID is alive.
pub fn active_sessions(claude_dir: &Path) -> Vec<ActiveSession> {
    let Ok(rd) = fs::read_dir(claude_dir.join("sessions")) else {
        return Vec::new();
    };
    let mut out: Vec<ActiveSession> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<ActiveSession>(&t).ok())
        .filter(|s| process_alive(s.pid))
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.started_at));
    out
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct MessageCounts {
    pub today_messages: u64,
    pub today_sessions: u64,
    pub week_messages: u64,
    pub week_sessions: u64,
}

/// Count prompts and distinct sessions for today and the rolling 7 days
/// from `~/.claude/history.jsonl` (millisecond timestamps, local calendar).
pub fn message_counts(claude_dir: &Path, now: chrono::DateTime<Local>) -> MessageCounts {
    let mut c = MessageCounts::default();
    let Ok(f) = fs::File::open(claude_dir.join("history.jsonl")) else {
        return c;
    };
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|d| Local.from_local_datetime(&d).earliest())
        .unwrap_or(now);
    let week_start = today_start - chrono::Duration::days(6);
    let (today_ms, week_ms) = (
        today_start.timestamp_millis(),
        week_start.timestamp_millis(),
    );

    let mut today_ids = HashSet::new();
    let mut week_ids = HashSet::new();
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let ts = v.get("timestamp").and_then(Value::as_i64).unwrap_or(0);
        if ts <= 0 {
            continue;
        }
        let sid = v
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if ts >= today_ms {
            c.today_messages += 1;
            today_ids.insert(sid.clone());
        }
        if ts >= week_ms {
            c.week_messages += 1;
            week_ids.insert(sid);
        }
    }
    c.today_sessions = today_ids.len() as u64;
    c.week_sessions = week_ids.len() as u64;
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_alive() {
        assert!(process_alive(std::process::id() as i64));
        assert!(!process_alive(0));
        assert!(!process_alive(-5));
    }

    #[test]
    fn active_sessions_filters_dead_pids() {
        let dir = tempfile::tempdir().unwrap();
        let s = dir.path().join("sessions");
        fs::create_dir_all(&s).unwrap();
        let me = std::process::id();
        fs::write(
            s.join("a.json"),
            format!(r#"{{"pid":{me},"sessionId":"x","cwd":"/tmp"}}"#),
        )
        .unwrap();
        fs::write(s.join("b.json"), r#"{"pid":999999999,"sessionId":"y"}"#).unwrap();
        fs::write(s.join("c.key"), "junk").unwrap();
        let act = active_sessions(dir.path());
        assert_eq!(act.len(), 1);
        assert_eq!(act[0].session_id, "x");
    }

    #[test]
    fn counts_messages() {
        let dir = tempfile::tempdir().unwrap();
        let now = Local::now();
        let ms = now.timestamp_millis();
        let old = ms - 10 * 86_400_000;
        fs::write(
            dir.path().join("history.jsonl"),
            format!(
                "{{\"timestamp\":{ms},\"sessionId\":\"a\"}}\n{{\"timestamp\":{ms},\"sessionId\":\"b\"}}\n{{\"timestamp\":{old},\"sessionId\":\"c\"}}\nbad\n"
            ),
        )
        .unwrap();
        let c = message_counts(dir.path(), now);
        assert_eq!(c.today_messages, 2);
        assert_eq!(c.today_sessions, 2);
        assert_eq!(c.week_messages, 2);
    }
}
