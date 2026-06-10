//! Browser-pane history + bookmarks (SP3 P3). Pure (no GTK/webkit): both the GTK
//! main thread (visit recording, address-bar completion) and the socket thread
//! (history/bookmark verbs) use these stores against the same per-profile files.
//! sqlite runs in WAL mode so a concurrent reader and writer do not block.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::backup::reserve_unique_backup_path;
use crate::profile::ProfileId;

const MAX_BOOKMARKS_JSON_BYTES: u64 = 16 * 1024 * 1024;

/// One visited URL with its aggregate visit metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
    /// Unix microseconds of the most recent visit.
    pub last_visit_us: i64,
}

/// Errors from the history/bookmark stores.
#[derive(Debug)]
pub enum HistoryError {
    Io(String),
    Db(String),
}

impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HistoryError::Io(e) => write!(f, "history io error: {e}"),
            HistoryError::Db(e) => write!(f, "history db error: {e}"),
        }
    }
}
impl std::error::Error for HistoryError {}

impl From<rusqlite::Error> for HistoryError {
    fn from(e: rusqlite::Error) -> Self {
        HistoryError::Db(e.to_string())
    }
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn is_recoverable_sqlite_error(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(
                err.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            )
    )
}

fn backup_corrupt_sqlite(path: &Path) -> Result<(), HistoryError> {
    let backup = reserve_unique_backup_path(path, "sqlite.bak");
    std::fs::rename(path, &backup).map_err(|e| HistoryError::Io(e.to_string()))?;
    for suffix in ["-wal", "-shm"] {
        let sidecar = sqlite_sidecar_path(path, suffix);
        if sidecar.exists() {
            let _ = std::fs::remove_file(sidecar);
        }
    }
    Ok(())
}

fn now_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// `~/.local/share/forktty/browser_profiles/<id>/history.sqlite` for the given profile.
/// `None` if the platform has no data dir.
pub fn history_path(profile: ProfileId) -> Option<PathBuf> {
    Some(profile_dir(profile)?.join("history.sqlite"))
}

/// `~/.local/share/forktty/browser_profiles/<id>/bookmarks.json` for the given profile.
pub fn bookmarks_path(profile: ProfileId) -> Option<PathBuf> {
    Some(profile_dir(profile)?.join("bookmarks.json"))
}

fn profile_dir(profile: ProfileId) -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| {
        d.join("forktty")
            .join("browser_profiles")
            .join(profile.to_string())
    })
}

