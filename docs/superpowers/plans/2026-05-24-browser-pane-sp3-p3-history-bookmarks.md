# Browser pane SP3 P3 — History + Bookmarks Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Tasks 1-4 are implemented
(`browser_history.rs`, `HistoryStore`, `BookmarkStore`, and
`browser.history.*` / `browser.bookmark.*` socket verbs, plus
`forktty browser history|bookmark` CLI mirrors). Task 5 GTK visit
recording/address completion remains pending.

**Goal:** Per-profile visited-URL history and bookmarks, queryable over the socket/CLI and surfaced as browser-pane address-bar completion.

**Architecture:** Pure history/bookmark stores live in `forktty-core` (rusqlite + serde-json, no GTK/webkit) so both the socket thread and the GTK thread share one implementation. The GTK browser pane records visits on `load-changed`/`notify::title` and reads history for address-bar completion. Socket verbs read/write the same on-disk stores directly (sqlite WAL allows concurrent reader + writer).

**Tech Stack:** Rust, `rusqlite` (bundled sqlite, WAL), `serde_json`, GTK4/WebKitGTK6 (visit recording + `EntryCompletion`), existing forktty socket JSON-RPC + CLI.

**Storage layout** (matches spec, under each profile dir `~/.local/share/forktty/browser_profiles/<id>/`):
- `history.sqlite` — table `visits(url TEXT PRIMARY KEY, title TEXT, visit_count INTEGER, last_visit_us INTEGER)`
- `bookmarks.json` — `[{ "url", "title", "added_at" }]`

**Feature gating note:** The stores are pure (no webkit), so — like P2's `ProfileStore` — they compile unconditionally in core. The socket history/bookmark verbs and CLI mirrors are wired in current `main` and do not require the `browser` feature. Visit recording and `EntryCompletion` (Task 5) remain GTK/`browser`-gated.

---

## File structure

- **Create** `crates/forktty-core/src/browser_history.rs` — `HistoryStore` (rusqlite) + `BookmarkStore` (json) + `Visit`/`Bookmark` types + per-profile path helpers.
- **Modify** `crates/forktty-core/Cargo.toml` — add `rusqlite` dep.
- **Modify** `crates/forktty-core/src/lib.rs` — `pub mod browser_history;` + re-exports.
- **Modify** `crates/forktty-socket/src/lib.rs` — `browser.history.*` + `browser.bookmark.*` verb arms; `METHODS` entries.
- **Modify** `crates/forktty-ui-gtk/src/socket_cli.rs` — `browser history|bookmark` CLI dispatch + HELP_TEXT.
- **Modify** `crates/forktty-ui-gtk/src/browser_pane.rs` — visit recording + address-bar `EntryCompletion`.

---

## Task 1: core `HistoryStore` (rusqlite)

**Files:**
- Create: `crates/forktty-core/src/browser_history.rs`
- Modify: `crates/forktty-core/Cargo.toml`
- Modify: `crates/forktty-core/src/lib.rs:13` (add `pub mod`), `:32` area (re-export)

- [x] **Step 1: Add the rusqlite dependency**

In `Cargo.toml` `[workspace.dependencies]` (`/home/simone/forktty/Cargo.toml`) add:

```toml
rusqlite = { version = "0.32", features = ["bundled"] }
```

In `crates/forktty-core/Cargo.toml` `[dependencies]` add (alphabetical, after `notify-rust`):

```toml
rusqlite.workspace = true
```

