//! Shared backup-path helper used when quarantining a corrupt on-disk file.

use std::path::{Path, PathBuf};

/// Reserve a backup path for `path` using `extension` by atomically creating
/// an empty placeholder file there (`create_new`). The caller renames or
/// writes the quarantined content over its own placeholder, so two processes
/// quarantining at once can never clobber each other's backup: the loser of
/// the race on the plain candidate gets a pid + timestamp nonce name instead.
pub(crate) fn reserve_unique_backup_path(path: &Path, extension: &str) -> PathBuf {
    let candidate = path.with_extension(extension);
    if try_reserve(&candidate) {
        return candidate;
    }
    for _ in 0..16 {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate = path.with_extension(format!("{extension}-{}-{nonce}", std::process::id()));
        if try_reserve(&candidate) {
            return candidate;
        }
    }
    // Reservation kept failing (e.g. unwritable parent). Fall back to the
    // nonce'd name unreserved; the caller's write will surface the error.
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_extension(format!("{extension}-{}-{nonce}", std::process::id()))
}

fn try_reserve(path: &Path) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_reservation_takes_the_plain_candidate_and_creates_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("store.json");
        let backup = reserve_unique_backup_path(&source, "json.bak");
        assert_eq!(backup, dir.path().join("store.json.bak"));
        assert!(backup.exists(), "placeholder must be created atomically");
    }

    #[test]
    fn second_reservation_gets_a_distinct_reserved_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("store.json");
        let first = reserve_unique_backup_path(&source, "json.bak");
        let second = reserve_unique_backup_path(&source, "json.bak");
        assert_ne!(first, second);
        assert!(second.exists(), "nonce placeholder must be created too");
    }

    #[test]
    fn rename_over_own_placeholder_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("store.json");
        std::fs::write(&source, b"corrupt").unwrap();
        let backup = reserve_unique_backup_path(&source, "json.bak");
        std::fs::rename(&source, &backup).unwrap();
        assert_eq!(std::fs::read(&backup).unwrap(), b"corrupt");
    }
}
