//! Headless browser-data import engine for ForkTTY. Discovers installed browsers,
//! reads + decrypts their cookies, reads history + bookmarks, and resolves a
//! source→destination import plan. No GTK / no webkit; everything here is unit-
//! testable headless. See `docs/superpowers/specs/2026-05-24-browser-pane-sp3-design.md`.

pub mod cookies;
pub mod history;
pub mod model;
pub mod plan;
pub mod sources;

pub use cookies::{
    chromium_key_from_password, chromium_v10_key, chromium_v11_key, decrypt_chromium_value,
    read_chromium_cookies, read_chromium_cookies_with_keys, read_firefox_cookies,
    ChromiumCookieKeys,
};
pub use history::{
    read_chromium_bookmarks, read_chromium_history, read_firefox_bookmarks, read_firefox_history,
};
pub use model::*;
pub use plan::{resolve_default_plan, resolve_separate_profiles_plan};
pub use sources::{discover, discover_chromium_family, discover_firefox};

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// All data read from a single source profile.
pub struct ImportedData {
    pub cookies: Vec<ImportedCookie>,
    pub visits: Vec<ImportedVisit>,
    pub bookmarks: Vec<ImportedBookmark>,
    pub result: ImportResult,
}

/// Which source data types the engine should read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImportReadSelection {
    pub cookies: bool,
    pub history: bool,
    pub bookmarks: bool,
}

impl ImportReadSelection {
    pub const fn all() -> Self {
        Self {
            cookies: true,
            history: true,
            bookmarks: true,
        }
    }
}

impl Default for ImportReadSelection {
    fn default() -> Self {
        Self::all()
    }
}