- [x] **Step 2: Write the failing tests** (append at the bottom of the new file's `#[cfg(test)] mod tests`)

Create `crates/forktty-core/src/browser_history.rs` with the test module first:

```rust
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
        h.record_visit("https://github.com/rust", "Rust Lang").unwrap();
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
}
```

- [x] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p forktty-core browser_history -- --nocapture`
Expected: FAIL (no `HistoryStore` / module not declared).

- [x] **Step 4: Implement the store** (prepend above the test module in the same file)

```rust
//! Browser-pane history + bookmarks (SP3 P3). Pure (no GTK/webkit): both the GTK
//! main thread (visit recording, address-bar completion) and the socket thread
//! (history/bookmark verbs) use these stores against the same per-profile files.
//! sqlite runs in WAL mode so a concurrent reader and writer do not block.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::profile::ProfileId;

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
    dirs::data_local_dir()
        .map(|d| d.join("forktty").join("browser_profiles").join(profile.to_string()))
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
        let conn = Connection::open(path)?;
        Self::init(conn)
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

    /// Most-recently-visited rows first, capped at `limit`.
    pub fn list(&self, limit: usize) -> Result<Vec<Visit>, HistoryError> {
        let mut stmt = self.conn.prepare(
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
        let like = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = self.conn.prepare(
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
```

- [x] **Step 5: Declare the module + re-export**

In `crates/forktty-core/src/lib.rs`, add after `pub mod browser_cmd;` (keep alphabetical-ish with siblings):

```rust
pub mod browser_history;
```

And add a re-export near the `profile` one (line ~32):

```rust
pub use browser_history::{Bookmark, BookmarkStore, HistoryError, HistoryStore, Visit};
```

(`Bookmark`/`BookmarkStore` are added in Task 2; declaring the re-export now will fail to compile until Task 2 — so for THIS task, re-export only `{HistoryError, HistoryStore, Visit}` and widen it in Task 2.)

- [x] **Step 6: Run tests to verify they pass**

Run: `cargo test -p forktty-core browser_history`
Expected: PASS (7 tests).

- [x] **Step 7: Commit**

```bash
git add -A crates/forktty-core Cargo.toml
git commit -m "feat(core): per-profile browser history store (SP3 P3)"
```

---

## Task 2: core `BookmarkStore` (json)

**Files:**
- Modify: `crates/forktty-core/src/browser_history.rs`
- Modify: `crates/forktty-core/src/lib.rs` (widen the re-export)

- [x] **Step 1: Write the failing tests** (add to the `tests` module)

```rust
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
```

- [x] **Step 2: Run tests to verify they fail**

Run: `cargo test -p forktty-core browser_history`
Expected: FAIL (`BookmarkStore` undefined).

- [x] **Step 3: Implement `BookmarkStore`** (add to `browser_history.rs`, above the test module)

```rust
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
        let items = match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<Vec<Bookmark>>(&bytes) {
                Ok(v) => v,
                Err(_) => {
                    let bak = path.with_extension("json.bak");
                    let _ = std::fs::write(&bak, &bytes);
                    Vec::new()
                }
            },
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(HistoryError::Io(e.to_string())),
        };
        Ok(Self { path: path.to_path_buf(), items })
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
            std::fs::write(&tmp, &bytes).map_err(|e| HistoryError::Io(e.to_string()))?;
            std::fs::rename(&tmp, &self.path).map_err(|e| HistoryError::Io(e.to_string()))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}
```

- [x] **Step 4: Widen the re-export** in `crates/forktty-core/src/lib.rs`:

```rust
pub use browser_history::{Bookmark, BookmarkStore, HistoryError, HistoryStore, Visit};
```

- [x] **Step 5: Run tests to verify they pass**

Run: `cargo test -p forktty-core browser_history`
Expected: PASS (12 tests total).

- [x] **Step 6: Commit**

```bash
git add -A crates/forktty-core
git commit -m "feat(core): per-profile bookmark store (SP3 P3)"
```

---

## Task 3: socket history + bookmark verbs

**Files:**
- Modify: `crates/forktty-socket/src/lib.rs` (`METHODS` list ~line 38; verb arms after `browser.profile.delete` ~line 880; tests at bottom)

Verbs to add (all functional regardless of `browser` feature — the stores are pure):
- `browser.history.list { profile?, limit? }` → `[Visit]`
- `browser.history.search { query, profile?, limit? }` → `[Visit]`
- `browser.history.clear { profile? }` → `{ "cleared": true }`
- `browser.bookmark.add { url, title?, profile? }` → `{ "added": true }`
- `browser.bookmark.list { profile? }` → `[Bookmark]`
- `browser.bookmark.remove { url, profile? }` → `{ "removed": bool }`

- [x] **Step 1: Add `METHODS` entries** (keep the list sorted; insert in order):

```rust
    "browser.bookmark.add",
    "browser.bookmark.list",
    "browser.bookmark.remove",
```
(before `"browser.click"`), and

```rust
    "browser.history.clear",
    "browser.history.list",
    "browser.history.search",
```
(after `"browser.fill"` / before `"browser.forward"` — verify final order is lexicographic).

- [x] **Step 2: Add a profile-resolution helper** near `profiles_store()` (~line 1645). Reuses the existing store to map an optional `profile` param (id or name) to a `ProfileId`, defaulting to `Default`:

```rust
/// Resolve an optional `profile` param (id or display name) to a `ProfileId`.
/// Absent → the Default profile. Present-but-unknown → NotFound.
fn resolve_profile_param(params: &Value) -> Result<forktty_core::ProfileId, DispatchError> {
    match params.get("profile") {
        None => Ok(forktty_core::ProfileId::default()),
        Some(Value::Null) => Ok(forktty_core::ProfileId::default()),
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                DispatchError::InvalidParam("Invalid parameter profile: expected string".to_string())
            })?;
            profiles_store()?
                .resolve(name)
                .ok_or(DispatchError::NotFound("profile".to_string()))
        }
    }
}
```

- [x] **Step 3: Add the verb arms** (after the `"browser.profile.delete"` arm, before `"surface.focus"`). Use a small `optional_usize_param` inline. History/bookmark IO maps errors to `DispatchError::Other`:

```rust
        "browser.history.list" => {
            let profile = resolve_profile_param(&params)?;
            let limit = params
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(100);
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let rows = store
                .list(limit)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(rows))
        }
        "browser.history.search" => {
            let query = required_string_param(&params, "query")?.to_string();
            let profile = resolve_profile_param(&params)?;
            let limit = params
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(100);
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let rows = store
                .search(&query, limit)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(rows))
        }
        "browser.history.clear" => {
            let profile = resolve_profile_param(&params)?;
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            store.clear().map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "cleared": true }))
        }
        "browser.bookmark.add" => {
            let url = required_string_param(&params, "url")?.to_string();
            if url.trim().is_empty() {
                return Err("Invalid parameter url: must not be empty".into());
            }
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let profile = resolve_profile_param(&params)?;
            let mut store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            store
                .add(&url, &title)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "added": true }))
        }
        "browser.bookmark.list" => {
            let profile = resolve_profile_param(&params)?;
            let store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(store.list()))
        }
        "browser.bookmark.remove" => {
            let url = required_string_param(&params, "url")?.to_string();
            let profile = resolve_profile_param(&params)?;
            let mut store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let removed = store
                .remove(&url)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "removed": removed }))
        }
