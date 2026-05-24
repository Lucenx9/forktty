# Browser pane SP3 — P2 Profiles Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Implemented. `SurfaceKind::Browser` carries a
`ProfileId`, `browser.profile.{list,create,delete}` and `browser open --profile`
are wired through the socket/CLI, and browser panes bind their WebKit session to
the surface profile. Deleting a profile removes metadata only; on-disk profile
data cleanup remains deferred.

**Goal:** Give each browser pane an isolated, named profile: a `ProfileId` on the `Browser` surface kind, a file-backed `ProfileStore`, and socket/CLI verbs to create/list/delete profiles and open a pane in a chosen profile.

**Architecture:** `forktty-core` gains a `ProfileId` newtype (UUID-backed, `Default` = the well-known P1 Default UUID) carried on `SurfaceKind::Browser`, plus a pure `ProfileStore` that reads/writes `browser_profiles/profiles.json`. `forktty-socket` adds `browser.profile.{list,create,delete}` and a `profile` arg to `browser.open`, resolving names→ids via the store. `forktty-ui-gtk` binds each pane's WebView to `session_for(&surface.profile.to_string())` (P1's factory). On-disk data-dir cleanup for deleted profiles is deferred.

**Tech Stack:** Rust, `uuid` (new workspace dep, v1, features `v4`+`serde`), serde/serde_json, the existing socket JSON-RPC + CLI patterns.

**Phase context:** Phase 2 of 4 in the SP3 epic (spec: `docs/superpowers/specs/2026-05-24-browser-pane-sp3-design.md`, "## P2 — Profiles"). Builds on P1 (`browser_session.rs`: `DEFAULT_PROFILE_ID = "00000000-0000-0000-0000-000000000001"`, `session_for(&str)`, `is_valid_profile_id`). Branch `feat/browser-pane-sp3-p2` is stacked on `feat/browser-pane-sp3` (P1, PR #105). The Default `ProfileId` MUST render to `DEFAULT_PROFILE_ID` so existing P1 sessions keep their data dir.

---

## File Structure

- **Create** `crates/forktty-core/src/profile.rs` — `ProfileId` newtype + `ProfileMeta` + `ProfileStore` (pure, file-backed). One responsibility: profile identity + metadata persistence.
- **Modify** `crates/forktty-core/src/model.rs` — `SurfaceKind::Browser` gains `profile`; `open_browser` gains a `profile` param; fix the in-file `Browser {…}` match/construct sites.
- **Modify** `crates/forktty-core/src/lib.rs` — `pub mod profile;` + re-exports (`ProfileId`, `ProfileMeta`, `ProfileStore`).
- **Modify** `crates/forktty-core/src/events.rs:155` — destructure `Browser { url, .. }`.
- **Modify** `crates/forktty-socket/src/lib.rs` — `browser.open` profile arg; `browser.profile.{list,create,delete}` dispatch arms + `METHODS` entries; one `Browser { .. }` match site.
- **Modify** `crates/forktty-ui-gtk/src/socket_cli.rs` — `forktty browser profile list|create|delete` + `--profile` on `open`; HELP_TEXT.
- **Modify** `crates/forktty-ui-gtk/src/gtk_app.rs` — pass `surface.profile` into `BrowserPaneWidget::new`; pass `ProfileId::default()` at the two `open_browser` callers; the `Browser { url }` match sites become `Browser { url, .. }`.
- **Modify** `Cargo.toml` (workspace) + `crates/forktty-core/Cargo.toml` — add `uuid`.

---

## Task 1: Core `ProfileId` newtype + `uuid` dep

**Files:**
- Modify: `Cargo.toml`, `crates/forktty-core/Cargo.toml`
- Create: `crates/forktty-core/src/profile.rs`
- Modify: `crates/forktty-core/src/lib.rs`

- [ ] **Step 1: Add the `uuid` dependency**

In the workspace root `Cargo.toml` under `[workspace.dependencies]`, add:

```toml
uuid = { version = "1", features = ["v4", "serde"] }
```

In `crates/forktty-core/Cargo.toml` under `[dependencies]`, add:

```toml
uuid.workspace = true
```

- [ ] **Step 2: Write the failing test**

Create `crates/forktty-core/src/profile.rs` with this at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_id_renders_to_the_well_known_p1_string() {
        assert_eq!(
            ProfileId::default().to_string(),
            "00000000-0000-0000-0000-000000000001"
        );
    }

    #[test]
    fn profile_id_serde_is_a_plain_string() {
        let id = ProfileId::default();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"00000000-0000-0000-0000-000000000001\"");
        let back: ProfileId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn profile_id_parses_from_string() {
        let id: ProfileId = "00000000-0000-0000-0000-000000000001".parse().unwrap();
        assert_eq!(id, ProfileId::default());
        assert!("not-a-uuid".parse::<ProfileId>().is_err());
    }

    #[test]
    fn new_profile_ids_are_unique() {
        assert_ne!(ProfileId::new(), ProfileId::new());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p forktty-core profile::`
Expected: compile failure — `ProfileId` not found.

- [ ] **Step 4: Write the implementation**

At the top of `profile.rs`:

```rust
//! Browser-pane profiles (SP3 P2): stable per-profile identity plus file-backed
//! metadata. A profile isolates one browsing identity (cookies, storage, history);
//! each `Browser` surface carries a `ProfileId`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity for a browser profile. Serializes as its hyphenated lowercase
/// UUID string, which is also the on-disk directory name under `browser_profiles/`.
/// `Default` is the well-known P1 Default profile, so sessions created before the
/// profile system keep their data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProfileId(Uuid);

impl ProfileId {
    /// A fresh, random profile id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProfileId {
    fn default() -> Self {
        // 00000000-0000-0000-0000-000000000001 — matches browser_session::DEFAULT_PROFILE_ID.
        Self(Uuid::from_u128(1))
    }
}

impl fmt::Display for ProfileId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Uuid's Display is hyphenated lowercase.
        write!(f, "{}", self.0)
    }
}