/// Per-profile visited-URL history backed by sqlite.
pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    /// Open (creating + migrating) `history.sqlite` at `path`, ensuring the parent dir.
    pub fn open(path: &Path) -> Result<Self, HistoryError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HistoryError::Io(e.to_string()))?;
        }
        match Self::open_sqlite(path) {
            Ok(store) => Ok(store),
            Err(e) if path.exists() && is_recoverable_sqlite_error(&e) => {
                backup_corrupt_sqlite(path)?;
                Self::open_sqlite(path).map_err(HistoryError::from)
            }
            Err(e) => Err(HistoryError::from(e)),
        }
    }

    fn open_sqlite(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_sqlite(conn)
    }

    /// Open the history store for `profile` at its default path.
    pub fn for_profile(profile: ProfileId) -> Result<Self, HistoryError> {
        let path = history_path(profile)
            .ok_or_else(|| HistoryError::Io("no data directory on this platform".to_string()))?;
        Self::open(&path)
    }

    /// In-memory store for tests.
    pub fn open_in_memory() -> Result<Self, HistoryError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, HistoryError> {
        Self::init_sqlite(conn).map_err(HistoryError::from)
    }

    fn init_sqlite(conn: Connection) -> rusqlite::Result<Self> {
        // WAL so a socket-side reader and a GTK-side writer coexist. Ignored for
        // in-memory connections, which is fine.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS visits (
                 url            TEXT PRIMARY KEY,
                 title          TEXT NOT NULL DEFAULT '',
                 visit_count    INTEGER NOT NULL DEFAULT 0,
                 last_visit_us  INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_visits_last_visit
                 ON visits(last_visit_us DESC);",
        )?;
        Ok(Self { conn })
    }

    /// Record a visit: insert new, or bump `visit_count` and `last_visit_us` for an
    /// existing url. A non-empty `title` overwrites the stored one; an empty title
    /// leaves the existing title intact.
    pub fn record_visit(&self, url: &str, title: &str) -> Result<(), HistoryError> {
        self.conn.execute(
            "INSERT INTO visits (url, title, visit_count, last_visit_us)
                 VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(url) DO UPDATE SET
                 visit_count   = visit_count + 1,
                 last_visit_us = ?3,
                 title         = CASE WHEN ?2 <> '' THEN ?2 ELSE title END",
            rusqlite::params![url, title, now_us()],
        )?;
        Ok(())
    }

    /// Update the stored title for an already-recorded visit without incrementing
    /// its visit count.
    pub fn update_title(&self, url: &str, title: &str) -> Result<(), HistoryError> {
        if title.is_empty() {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE visits SET title = ?2 WHERE url = ?1",
            rusqlite::params![url, title],
        )?;
        Ok(())
    }

    /// Import an aggregate visit row from another browser. Re-importing the same
    /// source keeps the highest visit count instead of incrementing endlessly.
    pub fn import_visit(
        &self,
        url: &str,
        title: &str,
        visit_count: i64,
    ) -> Result<(), HistoryError> {
        let count = visit_count.max(1);
        self.conn.execute(
            "INSERT INTO visits (url, title, visit_count, last_visit_us)
                 VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(url) DO UPDATE SET
                 visit_count   = MAX(visit_count, ?3),
                 last_visit_us = ?4,
                 title         = CASE WHEN ?2 <> '' THEN ?2 ELSE title END",
            rusqlite::params![url, title, count, now_us()],
        )?;
        Ok(())
    }

    /// Most-recently-visited rows first, capped at `limit`.
    pub fn list(&self, limit: usize) -> Result<Vec<Visit>, HistoryError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT url, title, visit_count, last_visit_us
                 FROM visits ORDER BY last_visit_us DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit as i64], Self::row_to_visit)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Case-insensitive substring match on url or title, most recent first, capped.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Visit>, HistoryError> {
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let like = format!("%{escaped}%");
        let mut stmt = self.conn.prepare_cached(
            "SELECT url, title, visit_count, last_visit_us
                 FROM visits
                 WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\'
                 ORDER BY last_visit_us DESC LIMIT ?2",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![like, limit as i64], Self::row_to_visit)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all history rows.
    pub fn clear(&self) -> Result<(), HistoryError> {
        self.conn.execute("DELETE FROM visits", [])?;
        Ok(())
    }

    fn row_to_visit(row: &rusqlite::Row<'_>) -> rusqlite::Result<Visit> {
        Ok(Visit {
            url: row.get(0)?,
            title: row.get(1)?,
            visit_count: row.get(2)?,
            last_visit_us: row.get(3)?,
        })
    }
}

/// A saved bookmark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
    /// Unix seconds when added.
    pub added_at: i64,
}

/// Per-profile bookmarks backed by a JSON array file. Loaded fully into memory;
/// every mutation rewrites the file atomically (temp + rename).
pub struct BookmarkStore {
    path: PathBuf,
    items: Vec<Bookmark>,
}