```

- [x] **Step 4: Write socket tests** (bottom of `lib.rs`, in the existing `#[cfg(test)]` block). They must isolate `data_local_dir()` with the SAME `EnvGuard` + `#[serial_test::serial]` pattern P2 introduced (see `browser_profile_create_list_then_open_with_profile`). Set `XDG_DATA_HOME` to a tempdir, drive verbs through `dispatch`, assert round-trips:

```rust
    #[tokio::test]
    #[serial_test::serial]
    async fn browser_history_records_via_bookmark_and_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("XDG_DATA_HOME", tmp.path());
        let state = test_state(); // reuse the harness builder used by other browser tests

        // bookmark add → list round-trip
        dispatch(&state, "browser.bookmark.add", json!({"url":"https://a.test/","title":"A"}))
            .await
            .unwrap();
        let listed = dispatch(&state, "browser.bookmark.list", json!({})).await.unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        // bookmark remove
        let removed = dispatch(&state, "browser.bookmark.remove", json!({"url":"https://a.test/"}))
            .await
            .unwrap();
        assert_eq!(removed["removed"], json!(true));

        // history list on a fresh profile is empty; clear is a no-op success
        let hist = dispatch(&state, "browser.history.list", json!({})).await.unwrap();
        assert!(hist.as_array().unwrap().is_empty());
        dispatch(&state, "browser.history.clear", json!({})).await.unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_history_search_requires_query() {
        let tmp = tempfile::tempdir().unwrap();
        let _guard = EnvGuard::set("XDG_DATA_HOME", tmp.path());
        let state = test_state();
        let err = dispatch(&state, "browser.history.search", json!({})).await.unwrap_err();
        assert_eq!(err.code(), "missing_param");
    }
```