impl FromStr for ProfileId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}
```

In `crates/forktty-core/src/lib.rs`, add `pub mod profile;` near the other `pub mod`
lines and re-export at the crate root next to the existing re-exports (grep for
`pub use crate::model::` to find them):

```rust
pub use crate::profile::{ProfileId, ProfileMeta, ProfileStore};
```

(The `ProfileMeta`/`ProfileStore` re-export will not resolve until Task 2 — add the
re-export now but if the crate fails to compile on `ProfileMeta`/`ProfileStore`, split
the `pub use` so only `ProfileId` is exported in this task and add the rest in Task 2.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p forktty-core profile::`
Expected: 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/forktty-core/Cargo.toml crates/forktty-core/src/profile.rs crates/forktty-core/src/lib.rs Cargo.lock
git commit -m "feat(core): SP3 P2 ProfileId newtype (uuid-backed, P1-compatible default)

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Core `ProfileStore` + `ProfileMeta`

File-backed profile metadata. Pure: the path is injected so tests use a temp dir.

**Files:**
- Modify: `crates/forktty-core/src/profile.rs`
- Modify: `crates/forktty-core/src/lib.rs` (finish the re-export if split in Task 1)

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `profile.rs`:

```rust
    fn temp_store() -> (tempfile::TempDir, ProfileStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("profiles.json");
        let store = ProfileStore::load(path).unwrap();
        (dir, store)
    }

    #[test]
    fn fresh_store_has_only_the_default_profile() {
        let (_d, store) = temp_store();
        let all = store.list();
        assert_eq!(all.len(), 1);
        assert!(all[0].is_default);
        assert_eq!(all[0].id, ProfileId::default());
    }

    #[test]
    fn create_adds_a_named_profile_and_persists() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Work").unwrap();
        assert_eq!(meta.display_name, "Work");
        assert!(!meta.is_default);
        // Reload from disk: the new profile survives.
        let reloaded = ProfileStore::load(store.path().to_path_buf()).unwrap();
        assert!(reloaded.list().iter().any(|p| p.id == meta.id && p.display_name == "Work"));
    }

    #[test]
    fn delete_removes_a_profile_but_refuses_the_default() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Throwaway").unwrap();
        assert!(store.delete(&meta.id).is_ok());
        assert!(!store.list().iter().any(|p| p.id == meta.id));
        // Default cannot be deleted.
        assert!(matches!(
            store.delete(&ProfileId::default()),
            Err(ProfileError::CannotDeleteDefault)
        ));
    }

    #[test]
    fn resolve_matches_by_id_or_display_name_case_insensitively() {
        let (_d, mut store) = temp_store();
        let meta = store.create("Work").unwrap();
        assert_eq!(store.resolve(&meta.id.to_string()), Some(meta.id));
        assert_eq!(store.resolve("work"), Some(meta.id));
        assert_eq!(store.resolve("  WORK "), Some(meta.id));
        assert_eq!(store.resolve("nope"), None);
    }
```

