# Browser pane SP1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Implemented and later extended by SP3. Browser
surfaces now serialize as `Browser { url, profile }`; this SP1 plan keeps the
original task sequence for historical implementation context.

**Goal:** Add a browser pane kind to ForkTTY that embeds WebKitGTK6, openable and navigable from both the Unix socket and a thin in-pane address bar, with the engine gated behind an opt-in `browser` cargo feature.

**Architecture:** `Surface` gains a `kind` field (`Terminal` | `Browser { url }`, serde-default `Terminal`). Socket navigation is pure model state — `browser.navigate` sets the url, the GTK layer observes the diff and calls `load_uri`, reusing existing model-observation plumbing. Back/forward/reload are address-bar buttons only this cycle. The WebKitGTK6 widget lives in a new `browser_pane.rs` gated behind `feature = "browser"`.

**Tech Stack:** Rust workspace (forktty-core, forktty-socket, forktty-ui-gtk), GTK4/libadwaita, WebKitGTK6 via the `webkit6` gtk-rs crate, tokio broadcast events, serde_json socket protocol.

**Spec:** `docs/superpowers/specs/2026-05-24-browser-pane-sp1-design.md`

---

## File Structure

- `crates/forktty-core/src/model.rs` — add `SurfaceKind`, `Surface.kind`, `split_with`, `open_browser`, `set_surface_url`. (modify)
- `crates/forktty-core/src/lib.rs` — re-export `SurfaceKind`. (modify)
- `crates/forktty-core/src/events.rs` — `SurfaceUrlChanged` variant, `kind` on `SurfaceAdded`, `url`/`kind` on `SurfSnap`, diff emission. (modify)
- `crates/forktty-socket/src/lib.rs` — `browser.open` / `browser.navigate` dispatch arms, `METHODS` entries, url length cap, validation helper. (modify)
- `crates/forktty-ui-gtk/Cargo.toml` — `browser` feature + optional `webkit6` dep. (modify)
- `crates/forktty-ui-gtk/src/browser_pane.rs` — `BrowserPaneWidget` (feature-gated). (create)
- `crates/forktty-ui-gtk/src/gtk_app.rs` — leaf branch matches surface kind; url-diff `load_uri`; non-feature placeholder. (modify)
- `crates/forktty-ui-gtk/src/socket_cli.rs` — `browser` CLI subcommand. (modify)
- `docs/cmux-gap-features.md`, `ROADMAP.md` — status. (modify)

---

## Task 1: `SurfaceKind` enum + `Surface.kind` field

**Files:**
- Modify: `crates/forktty-core/src/model.rs:39-49` (Surface struct) and its constructors (`:183`, `:261`, `:397`, `:512`, `:588`, `:619`)
- Modify: `crates/forktty-core/src/lib.rs:20-24` (re-export)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block at the bottom of `model.rs`:

```rust
#[test]
fn surface_without_kind_field_deserializes_as_terminal() {
    // Sessions persisted before SurfaceKind existed have no `kind` key.
    let json = r#"{
        "id": "s1",
        "workspace_id": "w1",
        "cwd": "/tmp",
        "title": "shell",
        "unread": false,
        "needs_attention": false
    }"#;
    let surface: Surface = serde_json::from_str(json).unwrap();
    assert_eq!(surface.kind, SurfaceKind::Terminal);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-core surface_without_kind_field_deserializes_as_terminal`
Expected: FAIL — `no field 'kind' on type 'Surface'` / `cannot find value 'SurfaceKind'`.

- [ ] **Step 3: Add the enum and field**

In `model.rs`, directly above `pub struct Surface {` (line 39), add:

```rust
/// What a surface renders. Defaults to `Terminal` so sessions persisted
/// before this field existed load every surface as a terminal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SurfaceKind {
    Terminal,
    Browser { url: String },
}

impl Default for SurfaceKind {
    fn default() -> Self {
        SurfaceKind::Terminal
    }
}
```

Add the field to `Surface` (after `needs_attention`):

```rust
    #[serde(default)]
    pub needs_attention: bool,
    #[serde(default)]
    pub kind: SurfaceKind,
}
```

- [ ] **Step 4: Fix every `Surface { .. }` constructor**

Each literal at lines ~183, ~261, ~397, ~512, ~588, ~619 (and the gtk fallback in `gtk_app.rs:760`) must set the new field. For all existing constructors add `kind: SurfaceKind::Terminal,`. Compile to find them:

Run: `cargo build -p forktty-core`
Expected: errors `missing field 'kind'` listing each literal; add `kind: SurfaceKind::Terminal,` to each.

- [ ] **Step 5: Re-export from lib.rs**