/// Engine error type.
#[derive(Debug)]
pub enum ImportError {
    Io(String),
    Db(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Io(msg) => write!(f, "import I/O error: {msg}"),
            ImportError::Db(msg) => write!(f, "import database error: {msg}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<rusqlite::Error> for ImportError {
    fn from(e: rusqlite::Error) -> Self {
        ImportError::Db(e.to_string())
    }
}

impl From<io::Error> for ImportError {
    fn from(e: io::Error) -> Self {
        ImportError::Io(e.to_string())
    }
}

/// Copies a sqlite database, including WAL sidecars when present. The copy avoids
/// the exclusive lock a running browser can hold while still seeing recent rows
/// that have not checkpointed from `-wal` into the main database file.
///
/// If `src` is absent, returns `Ok(default)` (missing file → treat as empty).
fn read_via_copy<T, F>(src: &Path, default: T, f: F) -> Result<T, ImportError>
where
    F: FnOnce(&Path) -> Result<T, ImportError>,
{
    if !src.exists() {
        return Ok(default);
    }
    let tmp = tempfile::NamedTempFile::new().map_err(ImportError::from)?;
    std::fs::copy(src, tmp.path()).map_err(ImportError::from)?;
    let copied_sidecars = copy_sqlite_sidecars(src, tmp.path())?;
    let result = f(tmp.path());
    for sidecar in copied_sidecars {
        let _ = std::fs::remove_file(sidecar);
    }
    result
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn copy_sqlite_sidecars(src: &Path, dst: &Path) -> Result<Vec<PathBuf>, ImportError> {
    let mut copied = Vec::new();
    for suffix in ["-wal", "-shm"] {
        let src_sidecar = sqlite_sidecar_path(src, suffix);
        if src_sidecar.exists() {
            let dst_sidecar = sqlite_sidecar_path(dst, suffix);
            std::fs::copy(&src_sidecar, &dst_sidecar).map_err(ImportError::from)?;
            copied.push(dst_sidecar);
        }
    }
    Ok(copied)
}

/// Read Firefox bookmarks, treating a missing `moz_bookmarks` table as empty.
///
/// A minimal or just-created Firefox profile has `moz_places` but no `moz_bookmarks`
/// table yet. That specific error is suppressed and returns `Ok(vec![])`. All other
/// rusqlite errors (corrupt DB, I/O failure) are propagated.
fn firefox_bookmarks_or_empty(p: &Path) -> Result<Vec<ImportedBookmark>, ImportError> {
    match read_firefox_bookmarks(p) {
        Ok(v) => Ok(v),
        // A minimal/just-created Firefox profile has no moz_bookmarks table yet.
        Err(e) if e.to_string().contains("no such table") => Ok(Vec::new()),
        Err(e) => Err(ImportError::from(e)),
    }
}

/// Headless import engine. Reads a source profile into in-memory structs.
pub struct ImportEngine;

impl ImportEngine {
    /// Read all importable data from `profile`. Missing files yield empty collections,
    /// not errors. A running browser that locks its SQLite files is handled by copying
    /// the DB to a temporary file before opening it.
    pub fn read_source(profile: &SourceProfile) -> Result<ImportedData, ImportError> {
        Self::read_source_with_selection(profile, ImportReadSelection::all())
    }

    /// Read only the selected importable data from `profile`.
    pub fn read_source_with_selection(
        profile: &SourceProfile,
        selection: ImportReadSelection,
    ) -> Result<ImportedData, ImportError> {
        let base = Path::new(&profile.path);

        if profile.family.is_chromium() {
            Self::read_chromium(base, &ChromiumCookieKeys::v10_only(), selection)
        } else {
            Self::read_firefox(base, selection)
        }
    }

    /// Async import entrypoint. Chromium-family profiles use the Secret Service
    /// keyring path for v11 cookies when available, then read SQLite files via
    /// the same copied-database path as the synchronous API.
    pub async fn read_source_async(profile: &SourceProfile) -> Result<ImportedData, ImportError> {
        Self::read_source_async_with_selection(profile, ImportReadSelection::all()).await
    }

    /// Async import entrypoint that reads only the selected data types.
    pub async fn read_source_async_with_selection(
        profile: &SourceProfile,
        selection: ImportReadSelection,
    ) -> Result<ImportedData, ImportError> {
        let base = Path::new(&profile.path);

        if profile.family.is_chromium() {
            let keys = if selection.cookies {
                ChromiumCookieKeys::for_family(profile.family).await
            } else {
                ChromiumCookieKeys::v10_only()
            };
            Self::read_chromium(base, &keys, selection)
        } else {
            Self::read_firefox(base, selection)
        }
    }

    fn read_firefox(
        base: &Path,
        selection: ImportReadSelection,
    ) -> Result<ImportedData, ImportError> {
        let cookies_path = base.join("cookies.sqlite");
        let places_path = base.join("places.sqlite");

        let cookies = if selection.cookies {
            read_via_copy(&cookies_path, vec![], |p| {
                read_firefox_cookies(p).map_err(ImportError::from)
            })?
        } else {
            vec![]
        };

        let visits = if selection.history {
            read_via_copy(&places_path, vec![], |p| {
                read_firefox_history(p).map_err(ImportError::from)
            })?
        } else {
            vec![]
        };

        // Bookmarks share places.sqlite but the table may be absent in minimal DBs → treat as empty.
        // Corrupt DB or other real errors are propagated (not swallowed).
        let bookmarks = if selection.bookmarks {
            read_via_copy(&places_path, vec![], firefox_bookmarks_or_empty)?
        } else {
            vec![]
        };

        let result = ImportResult {
            cookies: cookies.len(),
            history: visits.len(),
            bookmarks: bookmarks.len(),
            skipped: 0,
        };

        Ok(ImportedData {
            cookies,
            visits,
            bookmarks,
            result,
        })
    }

    fn read_chromium(
        base: &Path,
        cookie_keys: &ChromiumCookieKeys,
        selection: ImportReadSelection,
    ) -> Result<ImportedData, ImportError> {
        let cookies_path = base.join("Cookies");
        let history_path = base.join("History");
        let bookmarks_path = base.join("Bookmarks");

        let (cookies, skipped) = if selection.cookies {
            read_via_copy(&cookies_path, (vec![], 0usize), |p| {
                read_chromium_cookies_with_keys(p, cookie_keys).map_err(ImportError::from)
            })?
        } else {
            (vec![], 0)
        };

        let visits = if selection.history {
            read_via_copy(&history_path, vec![], |p| {
                read_chromium_history(p).map_err(ImportError::from)
            })?
        } else {
            vec![]
        };

        // Bookmarks JSON: Chromium writes `Bookmarks` atomically via temp-file + rename,
        // so reading it directly is safe — no partial-read or lock risk. Missing file → empty.
        let bookmarks = if selection.bookmarks {
            if bookmarks_path.exists() {
                read_chromium_bookmarks(&bookmarks_path).map_err(ImportError::from)?
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let result = ImportResult {
            cookies: cookies.len(),
            history: visits.len(),
            bookmarks: bookmarks.len(),
            skipped,
        };

        Ok(ImportedData {
            cookies,
            visits,
            bookmarks,
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firefox_profile_with_data(dir: &std::path::Path) -> SourceProfile {
        let conn = rusqlite::Connection::open(dir.join("cookies.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies (name TEXT,value TEXT,host TEXT,path TEXT,expiry INTEGER,isSecure INTEGER,isHttpOnly INTEGER);
             INSERT INTO moz_cookies VALUES ('sid','v','.a.test','/',0,0,0);",
        ).unwrap();
        drop(conn);
        let places = rusqlite::Connection::open(dir.join("places.sqlite")).unwrap();
        places.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY,url TEXT,title TEXT,visit_count INTEGER);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('https://a.test/','A',2);",
        ).unwrap();
        drop(places);
        SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: "P".into(),
            path: dir.to_string_lossy().into_owned(),
            is_default: true,
        }
    }

    #[test]
    fn read_source_collects_cookies_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let profile = firefox_profile_with_data(dir.path());
        let data = ImportEngine::read_source(&profile).unwrap();
        assert_eq!(data.cookies.len(), 1);
        assert_eq!(data.visits.len(), 1);
        assert_eq!(data.result.cookies, 1);
        assert_eq!(data.result.history, 1);
    }

    #[test]
    fn read_source_missing_files_yields_empty_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let profile = SourceProfile {
            family: BrowserFamily::Chromium,
            display_name: "empty".into(),
            path: dir.path().to_string_lossy().into_owned(),
            is_default: false,
        };
        let data = ImportEngine::read_source(&profile).unwrap();
        assert!(data.cookies.is_empty());
        assert_eq!(data.result.cookies, 0);
    }

    #[test]
    fn read_source_selection_skips_unselected_corrupt_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cookies.sqlite"), b"not a sqlite database").unwrap();
        let places = rusqlite::Connection::open(dir.path().join("places.sqlite")).unwrap();
        places
            .execute_batch(
                "CREATE TABLE moz_places (id INTEGER PRIMARY KEY,url TEXT,title TEXT,visit_count INTEGER);
                 INSERT INTO moz_places (url,title,visit_count) VALUES ('https://a.test/','A',2);",
            )
            .unwrap();
        drop(places);
        let profile = SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: "P".into(),
            path: dir.path().to_string_lossy().into_owned(),
            is_default: true,
        };

        assert!(ImportEngine::read_source(&profile).is_err());

        let data = ImportEngine::read_source_with_selection(
            &profile,
            ImportReadSelection {
                cookies: false,
                history: true,
                bookmarks: false,
            },
        )
        .unwrap();
        assert!(data.cookies.is_empty());
        assert!(data.bookmarks.is_empty());
        assert_eq!(data.visits.len(), 1);
        assert_eq!(data.result.history, 1);
        assert_eq!(data.result.cookies, 0);
    }

    #[test]
    fn read_source_missing_bookmarks_table_is_empty_but_corrupt_db_errors() {
        // places.sqlite with moz_places but NO moz_bookmarks → empty bookmarks, ok.
        let dir = tempfile::tempdir().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("places.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY,url TEXT,title TEXT,visit_count INTEGER);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('https://a.test/','A',1);",
        )
        .unwrap();
        drop(conn);
        let profile = SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: "P".into(),
            path: dir.path().to_string_lossy().into_owned(),
            is_default: true,
        };
        let data = ImportEngine::read_source(&profile).unwrap();
        assert!(
            data.bookmarks.is_empty(),
            "missing moz_bookmarks table should yield empty bookmarks"
        );
        assert_eq!(data.visits.len(), 1, "history should still be read");

        // A corrupt places.sqlite must NOT be silently swallowed — it must error.
        let corrupt_dir = tempfile::tempdir().unwrap();
        std::fs::write(
            corrupt_dir.path().join("places.sqlite"),
            b"not a sqlite file",
        )
        .unwrap();
        let corrupt_profile = SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: "corrupt".into(),
            path: corrupt_dir.path().to_string_lossy().into_owned(),
            is_default: false,
        };
        let result = ImportEngine::read_source(&corrupt_profile);
        assert!(
            result.is_err(),
            "corrupt places.sqlite should propagate an error, not return empty"
        );
    }

    #[test]
    fn read_via_copy_includes_wal_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch(
            "CREATE TABLE urls (url TEXT, title TEXT, visit_count INTEGER);
             INSERT INTO urls VALUES ('https://wal.test/','WAL',7);",
        )
        .unwrap();
        assert!(sqlite_sidecar_path(&db, "-wal").exists());

        let visits = read_via_copy(&db, vec![], |p| {
            read_chromium_history(p).map_err(ImportError::from)
        })
        .unwrap();
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://wal.test/");
        assert_eq!(visits[0].visit_count, 7);

        drop(conn);
    }
}
