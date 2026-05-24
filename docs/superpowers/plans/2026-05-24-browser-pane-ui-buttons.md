# Browser Pane UI Buttons Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Implemented. SP3 later changed
`WorkspaceModel::open_browser` to require a `ProfileId`; the GUI open-browser
button now opens `about:blank` with the Default profile.

**Goal:** Add a globe button to the pane chrome that opens a blank browser pane (address bar focused), and a close (×) button on browser panes — both absent when the `browser` cargo feature is off.

**Architecture:** Pure GTK glue in `forktty-ui-gtk`. A feature-gated `open_browser_active` helper mirrors `split_active_surface` but calls SP1's `WorkspaceModel::open_browser(ws, "about:blank", axis)` (no PTY spawn). The globe button is added to the terminal pane action chrome and the single-pane overlay under `#[cfg(feature = "browser")]`. `BrowserPaneWidget` gains a × button + `connect_close` callback, wired by gtk_app to the existing `show_close_pane_confirmation`.

**Tech Stack:** Rust, GTK4 (gtk4 crate), webkit6 (behind `browser` feature). No new deps.

---

## Background the implementer needs

- Crate: `crates/forktty-ui-gtk`. WebKit behind `browser = ["gtk-vte", "dep:webkit6"]`; GTK behind `gtk-vte`. Builds: `cargo build -p forktty-ui-gtk --features browser` and `--features gtk-vte`.
- `crates/forktty-ui-gtk/src/browser_pane.rs` is file-gated `#![cfg(feature = "browser")]`. `BrowserPaneWidget::new(initial_url)` builds a vertical `container` Box; inside it a horizontal `bar` Box gets `back`, `forward`, `reload` buttons then `address` (a `gtk::Entry` with `set_hexpand(true)`). The struct fields include `container`, `web_view`, `address`, `last_requested`. It already has `connect_address_activate<F: Fn(String)+'static>` and `connect_focus_in<F: Fn()+'static>`.
- `crates/forktty-ui-gtk/src/gtk_app.rs`:
  - `pane_action_button(icon_name: &str, tooltip: &str) -> gtk::Button` (~line 1064).
  - Terminal pane chrome (~line 939): an `actions` Box gets `split_h` (`view-dual-symbolic`), `split_v` (`view-paged-symbolic`), `close_separator`, `close` (`window-close-symbolic`). A `single_pane_actions` overlay Box (~line 960) gets `single_split_h`, `single_split_v` only. Click wiring is in an `if let Some(state) = state { ... } else { <set buttons insensitive> }` block (~lines 975-1018).
  - `focus_surface_and(state: &SocketAppState, surface_id: &str, f: impl FnOnce(&SocketAppState))` (~line 1075) focuses the surface then runs `f`.
  - `split_active_surface(state, axis)` (~line 5211): locks `state.model`, gets `model.active_workspace()`, calls `model.split_surface(&workspace.focused_surface_id, axis)`, then spawns a terminal. Browser panes need NO terminal spawn.
  - `show_close_pane_confirmation(parent, state, surface_id)` (~line 1131): the close path terminals use.
  - `browser_pane_widget(&self, surface_id: &str, url: &str) -> gtk::Widget` (~line 817): constructs `BrowserPaneWidget`, wires `connect_address_activate` and `connect_focus_in`, inserts into `self.browser_panes`. It has access to `self.model`, and the function lives on the controller. The terminal chrome's `parent` for the confirmation dialog: see how `show_close_pane_confirmation` is called from the terminal `close.connect_clicked` (~line 1009) — it captures a `parent` (a `&adw::ApplicationWindow` or similar) and `state`. Determine the equivalent handle available in `browser_pane_widget` (the controller holds `self.state`; for the window parent, follow whatever `browser_pane_widget` or nearby controller methods already use to reach the toplevel window — grep for how other controller-side code obtains the parent window).
- `WorkspaceModel::open_browser(workspace_id: &str, url: &str, profile: ProfileId, axis: SplitAxis) -> Option<Surface>` exists. `model.active_workspace() -> Option<&Workspace>`; `Workspace` has `.id: String`.
- `SplitAxis` is imported in gtk_app.rs already (used by split buttons).
- `save_session_from_state(state)` persists after structural changes (called at the end of `split_active_surface`).
- No headless tests for button clicks exist (split/close buttons have none — they need a display). This feature adds no automated tests; verification is build + clippy + fmt on both feature sets, plus manual smoke.
- Commit trailer: `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`.