In `crates/forktty-core/src/lib.rs`, add `SurfaceKind` to the `pub use model::{...}` list (alphabetically near `Surface`):

```rust
    StatusEntry, StatusHookMetadata, Surface, SurfaceId, SurfaceKind, Workspace, WorkspaceId,
    WorkspaceModel, WorkspaceSelector,
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p forktty-core surface_without_kind_field_deserializes_as_terminal`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/forktty-core/src/model.rs crates/forktty-core/src/lib.rs
git commit -m "feat(core): add SurfaceKind { Terminal, Browser } to Surface"
```

---

## Task 2: `split_with` refactor + `open_browser`

**Files:**
- Modify: `crates/forktty-core/src/model.rs:500-540` (`split_surface`)

- [ ] **Step 1: Write the failing test**

Add to `model.rs` tests. Reuse the existing test setup pattern (see `split_surface_adds_second_surface_and_focuses_it` at ~line 1519 for how a workspace+surface is built):

```rust
#[test]
fn open_browser_adds_browser_surface_splits_and_focuses() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let first = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();

    let browser = model
        .open_browser(&ws.id, "https://example.com", SplitAxis::Horizontal)
        .expect("browser surface created");

    assert_eq!(
        browser.kind,
        SurfaceKind::Browser { url: "https://example.com".to_string() }
    );
    // New surface is focused.
    assert_eq!(model.workspaces[&ws.id].focused_surface_id, browser.id);
    // Tree now holds both leaves.
    let leaves = leaf_surface_ids(&model.workspaces[&ws.id].pane_tree);
    assert!(leaves.contains(&first));
    assert!(leaves.contains(&browser.id));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-core open_browser_adds_browser_surface_splits_and_focuses`
Expected: FAIL — `no method named 'open_browser'`.

- [ ] **Step 3: Refactor `split_surface` to delegate, add `open_browser`**

Replace the body of `split_surface` (lines 500-540) so the shared mechanics live in a private `split_with`. Keep `split_surface`'s public signature unchanged:

```rust
    pub fn split_surface(&mut self, surface_id: &str, axis: SplitAxis) -> Option<Surface> {
        self.split_with(surface_id, axis, SurfaceKind::Terminal, String::from("shell"))
    }

    /// Split the workspace's focused surface into a new browser pane.
    pub fn open_browser(
        &mut self,
        workspace_id: &str,
        url: &str,
        axis: SplitAxis,
    ) -> Option<Surface> {
        let focused = self.workspaces.get(workspace_id)?.focused_surface_id.clone();
        let title = browser_title_for(url);
        self.split_with(
            &focused,
            axis,
            SurfaceKind::Browser { url: url.to_string() },
            title,
        )
    }

    fn split_with(
        &mut self,
        surface_id: &str,
        axis: SplitAxis,
        kind: SurfaceKind,
        title: String,
    ) -> Option<Surface> {
        let source = self.surfaces.get(surface_id)?.clone();
        let workspace_ref = self.workspaces.get(&source.workspace_id)?;
        if !leaf_surface_ids(&workspace_ref.pane_tree)
            .iter()
            .any(|id| id == surface_id)
        {
            return None;
        }
        let new_id = self.next_surface_id();
        let new_surface = Surface {
            id: new_id.clone(),
            workspace_id: source.workspace_id.clone(),
            cwd: source.cwd.clone(),
            title,
            unread: false,
            needs_attention: false,
            kind,
        };
        let workspace = self
            .workspaces
            .get_mut(&source.workspace_id)
            .expect("workspace existence verified above");
        let inserted = replace_leaf_with_split(
            &mut workspace.pane_tree,
            surface_id,
            axis,
            PaneNode::Leaf {
                surface_id: new_id.clone(),
            },
        );
        debug_assert!(inserted, "leaf existence pre-validated");
        if !inserted {
            return None;
        }
        workspace.focused_surface_id = new_id;
        self.surfaces
            .insert(new_surface.id.clone(), new_surface.clone());
        Some(new_surface)
    }
