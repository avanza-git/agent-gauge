//! OAuth token lookup, mirroring Claude Code's own order:
//! `CLAUDE_CODE_OAUTH_TOKEN` env → `~/.claude/.credentials.json` → macOS Keychain.
//!
//! The widget never refreshes the token itself — only Claude Code may.

use std::fs;
use std::path::Path;

use serde_json::Value;

/// Keychain service names Claude Code has used, tried in order.
#[cfg(target_os = "macos")]
const MACOS_KEYCHAIN_SERVICES: &[&str] = &["Claude Code-credentials", "Claude Code"];

/// Extract `claudeAiOauth.accessToken` from a credentials JSON blob.
pub fn extract_token(blob: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    let tok = v.get("claudeAiOauth")?.get("accessToken")?.as_str()?.trim();
    (!tok.is_empty()).then(|| tok.to_string())
}

pub fn load_token(claude_dir: &Path) -> Option<String> {
    if let Ok(env_tok) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        let t = env_tok.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }

    if let Ok(blob) = fs::read_to_string(claude_dir.join(".credentials.json"))
        && let Some(t) = extract_token(&blob)
    {
        return Some(t);
    }

    #[cfg(target_os = "macos")]
    for service in MACOS_KEYCHAIN_SERVICES {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output();
        if let Ok(out) = out
            && out.status.success()
            && let Some(t) = extract_token(String::from_utf8_lossy(&out.stdout).trim())
        {
            return Some(t);
        }
    }

    None
}

/// Subscription type ("max", "pro", …) from the credentials file, or "".
pub fn load_subscription_type(claude_dir: &Path) -> String {
    fs::read_to_string(claude_dir.join(".credentials.json"))
        .ok()
        .and_then(|b| serde_json::from_str::<Value>(&b).ok())
        .and_then(|v| {
            v.get("claudeAiOauth")?
                .get("subscriptionType")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default()
}

pub fn no_credentials_message() -> &'static str {
    if cfg!(target_os = "macos") {
        "No credentials -- run 'agent-gauge' once from a Terminal and allow Keychain access (or set CLAUDE_CODE_OAUTH_TOKEN)"
    } else {
        "No credentials found -- run 'claude' to log in"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_token() {
        let blob =
            r#"{"claudeAiOauth":{"accessToken":"  sk-ant-oat-x ","subscriptionType":"max"}}"#;
        assert_eq!(extract_token(blob).as_deref(), Some("sk-ant-oat-x"));
        assert_eq!(
            extract_token(r#"{"claudeAiOauth":{"accessToken":""}}"#),
            None
        );
        assert_eq!(extract_token("not json"), None);
    }

    #[test]
    fn reads_subscription_type() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"t","subscriptionType":"pro"}}"#,
        )
        .unwrap();
        assert_eq!(load_subscription_type(dir.path()), "pro");
    }
}