Ensure `tempfile` is a dev-dependency of `forktty-core` (grep `crates/forktty-core/Cargo.toml` for `tempfile`; if absent add `tempfile.workspace = true` under `[dev-dependencies]` — it is already a workspace dep used elsewhere; verify with `grep -rn "tempfile" crates/*/Cargo.toml Cargo.toml`).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p forktty-core profile::`
Expected: compile failure — `ProfileStore`, `ProfileMeta`, `ProfileError` not found.

- [ ] **Step 3: Implement**

Add to `profile.rs` (above the tests):

```rust
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Persisted metadata for one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub id: ProfileId,
    pub display_name: String,
    /// Unix seconds at creation.
    pub created_at: u64,
    pub is_default: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProfileError {
    CannotDeleteDefault,
    NotFound,
    Io(String),
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProfileError::CannotDeleteDefault => write!(f, "the default profile cannot be deleted"),
            ProfileError::NotFound => write!(f, "profile not found"),
            ProfileError::Io(e) => write!(f, "profile store io error: {e}"),
        }
    }
}
impl std::error::Error for ProfileError {}

/// File-backed list of profiles (`profiles.json`). The Default profile is always
/// present (synthesized if missing). The store owns metadata only; on-disk session
/// data directories are managed by the GTK layer.
#[derive(Debug)]
pub struct ProfileStore {
    path: PathBuf,
    profiles: Vec<ProfileMeta>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn default_meta() -> ProfileMeta {
    ProfileMeta {
        id: ProfileId::default(),
        display_name: "Default".to_string(),
        created_at: 0,
        is_default: true,
    }
}

impl ProfileStore {
    /// Load the store from `path`, creating an in-memory Default-only store if the
    /// file is absent. A present file always has the Default profile ensured.
    pub fn load(path: PathBuf) -> Result<Self, ProfileError> {
        let mut profiles: Vec<ProfileMeta> = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| ProfileError::Io(e.to_string()))?,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(ProfileError::Io(e.to_string())),
        };
        if !profiles.iter().any(|p| p.is_default) {
            profiles.insert(0, default_meta());
        }
        Ok(Self { path, profiles })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> &[ProfileMeta] {
        &self.profiles
    }

    fn save(&self) -> Result<(), ProfileError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ProfileError::Io(e.to_string()))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.profiles)
            .map_err(|e| ProfileError::Io(e.to_string()))?;
        std::fs::write(&self.path, bytes).map_err(|e| ProfileError::Io(e.to_string()))
    }

    /// Create a new non-default profile and persist.
    pub fn create(&mut self, display_name: &str) -> Result<ProfileMeta, ProfileError> {
        let meta = ProfileMeta {
            id: ProfileId::new(),
            display_name: display_name.trim().to_string(),
            created_at: now_secs(),
            is_default: false,
        };
        self.profiles.push(meta.clone());
        self.save()?;
        Ok(meta)
    }

    /// Delete a profile by id and persist. Refuses the Default profile.
    pub fn delete(&mut self, id: &ProfileId) -> Result<(), ProfileError> {
        if *id == ProfileId::default() {
            return Err(ProfileError::CannotDeleteDefault);
        }
        let before = self.profiles.len();
        self.profiles.retain(|p| p.id != *id);
        if self.profiles.len() == before {
            return Err(ProfileError::NotFound);
        }
        self.save()
    }

    /// Resolve an id-or-display-name string to a `ProfileId`. Display-name match is
    /// trimmed + case-insensitive.
    pub fn resolve(&self, id_or_name: &str) -> Option<ProfileId> {
        if let Ok(id) = id_or_name.parse::<ProfileId>() {
            if self.profiles.iter().any(|p| p.id == id) {
                return Some(id);
            }
        }
        let needle = id_or_name.trim().to_ascii_lowercase();
        self.profiles
            .iter()
            .find(|p| p.display_name.trim().to_ascii_lowercase() == needle)
            .map(|p| p.id)
    }
}
```

Finish the `lib.rs` re-export so `ProfileMeta`/`ProfileStore` are exported.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p forktty-core profile::`
Expected: all profile tests PASS (8 total).