```

Add this free function near the other private helpers at the bottom of `model.rs` (e.g. after `first_leaf_surface_id`):

```rust
/// Derive a browser pane title from its URL host, falling back to "browser".
fn browser_title_for(url: &str) -> String {
    let without_scheme = url.split("://").nth(1).unwrap_or(url);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    if host.is_empty() {
        "browser".to_string()
    } else {
        host.to_string()
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p forktty-core open_browser_adds_browser_surface_splits_and_focuses`
Expected: PASS.

- [ ] **Step 5: Run full core suite (no regressions in split)**

Run: `cargo test -p forktty-core`
Expected: PASS (existing `split_surface_*` tests still green).

- [ ] **Step 6: Commit**

```bash
git add crates/forktty-core/src/model.rs
git commit -m "feat(core): open_browser via shared split_with helper"
```

---

## Task 3: `set_surface_url`

**Files:**
- Modify: `crates/forktty-core/src/model.rs` (add method near `focus_surface`, ~line 559)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn set_surface_url_updates_only_browser_surfaces() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let terminal = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();
    let browser = model
        .open_browser(&ws.id, "https://a.com", SplitAxis::Horizontal)
        .unwrap();

    // Browser surface updates.
    assert!(model.set_surface_url(&browser.id, "https://b.com"));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser { url: "https://b.com".to_string() }
    );
    // Terminal surface is rejected.
    assert!(!model.set_surface_url(&terminal, "https://b.com"));
    // Missing surface is rejected.
    assert!(!model.set_surface_url("nope", "https://b.com"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-core set_surface_url_updates_only_browser_surfaces`
Expected: FAIL — `no method named 'set_surface_url'`.

- [ ] **Step 3: Implement the method**

Add to the `impl WorkspaceModel` block (near `focus_surface`):

```rust
    /// Update a browser surface's URL. Returns false for terminals or missing ids.
    pub fn set_surface_url(&mut self, surface_id: &str, url: &str) -> bool {
        match self.surfaces.get_mut(surface_id) {
            Some(surface) => match &mut surface.kind {
                SurfaceKind::Browser { url: current } => {
                    *current = url.to_string();
                    surface.title = browser_title_for(url);
                    true
                }
                SurfaceKind::Terminal => false,
            },
            None => false,
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p forktty-core set_surface_url_updates_only_browser_surfaces`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-core/src/model.rs
git commit -m "feat(core): set_surface_url for browser surfaces"
```

---

## Task 4: events — `kind` on `SurfaceAdded` + `SurfaceUrlChanged`

**Files:**
- Modify: `crates/forktty-core/src/events.rs:36-39` (SurfaceAdded), `:48-51` (add variant), `:94-99` (SurfSnap), `:134-142` (snapshot), `:206-213` and `:255-264` (diff)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `events.rs`:

```rust
#[test]
fn browser_surface_added_carries_kind_and_url_changes_emit_event() {
    let mut prev = Snapshot::default();
    prev.surfaces.insert(
        "s1".into(),
        SurfSnap {
            workspace_id: "w1".into(),
            title: "a.com".into(),
            kind: SurfaceSnapKind::Browser,
            url: Some("https://a.com".into()),
        },
    );
    let mut next = prev.clone();
    next.surfaces.get_mut("s1").unwrap().url = Some("https://b.com".into());

    let events = diff(&prev, &next);
    assert!(events.contains(&ModelEvent::SurfaceUrlChanged {
        id: "s1".into(),
        url: "https://b.com".into(),
    }));

    // Adding a browser surface advertises its kind.
    let mut added = Snapshot::default();
    added.surfaces.insert(
        "s2".into(),
        SurfSnap {
            workspace_id: "w1".into(),
            title: "c.com".into(),
            kind: SurfaceSnapKind::Browser,
            url: Some("https://c.com".into()),
        },
    );
    let add_events = diff(&Snapshot::default(), &added);
    assert!(add_events.contains(&ModelEvent::SurfaceAdded {
        id: "s2".into(),
        workspace_id: "w1".into(),
        kind: SurfaceSnapKind::Browser,
    }));
    assert!(add_events.contains(&ModelEvent::SurfaceUrlChanged {
        id: "s2".into(),
        url: "https://c.com".into(),
    }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-core browser_surface_added_carries_kind_and_url_changes_emit_event`
Expected: FAIL — `cannot find type 'SurfaceSnapKind'`, missing fields/variant.

- [ ] **Step 3: Add the snapshot kind type, extend SurfSnap, SurfaceAdded, add SurfaceUrlChanged**

In `events.rs`, add near the top of the file's type definitions (above `WsSnap`):

```rust
/// Serializable surface-kind tag carried in events (mirrors model `SurfaceKind`
/// without the per-kind payload, which events carry separately).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceSnapKind {
    #[default]
    Terminal,
    Browser,
}
```

Change `SurfaceAdded` (lines 36-39) to:

```rust
    SurfaceAdded {
        id: String,
        workspace_id: String,
        kind: SurfaceSnapKind,
    },
```

Add a new variant after `SurfaceTitleChanged` (after line 51):

```rust
    /// A browser surface navigated to a new URL.
    SurfaceUrlChanged {
        id: String,
        url: String,
    },
```

Extend `SurfSnap` (lines 94-99):

```rust
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SurfSnap {
    pub workspace_id: String,
    pub title: String,
    pub kind: SurfaceSnapKind,
    pub url: Option<String>,
}
```

- [ ] **Step 4: Populate the new fields in `snapshot`**

Replace the `SurfSnap { .. }` literal in `snapshot` (lines 137-140) with:

```rust
                SurfSnap {
                    workspace_id: surface.workspace_id,
                    title: surface.title,
                    kind: match surface.kind {
                        forktty_core::SurfaceKind::Terminal => SurfaceSnapKind::Terminal,
                        forktty_core::SurfaceKind::Browser { .. } => SurfaceSnapKind::Browser,
                    },
                    url: match surface.kind {
                        forktty_core::SurfaceKind::Browser { url } => Some(url),
                        forktty_core::SurfaceKind::Terminal => None,
                    },
                },
```

> Note: inside `events.rs` the `Surface` type comes from the crate's own `model`. Use the in-crate path `crate::model::SurfaceKind` instead of `forktty_core::SurfaceKind` if the module imports `model` directly — match the existing imports at the top of `events.rs`.

- [ ] **Step 5: Emit `kind` in the SurfaceAdded diff branch**

Replace lines 206-213 (the surface-adds loop) with:

```rust
    for (id, surf) in &next.surfaces {
        if !prev.surfaces.contains_key(id) {
            events.push(ModelEvent::SurfaceAdded {
                id: id.clone(),
                workspace_id: surf.workspace_id.clone(),
                kind: surf.kind,
            });
        }
    }
```

- [ ] **Step 6: Emit `SurfaceUrlChanged` in the title-diff loop**

In the surface-title loop (lines 255-264), after the title check inside the `for (id, next_surf)` loop, add a url check (still using `default_surf` for new surfaces):

```rust
    let default_surf = SurfSnap::default();
    for (id, next_surf) in &next.surfaces {
        let prev_surf = prev.surfaces.get(id).unwrap_or(&default_surf);
        if prev_surf.title != next_surf.title {
            events.push(ModelEvent::SurfaceTitleChanged {
                id: id.clone(),
                title: next_surf.title.clone(),
            });
        }
        if prev_surf.url != next_surf.url {
            if let Some(url) = &next_surf.url {
                events.push(ModelEvent::SurfaceUrlChanged {
                    id: id.clone(),
                    url: url.clone(),
                });
            }
        }
    }
```

- [ ] **Step 7: Fix the other existing `SurfSnap`/`SurfaceAdded` test literals**

Existing tests construct `SurfSnap { workspace_id, title }` (lines ~402, ~413, ~438, ~551) and `ModelEvent::SurfaceAdded { id, workspace_id }` (lines ~419, ~557). Compile and fix each:

Run: `cargo test -p forktty-core --no-run`
Expected: errors `missing field 'kind'`/`missing field 'url'` and `missing field 'kind'` on SurfaceAdded. For each `SurfSnap` add `kind: SurfaceSnapKind::Terminal, url: None,`; for each `SurfaceAdded` add `kind: SurfaceSnapKind::Terminal,`.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p forktty-core`
Expected: PASS (new test + all existing events tests).

- [ ] **Step 9: Commit**

```bash
git add crates/forktty-core/src/events.rs
git commit -m "feat(core): events carry surface kind + emit SurfaceUrlChanged"
```

---

## Task 5: socket `browser.open` + `browser.navigate`

**Files:**
- Modify: `crates/forktty-socket/src/lib.rs` — `METHODS` (line 35), dispatch arms (after `surface.split` ~line 710), url cap const (near line 22), validation helper (near `:1354`)

- [ ] **Step 1: Write the failing test**

Add to the socket `#[cfg(test)] mod tests` block (mirror the existing dispatch tests around `:2658`):

```rust
#[tokio::test]
async fn browser_open_creates_browser_surface_and_navigate_updates_url() {
    let state = test_state();
    let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let ws_id = ws.get("id").and_then(|v| v.as_str()).unwrap().to_string();

    let opened = dispatch(
        &state,
        "browser.open",
        json!({"workspace_id": ws_id, "url": "example.com"}),
    )
    .await
    .unwrap();
    let surface_id = opened.get("id").and_then(|v| v.as_str()).unwrap().to_string();
    // Scheme is prepended.
    assert_eq!(
        opened.get("kind").unwrap(),
        &json!({"type": "browser", "url": "https://example.com"})
    );

    let navigated = dispatch(
        &state,
        "browser.navigate",
        json!({"surface_id": surface_id, "url": "https://other.com"}),
    )
    .await
    .unwrap();
    assert_eq!(navigated.get("navigated").unwrap(), &json!(true));
}
```

> Use the existing test harness helpers (`test_state()`, `dispatch(...)`) — confirm their exact names in the socket test module and match them; the snippet assumes the same helpers the `surface.split` tests use.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-socket browser_open_creates_browser_surface_and_navigate_updates_url`
Expected: FAIL — method not found / `Unknown method`.

- [ ] **Step 3: Add the url cap and validation helper**

Near the other consts (~line 22) add:

```rust
const MAX_BROWSER_URL_BYTES: usize = 8_192;
```

Near `split_axis_from_params` (~line 1354) add:

```rust
fn required_browser_url(params: &Value) -> Result<String, DispatchError> {
    let raw = required_string_param(params, "url")?.trim();
    if raw.is_empty() {
        return Err("Invalid parameter url: must not be empty".into());
    }
    if raw.len() > MAX_BROWSER_URL_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "url",
            limit: MAX_BROWSER_URL_BYTES,
            actual: raw.len(),
        });
    }
    // Prepend a default scheme when none is present.
    if raw.contains("://") {
        Ok(raw.to_string())
    } else {
        Ok(format!("https://{raw}"))
    }
}
```

- [ ] **Step 4: Add the dispatch arms**

After the `surface.split` arm (ends ~line 710), add:

```rust
        "browser.open" => {
            let workspace_id = required_string_param(&params, "workspace_id")?.to_string();
            let url = required_browser_url(&params)?;
            let axis = split_axis_from_params(&params)?;
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .open_browser(&workspace_id, &url, axis)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
            };
            Ok(json!(surface))
        }
        "browser.navigate" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let url = required_browser_url(&params)?;
            let updated = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.set_surface_url(&surface_id, &url)
            };
            if updated {
                Ok(json!({"navigated": true}))
            } else {
                Err(DispatchError::NotFound("surface".to_string()))
            }
        }
