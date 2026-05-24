//! Persistent per-profile WebKit network sessions for browser panes (SP3 P1).
//! Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use std::path::{Path, PathBuf};

/// Well-known directory id for the Default profile. A fixed UUID string so that
/// SP3 P2's real `ProfileId::default()` resolves to this same on-disk directory
/// (no migration). P1 has no profile system, so this is the only profile used.
#[allow(dead_code)] // used in SP3 P1 Task 2
pub const DEFAULT_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// On-disk locations for one profile's browser data.
#[allow(dead_code)] // used in SP3 P1 Task 2
pub struct ProfileDirs {
    pub base: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub cookies_sqlite: PathBuf,
}

impl ProfileDirs {
    /// Compute the directory layout for `profile_id` under a data root
    /// (`<root>/browser_profiles/<id>/…`). Pure; creates nothing.
    #[allow(dead_code)] // used in SP3 P1 Task 2
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
#[allow(dead_code)] // used in SP3 P1 Task 2
pub fn data_root() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("forktty"))
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
