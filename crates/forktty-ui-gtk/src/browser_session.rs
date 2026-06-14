//! Persistent per-profile WebKit network sessions for browser panes (SP3 P1).
//! Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use webkit6::{CookiePersistentStorage, NetworkSession};

/// Well-known directory id for the Default profile. A fixed UUID string so that
/// SP3 P2's real `ProfileId::default()` resolves to this same on-disk directory
/// (no migration). P1 has no profile system, so this is the only profile used.
/// Used by tests in `browser_pane` and `browser_session` to construct sessions
/// with the default profile without pulling in `forktty_core`.
#[allow(dead_code)]
pub const DEFAULT_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Whether `id` is safe to use as a single on-disk directory name. Rejects empty
/// ids, anything longer than 64 bytes, and anything outside `[A-Za-z0-9_-]` — in
/// particular `/`, `\`, and `.`, which could traverse outside the profiles root.
fn is_valid_profile_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// On-disk locations for one profile's browser data.
pub struct ProfileDirs {
    #[allow(dead_code)] // retained for SP3 P2 profile management
    pub base: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub cookies_sqlite: PathBuf,
}

impl ProfileDirs {
    /// Compute the directory layout for `profile_id` under a data root
    /// (`<root>/browser_profiles/<id>/…`). Pure; creates nothing.
    pub fn under(data_root: &Path, profile_id: &str) -> Self {
        let base = data_root.join("browser_profiles").join(profile_id);
        let data = base.join("data");
        let cache = base.join("cache");
        let cookies_sqlite = base.join("cookies.sqlite");
        Self {
            base,
            data,
            cache,
            cookies_sqlite,
        }
    }
}

/// The forktty data root (`~/.local/share/forktty`), matching the rest of the app
/// (see `session.rs` `dirs::data_local_dir().join("forktty")`). `None` if the platform has no
/// data dir.
pub fn data_root() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join("forktty"))
}

// Note (P2): to evict a session, close it first to release its data-dir lock.
thread_local! {
    /// One persistent NetworkSession per profile id, reused across all panes on
    /// that profile. Two persistent sessions over the same data dir would conflict,
    /// so this cache is the single owner. GTK main thread only.
    static SESSIONS: RefCell<HashMap<String, NetworkSession>> = RefCell::new(HashMap::new());
}

/// Return a persistent `NetworkSession` for `profile_id`, creating and caching it on
/// first use. Falls back to an ephemeral session (logging a warning) if the data root
/// is unavailable or its directories cannot be created — the pane still works, just
/// without persistence for that run.
pub fn session_for(profile_id: &str) -> NetworkSession {
    if let Some(existing) = SESSIONS.with(|c| c.borrow().get(profile_id).cloned()) {
        return existing;
    }

    if !is_valid_profile_id(profile_id) {
        eprintln!(
            "forktty: invalid browser profile id {profile_id:?}; \
             using an ephemeral session this run"
        );
        return NetworkSession::new_ephemeral();
    }

    let session = build_persistent_session(profile_id).unwrap_or_else(|| {
        eprintln!(
            "forktty: browser profile '{profile_id}' has no persistent storage; \
             using an ephemeral session this run"
        );
        NetworkSession::new_ephemeral()
    });

    SESSIONS.with(|c| {
        c.borrow_mut()
            .insert(profile_id.to_string(), session.clone())
    });
    session
}

/// Build a persistent session rooted at the profile's data/cache dirs with sqlite
/// cookie storage. Returns `None` if directories can't be prepared.
fn build_persistent_session(profile_id: &str) -> Option<NetworkSession> {
    let root = data_root()?;
    let dirs = ProfileDirs::under(&root, profile_id);
    prepare_profile_dirs(&dirs).ok()?;

    let session = NetworkSession::new(Some(dirs.data.to_str()?), Some(dirs.cache.to_str()?));
    match (session.cookie_manager(), dirs.cookies_sqlite.to_str()) {
        (Some(cookie_manager), Some(cookies_path)) => {
            cookie_manager.set_persistent_storage(cookies_path, CookiePersistentStorage::Sqlite);
        }
        _ => eprintln!(
            "forktty: no persistent cookie storage for profile '{profile_id}'; \
             cookies will not be saved"
        ),
    }
    Some(session)
}

