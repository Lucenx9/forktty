# Browser pane SP3 — P1 Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Implemented. `browser_session.rs` provides
per-profile persistent `NetworkSession`s; P2 now passes the actual
`SurfaceKind::Browser.profile` value instead of only the fixed Default profile.

**Goal:** Browser panes persist cookies and website data across forktty restarts by binding each WebView to a persistent per-profile `NetworkSession` (P1 uses a single fixed Default profile).

**Architecture:** A new `browser_session.rs` owns the mapping profile-id → persistent `webkit6::NetworkSession`, rooted under `~/.local/share/forktty/browser_profiles/<id>/`, with sqlite cookie storage. `BrowserPaneWidget::new` takes a profile id and builds its WebView via `WebView::builder().network_session(&session).build()` instead of `WebView::new()`. P1 always passes the fixed Default profile id; the profile *system* (CRUD, per-pane selection) is P2.

**Tech Stack:** Rust, webkit6 0.5 (`NetworkSession`, `CookieManager`, `CookiePersistentStorage`), `dirs` crate (already a workspace dep), GTK4. All code behind the existing `browser` cargo feature.

**Phase context:** This is phase 1 of 4 in the SP3 epic (spec: `docs/superpowers/specs/2026-05-24-browser-pane-sp3-design.md`). P2 (profiles), P3 (history), P4 (import) get their own plans when reached. P1's Default profile directory name is the fixed UUID string `00000000-0000-0000-0000-000000000001` so P2's real `ProfileId::default()` resolves to the same on-disk directory — no migration, and P1 needs no new dependency.

---

## File Structure

- **Create** `crates/forktty-ui-gtk/src/browser_session.rs` — profile-id → persistent
  `NetworkSession` factory + cache + path helpers. Behind `#[cfg(feature = "browser")]`.
- **Modify** `crates/forktty-ui-gtk/src/browser_pane.rs` — `BrowserPaneWidget::new`
  gains a `profile_id: &str` parameter; WebView built from a session instead of
  `WebView::new()`.
- **Modify** `crates/forktty-ui-gtk/src/gtk_app.rs:824` — pass the Default profile id at
  the single `BrowserPaneWidget::new` callsite.
- **Modify** `crates/forktty-ui-gtk/src/lib.rs` (or wherever modules are declared) — add
  `mod browser_session;` under the `browser` feature gate.

---

## Task 1: Path helpers in `browser_session.rs`

Pure functions, unit-testable without GTK or a display.

**Files:**
- Create: `crates/forktty-ui-gtk/src/browser_session.rs`
- Modify: module declaration (see Task 4)

- [ ] **Step 1: Write the failing test**