---

## Task 1: Close (×) button on BrowserPaneWidget

**Files:**
- Modify: `crates/forktty-ui-gtk/src/browser_pane.rs`

- [ ] **Step 1: add the × button to the address bar row**

In `BrowserPaneWidget::new`, the `bar` Box currently does:
```rust
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);
```
Add a close button constructed alongside the others (near where `back`/`forward`/`reload` are created):
```rust
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Close Pane"));
        close.add_css_class("pane-close-action");
```
And append it LAST, after `address` (the entry keeps `hexpand`, so × sits flush right):
```rust
        bar.append(&back);
        bar.append(&forward);
        bar.append(&reload);
        bar.append(&address);
        bar.append(&close);
```

- [ ] **Step 2: store the close button on the struct**

Add a field to `struct BrowserPaneWidget`:
```rust
    close: gtk::Button,
```
And set it in the `Self { ... }` constructor literal (alongside `container`, `web_view`, `address`, `last_requested`):
```rust
            close,
```
(Place the `close` field after `address` in both the struct def and the literal for consistency.)

- [ ] **Step 3: add connect_close**

Add to `impl BrowserPaneWidget` (near `connect_address_activate`):
```rust
    /// Connect the close (×) button to a callback. The widget does not own the
    /// model or the confirmation dialog, so gtk_app wires this to the same
    /// `show_close_pane_confirmation` path terminal panes use.
    pub fn connect_close<F: Fn() + 'static>(&self, f: F) {
        self.close.connect_clicked(move |_| f());
    }
```

