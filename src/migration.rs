//! Copy legacy app data without replacing files or changing provider data.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Called after argument parsing so help/version remain free of filesystem writes.
/// Explicit configuration paths bypass migration of the default configuration.
pub fn initialize(migrate_config: bool) -> io::Result<()> {
    if migrate_config {
        let destination = crate::config::user_config_path();
        let root = destination.parent().unwrap().parent().unwrap();
        copy_missing(&root.join("claude-usage"), destination.parent().unwrap())
            .map_err(|e| io::Error::new(e.kind(), format!("cannot migrate configuration: {e}")))?;
    }
    let destination = crate::config::cache_dir();
    let source = destination.parent().unwrap().join("claude-usage");
    if let Err(e) = copy_missing(&source, &destination) {
        eprintln!("agent-gauge: cannot migrate cached data: {e}; continuing with available cache");
    }
    Ok(())
}

fn copy_missing(source: &Path, destination: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(source) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let text = name.to_string_lossy();
        if text.ends_with(".lock") || text.ends_with(".tmp") {
            continue;
        }
        let target = destination.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_missing(&entry.path(), &target)?;
        } else if kind.is_file() && !target.try_exists()? {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

// Publish a complete copy atomically; a competing migration cannot overwrite it.
fn copy_file(source: &Path, destination: &Path) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = destination.parent().unwrap();
    fs::create_dir_all(parent)?;
    let (temporary, mut output) = loop {
        let path = parent.join(format!(
            ".agent-gauge-migration-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        let mut input = fs::File::open(source)?;
        output.set_permissions(input.metadata()?.permissions())?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        match fs::hard_link(&temporary, destination) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            result => result,
        }
    })();
    drop(output);
    let cleanup = fs::remove_file(temporary);
    result.and(cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_data_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        copy_missing(&dir.path().join("old"), &dir.path().join("new")).unwrap();
        assert!(!dir.path().join("new").exists());
    }

    #[test]
    fn migration_preserves_originals_and_destination_and_is_repeatable() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        fs::create_dir_all(old.join("nested")).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("config.json"), "legacy config").unwrap();
        fs::write(new.join("config.json"), "current config").unwrap();
        fs::write(old.join("nested/cache.json"), "cached data").unwrap();
        fs::write(old.join("claude-usage.lock"), "123").unwrap();
        fs::write(old.join("config.json.tmp"), "partial").unwrap();
        for _ in 0..2 {
            copy_missing(&old, &new).unwrap();
        }
        assert_eq!(
            fs::read_to_string(new.join("config.json")).unwrap(),
            "current config"
        );
        assert_eq!(
            fs::read_to_string(new.join("nested/cache.json")).unwrap(),
            "cached data"
        );
        assert_eq!(
            fs::read_to_string(old.join("config.json")).unwrap(),
            "legacy config"
        );
        assert!(!new.join("claude-usage.lock").exists());
        assert!(!new.join("config.json.tmp").exists());
        assert_eq!(fs::read_dir(&new).unwrap().count(), 2);
    }

    #[test]
    fn copy_failure_is_reported_without_partial_destination() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("config.json");
        assert!(copy_file(&dir.path().join("missing"), &target).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
