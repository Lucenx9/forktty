# Browser pane UI buttons — open globe + close ×

Date: 2026-05-24
Standalone mini-feature, separate from browser-pane SP1/SP2. Depends only on SP1
(`SurfaceKind::Browser`, `WorkspaceModel::open_browser`, `BrowserPaneWidget`), all
merged to `main`. Independent of SP2 (scripting verbs).

## Goal

Let a user open and close a browser pane from the GUI, without the socket/CLI:

1. A **globe** button in the pane action chrome (next to the split controls) that
   splits the focused pane into a blank browser pane with the address bar focused —
   mirroring cmux's per-pane globe icon.
2. A **close (×)** button on the browser pane itself, since browser panes currently
   render only an address bar and have no pane-action chrome (so today they cannot be
   closed from the GUI).

Both must vanish cleanly when the build lacks the `browser` cargo feature.

Non-goals: splitting *from* a browser pane (no split buttons on browser panes);
a keyboard shortcut for open-browser; choosing the initial URL (always `about:blank`);
multiple tabs.

## Constraints discovered

- Terminal pane chrome is built in `gtk_app.rs` (~line 939): an `actions` Box holding
  `split_h` (`view-dual-symbolic`), `split_v` (`view-paged-symbolic`), a separator,
  and `close` (`window-close-symbolic`), wired via `pane_action_button(icon, tooltip)`.
  A second `single_pane_actions` overlay (split buttons only, no close) covers the
  lone-pane case.
- Split clicks route through `focus_surface_and(state, sid, |s| split_active_surface(s, axis))`.
  `close` routes through `show_close_pane_confirmation(parent, state, sid)`.
- Browser panes are rendered by `BrowserPaneWidget` (`browser_pane.rs`, file-gated
  `#![cfg(feature = "browser")]`): a vertical Box with an address-bar row
  (back/forward/reload buttons + entry) over the WebView. It has no split/close chrome.
  `focus_target()` returns the address entry (already the focus target).
- `WorkspaceModel::open_browser(workspace_id, url, axis) -> Option<Surface>` (SP1)
  splits the focused surface into a `Browser { url }` leaf.

## Architecture

```text
forktty-ui-gtk (all new code feature-gated under `browser`)

  gtk_app.rs
    pane chrome (terminal): + globe button -> open_browser_active(state, Horizontal)
    single_pane_actions overlay: + globe button (same handler)
    fn open_browser_active(state, axis): resolve active workspace ->
        model.open_browser(ws_id, "about:blank", axis); focus the new browser pane
        so its address bar takes keyboard focus.

  browser_pane.rs
    BrowserPaneWidget address-bar row: + close (×) button after reload
    connect_close(f): wires the × to a caller-supplied callback
    gtk_app browser_pane_widget(): connect_close -> show_close_pane_confirmation
```

### Why `open_browser_active` lives in gtk_app, not core

`WorkspaceModel::open_browser` already exists in core and is the single source of
truth for the model mutation. The GTK helper only resolves the active workspace id
and focuses the resulting pane — UI concerns. It mirrors the existing
`split_active_surface` helper, keeping symmetry with how splits are wired.

### Feature gating

The globe button (creation + append + handler) is wrapped in `#[cfg(feature =
"browser")]` in `gtk_app.rs`, so a non-`browser` build never constructs it — the
button is absent, not merely disabled. The × lives inside `BrowserPaneWidget`, which
is wholly `#![cfg(feature = "browser")]`; a browser pane can only exist when the
feature is on, so the × needs no additional gate.

## Components

### gtk_app.rs — terminal pane chrome

- After `close` is built (~line 946), under `#[cfg(feature = "browser")]`, create
  `let open_browser = pane_action_button("globe-symbolic", "Open Browser Pane");`
  and append it to `actions` next to the split buttons (before the close separator,
  so order is: split_h, split_v, globe, │, close).
- In the `if let Some(state)` block, wire it (feature-gated):
  ```text
  open_browser.connect_clicked(|_| focus_surface_and(state, sid, |s|
      open_browser_active(s, SplitAxis::Horizontal)));
  ```
- In the `else` (no state) branch, set it insensitive like the other buttons.

### gtk_app.rs — single_pane_actions overlay

