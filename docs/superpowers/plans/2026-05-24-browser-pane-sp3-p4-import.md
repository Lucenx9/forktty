# Browser pane SP3 P4 — Import Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A headless, GTK-free `forktty-import` crate that discovers installed browsers (Firefox + Chromium family), reads/decrypts their cookies, reads history + bookmarks, and resolves a source→destination import plan — fully unit-testable without a display, a keyring, or a real browser.

**Scope note (multi-agent coordination):** SP3 P3 (history/bookmark stores + socket verbs) is being implemented in parallel on `feat/browser-pane-sp3-p3`. To avoid collision, **this PR delivers only the `forktty-import` engine crate** (the hard, headless, conflict-free part). The engine emits its own plain data structs (`ImportedCookie`, `ImportedVisit`, `ImportedBookmark`). The two front-ends that consume it — the GTK `Adw` wizard (cookie injection via `CookieManager`, writing history/bookmarks into P3's `HistoryStore`/`BookmarkStore`) and the socket verbs `browser.import.sources`/`browser.import.run` — are a thin **integration follow-up** that lands after P2 + P3 + P4 are all merged to `main` (documented at the end). This keeps each PR reviewable and unblocked.

**Architecture:** One library crate, no GTK/webkit. Modules: `model` (data types), `plan` (import-plan resolver, ported from cmux), `sources` (discovery), `cookies` (read + decrypt), `history` (history + bookmarks readers), `lib` (engine orchestration). Chromium cookie decryption uses AES-128-CBC with a PBKDF2 key; the Secret Service path (`v11`) is behind an optional `keyring` cargo feature so the default build and CI need no D-Bus, always falling back to the `v10` `"peanuts"` key.

**Tech Stack:** Rust, `rusqlite` (bundled sqlite, read-only source DBs), `serde`/`serde_json`, `aes` + `cbc` + `cipher`, `pbkdf2` + `hmac` + `sha1`, `dirs`, optional `secret-service`.

**Base branch:** `feat/browser-pane-sp3-p2` (has `forktty_core::ProfileId` / `ProfileStore`, which the plan resolver references). This plan is implemented in the worktree at `/home/simone/forktty-p4` on branch `feat/browser-pane-sp3-p4`.

**Security framing:** Import reads another application's stored cookies from the *user's own machine* under the *user's own login*, writes them only into the user's own forktty profile. No secret leaves the machine. The `v11` keyring read goes through the OS Secret Service prompt — the OS's own consent gate. Cookies that fail to decrypt are skipped and counted, never fatal. This is local, user-initiated, user-authorized data migration.

---

## File structure

New crate `crates/forktty-import/`:
- `Cargo.toml`
- `src/lib.rs` — re-exports + `ImportEngine` orchestration + `ImportError`.
- `src/model.rs` — `BrowserFamily`, `SourceBrowser`, `SourceProfile`, `ImportSelection`, `ImportMode`, `ImportPlan`, `ImportEntry`, `ImportDestination`, `ImportedCookie`, `ImportedVisit`, `ImportedBookmark`, `ImportResult`.
- `src/plan.rs` — `resolve_default_plan`, `resolve_separate_profiles_plan` (+ private helpers), ported cmux tests.
- `src/sources.rs` — `discover()` + per-family probes.
- `src/cookies.rs` — `read_firefox_cookies`, `read_chromium_cookies`, `decrypt_chromium_value`, key derivation.
- `src/history.rs` — `read_firefox_history`/`read_firefox_bookmarks`, `read_chromium_history`/`read_chromium_bookmarks`.

Root `Cargo.toml`: add the crate to `members` + new workspace deps.

---

## Task 1: crate scaffold + model + plan resolver

**Files:**
- Create: `crates/forktty-import/Cargo.toml`, `crates/forktty-import/src/lib.rs`, `crates/forktty-import/src/model.rs`, `crates/forktty-import/src/plan.rs`
- Modify: root `Cargo.toml` (`members`, `[workspace.dependencies]`)

- [ ] **Step 1: Add workspace deps + member.** In root `/home/simone/forktty-p4/Cargo.toml`, add to `members` (after `crates/forktty-ui-gtk`):

```toml
    "crates/forktty-import",
```

Add to `[workspace.dependencies]` (alphabetical):

```toml
aes = "0.8"
cbc = "0.1"
cipher = "0.4"
hmac = "0.12"
pbkdf2 = { version = "0.12", default-features = false, features = ["hmac"] }
rusqlite = { version = "0.32", features = ["bundled"] }
secret-service = "4"
sha1 = "0.10"
```
(If `rusqlite` is already present from a merged P3, keep one copy.)

- [ ] **Step 2: Create `crates/forktty-import/Cargo.toml`**

```toml
[package]
name = "forktty-import"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
authors.workspace = true
homepage.workspace = true
repository.workspace = true
description = "Headless browser-data import engine for ForkTTY (Firefox + Chromium cookies/history/bookmarks)"

[features]
# Off by default: the v11 Secret Service path needs D-Bus. v10 "peanuts" is always available.
keyring = ["dep:secret-service"]

[dependencies]
aes.workspace = true
cbc.workspace = true
cipher.workspace = true
dirs.workspace = true
forktty-core = { path = "../forktty-core" }
hmac.workspace = true
pbkdf2.workspace = true
rusqlite.workspace = true
secret-service = { workspace = true, optional = true }
serde.workspace = true
serde_json.workspace = true
sha1.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 3: Create `src/model.rs`** with the data types. Write this in full:

```rust
//! Data types for the browser-import engine. All serde-serializable so the socket
//! front-end (a later integration step) can ship them over JSON-RPC unchanged.

use forktty_core::ProfileId;
use serde::{Deserialize, Serialize};

/// A supported source browser family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFamily {
    Firefox,
    Chrome,
    Chromium,
    Brave,
    Edge,
    Vivaldi,
}