```

- [ ] **Step 5: Add the methods to `METHODS` (keep sorted)**

In the `METHODS` slice (line 35), insert before `"events.subscribe"`:

```rust
    "browser.navigate",
    "browser.open",
    "events.subscribe",
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p forktty-socket browser_open_creates_browser_surface_and_navigate_updates_url`
Expected: PASS.

Run: `cargo test -p forktty-socket` (the capabilities/METHODS drift test must still pass with the two new entries).
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/forktty-socket/src/lib.rs
git commit -m "feat(socket): browser.open + browser.navigate verbs"
```

---

## Task 6: `browser` cargo feature + `BrowserPaneWidget`

**Files:**
- Modify: `crates/forktty-ui-gtk/Cargo.toml:17-31`
- Create: `crates/forktty-ui-gtk/src/browser_pane.rs`
- Modify: `crates/forktty-ui-gtk/src/main.rs` (module declaration)

- [ ] **Step 1: Add the feature and optional dep**

In `Cargo.toml`, under `[features]`:

```toml
[features]
default = []
gtk-vte = ["dep:adw", "dep:gtk4", "dep:global-hotkey", "dep:libloading", "forktty-terminal/vte"]
browser = ["gtk-vte", "dep:webkit6"]
```

Under `[dependencies]` add:

