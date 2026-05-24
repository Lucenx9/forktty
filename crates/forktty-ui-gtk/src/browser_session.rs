//! Persistent per-profile WebKit network sessions for browser panes (SP3 P1).
//! Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use webkit6::{CookiePersistentStorage, NetworkSession};

/// Well-known directory id for the Default profile. A fixed UUID string so that
/// SP3 P2's real `ProfileId::default()` resolves to this same on-disk directory
/// (no migration). P1 has no profile system, so this is the only profile used.
pub const DEFAULT_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";

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
/// (see `cli.rs` `dirs::data_dir().join("forktty")`). `None` if the platform has no
/// data dir.
pub fn data_root() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("forktty"))
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
    std::fs::create_dir_all(&dirs.data)
        .map_err(|e| {
            eprintln!(
                "forktty: cannot create browser data dir {:?}: {e}",
                dirs.data
            )
        })
        .ok()?;
    std::fs::create_dir_all(&dirs.cache)
        .map_err(|e| {
            eprintln!(
                "forktty: cannot create browser cache dir {:?}: {e}",
                dirs.cache
            )
        })
        .ok()?;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