- Mirror the globe button into `single_pane_actions` (feature-gated), wired the same
  way, so the lone pane can also spawn a browser.

### gtk_app.rs — open_browser_active helper

```text
#[cfg(feature = "browser")]
fn open_browser_active(state: &SocketAppState, axis: SplitAxis) {
    // resolve active workspace id (reuse the same resolution split uses),
    // model.open_browser(ws_id, "about:blank", axis);
    // the refresh tick rebuilds the layout; the new browser pane's
    // focus_target (address entry) receives focus on build.
}
```
Use the same active-workspace resolution `split_active_surface` uses (read the helper
and follow it exactly). Returns nothing; the periodic chrome refresh renders the new
pane (same path SP1's socket `browser.open` relies on).

### browser_pane.rs — close button

- In `BrowserPaneWidget::new`, add a `close` button (`window-close-symbolic`, tooltip
  "Close Pane") to the address-bar `bar` Box, appended **last — after the entry** —
  so the row reads: back, forward, reload, [entry (hexpand)], ×. The entry keeps
  `hexpand`, so the × sits flush at the far right (conventional close position).
- Add `pub fn connect_close<F: Fn() + 'static>(&self, f: F)` that connects the
  button's `clicked` to `f`.
- Do NOT wire the close action inside the widget (the widget does not own the model
  or the confirmation dialog) — expose `connect_close` and wire it from gtk_app.

### gtk_app.rs — browser_pane_widget wiring

- In `browser_pane_widget(surface_id, url)` (where `connect_address_activate` and
  `connect_focus_in` are already wired), add:
  ```text
  pane.connect_close({ state, parent, surface_id } move ||
      show_close_pane_confirmation(parent, state, surface_id));
  ```
  Match the capture/borrow pattern of the existing `connect_address_activate` wiring
  in that function.

## Data flow

```text
Open:  globe click -> focus_surface_and(focus the pane) -> open_browser_active
       -> model.open_browser(ws, "about:blank", Horizontal)
       -> chrome refresh tick -> rebuild_layout builds BrowserPaneWidget
       -> focus_target (address entry) focused -> user types a URL

Close: × click (browser pane) -> connect_close callback
       -> show_close_pane_confirmation(parent, state, surface_id)
       -> existing surface.close path -> pane pruned from browser_panes
```

## Error handling

- No active workspace when the globe is clicked: `open_browser_active` no-ops
  (open_browser returns None) — same benign behavior as split with no active
  workspace. No crash, no dialog.
- Closing the last/root surface: `show_close_pane_confirmation` already handles root
  replacement (the existing terminal close path); browser panes use the identical
  path, so no new edge case.
- Build without `browser`: globe button never constructed; browser panes never exist,
  so the × is unreachable. Terminal chrome unchanged.

## Testing

- **Manual** (needs a display, `--features browser`):
  - Hover a terminal pane → globe appears among the action buttons; click → a blank
    browser pane opens to the right with the address bar focused; type a URL → loads.
  - Lone pane: the single-pane overlay also shows the globe; click works.
  - On the browser pane, the × button closes it (confirmation dialog), pane removed.
  - Build with `--features gtk-vte` (no browser): no globe button anywhere; terminal
    chrome (split_h/v + close) unchanged; app builds and runs.
- **Automated:** GTK button click wiring has no headless coverage today (the existing
  split/close buttons have none — clicks need a display). No new unit tests; the model
  mutation (`open_browser` with any url incl. `about:blank`) is already covered by
  core tests. Verify both feature builds compile + fmt + clippy clean.

## Build sequence

1. browser_pane.rs: add the × button + `connect_close`. Verify
   `cargo build -p forktty-ui-gtk --features browser`.
2. gtk_app.rs: `open_browser_active` helper + globe button in both the pane chrome
   and the single-pane overlay (feature-gated) + wire `connect_close` in
   `browser_pane_widget`. Verify `--features browser` and `--features gtk-vte` both
   compile; fmt + clippy clean on both.
3. Manual smoke per the Testing section.

## Out of scope

- Split-from-browser-pane chrome (split H/V buttons on browser panes).
- Open-browser keyboard shortcut.
- Configurable initial URL / home page.
- Multiple tabs, devtools, downloads.