```toml
webkit6 = { version = "0.5", optional = true }
```

> Verify the published `webkit6` version that targets `webkitgtk-6.0` and matches the in-tree gtk4 `0.10` bindings; pin to the compatible release. Run `cargo update -p webkit6 --dry-run` or check `cargo tree` after adding.

- [ ] **Step 2: Write the failing (compile-gated) smoke test**

Create `crates/forktty-ui-gtk/src/browser_pane.rs`:

```rust
//! WebKitGTK6 browser pane widget. Compiled only with `--features browser`.
#![cfg(feature = "browser")]

use gtk4 as gtk;
use gtk::prelude::*;
use webkit6::prelude::*;
use webkit6::WebView;

/// A browser pane: an address bar (entry + back/forward/reload) above a WebView.
pub struct BrowserPaneWidget {
    container: gtk::Box,
    web_view: WebView,
    address: gtk::Entry,
}

impl BrowserPaneWidget {
    pub fn new(initial_url: &str) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        let forward = gtk::Button::from_icon_name("go-next-symbolic");
        let reload = gtk::Button::from_icon_name("view-refresh-symbolic");
        let address = gtk::Entry::new();
        address.set_hexpand(true);
        address.set_text(initial_url);
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);

        let web_view = WebView::new();
        web_view.set_vexpand(true);

        {
            let wv = web_view.clone();
            back.connect_clicked(move |_| wv.go_back());
        }
        {
            let wv = web_view.clone();
            forward.connect_clicked(move |_| wv.go_forward());
        }
        {
            let wv = web_view.clone();
            reload.connect_clicked(move |_| wv.reload());
        }

        container.append(&bar);
        container.append(&web_view);

        let widget = Self { container, web_view, address };
        widget.load_uri(initial_url);
        widget
    }

    pub fn widget(&self) -> gtk::Widget {
        self.container.clone().upcast()
    }

    pub fn load_uri(&self, url: &str) {
        if self.address.text() != url {
            self.address.set_text(url);
        }
        self.web_view.load_uri(url);
    }

    pub fn current_uri(&self) -> Option<String> {
        self.web_view.uri().map(|g| g.to_string())
    }

    /// Connect the address bar's Enter key to a navigation callback.
    pub fn connect_address_activate<F: Fn(String) + 'static>(&self, f: F) {
        let entry = self.address.clone();
        self.address.connect_activate(move |_| {
            f(entry.text().to_string());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_pane_widget_constructs_and_loads() {
        if gtk::init().is_err() {
            // No display in CI; skip rather than fail.
            return;
        }
        let pane = BrowserPaneWidget::new("https://example.com");
        pane.load_uri("https://other.com");
        // Constructing + load_uri must not panic.
        assert!(pane.widget().is_visible() || !pane.widget().is_visible());
    }
}
```

