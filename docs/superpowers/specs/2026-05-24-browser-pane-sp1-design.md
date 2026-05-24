# Browser pane — SP1: pane kind + WebKitGTK6 embed + navigation

Date: 2026-05-24
Gap feature: #3 in `docs/cmux-gap-features.md` (Built-in browser pane, scriptable).

Status: implemented on `main` and later extended by SP3 profiles. Browser
surfaces now carry both `url` and `profile`; this SP1 design keeps the original
phase boundary for historical context.

## Decomposition

Full agent-browser parity is the north star but spans 4+ independent subsystems.
It is split into sub-projects, each with its own spec → plan → ship cycle:

- **SP1 (this spec)** — Browser pane *kind* + WebKitGTK6 embed + navigation
  (open / navigate / back / forward / reload). The foundation; nothing scriptable
  works without the pane existing.
- **SP2** — Scriptable socket verbs: `snapshot` (accessibility tree), `click`,
  `fill`, `eval`, element refs. The agent-browser core. Builds the socket→GTK
  command channel (also used by socket-driven back/reload).
- **SP3** — Persistence, profiles, history/bookmarks, and import. P1/P2 and the
  core P3 stores are now implemented; P3 socket/CLI/GTK wiring and P4 import
  remain deferred.

## Goal (SP1)

Add a browser pane that renders web content inside the existing pane tree, openable
and navigable from both the Unix socket (automation) and a thin in-pane address bar
(humans). Default builds stay WebKit-free; the engine is gated behind an opt-in cargo
feature.

Non-goals (SP1): scripting verbs (snapshot/click/fill/eval), element refs, socket-
driven back/forward/reload, cookie/profile import, multiple tabs per pane.

## Constraints discovered

- System WebKit: only `webkit2gtk-4.1` (GTK3) was present; `webkitgtk-6.0` (the GTK4
  engine, Arch `extra/webkitgtk-6.0` 2.52.3) has since been installed. gtk4 is 4.22.4.
  GTK3 WebKit cannot embed in a GTK4 widget tree, so `webkitgtk-6.0` + the `webkit6`
  Rust crate (gtk-rs ecosystem) are mandatory and must be feature-gated.
- `Surface` has no pane-kind discriminator today; every surface is implicitly a
  terminal. SP1 introduces the kind.

## Architecture

```text
forktty-core (pure, no webkit dependency)
  Surface.kind: SurfaceKind { Terminal, Browser { url, profile } }   (serde default = Terminal)
  WorkspaceModel::open_browser(workspace_id, url, profile, axis) -> Option<Surface>
  WorkspaceModel::set_surface_url(surface_id, url) -> bool

forktty-socket (no webkit dependency)
  browser.open      { workspace_id, url, profile?, axis? }  -> model.open_browser -> event
  browser.navigate  { surface_id, url }           -> model.set_surface_url -> event
  (back/forward/reload are NOT socket verbs in SP1 — address-bar buttons only;
   socket-driven nav lands in SP2 with the command channel)
  events: SurfaceAdded carries kind; new SurfaceUrlChanged { id, url }

forktty-ui-gtk  [new cargo feature: browser = ["gtk-vte", "dep:webkit6"]]
  browser_pane.rs:  BrowserPaneWidget = Box { address bar (Entry + back/fwd/reload),
                    WebKitGTK6 WebView }
  gtk_app rebuild_layout: Leaf -> match surface kind { Terminal -> VTE,
                                                        Browser -> BrowserPaneWidget }
  url diff observed like title/status -> WebView.load_uri (no full layout rebuild)
```

### Why socket nav is pure model state

The socket server (tokio thread) cannot touch the WebView (GTK main loop). Rather
than build a socket→GTK command channel in SP1, navigation is expressed as model
state: `browser.navigate` sets the `url` field, the GTK side observes the diff (the
same mechanism that already propagates title/status changes) and calls `load_uri`.
This reuses existing plumbing and needs zero new cross-thread machinery.

Back / forward / reload have no natural model state (they act on the live WebView's
history). In SP1 they are address-bar buttons only — GTK-local, no socket. SP2 builds
the imperative command channel once (needed anyway for click/eval) and exposes
socket-driven back/reload then.

## Components

### forktty-core/src/model.rs

- `enum SurfaceKind { Terminal, Browser { url: String } }`, serialized with a `type`
  tag, default = `Terminal`.
- `Surface.kind: SurfaceKind` with `#[serde(default)]` so persisted sessions from
  before this change load every surface as `Terminal`.
- Refactor: extract the body of `split_surface` into a private
  `split_with(surface_id, axis, kind, title) -> Option<Surface>`; `split_surface`
  calls it with `SurfaceKind::Terminal` / title `"shell"`.
- `open_browser(workspace_id, url, axis) -> Option<Surface>`: splits the workspace's
  currently focused surface into a `Browser { url }` leaf via `split_with`; title =
  the URL host (fallback `"browser"`). Returns the new surface.
- `set_surface_url(surface_id, url) -> bool`: if the surface exists and is `Browser`,
  replace its `url`, return true; otherwise false.

### forktty-core/src/events.rs

- `SurfaceAdded` gains a `kind` (terminal/browser tag) so subscribers can distinguish.
- New variant `SurfaceUrlChanged { id: String, url: String }`.
- `SurfSnap` gains `url: Option<String>` (None for terminals); `diff` emits
  `SurfaceUrlChanged` when a browser surface's url changes between snapshots.