- [ ] **Step 5: Clippy + fmt + commit**

Run: `cargo clippy -p forktty-core -- -D warnings && cargo fmt -p forktty-core`

```bash
git add crates/forktty-core/src/profile.rs crates/forktty-core/src/lib.rs crates/forktty-core/Cargo.toml
git commit -m "feat(core): SP3 P2 file-backed ProfileStore + ProfileMeta

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Thread `profile` through `SurfaceKind::Browser` + `open_browser`

**Files:**
- Modify: `crates/forktty-core/src/model.rs`
- Modify: `crates/forktty-core/src/events.rs:155`

- [ ] **Step 1: Write the failing test**

In `model.rs` tests (near the existing `open_browser_*` tests ~line 2977), add:

```rust
    #[test]
    fn open_browser_records_the_requested_profile() {
        let mut model = Model::new(); // match how sibling tests construct the model
        let ws = /* create a workspace + focused surface as sibling tests do */;
        let custom = ProfileId::new();
        let surface = model
            .open_browser(&ws, "https://example.com", custom, SplitAxis::Horizontal)
            .expect("opens");
        match surface.kind {
            SurfaceKind::Browser { ref url, profile } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(profile, custom);
            }
            _ => panic!("expected a browser surface"),
        }
    }

    #[test]
    fn legacy_browser_surface_without_profile_loads_as_default() {
        // serde default: JSON written before the profile field existed.
        let json = r#"{"type":"browser","url":"https://example.com"}"#;
        let kind: SurfaceKind = serde_json::from_str(json).unwrap();
        match kind {
            SurfaceKind::Browser { url, profile } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(profile, ProfileId::default());
            }
            _ => panic!("expected browser"),
        }
    }
```

(Read the existing `open_browser_adds_browser_surface_splits_and_focuses` test at
`model.rs:2977` and mirror its exact model/workspace construction in the first test.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p forktty-core`
Expected: compile failure — `open_browser` takes 3 args, `Browser` has no `profile`.

- [ ] **Step 3: Implement**

In `model.rs`, change the `SurfaceKind::Browser` variant (line ~45):

```rust
    Browser {
        url: String,
        #[serde(default)]
        profile: crate::profile::ProfileId,
    },
```

Change `open_browser` (line ~555) to take a profile:

```rust
    pub fn open_browser(
        &mut self,
        workspace_id: &str,
        url: &str,
        profile: crate::profile::ProfileId,
        axis: SplitAxis,
    ) -> Option<Surface> {
        let focused = self
            .workspaces
            .get(workspace_id)?
            .focused_surface_id
            .clone();
        let title = browser_title_for(url);
        self.split_with(
            &focused,
            axis,
            SurfaceKind::Browser {
                url: url.to_string(),
                profile,
            },
            title,
        )
    }
```

Fix every other `SurfaceKind::Browser { url }` construct/match site in `model.rs`
(lines ~661, ~2988, ~3011, ~3071) and `events.rs:155`. For read-only matches that
don't need the profile, use `Browser { url, .. }`. For the `set_surface_url` path
(~661 `Browser { url: current }`), keep it as `Browser { url: current, .. }`. Run
`grep -n "SurfaceKind::Browser" crates/forktty-core/src/*.rs` and update each.

Update the in-file `open_browser_*` tests at ~2977/3011/3071 that call `open_browser`
to pass `ProfileId::default()` as the new third argument.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p forktty-core`
Expected: all core tests pass (including the 2 new ones).

- [ ] **Step 5: Clippy + fmt + commit**

Run: `cargo clippy -p forktty-core -- -D warnings && cargo fmt -p forktty-core`

```bash
git add crates/forktty-core/src/model.rs crates/forktty-core/src/events.rs
git commit -m "feat(core): SP3 P2 carry ProfileId on Browser surfaces

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Socket verbs — profile CRUD + `browser.open --profile`