/// Create the profile directory tree and make it private before WebKit writes
/// cookies, local storage, or cache entries into it.
fn prepare_profile_dirs(dirs: &ProfileDirs) -> std::io::Result<()> {
    create_private_dir_all(&dirs.data).map_err(|e| {
        eprintln!(
            "forktty: cannot create private browser data dir {:?}: {e}",
            dirs.data
        );
        e
    })?;
    create_private_dir_all(&dirs.cache).map_err(|e| {
        eprintln!(
            "forktty: cannot create private browser cache dir {:?}: {e}",
            dirs.cache
        );
        e
    })?;
    harden_existing_cookie_store(&dirs.cookies_sqlite).map_err(|e| {
        eprintln!(
            "forktty: cannot make browser cookie store private {:?}: {e}",
            dirs.cookies_sqlite
        );
        e
    })?;
    Ok(())
}

#[cfg(unix)]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    let dirs = private_profile_dirs(path);
    if let Some(parent) = dirs.first().and_then(|dir| dir.parent()) {
        std::fs::create_dir_all(parent)?;
    }
    for dir in dirs {
        ensure_private_dir(dir)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)
}

#[cfg(unix)]
fn harden_existing_cookie_store(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing symlinked browser cookie store {}", path.display()),
            ));
        }
        Ok(meta) if meta.is_file() => {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("browser cookie store is not a file: {}", path.display()),
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    Ok(())
}

#[cfg(not(unix))]
fn harden_existing_cookie_store(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn private_profile_dirs(path: &Path) -> Vec<&Path> {
    path.ancestors()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing symlinked browser profile dir {}", path.display()),
            ));
        }
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "browser profile path is not a directory: {}",
                    path.display()
                ),
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)?;
        }
        Err(err) => return Err(err),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_id_value_is_stable() {
        assert_eq!(DEFAULT_PROFILE_ID, "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn default_profile_id_matches_core_profile_default() {
        assert_eq!(
            DEFAULT_PROFILE_ID,
            forktty_core::ProfileId::default().to_string()
        );
    }

    #[test]
    fn profile_id_validation_accepts_safe_ids_rejects_traversal() {
        assert!(is_valid_profile_id(DEFAULT_PROFILE_ID));
        assert!(is_valid_profile_id("abc"));
        assert!(is_valid_profile_id("work_profile-2"));
        assert!(!is_valid_profile_id(""));
        assert!(!is_valid_profile_id("../evil"));
        assert!(!is_valid_profile_id("a/b"));
        assert!(!is_valid_profile_id("a.b"));
        assert!(!is_valid_profile_id("a\\b"));
        assert!(!is_valid_profile_id(&"x".repeat(65)));
    }

    #[test]
    fn default_profile_dirs_use_the_default_id_as_directory_name() {
        let dirs = ProfileDirs::under(std::path::Path::new("/tmp/ft-data"), DEFAULT_PROFILE_ID);
        assert_eq!(
            dirs.base.file_name().and_then(|s| s.to_str()),
            Some(DEFAULT_PROFILE_ID)
        );
    }

    #[test]
    fn profile_dirs_are_nested_under_profiles_root_by_id() {
        let root = std::path::Path::new("/tmp/ft-data");
        let dirs = ProfileDirs::under(root, "abc");
        assert_eq!(
            dirs.base,
            std::path::Path::new("/tmp/ft-data/browser_profiles/abc")
        );
        assert_eq!(
            dirs.data,
            std::path::Path::new("/tmp/ft-data/browser_profiles/abc/data")
        );
        assert_eq!(
            dirs.cache,
            std::path::Path::new("/tmp/ft-data/browser_profiles/abc/cache")
        );
        assert_eq!(
            dirs.cookies_sqlite,
            std::path::Path::new("/tmp/ft-data/browser_profiles/abc/cookies.sqlite")
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_profile_dirs_include_sensitive_profile_tree_only() {
        let path = std::path::Path::new("/tmp/ft-data/browser_profiles/abc/data");
        assert_eq!(
            private_profile_dirs(path),
            vec![
                std::path::Path::new("/tmp/ft-data/browser_profiles"),
                std::path::Path::new("/tmp/ft-data/browser_profiles/abc"),
                std::path::Path::new("/tmp/ft-data/browser_profiles/abc/data"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_private_dir_all_rejects_symlinked_profile_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let target = tmp.path().join("target");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::os::unix::fs::symlink(&target, root.join("browser_profiles")).unwrap();

        let err = create_private_dir_all(&root.join("browser_profiles/abc/data")).unwrap_err();

        assert!(err.to_string().contains("refusing symlinked"));
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o777
        );
    }

    #[cfg(unix)]
    #[test]
    fn harden_existing_cookie_store_rejects_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("target.sqlite");
        let link = tmp.path().join("cookies.sqlite");
        std::fs::write(&target, b"cookie").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o666)).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = harden_existing_cookie_store(&link).unwrap_err();

        assert!(err.to_string().contains("refusing symlinked"));
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o666
        );
    }
}