### forktty-socket/src/lib.rs

- `browser.open` arm: validate `workspace_id`, `url` (non-empty, length-bounded,
  prepend `https://` when no scheme); optional `axis` (default horizontal); lock
  model → `open_browser` → return the surface JSON. No `spawn_surface_terminal`.
- `browser.navigate` arm: validate `surface_id`, `url`; `set_surface_url`; NotFound
  when the surface is missing or not a browser.
- Add `"browser.navigate"` and `"browser.open"` to `METHODS` (kept sorted). The
  existing capabilities/match drift test guards the list.

### forktty-ui-gtk/src/browser_pane.rs (new, feature `browser`)

- `BrowserPaneWidget`: a `gtk::Box` (vertical) containing an address bar row
  (`gtk::Entry` + back / forward / reload buttons) and a `webkit6::WebView`.
- Methods: `new(url)`, `load_uri(url)`, `go_back`, `go_forward`, `reload`,
  `current_uri`.
- Entry `activate` → normalize → call back into the model `set_surface_url` (local,
  same path the socket uses), letting the url-diff observer drive `load_uri` so manual
  and automated navigation share one code path. Back/forward/reload buttons call the
  WebView directly.

### forktty-ui-gtk/src/gtk_app.rs

- `rebuild_layout` leaf branch: `match surface.kind { Terminal => terminal_pane_widget,
  Browser => browser_pane_widget }`.
- Per-surface url update path (alongside the existing title/status update) calls
  `BrowserPaneWidget::load_uri` when the model url changed, avoiding a full rebuild.
- `#[cfg(not(feature = "browser"))]`: a browser surface renders a static placeholder
  ("Browser feature not built") so the model and layout stay consistent regardless of
  build features.

## Data flow

```text
Open (socket):   browser.open -> open_browser (model split) -> events tick
                 -> SurfaceAdded{kind:Browser,url} -> rebuild_layout sees new leaf
                 -> BrowserPaneWidget::new + load_uri(url)

Navigate (socket): browser.navigate -> set_surface_url -> SurfaceUrlChanged
                 -> url-diff observer -> BrowserPaneWidget::load_uri

Navigate (human): address-bar Entry activate -> set_surface_url (local)
                 -> same url-diff path -> load_uri
                 back / forward / reload buttons -> WebView directly (no model)
```

## Error handling

- `browser.open`: empty/missing url → `Invalid parameter url`; url over the byte cap →
  `PayloadTooLarge`; missing workspace → `NotFound`. Missing scheme → prepend
  `https://`.
- `browser.navigate`: missing surface or non-browser kind → `NotFound`; url validated
  as above.
- WebKit load failures (DNS, TLS, 4xx/5xx) surface as the WebView's built-in error
  page; not reported through the socket.
- Build without the `browser` feature: browser surfaces still exist in the model and
  render the placeholder; socket verbs still mutate state (so headless automation that
  only reads events keeps working).

## Testing

- **core** (no webkit, pure):
  - `open_browser` adds a `Browser` surface, splits the tree, focuses the new leaf.
  - `set_surface_url` updates only `Browser` surfaces; returns false for `Terminal`
    / missing.
  - serde: a surface JSON without `kind` deserializes as `Terminal`.
  - events `diff`: browser surface url change yields `SurfaceUrlChanged`; `SurfaceAdded`
    carries the kind.
- **socket**:
  - `browser.open` returns a `Browser` surface; scheme prepended when absent.
  - `browser.navigate` updates the url; non-browser surface → error.
  - capabilities drift test covers the two new methods automatically.
- **gtk** (feature `browser`): compile-gated smoke test — `BrowserPaneWidget::new`
  constructs and `load_uri` does not panic. Kept minimal (headless WebKit is limited).
- **Manual**: build with `--features browser`, run, `browser.open` from the CLI/socket,
  observe the page render; type a URL in the address bar; exercise back/reload buttons.

## Build sequence

1. core: `SurfaceKind` + `Surface.kind` + `split_with`/`open_browser` +
   `set_surface_url` + unit tests. Verify `cargo test -p forktty-core`.
2. core events: `SurfaceUrlChanged` + kind in `SurfaceAdded` + `SurfSnap.url` + diff
   tests. Verify `cargo test -p forktty-core`.
3. socket: `browser.open` / `browser.navigate` arms + METHODS entries + tests. Verify
   `cargo test -p forktty-socket`.
4. gtk: `browser` cargo feature + `webkit6` dep + `browser_pane.rs` + rebuild_layout
   kind match + url-diff load_uri + non-feature placeholder. Verify
   `cargo build -p forktty-ui-gtk --features browser` and `--features gtk-vte` (no
   browser) both compile.
5. CLI: `forktty browser open <url>` / `forktty browser navigate <surface> <url>`
   subcommands + help text + tests.
6. Docs: `docs/cmux-gap-features.md` (#3 note SP1 done, SP2/SP3 backlog) + `ROADMAP.md`.

## Out of scope (SP1, see SP2/SP3)

- Scripting verbs (`snapshot`, `click`, `fill`, `eval`), element refs.
- Socket-driven back/forward/reload (needs the SP2 command channel).
- Cookie / history / browser-profile import.
- Multiple tabs within a single browser pane; downloads UI; devtools.