**Files:**
- Modify: `crates/forktty-socket/src/lib.rs`

**Context for the implementer:** Dispatch arms live in the big `match method { … }` in
`dispatch` (the `browser.open` arm is at ~line 733). `METHODS` is a sorted `&[&str]`
near the top (~line 45). Errors use `DispatchError` (`NotFound(String)`, and a
`String` variant via `?` on `Result<_, String>`). The socket has no direct data-dir
handle; resolve the profiles.json path with the same root core uses:
`dirs::data_local_dir().map(|d| d.join("forktty").join("browser_profiles").join("profiles.json"))`.
Add a small helper `fn profiles_store() -> Result<ProfileStore, DispatchError>` that
loads it (map `ProfileError`/missing-dir to a dispatch error string). Tests use the
headless harness; profile verbs touch only the store + model, no WebView.

- [ ] **Step 1: Write the failing test**

In the socket tests module (where `browser_open_creates_browser_surface_and_navigate_updates_url`
lives ~line 5095), add a test that drives the new verbs. Match the existing test
harness exactly (how it builds `state`, calls `dispatch`, reads the JSON result):

```rust
    #[tokio::test]
    async fn browser_profile_create_list_then_open_with_profile() {
        let (state, _backend) = test_state();
        // create
        let created = dispatch(&state, "browser.profile.create",
            json!({ "display_name": "Work" })).await.unwrap();
        let new_id = created.get("id").and_then(|v| v.as_str()).unwrap().to_string();
        // list includes Default + Work
        let listed = dispatch(&state, "browser.profile.list", json!({})).await.unwrap();
        let arr = listed.as_array().unwrap();
        assert!(arr.iter().any(|p| p["is_default"] == json!(true)));
        assert!(arr.iter().any(|p| p["display_name"] == json!("Work")));
        // open a pane in the Work profile (by name) — needs a workspace id from the harness
        let ws = /* workspace id, as other socket tests obtain it */;
        let opened = dispatch(&state, "browser.open",
            json!({ "workspace_id": ws, "url": "https://example.com", "profile": "Work" }))
            .await.unwrap();
        assert_eq!(opened["kind"], json!("browser")); // or however SurfaceSnap renders kind
        // delete Work
        dispatch(&state, "browser.profile.delete", json!({ "id": new_id })).await.unwrap();
    }
```

NOTE to implementer: the profiles.json path resolves to a real user dir; for test
isolation, if the harness does not already sandbox `$XDG_DATA_HOME`/`HOME`, set
`std::env::set_var("XDG_DATA_HOME", <tempdir>)` at the top of the test (and read how
sibling tests handle env/data-dir isolation — follow their pattern; do not pollute the
real `~/.local/share`). If sibling tests have no isolation pattern and the store would
write to the real home, report this as DONE_WITH_CONCERNS and propose injecting the
store path into `SocketAppState` instead.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p forktty-socket browser_profile`
Expected: failure — methods not handled / `dispatch` returns method-not-found.

- [ ] **Step 3: Implement the dispatch arms**

Add `profiles_store()` helper and arms in the `match method`:

```rust
        "browser.profile.list" => {
            let store = profiles_store()?;
            let out: Vec<_> = store
                .list()
                .iter()
                .map(|p| json!({
                    "id": p.id.to_string(),
                    "display_name": p.display_name,
                    "is_default": p.is_default,
                }))
                .collect();
            Ok(json!(out))
        }
        "browser.profile.create" => {
            let display_name = required_string_param(&params, "display_name")?.to_string();
            let mut store = profiles_store()?;
            let meta = store
                .create(&display_name)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "id": meta.id.to_string(), "display_name": meta.display_name }))
        }
        "browser.profile.delete" => {
            let id_str = required_string_param(&params, "id")?.to_string();
            let id: forktty_core::ProfileId = id_str
                .parse()
                .map_err(|_| DispatchError::NotFound("profile".to_string()))?;
            // Refuse if a live surface still uses this profile.
            {
                let model = state.model.lock().map_err(|_| "Lock poisoned".to_string())?;
                let in_use = model.list_surfaces(None).iter().any(|s| matches!(
                    &s.kind,
                    forktty_core::SurfaceKind::Browser { profile, .. } if *profile == id
                ));
                if in_use {
                    return Err(DispatchError::from(
                        "profile in use by an open browser pane".to_string(),
                    ));
                }
            }
            let mut store = profiles_store()?;
            store.delete(&id).map_err(|e| match e {
                forktty_core::ProfileError::NotFound => DispatchError::NotFound("profile".to_string()),
                other => DispatchError::from(other.to_string()),
            })?;
            Ok(json!({ "deleted": true }))
        }