impl BrowserFamily {
    /// Chromium-family browsers share the `Cookies`/`History`/`Bookmarks` layout and
    /// AES cookie encryption; Firefox is its own (plaintext cookies, `places.sqlite`).
    pub fn is_chromium(self) -> bool {
        !matches!(self, BrowserFamily::Firefox)
    }

    /// The Secret Service label whose secret derives the `v11` key (Chromium only).
    pub fn safe_storage_label(self) -> Option<&'static str> {
        match self {
            BrowserFamily::Firefox => None,
            BrowserFamily::Chrome => Some("Chrome Safe Storage"),
            BrowserFamily::Chromium => Some("Chromium Safe Storage"),
            BrowserFamily::Brave => Some("Brave Safe Storage"),
            BrowserFamily::Edge => Some("Microsoft Edge Safe Storage"),
            BrowserFamily::Vivaldi => Some("Vivaldi Safe Storage"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BrowserFamily::Firefox => "Firefox",
            BrowserFamily::Chrome => "Google Chrome",
            BrowserFamily::Chromium => "Chromium",
            BrowserFamily::Brave => "Brave",
            BrowserFamily::Edge => "Microsoft Edge",
            BrowserFamily::Vivaldi => "Vivaldi",
        }
    }
}

/// One discovered source browser with its profiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBrowser {
    pub family: BrowserFamily,
    pub profiles: Vec<SourceProfile>,
}

/// One profile inside a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProfile {
    pub family: BrowserFamily,
    /// Human-readable name (Firefox profile name / Chromium `profile.info_cache` name,
    /// falling back to the directory name).
    pub display_name: String,
    /// Absolute path to the profile directory.
    pub path: String,
    pub is_default: bool,
}

/// Import destination for one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ImportDestination {
    /// Merge into an existing forktty profile.
    Existing(ProfileId),
    /// Create a new forktty profile with this display name.
    Create(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// All selected sources merge into one destination profile.
    SingleDestination,
    /// Each source maps to its own destination profile.
    SeparateProfiles,
}

/// One unit of the plan: these source profiles flow into this destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportEntry {
    pub sources: Vec<SourceProfile>,
    pub destination: ImportDestination,
}

/// The resolved import plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub mode: ImportMode,
    pub entries: Vec<ImportEntry>,
}

/// A cookie ready to be written into a forktty profile's session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedCookie {
    pub name: String,
    pub value: String,
    pub host: String,
    pub path: String,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
}

/// A visited URL read from a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedVisit {
    pub url: String,
    pub title: String,
    pub visit_count: i64,
}

/// A bookmark read from a source browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedBookmark {
    pub url: String,
    pub title: String,
}

/// Per-entry result counts after running an import.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportResult {
    pub cookies: usize,
    pub history: usize,
    pub bookmarks: usize,
    /// Cookies that could not be decrypted/parsed and were skipped.
    pub skipped: usize,
}