Put this at the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_id_is_the_well_known_uuid_string() {
        assert_eq!(DEFAULT_PROFILE_ID, "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn profile_dirs_are_nested_under_profiles_root_by_id() {
        let root = std::path::Path::new("/tmp/ft-data");
        let dirs = ProfileDirs::under(root, "abc");
        assert_eq!(dirs.base, root.join("browser_profiles").join("abc"));
        assert_eq!(dirs.data, dirs.base.join("data"));
        assert_eq!(dirs.cache, dirs.base.join("cache"));
        assert_eq!(dirs.cookies_sqlite, dirs.base.join("cookies.sqlite"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk --features browser browser_session`
Expected: FAIL to compile — `DEFAULT_PROFILE_ID` / `ProfileDirs` not found.

- [ ] **Step 3: Write minimal implementation**

Top of `crates/forktty-ui-gtk/src/browser_session.rs`:

```rust
//! Persistent per-profile WebKit network sessions for browser panes (SP3 P1).
//! Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use std::path::{Path, PathBuf};

/// Well-known directory id for the Default profile. A fixed UUID string so that
/// SP3 P2's real `ProfileId::default()` resolves to this same on-disk directory
/// (no migration). P1 has no profile system, so this is the only profile used.
pub const DEFAULT_PROFILE_ID: &str = "00000000-0000-0000-0000-000000000001";

/// On-disk locations for one profile's browser data.
pub struct ProfileDirs {
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
        Self { base, data, cache, cookies_sqlite }
    }
}

/// The forktty data root (`~/.local/share/forktty`), matching the rest of the app
/// (see `cli.rs` `dirs::data_dir().join("forktty")`). `None` if the platform has no
/// data dir.
pub fn data_root() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("forktty"))
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p forktty-ui-gtk --features browser browser_session`
Expected: PASS (2 tests). Note the module must be declared first — if it errors with
"file not found for module", do Task 4 Step 1 now, then re-run.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/browser_session.rs
git commit -m "feat(browser): SP3 P1 profile directory path helpers

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: `session_for` — persistent NetworkSession factory + cache

Creates (or returns a cached) persistent `NetworkSession` for a profile, ensuring its
directories exist and wiring sqlite cookie persistence. GTK-main-thread only, so the
cache is a `thread_local!` (no locking). Session creation needs WebKit initialized at
runtime, so it is verified by build + manual smoke, not an automated test.

**Files:**
- Modify: `crates/forktty-ui-gtk/src/browser_session.rs`

- [ ] **Step 1: Add the factory + cache**

Append to `browser_session.rs` (above the `tests` module):

```rust
use std::cell::RefCell;
use std::collections::HashMap;

use webkit6::prelude::*;
use webkit6::{CookiePersistentStorage, NetworkSession};

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
    if let Some(existing) =
        SESSIONS.with(|c| c.borrow().get(profile_id).cloned())
    {
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
    std::fs::create_dir_all(&dirs.data).ok()?;
    std::fs::create_dir_all(&dirs.cache).ok()?;

    let session = NetworkSession::new(
        Some(dirs.data.to_str()?),
        Some(dirs.cache.to_str()?),
    );
    if let Some(cookie_manager) = session.cookie_manager() {
        if let Some(cookies_path) = dirs.cookies_sqlite.to_str() {
            cookie_manager
                .set_persistent_storage(cookies_path, CookiePersistentStorage::Sqlite);
        }
    }
    Some(session)
}
```

- [ ] **Step 2: Verify it compiles (both features)**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: builds clean.
Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: builds clean (the module is `#![cfg(feature = "browser")]`, so it's absent
here — no error).

- [ ] **Step 3: Clippy + fmt**

Run: `cargo clippy -p forktty-ui-gtk --features browser -- -D warnings && cargo fmt -p forktty-ui-gtk`
Expected: no warnings, no diff.

- [ ] **Step 4: Commit**

```bash
git add crates/forktty-ui-gtk/src/browser_session.rs
git commit -m "feat(browser): SP3 P1 persistent NetworkSession factory + cache

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: Bind the WebView to the profile session

`BrowserPaneWidget::new` takes a `profile_id` and builds its WebView from
`session_for(profile_id)` instead of `WebView::new()`.

**Files:**
- Modify: `crates/forktty-ui-gtk/src/browser_pane.rs:49` (signature) and `:68` (WebView)

- [ ] **Step 1: Change the constructor signature**

In `browser_pane.rs`, change:

```rust
    pub fn new(initial_url: &str) -> Self {
```

to:

```rust
    pub fn new(profile_id: &str, initial_url: &str) -> Self {
```

- [ ] **Step 2: Build the WebView from the profile session**

Replace (line ~68):

```rust
        let web_view = WebView::new();
        web_view.set_vexpand(true);
```

with:

```rust
        let session = crate::browser_session::session_for(profile_id);
        let web_view = WebView::builder().network_session(&session).build();
        web_view.set_vexpand(true);
```

- [ ] **Step 3: Verify it fails to build at the callsite**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: FAIL — `gtk_app.rs:824` calls `BrowserPaneWidget::new(url)` with one arg.
(This confirms the next task's edit is needed.)

- [ ] **Step 4: Commit (will pass after Task 4)**

Defer the commit; Task 4 fixes the callsite and the two changes commit together. Skip
to Task 4. (No commit in this task.)

---

## Task 4: Module declaration + callsite + smoke

Declare the new module, pass the Default profile id at the callsite, verify both
feature builds, and document the manual smoke test.

**Files:**
- Modify: module declaration file (the file with the other `mod browser_*;` lines)
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs:824`

- [ ] **Step 1: Declare the module**

Find where `browser_pane` is declared (grep for `mod browser_pane`). Add next to it,
matching its feature gate:

```rust
#[cfg(feature = "browser")]
mod browser_session;
```

Run to find the exact file/line:
`grep -rn "mod browser_pane" crates/forktty-ui-gtk/src`

- [ ] **Step 2: Update the callsite**

In `gtk_app.rs:824`, change:

```rust
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(url));
```

to:

```rust
        let pane = Rc::new(crate::browser_pane::BrowserPaneWidget::new(
            crate::browser_session::DEFAULT_PROFILE_ID,
            url,
        ));
```

- [ ] **Step 3: Verify both feature builds + clippy + fmt**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: builds clean (callsite now matches the 2-arg signature).
Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: builds clean.
Run: `cargo clippy -p forktty-ui-gtk --features browser -- -D warnings && cargo fmt -p forktty-ui-gtk --check`
Expected: no warnings, no diff.

- [ ] **Step 4: Commit**

```bash
git add crates/forktty-ui-gtk/src/browser_pane.rs crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/lib.rs
git commit -m "feat(browser): SP3 P1 persist browser panes via per-profile session

Browser panes now bind to a persistent NetworkSession rooted under
~/.local/share/forktty/browser_profiles/<id>, so cookies and website data
survive restarts. P1 uses the fixed Default profile.

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

(Adjust the `git add` paths to the actual module-declaration file if it isn't
`lib.rs`.)

- [ ] **Step 5: Manual smoke (needs a display)**

Document the result in the PR; this cannot be automated (WebView needs a display).

1. `cargo run -p forktty-ui-gtk --features browser` (or the app's run command).
2. Open a browser pane (globe button), navigate to a site, log in.
3. Confirm `~/.local/share/forktty/browser_profiles/00000000-0000-0000-0000-000000000001/cookies.sqlite`
   exists and is non-empty: `ls -l` it.
4. Quit forktty, relaunch, reopen a browser pane to that site → still logged in.

---

## Self-Review

**Spec coverage (P1 section of the SP3 spec):**
- "per-pane persistent `NetworkSession`" → Task 2 `session_for` + Task 3 builder bind. ✅
- "data_dir = …/browser_profiles/<id>/data, cache likewise" → Task 1 `ProfileDirs`. ✅
- "set_persistent_storage(cookies.sqlite, Sqlite)" → Task 2 `build_persistent_session`. ✅
- "one fixed Default profile id" → Task 1 `DEFAULT_PROFILE_ID`, Task 4 callsite. ✅
- "reuse one session per profile across panes" → Task 2 `thread_local SESSIONS` cache. ✅
- "fall back to ephemeral on dir failure, log, don't crash" → Task 2 `unwrap_or_else`. ✅
- "no core change in P1" → confirmed; only ui-gtk touched. ✅
- "both feature builds compile" → Tasks 2 & 4 verify `browser` and `gtk-vte`. ✅

**Placeholder scan:** No TBD/TODO; every code step shows full code. ✅

**Type consistency:** `DEFAULT_PROFILE_ID: &str`, `ProfileDirs::under(&Path, &str)`,
`session_for(&str) -> NetworkSession`, `BrowserPaneWidget::new(&str, &str)` — used
consistently across Tasks 1-4. ✅

**Note for executor:** Task 3 intentionally has no commit; its build *fails* until Task
4 fixes the callsite, so the two tasks commit together at Task 4 Step 4. This is the one
deviation from one-commit-per-task and is deliberate (an API signature change spanning
two files).