```

(Adapt `DispatchError::from(String)` / the `?`-on-`Result<_,String>` form to however
this file converts a `String` into a `DispatchError` — grep for an existing
`.map_err(|_| "…".to_string())?` to copy the idiom. `list_surfaces(None)` lists across
all workspaces; confirm that signature, else iterate workspaces.)

Update the `browser.open` arm to resolve + pass a profile:

```rust
        "browser.open" => {
            let workspace_id = required_string_param(&params, "workspace_id")?.to_string();
            let url = required_browser_url(&params)?;
            let axis = split_axis_from_params(&params)?;
            let profile = match params.get("profile").and_then(|v| v.as_str()) {
                Some(s) => profiles_store()?
                    .resolve(s)
                    .ok_or(DispatchError::NotFound("profile".to_string()))?,
                None => forktty_core::ProfileId::default(),
            };
            let surface = {
                let mut model = state.model.lock().map_err(|_| "Lock poisoned".to_string())?;
                model
                    .open_browser(&workspace_id, &url, profile, axis)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
            };
            Ok(json!(surface))
        }
```

Add the three methods to `METHODS`, keeping it sorted:
`"browser.profile.create"`, `"browser.profile.delete"`, `"browser.profile.list"`.

Fix the `Browser { .. }` match at `lib.rs:1483` if the new field breaks it (it uses
`{ .. }`, so it should be fine — verify).

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p forktty-socket`
Expected: the new test + all existing socket tests pass.

- [ ] **Step 5: Clippy + fmt + commit**

Run: `cargo clippy -p forktty-socket -- -D warnings && cargo fmt -p forktty-socket`

```bash
git add crates/forktty-socket/src/lib.rs
git commit -m "feat(socket): SP3 P2 browser.profile.{list,create,delete} + open --profile

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: CLI subcommands + ui-gtk wiring

**Files:**
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`

**Context:** `socket_cli.rs` parses `forktty browser <sub>` via slice-patterns over
positionals from `parse_flags`, calls `send_socket_request(&ctx.socket_path, method,
params)`, and `print_json`. The SP2 helpers `browser_surface_cmd`, `browser_click`,
etc. are the pattern to copy. `HELP_TEXT` lists subcommands. In `gtk_app.rs`: the pane
is built in `browser_pane_widget` (calls `BrowserPaneWidget::new(DEFAULT_PROFILE_ID,
url)` from P1) — change it to pass the surface's profile; the two `open_browser`
callers (~5337, ~8925) must pass a `ProfileId`; the `Browser { url }` match sites
(~744, ~805, ~877) become `Browser { url, .. }` (or bind `profile` where needed).

- [ ] **Step 1: CLI — add `profile` subcommands + `--profile` on open**

In the `browser` subcommand dispatch in `socket_cli.rs`, add handling so:
- `forktty browser profile list` → method `browser.profile.list`, params `{}`.
- `forktty browser profile create <name>` → `browser.profile.create`, `{display_name}`.
- `forktty browser profile delete <id>` → `browser.profile.delete`, `{id}`.
- `forktty browser open [--profile <id|name>] <url>` → include `"profile"` in params
  when the flag is present (add `"profile"` to the recognized flags list for `open`).

Follow the exact positional slice-pattern + `parse_flags` + `send_socket_request` +
`print_json` idiom of the existing `browser_*` functions. Add explicit
too-many-args/missing-args arms returning a clear "usage" error, matching how
`browser_click`/`browser_fill` do it. Add the new lines to `HELP_TEXT`.

- [ ] **Step 2: ui-gtk — bind pane to its profile + pass profile to open_browser**