> Note for the implementer: match the EXACT names of the existing test harness builder (`test_state` / equivalent) and `EnvGuard` constructor used by the P2 test `browser_profile_create_list_then_open_with_profile`. Read that test first and copy its setup verbatim; do not invent new helpers. Also confirm `capabilities_lists_only_dispatchable_methods` still passes with the new `METHODS` entries.

- [x] **Step 5: Run tests**

Run: `cargo test -p forktty-socket browser_` and `cargo test -p forktty-socket capabilities`
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add -A crates/forktty-socket
git commit -m "feat(socket): browser history + bookmark verbs (SP3 P3)"
```

---

## Task 4: CLI mirrors (`forktty browser history|bookmark`)

**Files:**
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs` (HELP_TEXT ~line 75; `handle_browser` match ~line 1376; new fns; tests at bottom)

- [x] **Step 1: Add HELP_TEXT lines** after the `profile delete` line (~line 77):

```
  forktty browser history list [--profile <id|name>] [--limit <n>]
  forktty browser history search <query> [--profile <id|name>] [--limit <n>]
  forktty browser history clear [--profile <id|name>]
  forktty browser bookmark add <url> [--title <t>] [--profile <id|name>]
  forktty browser bookmark list [--profile <id|name>]
  forktty browser bookmark remove <url> [--profile <id|name>]
```

- [x] **Step 2: Add dispatch arms** in `handle_browser` (after `"profile" => browser_profile(...)`):

```rust
        "history" => browser_history(context, rest),
        "bookmark" => browser_bookmark(context, rest),
```
And extend the "requires a subcommand" / unknown-subcommand error string to include `history | bookmark`.

- [x] **Step 3: Implement `browser_history` + `browser_bookmark`** following the EXACT structure of the existing `browser_profile` fn (read it first — same `parse_options`, `non_blank_string_option`, `send_socket_request`, JSON-printing helpers). Sketch:

```rust
fn browser_history(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let (sub, rest) = split_subcommand(&args); // mirror browser_profile's split
    match sub.as_deref() {
        Some("list") => {
            let parsed = parse_options(rest, &["profile", "limit"], "browser history list")?;
            let mut params = serde_json::Map::new();
            if let Some(p) = non_blank_string_option(&parsed.options, "profile", "--profile")? {
                params.insert("profile".into(), Value::String(p.to_string()));
            }
            if let Some(n) = parsed.options.get("limit") {
                params.insert("limit".into(), Value::Number(n.parse::<u64>()
                    .map_err(|_| CliError::new("--limit must be a number"))?.into()));
            }
            let result = send_socket_request(&context.socket_path, "browser.history.list",
                Value::Object(params))?;
            print_json_or(&result, context); // match how browser_profile prints lists
            Ok(())
        }
        Some("search") => { /* required <query> positional + --profile/--limit, calls browser.history.search */ }
        Some("clear")  => { /* optional --profile, calls browser.history.clear */ }
        _ => Err(CliError::new("browser history requires: list | search <query> | clear")),
    }
}

fn browser_bookmark(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    // add <url> [--title] [--profile] -> browser.bookmark.add
    // list [--profile]               -> browser.bookmark.list
    // remove <url> [--profile]       -> browser.bookmark.remove
}
```

> The implementer must match the real helper names in this file (e.g. how `browser_profile` splits its subcommand, parses options, and prints JSON results). Do NOT introduce a `split_subcommand`/`print_json_or` if the file already has differently-named equivalents — use the existing ones.

- [x] **Step 4: Add CLI tests** mirroring the 5 existing `browser_profile` CLI tests that use the `with_socket_response` harness. Cover: `history list` sends `browser.history.list`; `history search foo` sends the query; `bookmark add <url> --title X` sends url+title; `bookmark remove <url>`; and an arg-validation error (e.g. `history search` with no query → error, no socket call).

- [x] **Step 5: Run tests**

Run: `cargo test -p forktty-ui-gtk --features gtk-vte socket_cli` (CLI tests do not need the browser feature)
Expected: PASS.

- [x] **Step 6: Commit**

```bash
git add -A crates/forktty-ui-gtk/src/socket_cli.rs
git commit -m "feat(cli): browser history + bookmark commands (SP3 P3)"
```

---