impl ImportResult {
    pub fn add(&mut self, other: &ImportResult) {
        self.cookies += other.cookies;
        self.history += other.history;
        self.bookmarks += other.bookmarks;
        self.skipped += other.skipped;
    }
}
```

- [ ] **Step 4: Write the plan resolver tests FIRST** in `src/plan.rs` (ported from cmux `BrowserImportMappingTests.swift`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use forktty_core::{ProfileId, ProfileMeta};

    fn src(name: &str, default: bool) -> SourceProfile {
        SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: name.to_string(),
            path: format!("/tmp/src-{}", name.trim()),
            is_default: default,
        }
    }

    fn dest(name: &str, id: ProfileId) -> ProfileMeta {
        ProfileMeta { id, display_name: name.to_string(), created_at: 0, is_default: false }
    }

    #[test]
    fn default_plan_uses_separate_mode_for_multiple_sources() {
        let default_id = ProfileId::default();
        let plan = resolve_default_plan(
            &[src("You", true), src("austin", false)],
            &[dest("Default", default_id)],
            default_id,
        );
        assert_eq!(plan.mode, ImportMode::SeparateProfiles);
        assert_eq!(plan.entries.len(), 2);
        let names: Vec<_> = plan
            .entries
            .iter()
            .map(|e| e.sources[0].display_name.clone())
            .collect();
        assert_eq!(names, vec!["You", "austin"]);
    }

    #[test]
    fn default_plan_uses_single_destination_for_one_source() {
        let default_id = ProfileId::default();
        let plan = resolve_default_plan(&[src("You", true)], &[], default_id);
        assert_eq!(plan.mode, ImportMode::SingleDestination);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].sources[0].display_name, "You");
        assert_eq!(plan.entries[0].destination, ImportDestination::Existing(default_id));
    }

    #[test]
    fn separate_plan_reuses_existing_same_named_destination() {
        let work_id = ProfileId::new();
        let plan = resolve_separate_profiles_plan(
            &[src(" you ", true)],
            &[dest("You", work_id)],
        );
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].destination, ImportDestination::Existing(work_id));
    }

    #[test]
    fn separate_plan_stable_create_names_on_collision() {
        let plan = resolve_separate_profiles_plan(
            &[src("Work", true), src("Work", false)],
            &[],
        );
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].destination, ImportDestination::Create("Work".to_string()));
        assert_eq!(plan.entries[1].destination, ImportDestination::Create("Work (2)".to_string()));
    }

    #[test]
    fn empty_selection_yields_empty_single_destination_plan() {
        let id = ProfileId::default();
        let plan = resolve_default_plan(&[], &[], id);
        assert_eq!(plan.mode, ImportMode::SingleDestination);
        assert!(plan.entries.is_empty());
    }
}
```

- [ ] **Step 5: Implement the resolver** in `src/plan.rs` (above the tests). Faithful port of cmux's `BrowserImportPlanResolver`:

```rust
//! Source→destination import-plan resolver, ported from cmux's
//! `BrowserImportPlanResolver`. Pure: takes selected source profiles + existing
//! destination profiles, returns an `ImportPlan`. No IO.

use forktty_core::{ProfileId, ProfileMeta};

use crate::model::{ImportDestination, ImportEntry, ImportMode, ImportPlan, SourceProfile};

fn normalized(name: &str) -> String {
    name.trim().to_lowercase()
}

/// First destination whose display name matches `source_name` (trimmed, case-insensitive).
fn matching_destination(source_name: &str, destinations: &[ProfileMeta]) -> Option<ProfileId> {
    let norm = normalized(source_name);
    if norm.is_empty() {
        return None;
    }
    destinations
        .iter()
        .find(|d| normalized(&d.display_name) == norm)
        .map(|d| d.id)
}

/// A create-name not already taken (normalized): `base`, then `base (2)`, `base (3)`, …
/// Empty base falls back to `Profile`.
fn next_create_name(base: &str, taken: &std::collections::HashSet<String>) -> String {
    let trimmed = base.trim();
    let resolved = if trimmed.is_empty() { "Profile" } else { trimmed };
    if !taken.contains(&normalized(resolved)) {
        return resolved.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{resolved} ({suffix})");
        if !taken.contains(&normalized(&candidate)) {
            return candidate;
        }
        suffix += 1;
    }
}

/// cmux `defaultPlan`: ≤1 source → SingleDestination (match-by-name else preferred);
/// >1 source → SeparateProfiles.
pub fn resolve_default_plan(
    selected: &[SourceProfile],
    destinations: &[ProfileMeta],
    preferred_single_destination: ProfileId,
) -> ImportPlan {
    if selected.len() <= 1 {
        let destination = selected
            .first()
            .and_then(|s| matching_destination(&s.display_name, destinations))
            .map(ImportDestination::Existing)
            .unwrap_or(ImportDestination::Existing(preferred_single_destination));
        return ImportPlan {
            mode: ImportMode::SingleDestination,
            entries: selected
                .iter()
                .map(|s| ImportEntry {
                    sources: vec![s.clone()],
                    destination: destination.clone(),
                })
                .collect(),
        };
    }
    resolve_separate_profiles_plan(selected, destinations)
}

/// cmux `separateProfilesPlan`: one destination per source; reuse a same-named
/// existing destination, else create a stable de-duplicated name.
pub fn resolve_separate_profiles_plan(
    selected: &[SourceProfile],
    destinations: &[ProfileMeta],
) -> ImportPlan {
    let mut reserved: std::collections::HashSet<String> =
        destinations.iter().map(|d| normalized(&d.display_name)).collect();
    let entries = selected
        .iter()
        .map(|s| {
            let destination = if let Some(id) = matching_destination(&s.display_name, destinations) {
                ImportDestination::Existing(id)
            } else {
                let name = next_create_name(&s.display_name, &reserved);
                reserved.insert(normalized(&name));
                ImportDestination::Create(name)
            };
            ImportEntry { sources: vec![s.clone()], destination }
        })
        .collect();
    ImportPlan { mode: ImportMode::SeparateProfiles, entries }
}
```