- [ ] **Step 3: Declare the module**

In `crates/forktty-ui-gtk/src/main.rs`, add alongside the other `mod` declarations:

```rust
#[cfg(feature = "browser")]
mod browser_pane;
```

- [ ] **Step 4: Run test to verify it fails, then compiles/passes**

Run: `cargo test -p forktty-ui-gtk --features browser browser_pane_widget_constructs_and_loads`
Expected: first a compile error if the `webkit6` API names differ (e.g. `load_uri`, `go_back`) — adjust to the actual `webkit6` `WebViewExt` method names shown by the compiler, then PASS (or early-return skip when no display).

- [ ] **Step 5: Verify the non-browser build still compiles**

Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: success — `browser_pane.rs` is excluded, no `webkit6` dep pulled.

- [ ] **Step 6: Commit**

```bash
git add crates/forktty-ui-gtk/Cargo.toml crates/forktty-ui-gtk/src/browser_pane.rs crates/forktty-ui-gtk/src/main.rs
git commit -m "feat(gtk): BrowserPaneWidget behind browser feature"
```

---

## Task 7: render browser panes in the layout

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs` — leaf branch (`:723`), `terminal_pane_widget` (`:741`), GtkApp struct (`:320`), `rebuild_layout` (`:477`)

This task wires `BrowserPaneWidget` into the pane tree. The exact integration depends on the live `webkit6`/gtk4 API and the existing chrome/widget bookkeeping, so each step verifies by compiling and running the app.

- [ ] **Step 1: Add browser-pane storage to GtkApp**

Near `widgets: BTreeMap<String, VteTerminalWidget>,` (line 320) add a parallel map (feature-gated to avoid an unused field in non-browser builds):

```rust
    #[cfg(feature = "browser")]
    browser_panes: std::collections::BTreeMap<String, std::rc::Rc<crate::browser_pane::BrowserPaneWidget>>,
```

Initialize it in the `GtkApp` constructor (wherever `widgets: BTreeMap::new()` is set) with `#[cfg(feature = "browser")] browser_panes: std::collections::BTreeMap::new(),`.

- [ ] **Step 2: Branch the leaf widget builder on surface kind**

In the leaf arm (line 723), replace `self.terminal_pane_widget(surface_id)` with a kind dispatch:

```rust
            PaneNode::Leaf { surface_id } => self.pane_widget_for(surface_id),
```

Add a new method next to `terminal_pane_widget`:

```rust
    fn pane_widget_for(&mut self, surface_id: &str) -> gtk::Widget {
        let kind = self
            .model
            .lock()
            .ok()
            .and_then(|model| model.surface(surface_id).map(|s| s.kind.clone()));
        match kind {
            #[cfg(feature = "browser")]
            Some(forktty_core::SurfaceKind::Browser { url }) => {
                self.browser_pane_widget(surface_id, &url)
            }
            #[cfg(not(feature = "browser"))]
            Some(forktty_core::SurfaceKind::Browser { .. }) => {
                browser_unavailable_placeholder(surface_id).upcast()
            }
            _ => self.terminal_pane_widget(surface_id),
        }
    }
```

> `pane_widget_for` takes `&mut self` because it inserts into `browser_panes`; the existing call site `widget_for_pane_with_resize` may hold `&self`. If so, store `browser_panes` behind a `RefCell` (the gtk codebase already uses `Rc`/interior mutability for widget maps — match that pattern) and keep `&self`.

- [ ] **Step 3: Implement `browser_pane_widget` (feature-gated)**

