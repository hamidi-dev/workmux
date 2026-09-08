//! Retry transient filesystem cleanup failures without losing orphan identities.

use anyhow::{Context, Result};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::cleanup::DirectoryIdentity;

const RETRY_TIMEOUT: Duration = Duration::from_secs(5);
const INITIAL_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_millis(500);

pub(super) struct PendingCleanup {
    path: PathBuf,
    record_path: PathBuf,
    identity: DirectoryIdentity,
}

impl PendingCleanup {
    /// Persist evidence before rename, so Git cleanup cannot make an orphan untracked.
    pub fn prepare(
        original_path: &Path,
        path: PathBuf,
        identity: DirectoryIdentity,
    ) -> Result<Self> {
        Self::prepare_in(
            &crate::xdg::state_dir()?.join("pending-cleanup"),
            original_path,
            path,
            identity,
        )
    }

    fn prepare_in(
        state_dir: &Path,
        original_path: &Path,
        path: PathBuf,
        identity: DirectoryIdentity,
    ) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let name = path
            .file_name()
            .context("Quarantine path has no filename")?;
        let mut record_name = name.to_os_string();
        record_name.push(".json");
        let record_path = state_dir.join(record_name);
        let record = serde_json::json!({
            "version": 1,
            "original_path": original_path.to_string_lossy(),
            "original_path_bytes": original_path.as_os_str().as_encoded_bytes(),
            "quarantine_path": path.to_string_lossy(),
            "quarantine_path_bytes": path.as_os_str().as_encoded_bytes(),
            "device": identity.device,
            "inode": identity.inode,
            "scope": "filesystem only; Git cleanup may be incomplete; do not replay branch deletion",
        });
        crate::util::write_atomic_durable(&record_path, &serde_json::to_vec_pretty(&record)?)
            .context("Failed to persist pending cleanup before quarantine")?;
        tracing::info!(record = %record_path.display(), path = %path.display(), "cleanup:pending filesystem cleanup recorded");
        Ok(Self {
            path,
            record_path,
            identity,
        })
    }

    pub fn remove(self) -> Result<()> {
        remove_with_retry(&self.path, self.identity, RETRY_TIMEOUT).map_err(|error| {
            let snapshot = super::cleanup_diagnostics::remaining_entries(&self.path);
            tracing::warn!(
                path = %self.path.display(),
                record = %self.record_path.display(),
                error = %error,
                "cleanup:quarantine deletion failed; pending record retained"
            );
            let context = format!(
                "Failed to remove quarantined worktree {} (kind={:?}, errno={:?}); pending cleanup record retained at {}; {}",
                self.path.display(), error.kind(), error.raw_os_error(),
                self.record_path.display(), snapshot,
            );
            anyhow::Error::new(error).context(context)
        })?;
        std::fs::remove_file(&self.record_path)
            .context("Worktree deleted, but failed to clear pending cleanup record")?;
        Ok(())
    }
}

fn remove_with_retry(
    path: &Path,
    identity: DirectoryIdentity,
    timeout: Duration,
) -> io::Result<()> {
    retry_directory_not_empty(timeout, || super::cleanup_tree::remove(path, identity))
}

fn retry_directory_not_empty(
    timeout: Duration,
    mut remove: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let started = Instant::now();
    let mut backoff = INITIAL_BACKOFF;
    loop {
        let error = match remove() {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };
        if error.kind() != io::ErrorKind::DirectoryNotEmpty || started.elapsed() >= timeout {
            return Err(error);
        }
        let delay = backoff.min(timeout.saturating_sub(started.elapsed()));
        tracing::debug!(?delay, "cleanup:retrying nonempty quarantine directory");
        std::thread::sleep(delay);
        if started.elapsed() >= timeout {
            return Err(error);
        }
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    fn identity(path: &Path) -> DirectoryIdentity {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        DirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }

    #[test]
    fn retries_transient_nonempty_errors() {
        let mut attempts = 0;
        retry_directory_not_empty(Duration::from_secs(1), || {
            attempts += 1;
            if attempts < 3 {
                Err(io::Error::from(io::ErrorKind::DirectoryNotEmpty))
            } else {
                Ok(())
            }
        })
        .unwrap();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn stops_retrying_at_deadline_and_does_not_retry_other_errors() {
        let started = Instant::now();
        let mut attempts = 0;
        let error = retry_directory_not_empty(Duration::from_millis(120), || {
            attempts += 1;
            Err(io::Error::from(io::ErrorKind::DirectoryNotEmpty))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::DirectoryNotEmpty);
        assert!(attempts > 1);
        assert!(started.elapsed() < Duration::from_secs(1));
        let mut attempts = 0;
        retry_directory_not_empty(Duration::from_secs(5), || {
            attempts += 1;
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert_eq!(attempts, 1);
    }

    #[test]
    fn persists_before_rename_and_clears_only_after_success() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("worktree");
        let trash = root.path().join(".workmux_trash_test");
        std::fs::create_dir(&original).unwrap();
        std::fs::write(original.join("file"), "contents").unwrap();
        let pending = PendingCleanup::prepare_in(
            &root.path().join("state"),
            &original,
            trash.clone(),
            identity(&original),
        )
        .unwrap();
        let record_path = pending.record_path.clone();
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
        assert_eq!(record["quarantine_path"], trash.to_str().unwrap());
        assert!(original.exists());
        assert!(!trash.exists());
        std::fs::rename(original, &trash).unwrap();
        pending.remove().unwrap();
        assert!(!record_path.exists());
        assert!(!trash.exists());
    }

    #[test]
    fn preserves_non_utf8_paths_and_refuses_unwritable_record_location() {
        use std::os::unix::ffi::OsStringExt;
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("worktree");
        std::fs::create_dir(&original).unwrap();
        let trash = root.path().join(".workmux_trash_test");
        let state = root.path().join("state");
        std::fs::write(&state, "not a directory").unwrap();
        assert!(
            PendingCleanup::prepare_in(&state, &original, trash.clone(), identity(&original))
                .is_err()
        );
        assert!(original.is_dir());
        assert!(!trash.exists());
        std::fs::remove_file(&state).unwrap();
        // Record serialization must preserve OS paths even on filesystems that
        // cannot themselves create non-UTF-8 names.
        let non_utf8 = root
            .path()
            .join(std::ffi::OsString::from_vec(b"worktree-\xff".to_vec()));
        let pending =
            PendingCleanup::prepare_in(&state, &non_utf8, trash, identity(&original)).unwrap();
        let record: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&pending.record_path).unwrap()).unwrap();
        let bytes: Vec<u8> = serde_json::from_value(record["original_path_bytes"].clone()).unwrap();
        assert_eq!(bytes, non_utf8.as_os_str().as_encoded_bytes());
    }

    #[test]
    fn preserves_record_and_rejects_replaced_directory() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original");
        let trash = root.path().join(".workmux_trash_test");
        std::fs::create_dir(&original).unwrap();
        let pending = PendingCleanup::prepare_in(
            &root.path().join("state"),
            &original,
            trash.clone(),
            identity(&original),
        )
        .unwrap();
        let record_path = pending.record_path.clone();
        std::fs::create_dir(&trash).unwrap();
        std::fs::write(trash.join("sentinel"), "keep").unwrap();
        assert!(format!("{:#}", pending.remove().unwrap_err()).contains("identity changed"));
        assert!(record_path.exists());
        assert!(trash.join("sentinel").exists());
    }
}