- [ ] **Step 6: Create `src/lib.rs`** declaring modules + re-exports (engine added in Task 5):

```rust
//! Headless browser-data import engine for ForkTTY. Discovers installed browsers,
//! reads + decrypts their cookies, reads history + bookmarks, and resolves a
//! source→destination import plan. No GTK / no webkit; everything here is unit-
//! testable headless. See `docs/superpowers/specs/2026-05-24-browser-pane-sp3-design.md`.

pub mod cookies;
pub mod history;
pub mod model;
pub mod plan;
pub mod sources;

pub use model::*;
pub use plan::{resolve_default_plan, resolve_separate_profiles_plan};
```

(For THIS task, `cookies`/`history`/`sources` modules don't exist yet — to keep the crate compiling after Task 1, create empty stub files `src/cookies.rs`, `src/history.rs`, `src/sources.rs` each containing only a `//! stub, implemented in a later task` doc comment, and DON'T re-export from them yet. Replace the `pub use` line with just `pub use model::*; pub use plan::{...};` until Task 5.)

- [ ] **Step 7: Build + test**

```
cargo test -p forktty-import plan
cargo clippy -p forktty-import -- -D warnings
cargo fmt -p forktty-import
```
Expected: 5 plan tests pass, clean.

- [ ] **Step 8: Commit**

```bash
git add -A crates/forktty-import Cargo.toml Cargo.lock
git commit -m "feat(import): forktty-import crate scaffold + model + plan resolver (SP3 P4)"
```

---

## Task 2: cookie reading + Chromium decrypt

**Files:**
- Modify: `crates/forktty-import/src/cookies.rs`

- [ ] **Step 1: Write tests FIRST** (synthetic data; no real browser). Put in `cookies.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // A v10 Chromium blob = b"v10" || AES-128-CBC(key, IV=16 spaces, PKCS7(plaintext)).
    fn make_v10_blob(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Enc = cbc::Encryptor<aes::Aes128>;
        let iv = [0x20u8; 16];
        let mut buf = plaintext.to_vec();
        let pad_len = 16 - (buf.len() % 16);
        buf.extend(std::iter::repeat(0u8).take(pad_len)); // room for padding
        let ct = Enc::new(key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .unwrap()
            .to_vec();
        let mut out = b"v10".to_vec();
        out.extend_from_slice(&ct);
        out
    }

    #[test]
    fn v10_key_is_pbkdf2_peanuts() {
        // Known vector: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16).
        let key = chromium_v10_key();
        assert_eq!(key.len(), 16);
        // Recompute independently and compare (guards against silent param drift).
        let mut expected = [0u8; 16];
        pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(b"peanuts", b"saltysalt", 1, &mut expected).unwrap();
        assert_eq!(key, expected);
    }

    #[test]
    fn decrypt_v10_roundtrip() {
        let key = chromium_v10_key();
        let blob = make_v10_blob(&key, b"session=abc123");
        let value = decrypt_chromium_value(&blob, &key).unwrap();
        assert_eq!(value, "session=abc123");
    }

    #[test]
    fn decrypt_rejects_unknown_prefix() {
        let key = chromium_v10_key();
        assert!(decrypt_chromium_value(b"v99garbage", &key).is_none());
        assert!(decrypt_chromium_value(b"", &key).is_none());
    }

    #[test]
    fn read_firefox_cookies_from_synthetic_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cookies.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies (
                 name TEXT, value TEXT, host TEXT, path TEXT,
                 expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER);
             INSERT INTO moz_cookies VALUES ('sid','xyz','.a.test','/',1893456000,1,1);",
        )
        .unwrap();
        drop(conn);
        let cookies = read_firefox_cookies(&db).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "xyz");
        assert_eq!(cookies[0].host, ".a.test");
        assert!(cookies[0].secure);
        assert!(cookies[0].http_only);
        assert_eq!(cookies[0].expires, Some(1893456000));
    }

    #[test]
    fn read_chromium_cookies_decrypts_with_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let blob = make_v10_blob(&key, b"tok");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',13300000000000000,1,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);
        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "tok");
        assert_eq!(cookies[0].host, ".a.test");
        assert!(cookies[0].secure);
        assert!(!cookies[0].http_only);
    }

    #[test]
    fn read_chromium_cookies_counts_skips_on_bad_blob() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','bad','',?1,'/',0,0,0)",
            rusqlite::params![b"v99nonsense".to_vec()],
        )
        .unwrap();
        drop(conn);
        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();
        assert!(cookies.is_empty());
        assert_eq!(skipped, 1);
    }
}
```