- [ ] **Step 4: build + lint**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: compiles.
Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: compiles (file is gated; unaffected).
Run: `cargo fmt -p forktty-ui-gtk` then `cargo clippy -p forktty-ui-gtk --features browser --all-targets -- -D warnings`
Expected: clean. (If `connect_close` is flagged dead_code because Task 2 hasn't wired it yet, add `#[allow(dead_code)]` on `connect_close` with a comment that gtk_app wires it in Task 2; remove the allow in Task 2 if it becomes unnecessary — but since the whole impl block may already carry `#[allow(dead_code)]`, check first and only add if needed.)

- [ ] **Step 5: commit**

```bash
git add crates/forktty-ui-gtk/src/browser_pane.rs
git commit -m "feat(gtk): close button on browser pane

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Globe open-browser button + wire close

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`

- [ ] **Step 1: add the open_browser_active helper**

Add near `split_active_surface` (~line 5211), feature-gated:
```rust
#[cfg(feature = "browser")]
fn open_browser_active(state: &SocketAppState, axis: SplitAxis) {
    let opened = {
        let mut model = match state.model.lock() {
            Ok(model) => model,
            Err(_) => {
                eprintln!("Failed to open browser pane: workspace model lock poisoned");
                return;
            }
        };
        let Some(workspace) = model.active_workspace() else {
            return;
        };
        let workspace_id = workspace.id.clone();
        model.open_browser(&workspace_id, "about:blank", axis)
    };
    if opened.is_some() {
        // No PTY: browser surfaces have no child process. The chrome refresh
        // tick rebuilds the layout and focuses the new pane's address entry.
        save_session_from_state(state);
    }
}
```

- [ ] **Step 2: add the globe button to the terminal pane chrome**

In the pane chrome builder, after the `close` button is created and the `actions` appends happen (~line 953), add the globe button feature-gated. Where the code currently is:
```rust
    actions.append(&split_h);
    actions.append(&split_v);
    actions.append(&close_separator);
    actions.append(&close);
```
Insert the globe between the split buttons and the separator so order is split_h, split_v, globe, │, close:
```rust
    actions.append(&split_h);
    actions.append(&split_v);
    #[cfg(feature = "browser")]
    let open_browser = pane_action_button("globe-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    actions.append(&open_browser);
    actions.append(&close_separator);
    actions.append(&close);
```

- [ ] **Step 3: add the globe button to the single-pane overlay**

Where `single_pane_actions` is built (~line 971):
```rust
    single_pane_actions.append(&single_split_h);
    single_pane_actions.append(&single_split_v);
```
Add:
```rust
    #[cfg(feature = "browser")]
    let single_open_browser = pane_action_button("globe-symbolic", "Open Browser Pane");
    #[cfg(feature = "browser")]
    single_pane_actions.append(&single_open_browser);
```

- [ ] **Step 4: wire the globe clicks (and insensitive in the else branch)**

In the `if let Some(state) = state { ... }` block (~line 975), after the existing split wiring, add feature-gated handlers mirroring the split wiring (which uses `focus_surface_and(&state_clone, &sid_clone, |s| split_active_surface(s, axis))`):
```rust
        #[cfg(feature = "browser")]
        {
            let state_for_browser = state.clone();
            let sid_browser = surface_id_owned.clone();
            open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_browser, &sid_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
            let state_for_single_browser = state.clone();
            let sid_single_browser = surface_id_owned.clone();
            single_open_browser.connect_clicked(move |_| {
                focus_surface_and(&state_for_single_browser, &sid_single_browser, |s| {
                    open_browser_active(s, SplitAxis::Horizontal)
                });
            });
        }
```
Note `surface_id_owned` is the `let surface_id_owned = surface_id.to_string();` already present in this block — confirm the exact binding name by reading the surrounding split wiring and reuse it (do not introduce a new clone source). In the `else` branch (~line 1012, where split buttons are set insensitive) add feature-gated:
```rust
        #[cfg(feature = "browser")]
        {
            open_browser.set_sensitive(false);
            single_open_browser.set_sensitive(false);
        }
```

- [ ] **Step 5: wire the browser pane × to the close confirmation**

In `browser_pane_widget` (~line 817), where `connect_address_activate` and `connect_focus_in` are wired on `pane`, add a `connect_close` wiring that calls `show_close_pane_confirmation`. Follow the existing captures in that function (it already clones `self.model` and `surface_id`). For the parent window argument, use the SAME handle that the terminal `close.connect_clicked` passes to `show_close_pane_confirmation` — determine how the controller reaches it (grep how `browser_pane_widget`/the controller obtains the toplevel; if the controller stores a window/parent handle, clone it; if `show_close_pane_confirmation` can derive the parent from a widget, pass the pane widget). Wire it as:
```rust
        {
            let state = self.state.clone();      // adapt to the real field/Option handling
            let parent = /* the toplevel window handle used by the terminal close path */;
            let id = surface_id.to_string();
            pane.connect_close(move || {
                if let Some(state) = state.as_ref() {       // adapt if self.state is Option
                    show_close_pane_confirmation(&parent, state, &id);
                }
            });
        }
```
IMPORTANT: `self.state` may be `Option<SocketAppState>` — match how other wirings in `browser_pane_widget` / the controller handle it (the address-activate wiring uses `self.model` directly; the close path needs `state` + parent). If obtaining the parent window cleanly is not possible from this function, STOP and report the blocker rather than guessing — the close button must invoke the SAME confirmation flow as terminals, not a divergent close.

- [ ] **Step 6: build both feature sets + lint**

Run: `cargo build -p forktty-ui-gtk --features browser` → compiles.
Run: `cargo build -p forktty-ui-gtk --features gtk-vte` → compiles (no globe code reachable; all gated).
Run: `cargo fmt -p forktty-ui-gtk`.
Run: `cargo clippy -p forktty-ui-gtk --features browser --all-targets -- -D warnings` → clean.
Run: `cargo clippy -p forktty-ui-gtk --all-targets -- -D warnings` (default features) → clean.
Run: `cargo test -p forktty-ui-gtk` and `cargo test -p forktty-ui-gtk --features browser` → pass (no new tests; confirm nothing broke).

- [ ] **Step 7: commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/browser_pane.rs
git commit -m "feat(gtk): globe button opens blank browser pane; wire pane close

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Manual verification (needs a display, --features browser)

1. `cargo run -p forktty-ui-gtk --features browser` (or the project's run command).
2. Hover a terminal pane → globe appears among the action buttons. Click → a blank browser pane opens to the right; the address bar has keyboard focus. Type a URL + Enter → it loads.
3. With a single pane, the single-pane overlay also shows the globe; clicking it works.
4. On a browser pane, click × → close confirmation → pane closes, removed from the layout.
5. Rebuild with `--features gtk-vte` (no browser): no globe button anywhere; terminal chrome (split_h/v + close) unchanged; app builds and runs.

## Out of scope

- Split-from-browser chrome (split buttons on browser panes).
- Open-browser keyboard shortcut.
- Configurable initial/home URL.
