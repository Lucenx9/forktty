//! Shared backup-path helper used when quarantining a corrupt on-disk file.

use std::path::{Path, PathBuf};

/// Pick a backup path for `path` using `extension`. If the plain candidate
/// already exists, append a pid + timestamp nonce so an earlier backup is never
/// clobbered.
pub(crate) fn unique_backup_path(path: &Path, extension: &str) -> PathBuf {
    let candidate = path.with_extension(extension);
    if !candidate.exists() {
        return candidate;
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("{extension}-{}-{nonce}", std::process::id()))
}