- [ ] **Step 2: Run tests, confirm they FAIL**

Run: `cargo test -p forktty-import cookies`
Expected: FAIL (functions undefined).

- [ ] **Step 3: Implement `cookies.rs`** (above the tests). Notes:
- `chromium_v10_key()` = PBKDF2-HMAC-SHA1(`b"peanuts"`, `b"saltysalt"`, 1 iter, 16 bytes).
- `decrypt_chromium_value(blob, key)`: require ≥3-byte `v10`/`v11` prefix; AES-128-CBC, IV = `[0x20;16]`; PKCS7-unpad; UTF-8 (lossy→`None` on invalid). Returns `Option<String>` (None = skip).
- Firefox: open the sqlite read-only via `OpenFlags::SQLITE_OPEN_READ_ONLY`; query `moz_cookies`; `expiry` 0 → `None`.
- Chromium: read-only; `cookies` table; for each row, if `encrypted_value` non-empty decrypt, else fall back to plaintext `value`; count decrypt failures as `skipped`. Chromium `expires_utc` is microseconds since 1601 — convert to Unix seconds: `if v == 0 { None } else { Some(v/1_000_000 - 11_644_473_600) }`.

```rust
//! Read + decrypt cookies from source browsers. Firefox cookies are plaintext;
//! Chromium-family `encrypted_value` blobs are AES-128-CBC (Linux v10/v11).

use std::path::Path;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OpenFlags};

use crate::model::ImportedCookie;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// The Linux Chromium `v10` key: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16B).
pub fn chromium_v10_key() -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(b"peanuts", b"saltysalt", 1, &mut key)
        .expect("pbkdf2 into a 16-byte buffer never fails");
    key
}

/// Decrypt a Chromium `encrypted_value` blob with `key`. Returns `None` (caller skips
/// + counts) on any structural or decode failure — never panics.
pub fn decrypt_chromium_value(blob: &[u8], key: &[u8; 16]) -> Option<String> {
    if blob.len() < 3 {
        return None;
    }
    let prefix = &blob[0..3];
    if prefix != b"v10" && prefix != b"v11" {
        return None;
    }
    let ct = &blob[3..];
    if ct.is_empty() || ct.len() % 16 != 0 {
        return None;
    }
    let iv = [0x20u8; 16];
    let mut buf = ct.to_vec();
    let pt = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?;
    String::from_utf8(pt.to_vec()).ok()
}

fn open_ro(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Read Firefox plaintext cookies from `cookies.sqlite`.
pub fn read_firefox_cookies(db: &Path) -> rusqlite::Result<Vec<ImportedCookie>> {
    let conn = open_ro(db)?;
    let mut stmt = conn.prepare(
        "SELECT name, value, host, path, expiry, isSecure, isHttpOnly FROM moz_cookies",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let expiry: i64 = row.get(4)?;
            Ok(ImportedCookie {
                name: row.get(0)?,
                value: row.get(1)?,
                host: row.get(2)?,
                path: row.get(3)?,
                expires: if expiry == 0 { None } else { Some(expiry) },
                secure: row.get::<_, i64>(5)? != 0,
                http_only: row.get::<_, i64>(6)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read Chromium-family cookies, decrypting `encrypted_value` with `key`. Returns
/// `(cookies, skipped)` where `skipped` counts rows that failed to decrypt.
pub fn read_chromium_cookies(db: &Path, key: &[u8; 16]) -> rusqlite::Result<(Vec<ImportedCookie>, usize)> {
    let conn = open_ro(db)?;
    let mut stmt = conn.prepare(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly FROM cookies",
    )?;
    let mut cookies = Vec::new();
    let mut skipped = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let plain: String = row.get(2)?;
        let enc: Vec<u8> = row.get(3)?;
        let path: String = row.get(4)?;
        let expires_utc: i64 = row.get(5)?;
        let secure: bool = row.get::<_, i64>(6)? != 0;
        let http_only: bool = row.get::<_, i64>(7)? != 0;

        let value = if enc.is_empty() {
            plain
        } else {
            match decrypt_chromium_value(&enc, key) {
                Some(v) => v,
                None => {
                    skipped += 1;
                    continue;
                }
            }
        };
        // Chromium epoch is 1601-01-01 in microseconds.
        let expires = if expires_utc == 0 {
            None
        } else {
            Some(expires_utc / 1_000_000 - 11_644_473_600)
        };
        cookies.push(ImportedCookie { name, value, host, path, expires, secure, http_only });
    }
    Ok((cookies, skipped))
}
```