- In `browser_pane_widget`, replace the P1 `crate::browser_session::DEFAULT_PROFILE_ID`
  argument with the pane surface's profile string. Read the surface kind for this
  `surface_id` from the model to get its `ProfileId`; pass `profile.to_string()` to
  `BrowserPaneWidget::new`. (If the function already resolves the surface/url, reuse
  that lookup; otherwise look up the surface kind via the model accessor used nearby.)
  Fall back to `ProfileId::default().to_string()` if the surface isn't found.
- The two `model.open_browser(&ws, "about:blank", axis)` callers (~5337, ~8925): add
  `forktty_core::ProfileId::default()` as the third argument. (P2 has no per-pane
  profile picker in the GUI — the globe button always opens the Default profile; this
  is intentional per the spec.)
- Fix the `Browser { url }` match sites (~744, ~805, ~877, ~8942) to `Browser { url, .. }`.

- [ ] **Step 3: ui-gtk — remove a deleted profile's data dir (best-effort)**

There is no GUI delete path in P2, so no new GTK code is required for deletion; the
socket `browser.profile.delete` only removes metadata. Add a one-line doc comment at
the socket delete arm (Task 4) noting "on-disk data dir cleanup is deferred to the GUI
profile manager (P4 chrome work)". **Do not** delete directories from the socket thread
in P2 (a cached NetworkSession may still hold the dir). If you already added dir removal
in Task 4, remove it. (This step is a no-op verification — confirm no fs dir removal was
introduced.)

- [ ] **Step 4: Verify all crates build + test, both features**

Run:
```
cargo test -p forktty-core
cargo test -p forktty-socket
cargo build -p forktty-ui-gtk --features browser
cargo build -p forktty-ui-gtk --features gtk-vte
cargo clippy -p forktty-ui-gtk --features browser -- -D warnings
cargo clippy -p forktty-ui-gtk --features gtk-vte -- -D warnings
cargo fmt --all --check
cargo test -p forktty-ui-gtk --features browser
```
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/socket_cli.rs crates/forktty-ui-gtk/src/gtk_app.rs
git commit -m "feat(browser): SP3 P2 CLI profile verbs + bind panes to their profile

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

- [ ] **Step 6: Manual smoke (needs display)** — document in the PR:
  `forktty browser profile create Work`; open a pane in Default and one in Work
  (`forktty browser open --profile Work https://…`); log into the same site as two
  different users → the two panes are cookie-isolated; restart → both persist.

---

## Self-Review

**Spec coverage (P2 section):**
- `SurfaceKind::Browser { url, profile }` + serde default → Task 3. ✅
- `ProfileId(Uuid)`, Default == DEFAULT_PROFILE_ID → Task 1. ✅
- `ProfileStore` (profiles.json, list/create/delete, default synth, undeletable default) → Task 2. ✅
- socket `browser.profile.{list,create,delete}` + `browser.open --profile` (id-or-name) → Task 4. ✅
- in-use delete refusal → Task 4 (model scan). ✅
- CLI mirrors → Task 5 Step 1. ✅
- per-pane session binding by profile → Task 5 Step 2. ✅
- GUI per-pane profile switcher deferred → noted in Task 5 Step 2 + spec. ✅
- session restore reopens pane in its profile → serde default + binding handle this; legacy-load test in Task 3. ✅

**Placeholder scan:** Test bodies that say "as sibling tests do" require the implementer
to read one referenced test and mirror it — these are not placeholders for production
code, but the implementer MUST fill the model/workspace construction by copying the
named existing test. Flagged explicitly in Tasks 3 & 4.

**Type consistency:** `ProfileId` (Copy, Display, FromStr, serde transparent),
`ProfileStore::{load(PathBuf), list()->&[ProfileMeta], create(&str)->Result<ProfileMeta>,
delete(&ProfileId)->Result<(),ProfileError>, resolve(&str)->Option<ProfileId>}`,
`open_browser(ws, url, ProfileId, axis)` — used consistently across Tasks 1-5.

**Note for executor:** profiles.json path is resolved independently in core (tests
inject it) and socket (`dirs::data_local_dir()`); keep them pointing at the same real
location (`…/forktty/browser_profiles/profiles.json`). Watch test isolation in Task 4
(do not write to the real `~/.local/share`).