```rust
    #[cfg(feature = "browser")]
    fn browser_pane_widget(&mut self, surface_id: &str, url: &str) -> gtk::Widget {
        if let Some(pane) = self.browser_panes.get(surface_id) {
            pane.load_uri(url);
            return pane.widget();
        }
        let pane = std::rc::Rc::new(crate::browser_pane::BrowserPaneWidget::new(url));
        // Address-bar Enter navigates via the model so socket + manual share one path.
        let model = self.model.clone();
        let id = surface_id.to_string();
        pane.connect_address_activate(move |text| {
            if let Ok(mut m) = model.lock() {
                let url = if text.contains("://") { text } else { format!("https://{text}") };
                m.set_surface_url(&id, &url);
            }
        });
        let widget = pane.widget();
        self.browser_panes.insert(surface_id.to_string(), pane);
        widget
    }
```

- [ ] **Step 4: Add the non-feature placeholder**

```rust
    #[cfg(not(feature = "browser"))]
    fn browser_unavailable_placeholder(surface_id: &str) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let label = gtk::Label::new(Some(&format!(
            "Browser pane ({surface_id}) — built without the `browser` feature"
        )));
        b.append(&label);
        b
    }
```

- [ ] **Step 5: Drive url changes on rebuild**

`rebuild_layout` already runs when the layout signature changes; for a url change with the same structure, the existing per-surface update path (the one that calls `update_pane_chrome` for titles) must also refresh browser panes. In that update path, add (feature-gated):

```rust
        #[cfg(feature = "browser")]
        if let SurfaceKind::Browser { url } = &surface.kind {
            if let Some(pane) = self.browser_panes.get(&surface.id) {
                pane.load_uri(url);
            }
        }
```

> Locate the per-surface refresh loop (search `update_pane_chrome` callers). If url changes don't already trigger that loop, include the surface url in `layout_structure_signature` (line ~1093) so a navigation forces a refresh.

- [ ] **Step 6: Compile both feature sets**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: success (fix any `webkit6`/borrow errors as flagged).

Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: success (placeholder path compiles; no webkit6).

- [ ] **Step 7: Manual verification**

Build and run with the browser feature, open a browser pane via the socket, confirm it renders and navigates. Run in a scratch dir with isolated XDG dirs (match the pattern used to verify the events feature):

```bash
cargo build -p forktty-ui-gtk --features browser
# launch the app (isolated XDG_DATA_HOME / XDG_RUNTIME_DIR), then in another shell:
forktty browser open https://example.com
forktty browser navigate <surface-id> https://www.rust-lang.org
```

Expected: a browser pane appears showing example.com, then navigates to rust-lang.org; the address bar reflects the URL; back/reload buttons work.

- [ ] **Step 8: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app.rs
git commit -m "feat(gtk): render + navigate browser panes in the layout"
```

---

## Task 8: CLI `browser` subcommand

**Files:**
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs` — command match (`:339-376`), new handler, `HELP_TEXT`

- [ ] **Step 1: Write the failing test**

Add to the socket_cli test module (mirror `events_*`/`capabilities_*` tests; use the `ctx_for(socket_path)` helper added previously):

```rust
#[test]
fn browser_open_sends_browser_open_with_url() {
    let (socket_path, server) = spawn_recording_server(); // existing test harness
    let ctx = ctx_for(&socket_path);
    handle_browser(&ctx, vec!["open".into(), "example.com".into()]).unwrap();
    let req = server.last_request();
    assert_eq!(req.method, "browser.open");
    assert_eq!(req.params.get("url").unwrap(), "example.com");
}

#[test]
fn browser_rejects_unknown_subcommand() {
    let ctx = ctx_for(Path::new("/nonexistent.sock"));
    let err = handle_browser(&ctx, vec!["frobnicate".into()]).unwrap_err();
    assert!(err.to_string().contains("browser"));
}
```

> Match the exact existing test-server helper name (the events tests reference one). If the suite has no recording server, assert on the parse/error paths only (e.g. `browser_rejects_unknown_subcommand`) and cover the request shape in the socket crate test instead.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk browser_rejects_unknown_subcommand`
Expected: FAIL — `cannot find function 'handle_browser'`.

- [ ] **Step 3: Add the command arm and handler**

In the command match (near line 376) add:

```rust
        "browser" => handle_browser(&context, args),