- [ ] **Step 4: (optional `keyring` feature) v11 key.** Add behind `#[cfg(feature = "keyring")]` a `chromium_v11_key(label: &str) -> Option<[u8;16]>` that reads the Secret Service secret under `label` and PBKDF2s it the same way (1 iter, salt `saltysalt`). The default (no feature) path uses only `chromium_v10_key()`. Keep this minimal and untested in CI (D-Bus required); the v10 path is what tests cover. Document that a caller tries v11 first (if feature on + keyring unlocked) then falls back to v10.

- [ ] **Step 5: Run tests + lint**

```
cargo test -p forktty-import cookies
cargo clippy -p forktty-import -- -D warnings
cargo clippy -p forktty-import --features keyring -- -D warnings
cargo fmt -p forktty-import
```
Expected: 7 cookie tests pass; both feature configs clippy-clean.

- [ ] **Step 6: Commit**

```bash
git add -A crates/forktty-import/src/cookies.rs
git commit -m "feat(import): Firefox + Chromium cookie reading and v10 decrypt (SP3 P4)"
```

---

## Task 3: source discovery

**Files:**
- Modify: `crates/forktty-import/src/sources.rs`

Discovery probes known config roots under `$HOME`. To keep it testable, the probe functions take an explicit root path; `discover()` calls them with the real `dirs::home_dir()`-based roots.

| Family | Config root (under home) | Profile dirs | Cookie file |
|--------|--------------------------|--------------|-------------|
| Firefox | `.mozilla/firefox` | dirs containing `cookies.sqlite` (parse `profiles.ini` for names) | `cookies.sqlite` |
| Chrome | `.config/google-chrome` | `Default`, `Profile *` (names from `Local State` → `profile.info_cache`) | `Cookies` |
| Chromium | `.config/chromium` | same | `Cookies` |
| Brave | `.config/BraveSoftware/Brave-Browser` | same | `Cookies` |
| Edge | `.config/microsoft-edge` | same | `Cookies` |
| Vivaldi | `.config/vivaldi` | same | `Cookies` |