impl BookmarkStore {
    /// Open `bookmarks.json` at `path`. A missing file yields an empty store. A file
    /// that fails to parse is backed up to `<path>.bak` and the store starts empty
    /// (navigation must never break on corrupt bookmarks).
    pub fn open(path: &Path) -> Result<Self, HistoryError> {
        let items = match read_optional_file_bounded(path, MAX_BOOKMARKS_JSON_BYTES, "bookmarks")? {
            Some(bytes) => match serde_json::from_slice::<Vec<Bookmark>>(&bytes) {
                Ok(v) => v,
                Err(_) => {
                    let bak = reserve_unique_backup_path(path, "json.bak");
                    let _ = std::fs::write(&bak, &bytes);
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        Ok(Self {
            path: path.to_path_buf(),
            items,
        })
    }

    /// Open the bookmark store for `profile` at its default path.
    pub fn for_profile(profile: ProfileId) -> Result<Self, HistoryError> {
        let path = bookmarks_path(profile)
            .ok_or_else(|| HistoryError::Io("no data directory on this platform".to_string()))?;
        Self::open(&path)
    }

    pub fn list(&self) -> &[Bookmark] {
        &self.items
    }

    /// Add a bookmark, or update the title if the url already exists. Persists.
    pub fn add(&mut self, url: &str, title: &str) -> Result<(), HistoryError> {
        if let Some(existing) = self.items.iter_mut().find(|b| b.url == url) {
            existing.title = title.to_string();
        } else {
            self.items.push(Bookmark {
                url: url.to_string(),
                title: title.to_string(),
                added_at: now_us() / 1_000_000,
            });
        }
        self.save()
    }

    /// Remove the bookmark with `url`. Returns whether one was removed. Persists.
    pub fn remove(&mut self, url: &str) -> Result<bool, HistoryError> {
        let before = self.items.len();
        self.items.retain(|b| b.url != url);
        let removed = self.items.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<(), HistoryError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| HistoryError::Io(e.to_string()))?;
        }
        let bytes =
            serde_json::to_vec_pretty(&self.items).map_err(|e| HistoryError::Io(e.to_string()))?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = self
            .path
            .with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
        let result = (|| -> Result<(), HistoryError> {
            let mut tmp_file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(|e| HistoryError::Io(e.to_string()))?;
            std::io::Write::write_all(&mut tmp_file, &bytes)
                .map_err(|e| HistoryError::Io(e.to_string()))?;
            tmp_file
                .sync_all()
                .map_err(|e| HistoryError::Io(e.to_string()))?;
            std::fs::rename(&tmp, &self.path).map_err(|e| HistoryError::Io(e.to_string()))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

fn read_optional_file_bounded(
    path: &Path,
    limit: u64,
    label: &str,
) -> Result<Option<Vec<u8>>, HistoryError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(HistoryError::Io(err.to_string())),
    };
    if metadata.len() > limit {
        return Err(HistoryError::Io(format!(
            "{label} file is {} bytes, exceeds limit of {limit} bytes",
            metadata.len()
        )));
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|err| HistoryError::Io(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> HistoryStore {
        // In-memory db keeps tests headless and fast.
        HistoryStore::open_in_memory().expect("open in-memory history")
    }

    #[test]
    fn record_visit_inserts_then_increments() {
        let h = store();
        h.record_visit("https://a.test/", "A").unwrap();
        h.record_visit("https://a.test/", "A v2").unwrap();
        let rows = h.list(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://a.test/");
        assert_eq!(rows[0].visit_count, 2);
        // Latest non-empty title wins.
        assert_eq!(rows[0].title, "A v2");
    }

    #[test]
    fn record_visit_keeps_existing_title_when_new_is_empty() {
        let h = store();
        h.record_visit("https://a.test/", "Real Title").unwrap();
        h.record_visit("https://a.test/", "").unwrap();
        let rows = h.list(10).unwrap();
        assert_eq!(rows[0].title, "Real Title");
        assert_eq!(rows[0].visit_count, 2);
    }

    #[test]
    fn update_title_does_not_increment_visit_count() {
        let h = store();
        h.record_visit("https://a.test/", "").unwrap();
        h.update_title("https://a.test/", "Real Title").unwrap();
        let rows = h.list(10).unwrap();
        assert_eq!(rows[0].title, "Real Title");
        assert_eq!(rows[0].visit_count, 1);
    }

    #[test]
    fn import_visit_is_idempotent_by_url() {
        let h = store();
        h.import_visit("https://a.test/", "Imported", 7).unwrap();
        h.import_visit("https://a.test/", "Imported Again", 7)
            .unwrap();
        let rows = h.list(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].visit_count, 7);
        assert_eq!(rows[0].title, "Imported Again");
    }

    #[test]
    fn list_orders_by_last_visit_desc() {
        let h = store();
        h.record_visit("https://first.test/", "1").unwrap();
        h.record_visit("https://second.test/", "2").unwrap();
        h.record_visit("https://first.test/", "1").unwrap(); // bump first
        let rows = h.list(10).unwrap();
        assert_eq!(rows[0].url, "https://first.test/");
        assert_eq!(rows[1].url, "https://second.test/");
    }

    #[test]
    fn list_respects_limit() {
        let h = store();
        for i in 0..5 {
            h.record_visit(&format!("https://x.test/{i}"), "x").unwrap();
        }
        assert_eq!(h.list(3).unwrap().len(), 3);
    }

    #[test]
    fn search_matches_url_or_title_substring_case_insensitive() {
        let h = store();
        h.record_visit("https://github.com/rust", "Rust Lang")
            .unwrap();
        h.record_visit("https://example.com/", "Example").unwrap();
        let by_url = h.search("GITHUB", 10).unwrap();
        assert_eq!(by_url.len(), 1);
        assert_eq!(by_url[0].url, "https://github.com/rust");
        let by_title = h.search("exampl", 10).unwrap();
        assert_eq!(by_title.len(), 1);
        assert_eq!(by_title[0].url, "https://example.com/");
    }

    #[test]
    fn clear_removes_all_rows() {
        let h = store();
        h.record_visit("https://a.test/", "A").unwrap();
        h.clear().unwrap();
        assert!(h.list(10).unwrap().is_empty());
    }

    #[test]
    fn open_creates_file_and_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        {
            let h = HistoryStore::open(&path).unwrap();
            h.record_visit("https://persist.test/", "P").unwrap();
        }
        let h2 = HistoryStore::open(&path).unwrap();
        assert_eq!(h2.list(10).unwrap().len(), 1);
    }

    #[test]
    fn open_recovers_corrupt_history_db_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        std::fs::write(&path, b"not a sqlite database").unwrap();

        let h = HistoryStore::open(&path).unwrap();
        assert!(h.list(10).unwrap().is_empty());
        assert!(path.with_extension("sqlite.bak").exists());

        h.record_visit("https://recovered.test/", "Recovered")
            .unwrap();
        let h2 = HistoryStore::open(&path).unwrap();
        let rows = h2.list(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://recovered.test/");
    }

    #[test]
    fn search_handles_backslash_query_without_error() {
        let h = store();
        h.record_visit("https://a.test/path", "Title").unwrap();
        // A backslash in the query must not blow up the LIKE pattern.
        assert!(h.search("a\\b", 10).unwrap().is_empty());
        assert_eq!(h.search("a.test", 10).unwrap().len(), 1);
    }

    fn bm_store() -> (tempfile::TempDir, BookmarkStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        (dir, BookmarkStore::open(&path).unwrap())
    }

    #[test]
    fn bookmark_add_then_list() {
        let (_d, mut b) = bm_store();
        b.add("https://a.test/", "A").unwrap();
        let all = b.list();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].url, "https://a.test/");
        assert_eq!(all[0].title, "A");
    }

    #[test]
    fn bookmark_add_dedupes_by_url_updating_title() {
        let (_d, mut b) = bm_store();
        b.add("https://a.test/", "Old").unwrap();
        b.add("https://a.test/", "New").unwrap();
        assert_eq!(b.list().len(), 1);
        assert_eq!(b.list()[0].title, "New");
    }

    #[test]
    fn bookmark_remove() {
        let (_d, mut b) = bm_store();
        b.add("https://a.test/", "A").unwrap();
        assert!(b.remove("https://a.test/").unwrap());
        assert!(b.list().is_empty());
        // Removing a missing url returns false, not an error.
        assert!(!b.remove("https://nope.test/").unwrap());
    }

    #[test]
    fn bookmark_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        {
            let mut b = BookmarkStore::open(&path).unwrap();
            b.add("https://persist.test/", "P").unwrap();
        }
        let b2 = BookmarkStore::open(&path).unwrap();
        assert_eq!(b2.list().len(), 1);
    }

    #[test]
    fn bookmark_malformed_file_starts_fresh_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        std::fs::write(&path, b"{ this is not valid json").unwrap();
        let b = BookmarkStore::open(&path).unwrap();
        assert!(b.list().is_empty());
        // Original bytes preserved alongside as a .bak.
        let bak = path.with_extension("json.bak");
        assert!(bak.exists());
    }

    #[test]
    fn bookmark_malformed_backup_does_not_overwrite_existing_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        let first_backup = path.with_extension("json.bak");
        std::fs::write(&path, b"{ this is not valid json").unwrap();
        std::fs::write(&first_backup, b"previous backup").unwrap();

        let b = BookmarkStore::open(&path).unwrap();
        assert!(b.list().is_empty());
        assert_eq!(std::fs::read(&first_backup).unwrap(), b"previous backup");

        let backup_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("bookmarks.json.bak")
            })
            .count();
        assert_eq!(backup_count, 2);
    }

    #[test]
    fn bookmark_open_rejects_oversized_file_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bookmarks.json");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_BOOKMARKS_JSON_BYTES + 1).unwrap();

        let err = match BookmarkStore::open(&path) {
            Ok(_) => panic!("oversized bookmarks should be rejected"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exceeds limit"));
    }
}