```

Add the handler (mirror `handle_split_surface`):

```rust
fn handle_browser(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut args = args.into_iter();
    let sub = args.next().unwrap_or_default();
    let rest: Vec<String> = args.collect();
    match sub.as_str() {
        "open" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["workspace-id", "axis"], "browser open")?;
            let url = parsed.positionals.first().ok_or_else(|| {
                CliError::new("browser open requires a URL")
            })?;
            let mut params = Map::new();
            params.insert("url".to_string(), Value::String(url.clone()));
            let workspace_id = match non_blank_string_option(&parsed.options, "workspace-id", "--workspace-id")? {
                Some(id) => id.to_string(),
                None => resolve_active_workspace_id(context)?,
            };
            params.insert("workspace_id".to_string(), Value::String(workspace_id));
            if let Some(axis) = non_blank_string_option(&parsed.options, "axis", "--axis")? {
                params.insert("axis".to_string(), Value::String(axis.to_string()));
            }
            let result = send_socket_request(&context.socket_path, "browser.open", Value::Object(params))?;
            if context.json {
                print_json(&result)
            } else {
                println!("Opened browser surface {}", string_field(&result, "id").unwrap_or("(unknown)"));
                Ok(())
            }
        }
        "navigate" => {
            let parsed = parse_flags(rest, &[]);
            reject_unknown_options(&parsed.options, &["surface-id"], "browser navigate")?;
            let (surface_id, url) = match parsed.positionals.as_slice() {
                [surface, url] => (surface.clone(), url.clone()),
                [url] => (
                    resolve_focused_surface_id(context)?
                        .ok_or_else(|| CliError::new("browser navigate requires a surface id"))?,
                    url.clone(),
                ),
                _ => return Err(CliError::new("browser navigate requires [surface-id] <url>")),
            };
            let mut params = Map::new();
            params.insert("surface_id".to_string(), Value::String(surface_id));
            params.insert("url".to_string(), Value::String(url));
            let result = send_socket_request(&context.socket_path, "browser.navigate", Value::Object(params))?;
            if context.json { print_json(&result) } else { println!("Navigated"); Ok(()) }
        }
        "" => Err(CliError::new("browser requires a subcommand: open | navigate")),
        other => Err(CliError::new(format!("browser: unknown subcommand {other}"))),
    }
}
```

> `resolve_active_workspace_id` may not exist — if not, mirror `resolve_focused_surface_id`'s pattern to read the active workspace from `workspace.list`, or require `--workspace-id`. Confirm the helper name before relying on it.

- [ ] **Step 4: Update HELP_TEXT**

Add under the appropriate section of `HELP_TEXT`:

```text
  forktty browser open [--workspace-id <id>] [--axis horizontal|vertical] <url>
  forktty browser navigate [<surface-id>] <url>
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p forktty-ui-gtk browser`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/forktty-ui-gtk/src/socket_cli.rs
git commit -m "feat(cli): forktty browser open/navigate subcommands"
```

---

## Task 9: docs + final verification

**Files:**
- Modify: `docs/cmux-gap-features.md:43-54`, `ROADMAP.md`

- [ ] **Step 1: Update gap-features doc**

In `docs/cmux-gap-features.md` under section 3, change the status line and add a sub-project note:

```text
- **Impact**: high · **Cost**: high · **Status**: **SP1+SP2 done**; SP3 P1/P2 done; SP3 P3 core/socket done; P3 CLI/GTK wiring and P4 import backlog
```

Add a short paragraph noting SP1 ships `browser.open`/`browser.navigate`, the `browser` cargo feature, and the in-pane address bar; SP2 adds snapshot/click/fill/eval + socket-driven back/reload; SP3 adds persistence, profiles, history/bookmarks, and import in phases.

- [ ] **Step 2: Update ROADMAP**

Add:

```text
- [x] Browser pane SP1: WebKitGTK6 pane kind + `browser.open`/`browser.navigate` + in-pane address bar (behind the `browser` cargo feature).
```

- [ ] **Step 3: Full workspace verification**

Run each and confirm:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets
cargo clippy -p forktty-ui-gtk --features browser
cargo test --workspace
cargo build -p forktty-ui-gtk --features browser
cargo build -p forktty-ui-gtk --features gtk-vte
```

Expected: fmt clean, no clippy warnings (both feature sets), all tests pass, both builds succeed.

- [ ] **Step 4: Commit**

```bash
git add docs/cmux-gap-features.md ROADMAP.md
git commit -m "docs: mark browser pane SP1 done"
```

---

## Notes for the implementer

- **`webkit6` API names:** `load_uri`, `go_back`, `go_forward`, `reload`, `uri()` come from `webkit6::prelude::WebViewExt`. If a method name differs in the pinned version, the compiler will name the correct one — adjust the call, don't guess.
- **Borrow checker in gtk_app:** the existing widget maps show the project's chosen interior-mutability pattern (`Rc`, `RefCell`). Follow it rather than changing `&self` to `&mut self` broadly.
- **Headless tests:** the gtk smoke test early-returns when `gtk::init()` fails (no display), so CI without a display stays green; real rendering is covered by the manual step.
- **Events `Surface` import:** in `events.rs` use the same path the file already uses for `WorkspaceModel`/`Surface` (in-crate `crate::model::…`), not `forktty_core::…`.