- [ ] **Step 1: Write tests FIRST** against synthetic fake-profile trees in a tempdir:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_firefox_reads_profiles_ini_names() {
        let dir = tempfile::tempdir().unwrap();
        let ff = dir.path().join(".mozilla/firefox");
        fs::create_dir_all(ff.join("abc.default-release")).unwrap();
        fs::write(ff.join("abc.default-release/cookies.sqlite"), b"x").unwrap();
        fs::write(
            ff.join("profiles.ini"),
            "[Profile0]\nName=default-release\nPath=abc.default-release\nDefault=1\n",
        )
        .unwrap();
        let browser = discover_firefox(&ff).unwrap();
        assert_eq!(browser.family, BrowserFamily::Firefox);
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "default-release");
        assert!(browser.profiles[0].is_default);
    }

    #[test]
    fn discover_chromium_reads_local_state_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"x").unwrap();
        fs::create_dir_all(root.join("Profile 1")).unwrap();
        fs::write(root.join("Profile 1/Cookies"), b"x").unwrap();
        fs::write(
            root.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"You"},"Profile 1":{"name":"Work"}}}}"#,
        )
        .unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        let mut names: Vec<_> = browser.profiles.iter().map(|p| p.display_name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["Work", "You"]);
        assert!(browser.profiles.iter().any(|p| p.display_name == "You" && p.is_default));
    }

    #[test]
    fn discover_chromium_falls_back_to_dir_name_without_local_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".config/chromium");
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"x").unwrap();
        let browser = discover_chromium_family(BrowserFamily::Chromium, &root).unwrap();
        assert_eq!(browser.profiles.len(), 1);
        assert_eq!(browser.profiles[0].display_name, "Default");
    }

    #[test]
    fn discover_returns_none_for_absent_root() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover_firefox(&dir.path().join("nope")).is_none());
        assert!(discover_chromium_family(BrowserFamily::Chrome, &dir.path().join("nope")).is_none());
    }
}
```

- [ ] **Step 2: Run, confirm FAIL.** `cargo test -p forktty-import sources` → FAIL.

- [ ] **Step 3: Implement `sources.rs`.** Provide `discover_firefox(root: &Path) -> Option<SourceBrowser>`, `discover_chromium_family(family, root: &Path) -> Option<SourceBrowser>`, and `discover() -> Vec<SourceBrowser>` (calls both for each family at the real roots, dropping `None`).
  - Firefox: return `None` if root absent. Parse `profiles.ini` (simple INI: `[ProfileN]` sections with `Name`, `Path`, optional `Default=1`, optional `IsRelative`). For each section whose resolved path contains `cookies.sqlite`, emit a `SourceProfile`. If `profiles.ini` is missing, fall back to scanning subdirs containing `cookies.sqlite`, using the dir name as the display name.
  - Chromium: return `None` if root absent. Candidate profile dirs = `Default` + any `Profile *` that contain a `Cookies` file. Names from `Local State` → `profile.info_cache.<dir>.name`, falling back to the dir name. `is_default = (dir == "Default")`.
  - Use `serde_json::Value` to read `Local State` leniently; never fail discovery on a malformed `Local State` (fall back to dir names).
  - INI parsing: do NOT pull a new dep; a tiny hand-rolled line parser is sufficient and testable.

- [ ] **Step 4: Test + lint.** `cargo test -p forktty-import sources`, `cargo clippy -p forktty-import -- -D warnings`, `cargo fmt -p forktty-import`. Expected: 4 tests pass, clean.

- [ ] **Step 5: Commit**

```bash
git add -A crates/forktty-import/src/sources.rs
git commit -m "feat(import): installed-browser source discovery (SP3 P4)"
```

---

## Task 4: history + bookmarks readers

**Files:**
- Modify: `crates/forktty-import/src/history.rs`

- [ ] **Step 1: Write tests FIRST** against synthetic source DBs/JSON:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn firefox_history_from_places() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("places.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('https://a.test/','A',3);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('place:internal','x',1);",
        ).unwrap();
        drop(conn);
        let visits = read_firefox_history(&db).unwrap();
        // Only http(s) URLs; place: internal entries dropped.
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://a.test/");
        assert_eq!(visits[0].visit_count, 3);
    }

    #[test]
    fn firefox_bookmarks_from_places() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("places.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, fk INTEGER, title TEXT, type INTEGER);
             INSERT INTO moz_places (id,url,title) VALUES (1,'https://b.test/','B page');
             INSERT INTO moz_bookmarks (fk,title,type) VALUES (1,'B mark',1);",
        ).unwrap();
        drop(conn);
        let bms = read_firefox_bookmarks(&db).unwrap();
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].url, "https://b.test/");
        assert_eq!(bms[0].title, "B mark");
    }

    #[test]
    fn chromium_history_from_urls_table() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE urls (url TEXT, title TEXT, visit_count INTEGER);
             INSERT INTO urls VALUES ('https://c.test/','C',5);",
        ).unwrap();
        drop(conn);
        let visits = read_chromium_history(&db).unwrap();
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://c.test/");
        assert_eq!(visits[0].visit_count, 5);
    }

    #[test]
    fn chromium_bookmarks_from_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        fs::write(
            &path,
            r#"{"roots":{"bookmark_bar":{"children":[
                {"type":"url","name":"D","url":"https://d.test/"},
                {"type":"folder","name":"f","children":[
                    {"type":"url","name":"E","url":"https://e.test/"}]}]}}}"#,
        ).unwrap();
        let mut bms = read_chromium_bookmarks(&path).unwrap();
        bms.sort_by(|a, b| a.url.cmp(&b.url));
        assert_eq!(bms.len(), 2);
        assert_eq!(bms[0].url, "https://d.test/");
        assert_eq!(bms[1].url, "https://e.test/");
    }
}
```

- [ ] **Step 2: Run, confirm FAIL.** `cargo test -p forktty-import history` → FAIL.

