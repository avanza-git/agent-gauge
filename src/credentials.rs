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
    oauth_str(blob, "accessToken")
}

/// Extract `claudeAiOauth.subscriptionType` ("max", "pro", …) from a credentials JSON blob.
pub fn extract_subscription_type(blob: &str) -> Option<String> {
    oauth_str(blob, "subscriptionType")
}

fn oauth_str(blob: &str, key: &str) -> Option<String> {
    let v: Value = serde_json::from_str(blob).ok()?;
    let s = v.get("claudeAiOauth")?.get(key)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Claude Code's stored credentials JSON, in lookup order: the credentials file,
/// then (macOS) the login Keychain. Lazy, so the Keychain is only asked when
/// the file doesn't have what the caller needs.
fn credential_blobs(claude_dir: &Path) -> impl Iterator<Item = String> + '_ {
    let file =
        std::iter::once_with(move || fs::read_to_string(claude_dir.join(".credentials.json")).ok());
    #[cfg(target_os = "macos")]
    let keychain = MACOS_KEYCHAIN_SERVICES.iter().map(|service| {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", service, "-w"])
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    });
    #[cfg(not(target_os = "macos"))]
    let keychain = std::iter::empty();
    file.chain(keychain).flatten()
}

pub fn load_token(claude_dir: &Path) -> Option<String> {
    if let Ok(env_tok) = std::env::var("CLAUDE_CODE_OAUTH_TOKEN") {
        let t = env_tok.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    credential_blobs(claude_dir).find_map(|b| extract_token(&b))
}

/// Subscription type ("max", "pro", …) from the stored credentials, or "".
pub fn load_subscription_type(claude_dir: &Path) -> String {
    credential_blobs(claude_dir)
        .find_map(|b| extract_subscription_type(&b))
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
    fn extracts_subscription_type() {
        let blob = r#"{"claudeAiOauth":{"accessToken":"t","subscriptionType":"pro"}}"#;
        assert_eq!(extract_subscription_type(blob).as_deref(), Some("pro"));
        assert_eq!(
            extract_subscription_type(r#"{"claudeAiOauth":{"accessToken":"t"}}"#),
            None
        );
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
