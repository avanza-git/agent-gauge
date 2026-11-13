//! Exercise startup migration through the CLI with isolated user directories.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-gauge"))
        .args(args)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .output()
        .unwrap()
}

#[test]
fn help_and_version_use_new_name_without_migrating() {
    let dir = tempfile::tempdir().unwrap();
    for option in ["--help", "--version"] {
        let output = run(dir.path(), &[option]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("agent-gauge"));
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn startup_copies_legacy_config_and_cache_without_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let old_config = root.join("config/claude-usage/config.json");
    let old_cache = root.join("cache/claude-usage");
    fs::create_dir_all(old_config.parent().unwrap()).unwrap();
    fs::create_dir_all(&old_cache).unwrap();
    fs::write(&old_config, r#"{"theme":"nord","custom_key":true}"#).unwrap();
    fs::write(old_cache.join("codex_limits.json"), "cached limits").unwrap();
    fs::write(old_cache.join("claude-usage.lock"), "old pid").unwrap();
    fs::write(old_cache.join("snapshot.json.tmp"), "partial").unwrap();
    let output = run(root, &["--export", "json", "--days", "0"]);
    assert!(output.status.success(), "{output:?}");
    let new_config = root.join("config/agent-gauge/config.json");
    assert_eq!(
        fs::read(&new_config).unwrap(),
        fs::read(&old_config).unwrap()
    );
    assert_eq!(
        fs::read_to_string(root.join("cache/agent-gauge/codex_limits.json")).unwrap(),
        "cached limits"
    );
    assert!(!root.join("cache/agent-gauge/claude-usage.lock").exists());
    assert!(!root.join("cache/agent-gauge/snapshot.json.tmp").exists());
    fs::write(&new_config, r#"{"theme":"dracula"}"#).unwrap();
    assert!(run(root, &["--export", "json"]).status.success());
    assert_eq!(
        fs::read_to_string(new_config).unwrap(),
        r#"{"theme":"dracula"}"#
    );
    assert!(fs::read_to_string(old_config).unwrap().contains("nord"));
}

#[test]
fn required_config_failure_aborts_but_custom_config_bypasses_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("config/claude-usage")).unwrap();
    fs::write(root.join("config/claude-usage/config.json"), "{}").unwrap();
    fs::write(
        root.join("config/agent-gauge"),
        "blocks destination directory",
    )
    .unwrap();
    let output = run(root, &["--export", "json"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot migrate configuration"));
    let custom = root.join("custom.json");
    fs::write(&custom, r#"{"theme":"terminal"}"#).unwrap();
    let output = run(
        root,
        &["--config", custom.to_str().unwrap(), "--export", "json"],
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(custom).unwrap(),
        r#"{"theme":"terminal"}"#
    );
}

#[test]
fn cache_copy_failure_warns_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("cache/claude-usage")).unwrap();
    fs::write(root.join("cache/claude-usage/codex_limits.json"), "{}").unwrap();
    fs::write(
        root.join("cache/agent-gauge"),
        "blocks destination directory",
    )
    .unwrap();
    let output = run(root, &["--export", "json"]);
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot migrate cached data"));
}