- [ ] **Step 3: Implement `history.rs`.**
  - `read_firefox_history(places: &Path)`: read-only; `SELECT url,title,visit_count FROM moz_places WHERE url LIKE 'http%'`; null title → empty string; null visit_count → 0.
  - `read_firefox_bookmarks(places: &Path)`: read-only; join `moz_bookmarks` (type=1) to `moz_places` on `fk=moz_places.id`; bookmark title falls back to place title; only `http(s)` urls.
  - `read_chromium_history(history: &Path)`: read-only; `SELECT url,title,visit_count FROM urls WHERE url LIKE 'http%'`.
  - `read_chromium_bookmarks(path: &Path)`: parse the `Bookmarks` JSON with `serde_json::Value`; recursively walk `roots.*.children`, collecting `{type:"url"}` nodes (`name`→title, `url`). Folders recurse. Malformed JSON → empty vec (don't error).
  - Filter to `http`/`https` URLs throughout (matches P3's `is_recordable_url` posture).

- [ ] **Step 4: Test + lint.** `cargo test -p forktty-import history`, clippy, fmt. Expected: 4 tests pass, clean.

- [ ] **Step 5: Commit**

```bash
git add -A crates/forktty-import/src/history.rs
git commit -m "feat(import): Firefox + Chromium history and bookmark readers (SP3 P4)"
```

---

## Task 5: engine orchestration

**Files:**
- Modify: `crates/forktty-import/src/lib.rs`

Ties the modules together into a headless engine that, given a source profile, reads everything into in-memory structs. It does NOT touch a forktty `CookieManager` or P3 history store — those writes belong to the (deferred) GTK/socket integration. This keeps the engine pure and fully testable.

- [ ] **Step 1: Write tests FIRST** in `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use std::fs;

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
}
```

- [ ] **Step 2: Run, confirm FAIL.**

- [ ] **Step 3: Implement the engine** in `lib.rs`. Add `pub struct ImportedData { cookies, visits, bookmarks, result }` and `pub struct ImportEngine;` with `read_source(&SourceProfile) -> Result<ImportedData, ImportError>`. Wire the module re-exports (now that all modules exist). Logic:
  - Firefox: cookie file `<path>/cookies.sqlite`; history/bookmarks `<path>/places.sqlite`. Missing file → empty (skip that category), not an error.
  - Chromium: cookie file `<path>/Cookies` (key = `chromium_v10_key()`, or v11 first if `keyring` feature + label); history `<path>/History`; bookmarks `<path>/Bookmarks`.
  - **Locked-DB workaround:** before opening a source sqlite, copy it to a tempfile and read the copy (a running browser holds a lock). Provide a small `read_via_copy(path, f)` helper using `tempfile::NamedTempFile`; if the copy fails (file absent), treat as empty.
  - Populate `ImportResult` counts (`cookies`/`history`/`bookmarks`/`skipped`).
  - `ImportError` enum (`Io(String)`, `Db(String)`) with Display + Error; `From<rusqlite::Error>`.

  Replace the `lib.rs` re-export block with the full set:
  ```rust
  pub use cookies::{chromium_v10_key, decrypt_chromium_value, read_chromium_cookies, read_firefox_cookies};
  pub use history::{read_chromium_bookmarks, read_chromium_history, read_firefox_bookmarks, read_firefox_history};
  pub use model::*;
  pub use plan::{resolve_default_plan, resolve_separate_profiles_plan};
  pub use sources::{discover, discover_chromium_family, discover_firefox};
  ```

- [ ] **Step 4: Full crate test + lint**

```
cargo test -p forktty-import
cargo clippy -p forktty-import -- -D warnings
cargo clippy -p forktty-import --features keyring -- -D warnings
cargo fmt -p forktty-import
```
Expected: all tests pass (plan 5 + cookies 7 + sources 4 + history 4 + engine 2 = 22), clean both feature configs.

- [ ] **Step 5: Commit**

```bash
git add -A crates/forktty-import/src/lib.rs
git commit -m "feat(import): import engine orchestration (SP3 P4)"
```

---

## Final verification (after all tasks)

```
cargo test -p forktty-import
cargo build -p forktty-import
cargo build -p forktty-import --features keyring
cargo clippy -p forktty-import -- -D warnings
cargo fmt --all --check
# Workspace still builds (new crate added to members):
cargo build --workspace
```

All green, clippy/fmt clean (default + `keyring`), workspace builds.

---

## Deferred integration follow-up (separate PR, after P2 + P3 + P4 merge to `main`)

The engine is consumed by two thin front-ends, intentionally NOT in this PR because they touch files the P3 branch is editing and require P3's `HistoryStore`/`BookmarkStore` to be on `main`:

1. **Socket/CLI** (`forktty-socket`, `forktty-ui-gtk/socket_cli.rs`): `browser.import.sources` → `discover()`; `browser.import.run { sources, mode?, mappings? }` → resolve plan, for each entry create/resolve the destination `ProfileId` via `ProfileStore`, read each source via `ImportEngine`, write history/bookmarks via P3's `HistoryStore::for_profile`/`BookmarkStore::for_profile`, return aggregated `ImportResult`. Cookies are returned in the result but injected GTK-side (next item). CLI: `forktty browser import list|run`.
2. **GTK wizard** (`forktty-ui-gtk/import_wizard.rs`, `browser`-gated): `Adw` 3-step window (source select → destination map → run+progress). Cookie injection uses the destination profile's `NetworkSession::cookie_manager().add_cookie(soup::Cookie, …)` over the SP2-style command bridge (`soup` becomes a direct dep here). Opened from an "Import…" entry in the browser-pane chrome.

These are deliberately scoped out to keep this PR focused on the headless, fully-tested engine and to avoid colliding with the in-flight P3 branch.