## Task 5: GTK visit recording + address-bar completion

**Files:**
- Modify: `crates/forktty-ui-gtk/src/browser_pane.rs`

This task is GTK/`browser`-gated. Per the established browser-pane test posture, WebView-dependent behavior is verified manually (needs a display); add only what unit tests can cover headlessly.

- [ ] **Step 1: Record visits.** In `BrowserPaneWidget::new`, after the WebView is built, connect `load-changed` and capture the profile. On `LoadEvent::Committed` for the top frame, read `web_view.uri()`; on `notify::title`, read `web_view.title()`. Skip `about:blank`, empty, and non-`http(s)` URIs. Open `HistoryStore::for_profile(profile)` and `record_visit(url, title)`. Open the store lazily per record (cheap; sqlite WAL) OR cache one `HistoryStore` in the widget — cache it in a `RefCell<Option<HistoryStore>>` field to avoid reopening on every signal. Errors: log a warning and continue (navigation must not break).

```rust
// profile_id: &str is already the ctor arg. Parse to ProfileId for the store:
let profile = profile_id.parse::<forktty_core::ProfileId>().unwrap_or_default();
```

Wire (sketch — adapt to the existing signal style in this file):

```rust
{
    let profile = profile;
    web_view.connect_load_changed(move |wv, event| {
        if event == webkit6::LoadEvent::Committed {
            if let Some(uri) = wv.uri() {
                let u = uri.to_string();
                if u.starts_with("http://") || u.starts_with("https://") {
                    let title = wv.title().map(|t| t.to_string()).unwrap_or_default();
                    if let Ok(store) = forktty_core::HistoryStore::for_profile(profile) {
                        let _ = store.record_visit(&u, &title);
                    }
                }
            }
        }
    });
}
```

(A `notify::title` handler can additionally re-record to capture the final title once it loads; same guard. Keep it simple — `Committed` plus one title refresh is enough.)

- [ ] **Step 2: Address-bar completion.** Give `address` a `gtk::EntryCompletion` backed by a `gtk::ListStore` (single text column). Populate it from `HistoryStore::for_profile(profile).list(N)` URLs (plus bookmark URLs flagged) when the pane is built and refresh on `load-changed`. Set `completion.set_text_column(0)`, `entry.set_completion(Some(&completion))`. Selecting a completion navigates (the existing `connect_address_activate` already navigates on Enter; `EntryCompletion`'s match-selected can set the entry text then trigger activate).

- [ ] **Step 3: Pure unit test (headless).** Add a test that does NOT construct a WebView — e.g. assert the URL filter predicate. Extract the "should this URL be recorded?" check into a free fn:

```rust
fn is_recordable_url(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}
```

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_http_urls_are_recordable() {
        assert!(is_recordable_url("https://a.test/"));
        assert!(is_recordable_url("http://a.test/"));
        assert!(!is_recordable_url("about:blank"));
        assert!(!is_recordable_url(""));
        assert!(!is_recordable_url("data:text/html,x"));
    }
}
```

Use `is_recordable_url` in the `load-changed` handler so the test covers real logic.

- [ ] **Step 4: Build both features + run tests**

Run:
```
cargo build -p forktty-ui-gtk --features browser
cargo build -p forktty-ui-gtk --features gtk-vte
cargo test -p forktty-ui-gtk --features browser browser_pane
cargo clippy -p forktty-ui-gtk --features browser -- -D warnings
cargo fmt --all
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A crates/forktty-ui-gtk/src/browser_pane.rs
git commit -m "feat(gtk): browser-pane visit recording + address completion (SP3 P3)"
```

---

## Final verification (after all tasks)

```
cargo test -p forktty-core
cargo test -p forktty-socket
cargo test -p forktty-ui-gtk --features browser
cargo test -p forktty-ui-gtk --features gtk-vte
cargo clippy --workspace --features browser -- -D warnings   # (run from ui-gtk if workspace feature unification complains; mirror P2's clippy invocation)
cargo fmt --all --check
```

All green + clippy/fmt clean on both `browser` and `gtk-vte`. Manual smoke (needs display): visit pages in a pane → `forktty browser history list` shows them with counts; typing in the address bar offers visited URLs; `bookmark add/list/remove` round-trip.
