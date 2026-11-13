//! Single-instance guard: an exclusive advisory lock on a per-user file.
//! The OS releases it when the process dies, so a crash never leaves a stale lock.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct InstanceLock {
    _files: Vec<File>,
}

fn lock_paths() -> [PathBuf; 2] {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let cache = crate::config::cache_dir();
    let legacy_cache = cache.parent().unwrap().join("claude-usage");
    [
        runtime
            .as_ref()
            .unwrap_or(&legacy_cache)
            .join("claude-usage.lock"),
        runtime.as_ref().unwrap_or(&cache).join("agent-gauge.lock"),
    ]
}

impl InstanceLock {
    /// `Ok(Some)` = we are the only instance; `Ok(None)` = another one holds it.
    pub fn acquire() -> std::io::Result<Option<Self>> {
        Self::acquire_paths(&lock_paths())
    }

    fn acquire_paths(paths: &[PathBuf]) -> std::io::Result<Option<Self>> {
        let mut files = Vec::new();
        for path in paths {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)?;
            match file.try_lock() {
                Ok(()) => {
                    let _ = file.set_len(0);
                    let _ = writeln!(file, "{}", std::process::id());
                    files.push(file);
                }
                Err(fs::TryLockError::WouldBlock) => return Ok(None),
                Err(fs::TryLockError::Error(e)) => return Err(e),
            }
        }
        Ok(Some(Self { _files: files }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_instance_blocks_new_instance_and_release_allows_restart() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [
            dir.path().join("claude-usage.lock"),
            dir.path().join("agent-gauge.lock"),
        ];
        let legacy = InstanceLock::acquire_paths(&paths[..1]).unwrap().unwrap();
        assert!(InstanceLock::acquire_paths(&paths).unwrap().is_none());
        drop(legacy);
        let current = InstanceLock::acquire_paths(&paths).unwrap().unwrap();
        assert!(InstanceLock::acquire_paths(&paths[..1]).unwrap().is_none());
        drop(current);
        assert!(InstanceLock::acquire_paths(&paths).unwrap().is_some());
    }

    #[test]
    fn busy_new_lock_releases_legacy_lock() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [
            dir.path().join("legacy.lock"),
            dir.path().join("current.lock"),
        ];
        let _current = InstanceLock::acquire_paths(&paths[1..]).unwrap().unwrap();
        assert!(InstanceLock::acquire_paths(&paths).unwrap().is_none());
        assert!(InstanceLock::acquire_paths(&paths[..1]).unwrap().is_some());
    }
}
