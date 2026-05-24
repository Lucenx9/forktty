# Browser Pane SP2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Current status on `main`:** Implemented and later extended by SP3 profiles.
`SocketAppState` now also carries profile-store locking, and browser surfaces
carry a profile ID in addition to the URL.

**Goal:** Add scriptable browser-pane verbs (`snapshot`, `click`, `fill`, JS-evaluate) and socket-driven `back`/`forward`/`reload` via a request/reply command channel from the socket thread to the GTK WebView.

**Architecture:** Pure command/result types live in `forktty-core`. The socket server holds an optional `async_channel::Sender<BrowserCommand>`; each command carries a `tokio::oneshot` reply sender. The GTK main thread pumps the receiver, runs JavaScript against the addressed WebView (an injected `window.__forktty` driver for snapshot/click/fill, raw script for JS-evaluate), and fulfils the reply. The socket handler awaits the reply with a bounded timeout.

**Tech Stack:** Rust, `async_channel` (runtime-agnostic), `tokio` (oneshot/timeout), `webkit6` (WebKitGTK6, behind the existing `browser` cargo feature), `serde_json`.

---

## Background the implementer needs

- Three crates: `forktty-core` (pure data, no GTK/webkit), `forktty-socket` (JSON-RPC over a Unix socket, no webkit), `forktty-ui-gtk` (GTK4 app + `forktty` CLI; GTK behind the `gtk-vte` feature, WebKit behind `browser = ["gtk-vte", "dep:webkit6"]`).
- SP1 (merged) added `SurfaceKind::{Terminal, Browser { url }}` (`crates/forktty-core/src/model.rs:42`), socket verbs `browser.open` / `browser.navigate` (`crates/forktty-socket/src/lib.rs:714`), and `BrowserPaneWidget` (`crates/forktty-ui-gtk/src/browser_pane.rs`).
- `dispatch` in `forktty-socket/src/lib.rs:360` is `async fn dispatch(state: &SocketAppState, method: &str, params: Value) -> Result<Value, DispatchError>`. You can `.await` inside the new arms.
- `DispatchError` variants (`lib.rs:89`): `MethodNotFound`, `MissingParam`, `NotFound(String)`, `PayloadTooLarge { field: &'static str, limit, actual }`, `Conflict`, `AlreadyExists`, `NotReady`, `InvalidParam`, `Other`. `impl From<&str>` and `From<String>` map to `Other`. `DispatchError::code()` returns the stable string code (`not_found`, `payload_too_large`, ...).
- `const METHODS: &[&str]` (`lib.rs:~37`) is the sorted source-of-truth method list; a drift test asserts it matches the dispatch arms. Every new verb MUST be added here, kept sorted.
- `SocketAppState` (`lib.rs:211`): `{ model: Arc<Mutex<WorkspaceModel>>, terminal, shell, socket_path, notification_dispatch, events: broadcast::Sender<ModelEvent> }`. Constructed via `new(model, terminal, shell, socket_path)` plus builder `with_notification_dispatch`.
- Param helpers in `lib.rs`: `required_string_param(params, "name") -> Result<&str, DispatchError>` (`:1365`), `required_surface_id(params) -> Result<&str, _>` (`:1350`).
- Model accessor `WorkspaceModel::surface(id) -> Option<&Surface>` (`model.rs:541`); `Surface.kind: SurfaceKind` (`model.rs:62`).
- GTK: `VteController` (in `gtk_app.rs`) owns `browser_panes: Rc<RefCell<BTreeMap<String, Rc<crate::browser_pane::BrowserPaneWidget>>>>` (`:329`). The controller is shared as `Rc<RefCell<VteController>>` into main-context callbacks (`gtk_app.rs:3555`). `SocketAppState` is built at `gtk_app.rs:3323`; the socket server thread is launched near `gtk_app.rs:8373` via `runtime.block_on(serve(listener, state))`.
- Run tests per crate: `cargo test -p forktty-core`, `cargo test -p forktty-socket`. GTK browser build: `cargo build -p forktty-ui-gtk --features browser`; non-browser: `cargo build -p forktty-ui-gtk --features gtk-vte`.
- Commit message trailer (every commit): `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`.

---

## Task 1: Core command/result types

**Files:**
- Create: `crates/forktty-core/src/browser_cmd.rs`
- Modify: `crates/forktty-core/src/lib.rs` (add `pub mod browser_cmd;` + re-export)
- Modify: `crates/forktty-core/Cargo.toml` (add `async-channel`, `tokio` sync)
- Modify: root `Cargo.toml` (add `async-channel` to `[workspace.dependencies]`)

- [ ] **Step 1: Add workspace + core dependencies**

In root `Cargo.toml`, under `[workspace.dependencies]`, add (alphabetically near other entries):

```toml
async-channel = "2"
```

In `crates/forktty-core/Cargo.toml`, under `[dependencies]`, add:

```toml
async-channel = { workspace = true }
tokio = { workspace = true }
```

(The root `tokio` entry already enables the `sync` feature, which provides `oneshot`.)

- [ ] **Step 2: Write the module (types + tests together)**

Create `crates/forktty-core/src/browser_cmd.rs` with the full content below (types and tests in one file; the build-fail/pass cycle is exercised in Step 4):

```rust
//! Pure command/result types for the browser-pane scripting channel (SP2).
//!
//! These cross the socket thread -> GTK main thread boundary. They hold no
//! GTK/WebKit types so they can live in the pure core crate; `forktty-socket`
//! builds the commands and `forktty-ui-gtk` consumes them behind the `browser`
//! cargo feature.

use tokio::sync::oneshot;

/// Maximum byte length of a JSON result returned by a browser command.
pub const MAX_BROWSER_RESULT_BYTES: usize = 256 * 1024;

/// Maximum byte length of a JS-evaluate script.
pub const MAX_BROWSER_SCRIPT_BYTES: usize = 64 * 1024;

/// What a browser command asks the WebView to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserOp {
    /// Walk the accessibility tree, assign element refs, return a JSON tree.
    Snapshot,
    /// Click the element previously assigned `reference` by a snapshot.
    Click { reference: String },
    /// Set the value of the element `reference` to `value`.
    Fill { reference: String, value: String },
    /// Run arbitrary JavaScript, return its JSON-serialized result.
    Eval { script: String },
    /// Navigate back / forward in session history.
    Back,
    Forward,
    /// Reload the current page.
    Reload,
}

/// Result of running a [`BrowserCommand`] against the WebView.
#[derive(Debug)]
pub enum CmdResult {
    /// Operation completed, no payload (nav / click / fill).
    Ok,
    /// Operation produced a JSON value (snapshot / JS-evaluate), already serialized.
    Json(String),
    /// Operation failed.
    Err(BrowserCmdError),
}

/// Why a browser command failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserCmdError {
    /// The addressed surface no longer exists.
    SurfaceGone,
    /// The addressed surface is not a browser surface.
    NotABrowser,
    /// No live WebView is realized for the surface.
    NoWebView,
    /// A snapshot ref was not found (stale after navigation).
    RefNotFound,
    /// JavaScript threw or evaluation failed.
    JsError(String),
    /// The result exceeded [`MAX_BROWSER_RESULT_BYTES`].
    TooLarge,
    /// An unexpected internal failure (e.g. the reply channel was dropped).
    Internal(String),
}

impl std::fmt::Display for BrowserCmdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BrowserCmdError::SurfaceGone => f.write_str("surface no longer exists"),
            BrowserCmdError::NotABrowser => f.write_str("surface is not a browser"),
            BrowserCmdError::NoWebView => f.write_str("no live web view for surface"),
            BrowserCmdError::RefNotFound => f.write_str("element ref not found"),
            BrowserCmdError::JsError(msg) => write!(f, "javascript error: {msg}"),
            BrowserCmdError::TooLarge => f.write_str("result too large"),
            BrowserCmdError::Internal(msg) => write!(f, "internal browser error: {msg}"),
        }
    }
}

/// A command sent from the socket thread to the GTK WebView, with a one-shot
/// reply channel the GTK side fulfils once the operation settles.
#[derive(Debug)]
pub struct BrowserCommand {
    pub surface_id: String,
    pub op: BrowserOp,
    pub reply: oneshot::Sender<CmdResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_op_equality() {
        assert_eq!(BrowserOp::Snapshot, BrowserOp::Snapshot);
        assert_eq!(
            BrowserOp::Click { reference: "e1".into() },
            BrowserOp::Click { reference: "e1".into() }
        );
        assert_ne!(
            BrowserOp::Click { reference: "e1".into() },
            BrowserOp::Click { reference: "e2".into() }
        );
    }

    #[test]
    fn error_display_strings_are_stable() {
        assert_eq!(BrowserCmdError::RefNotFound.to_string(), "element ref not found");
        assert_eq!(BrowserCmdError::NotABrowser.to_string(), "surface is not a browser");
        assert_eq!(
            BrowserCmdError::JsError("boom".into()).to_string(),
            "javascript error: boom"
        );
    }

    #[test]
    fn size_constants_are_sane() {
        assert!(MAX_BROWSER_SCRIPT_BYTES < MAX_BROWSER_RESULT_BYTES);
    }

    #[tokio::test]
    async fn command_reply_round_trips() {
        let (tx, rx) = oneshot::channel();
        let cmd = BrowserCommand {
            surface_id: "s1".into(),
            op: BrowserOp::Snapshot,
            reply: tx,
        };
        cmd.reply.send(CmdResult::Json("{}".into())).unwrap();
        match rx.await.unwrap() {
            CmdResult::Json(s) => assert_eq!(s, "{}"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Register the module + re-export**

In `crates/forktty-core/src/lib.rs`, add to the `pub mod` block (after `pub mod agents;`):

```rust
pub mod browser_cmd;
```

Add a re-export near the other `pub use` lines:

```rust
pub use browser_cmd::{
    BrowserCmdError, BrowserCommand, BrowserOp, CmdResult, MAX_BROWSER_RESULT_BYTES,
    MAX_BROWSER_SCRIPT_BYTES,
};
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-core browser_cmd`
Expected: PASS (4 tests). If the `tokio::test` macro is unavailable, confirm the root workspace `tokio` dep has the `macros` feature (it does: `["rt-multi-thread", "macros", "sync", ...]`).

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-core/src/browser_cmd.rs crates/forktty-core/src/lib.rs crates/forktty-core/Cargo.toml Cargo.toml Cargo.lock
git commit -m "feat(core): browser command/result types for SP2 scripting channel

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 2: Socket state + dispatch arms

**Files:**
- Modify: `crates/forktty-socket/Cargo.toml` (add `async-channel`)
- Modify: `crates/forktty-socket/src/lib.rs` (state field, builder, timeout const, helper, 7 dispatch arms, METHODS)

- [ ] **Step 1: Add the dependency**

In `crates/forktty-socket/Cargo.toml` under `[dependencies]`:

```toml
async-channel = { workspace = true }
```

- [ ] **Step 2: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the end of `crates/forktty-socket/src/lib.rs`. These use a stub receiver to stand in for the GTK side. A `test_state()` helper already exists in the test module (used by `dispatches_minimum_socket_methods_directly`); reuse it. First grep the test module to confirm helper names (`test_state`, how `workspace.create` / `surface.list` results are shaped) and adjust if needed.

```rust
// --- SP2 browser scripting verbs ---------------------------------------

/// Build a state plus a browser-command receiver. The caller drains the
/// receiver to simulate the GTK side.
fn state_with_browser_channel(
) -> (SocketAppState, async_channel::Receiver<forktty_core::BrowserCommand>) {
    let state = test_state();
    let (tx, rx) = async_channel::unbounded();
    (state.with_browser_cmd(tx), rx)
}

/// Open a browser surface in `state`'s model and return its surface id.
async fn open_browser_surface(state: &SocketAppState) -> String {
    let ws = dispatch(state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();
    let surface = dispatch(
        state,
        "browser.open",
        json!({"workspace_id": workspace_id, "url": "https://example.com"}),
    )
    .await
    .unwrap();
    surface.get("id").unwrap().as_str().unwrap().to_string()
}

#[tokio::test]
async fn browser_snapshot_unavailable_without_channel() {
    let state = test_state();
    let sid = open_browser_surface(&state).await;
    let err = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("browser automation unavailable"));
}

#[tokio::test]
async fn browser_snapshot_returns_stub_json() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        assert_eq!(cmd.op, forktty_core::BrowserOp::Snapshot);
        cmd.reply
            .send(forktty_core::CmdResult::Json("{\"role\":\"root\"}".into()))
            .unwrap();
    });
    let result = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
        .await
        .unwrap();
    assert_eq!(result, json!({"role": "root"}));
    responder.await.unwrap();
}

#[tokio::test]
async fn browser_back_returns_ok() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        assert_eq!(cmd.op, forktty_core::BrowserOp::Back);
        cmd.reply.send(forktty_core::CmdResult::Ok).unwrap();
    });
    let result = dispatch(&state, "browser.back", json!({"surface_id": sid}))
        .await
        .unwrap();
    assert_eq!(result, json!({"ok": true}));
    responder.await.unwrap();
}

#[tokio::test]
async fn browser_eval_rejects_oversize_script() {
    let (state, _rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let big = "x".repeat(forktty_core::MAX_BROWSER_SCRIPT_BYTES + 1);
    let err = dispatch(&state, "browser.eval", json!({"surface_id": sid, "script": big}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "payload_too_large");
}

#[tokio::test]
async fn browser_click_on_terminal_surface_is_not_found() {
    let (state, _rx) = state_with_browser_channel();
    let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap();
    let surfaces = dispatch(&state, "surface.list", json!({"workspace_id": workspace_id}))
        .await
        .unwrap();
    let term_id = surfaces.as_array().unwrap()[0]
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let err = dispatch(&state, "browser.click", json!({"surface_id": term_id, "ref": "e1"}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn browser_click_maps_ref_not_found_reply() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        cmd.reply
            .send(forktty_core::CmdResult::Err(
                forktty_core::BrowserCmdError::RefNotFound,
            ))
            .unwrap();
    });
    let err = dispatch(&state, "browser.click", json!({"surface_id": sid, "ref": "e1"}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    responder.await.unwrap();
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p forktty-socket browser_`
Expected: FAIL to compile — `with_browser_cmd` and the dispatch arms do not exist yet.

- [ ] **Step 4: Add the state field, builder, timeout const**

In `crates/forktty-socket/src/lib.rs`, add an import near the top:

```rust
use forktty_core::{BrowserCommand, BrowserCmdError, BrowserOp, CmdResult, MAX_BROWSER_SCRIPT_BYTES};
```

Add a constant near `MAX_BROWSER_URL_BYTES`:

```rust
const BROWSER_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
```

Add the field to `SocketAppState` (after `events`):

```rust
    /// Sends scripting commands to the GTK WebView. `None` when no browser
    /// engine is wired (no `browser` feature, or headless), in which case the
    /// browser scripting verbs report unavailable.
    pub browser_cmd: Option<async_channel::Sender<BrowserCommand>>,
```

In `SocketAppState::new`, initialize it `None`:

```rust
            browser_cmd: None,
```

Add the builder next to `with_notification_dispatch`:

```rust
    pub fn with_browser_cmd(mut self, sender: async_channel::Sender<BrowserCommand>) -> Self {
        self.browser_cmd = Some(sender);
        self
    }
```

- [ ] **Step 5: Add the dispatch helper**

Add these free functions near `required_browser_url` (`lib.rs:1400`):

```rust
/// Validate the surface is a browser, send `op` to the GTK side, and await the
/// reply within [`BROWSER_CMD_TIMEOUT`]. Maps [`CmdResult`] to a JSON value or a
/// [`DispatchError`].
async fn dispatch_browser_cmd(
    state: &SocketAppState,
    surface_id: String,
    op: BrowserOp,
) -> Result<Value, DispatchError> {
    {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        match model.surface(&surface_id) {
            None => return Err(DispatchError::NotFound("surface".to_string())),
            Some(surface) => {
                if !matches!(surface.kind, forktty_core::SurfaceKind::Browser { .. }) {
                    return Err(DispatchError::NotFound("browser surface".to_string()));
                }
            }
        }
    }
    let Some(sender) = state.browser_cmd.clone() else {
        return Err("browser automation unavailable".into());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    sender
        .send(BrowserCommand {
            surface_id,
            op,
            reply: reply_tx,
        })
        .await
        .map_err(|_| DispatchError::from("browser automation unavailable"))?;
    let result = tokio::time::timeout(BROWSER_CMD_TIMEOUT, reply_rx)
        .await
        .map_err(|_| DispatchError::from("browser command timed out"))?
        .map_err(|_| DispatchError::Other("browser reply dropped".to_string()))?;
    match result {
        CmdResult::Ok => Ok(json!({"ok": true})),
        CmdResult::Json(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|e| DispatchError::Other(format!("invalid browser result json: {e}"))),
        CmdResult::Err(err) => Err(browser_cmd_error_to_dispatch(err)),
    }
}

fn browser_cmd_error_to_dispatch(err: BrowserCmdError) -> DispatchError {
    match err {
        BrowserCmdError::SurfaceGone => DispatchError::NotFound("surface".to_string()),
        BrowserCmdError::NotABrowser => DispatchError::NotFound("browser surface".to_string()),
        BrowserCmdError::NoWebView => DispatchError::NotFound("web view".to_string()),
        BrowserCmdError::RefNotFound => DispatchError::NotFound("element ref".to_string()),
        BrowserCmdError::TooLarge => DispatchError::PayloadTooLarge {
            field: "result",
            limit: forktty_core::MAX_BROWSER_RESULT_BYTES,
            actual: forktty_core::MAX_BROWSER_RESULT_BYTES + 1,
        },
        BrowserCmdError::JsError(msg) => DispatchError::Other(msg),
        BrowserCmdError::Internal(msg) => DispatchError::Other(msg),
    }
}
```

- [ ] **Step 6: Add the 7 dispatch arms**

In `dispatch`, after the `"browser.navigate"` arm (`lib.rs:744`), add (the JS-evaluate arm is `browser.eval`):

```rust
        "browser.snapshot" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Snapshot).await
        }
        "browser.click" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let reference = required_string_param(&params, "ref")?.to_string();
            if reference.is_empty() {
                return Err("Invalid parameter ref: must not be empty".into());
            }
            dispatch_browser_cmd(state, surface_id, BrowserOp::Click { reference }).await
        }
        "browser.fill" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let reference = required_string_param(&params, "ref")?.to_string();
            if reference.is_empty() {
                return Err("Invalid parameter ref: must not be empty".into());
            }
            let value = required_string_param(&params, "value")?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Fill { reference, value }).await
        }
        "browser.eval" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let script = required_string_param(&params, "script")?.to_string();
            if script.is_empty() {
                return Err("Invalid parameter script: must not be empty".into());
            }
            if script.len() > MAX_BROWSER_SCRIPT_BYTES {
                return Err(DispatchError::PayloadTooLarge {
                    field: "script",
                    limit: MAX_BROWSER_SCRIPT_BYTES,
                    actual: script.len(),
                });
            }
            dispatch_browser_cmd(state, surface_id, BrowserOp::Eval { script }).await
        }
        "browser.back" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Back).await
        }
        "browser.forward" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Forward).await
        }
        "browser.reload" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Reload).await
        }
```

- [ ] **Step 7: Add to METHODS (sorted)**

In `const METHODS: &[&str]`, replace the existing two `browser.*` lines with this full block (alphabetical, no duplicates):

```rust
    "browser.back",
    "browser.click",
    "browser.eval",
    "browser.fill",
    "browser.forward",
    "browser.navigate",
    "browser.open",
    "browser.reload",
    "browser.snapshot",
```

- [ ] **Step 8: Run tests**

Run: `cargo test -p forktty-socket`
Expected: PASS, including the new `browser_*` tests and the capabilities/METHODS drift test.

- [ ] **Step 9: Commit**

```bash
git add crates/forktty-socket/Cargo.toml crates/forktty-socket/src/lib.rs Cargo.lock
git commit -m "feat(socket): browser scripting verbs over command channel

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 3: JS driver + WebView execution methods

**Files:**
- Create: `crates/forktty-ui-gtk/src/driver.js`
- Modify: `crates/forktty-ui-gtk/src/browser_pane.rs`
- Modify: `crates/forktty-ui-gtk/Cargo.toml` (ensure `serde_json` present)

- [ ] **Step 1: Write the driver script**

Create `crates/forktty-ui-gtk/src/driver.js`:

```javascript
// ForkTTY browser-pane scripting driver (SP2). Injected as a persistent
// WebKit user script at document-start, so window.__forktty is present on
// every page and after every navigation. Idempotent: re-running keeps state.
(function () {
  if (window.__forktty) return;

  var refMap = new Map();
  var counter = 0;

  function isHidden(el) {
    if (el.hidden) return true;
    if (el.getAttribute && el.getAttribute("aria-hidden") === "true") return true;
    var s = window.getComputedStyle(el);
    return s.display === "none" || s.visibility === "hidden";
  }

  function roleOf(el) {
    var r = el.getAttribute && el.getAttribute("role");
    if (r) return r;
    var tag = el.tagName.toLowerCase();
    var implicit = {
      a: "link", button: "button", input: "textbox", textarea: "textbox",
      select: "combobox", h1: "heading", h2: "heading", h3: "heading",
      h4: "heading", h5: "heading", h6: "heading", nav: "navigation",
      main: "main", img: "img", form: "form"
    };
    return implicit[tag] || "";
  }

  function nameOf(el) {
    var label = el.getAttribute && el.getAttribute("aria-label");
    if (label) return label.trim();
    var ph = el.getAttribute && el.getAttribute("placeholder");
    if (ph) return ph.trim();
    if (el.tagName === "INPUT" || el.tagName === "TEXTAREA") {
      return (el.value || "").trim();
    }
    var text = (el.textContent || "").trim();
    return text.length > 120 ? text.slice(0, 120) : text;
  }

  function isInteresting(el) {
    return roleOf(el) !== "";
  }

  function walk(el) {
    var node = null;
    if (isInteresting(el)) {
      var ref = "e" + (++counter);
      refMap.set(ref, el);
      node = {
        ref: ref,
        role: roleOf(el),
        name: nameOf(el),
        value: el.value !== undefined ? String(el.value) : "",
        children: []
      };
    }
    var kids = el.children || [];
    for (var i = 0; i < kids.length; i++) {
      var child = kids[i];
      if (isHidden(child)) continue;
      var childNode = walk(child);
      if (childNode) {
        if (node) node.children.push(childNode);
        else return childNode; // collapse: surface descendant when parent uninteresting
      }
    }
    return node;
  }

  window.__forktty = {
    snapshot: function () {
      refMap = new Map();
      counter = 0;
      var root = walk(document.body) || { role: "document", name: "", value: "", children: [] };
      return JSON.stringify(root);
    },
    click: function (ref) {
      var el = refMap.get(ref);
      if (!el) throw "ref-not-found";
      el.scrollIntoView({ block: "center" });
      el.click();
      return true;
    },
    fill: function (ref, value) {
      var el = refMap.get(ref);
      if (!el) throw "ref-not-found";
      el.focus();
      el.value = value;
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    }
  };
})();
```

- [ ] **Step 2: Write the failing test (JS-call string builders)**

The live WebView calls can't run headless, but the JS-call string builders are pure and MUST be correct (quoting). Add to the `tests` module in `crates/forktty-ui-gtk/src/browser_pane.rs`:

```rust
    #[test]
    fn driver_script_is_present() {
        assert!(DRIVER_JS.contains("window.__forktty"));
        assert!(DRIVER_JS.contains("snapshot"));
    }

    #[test]
    fn click_call_quotes_ref() {
        assert_eq!(click_js("e1"), "window.__forktty.click(\"e1\")");
        // A ref containing a quote is JSON-escaped, not injected.
        assert_eq!(click_js("e\"1"), "window.__forktty.click(\"e\\\"1\")");
    }

    #[test]
    fn fill_call_quotes_ref_and_value() {
        assert_eq!(
            fill_js("e2", "hello"),
            "window.__forktty.fill(\"e2\",\"hello\")"
        );
        assert_eq!(
            fill_js("e2", "a\"b"),
            "window.__forktty.fill(\"e2\",\"a\\\"b\")"
        );
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p forktty-ui-gtk --features browser browser_pane`
Expected: FAIL to compile — `DRIVER_JS`, `click_js`, `fill_js` do not exist.

- [ ] **Step 4: Add the constant + builders + run_js**

At the top of `crates/forktty-ui-gtk/src/browser_pane.rs` (after the `use` lines):

```rust
use forktty_core::BrowserCmdError;

/// The scripting driver injected into every page (SP2).
pub const DRIVER_JS: &str = include_str!("driver.js");

/// Build the JS call for `window.__forktty.click(ref)`, JSON-quoting `reference`.
pub fn click_js(reference: &str) -> String {
    format!(
        "window.__forktty.click({})",
        serde_json::to_string(reference).unwrap_or_else(|_| "\"\"".to_string())
    )
}

/// Build the JS call for `window.__forktty.fill(ref, value)`, JSON-quoting both.
pub fn fill_js(reference: &str, value: &str) -> String {
    format!(
        "window.__forktty.fill({},{})",
        serde_json::to_string(reference).unwrap_or_else(|_| "\"\"".to_string()),
        serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
    )
}
```

In `BrowserPaneWidget::new`, after constructing `web_view`, register the driver as a persistent user script (before the button handlers is fine):

```rust
        {
            use webkit6::{UserContentInjectedFrames, UserScript, UserScriptInjectionTime};
            let content_manager = web_view.user_content_manager().expect("ucm");
            let script = UserScript::new(
                DRIVER_JS,
                UserContentInjectedFrames::TopFrame,
                UserScriptInjectionTime::Start,
                &[],
                &[],
            );
            content_manager.add_script(&script);
        }
```

(If `web_view.user_content_manager()` returns a non-`Option` value in this `webkit6` version, drop the `.expect`. `WebView::new()` always has a default content manager. Verify against `webkit6` 0.5 docs.)

Add the execution method to the `impl BrowserPaneWidget` block:

```rust
    /// Run JavaScript in the page, delivering the JSON-serialized result (or an
    /// error) to `on_done`. `on_done` runs on the GTK main thread once the GIO
    /// async call settles.
    pub fn run_js<F>(&self, js: &str, on_done: F)
    where
        F: FnOnce(Result<String, BrowserCmdError>) + 'static,
    {
        use webkit6::gio::Cancellable;
        self.web_view.evaluate_javascript(
            js,
            None,
            None,
            Cancellable::NONE,
            move |result| {
                let mapped = match result {
                    Ok(value) => match value.to_json(0) {
                        Some(s) => {
                            let s = s.to_string();
                            if s.len() > forktty_core::MAX_BROWSER_RESULT_BYTES {
                                Err(BrowserCmdError::TooLarge)
                            } else {
                                Ok(s)
                            }
                        }
                        None => Ok("null".to_string()),
                    },
                    Err(err) => {
                        let msg = err.to_string();
                        if msg.contains("ref-not-found") {
                            Err(BrowserCmdError::RefNotFound)
                        } else {
                            Err(BrowserCmdError::JsError(msg))
                        }
                    }
                };
                on_done(mapped);
            },
        );
    }
```

Note: the exact `evaluate_javascript` signature/arity and the `JSCValue::to_json` accessor come from `webkit6` 0.5. If the closure receives a different type (some versions pass `Result<JSCValue, glib::Error>`), adapt the match; the mapping logic stays the same. Confirm with `cargo doc -p webkit6` or the crate source. Ensure `serde_json` is in `crates/forktty-ui-gtk/Cargo.toml` (`serde_json = { workspace = true }`); add if missing.

- [ ] **Step 5: Run the build + tests**

Run: `cargo test -p forktty-ui-gtk --features browser browser_pane`
Expected: PASS (the 3 new pure tests; the live `#[ignore]` test stays ignored).
Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: compiles (the driver wiring is feature-gated, no break without `browser`).

- [ ] **Step 6: Commit**

```bash
git add crates/forktty-ui-gtk/src/driver.js crates/forktty-ui-gtk/src/browser_pane.rs crates/forktty-ui-gtk/Cargo.toml Cargo.lock
git commit -m "feat(gtk): inject scripting driver + run_js on browser pane

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 4: Wire the command channel into the GTK app

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`
- Modify: `crates/forktty-ui-gtk/Cargo.toml` (ensure `async-channel` present)
- Modify: `crates/forktty-ui-gtk/src/browser_pane.rs` (nav methods if missing)

- [ ] **Step 1: Add the dependency**

Ensure `crates/forktty-ui-gtk/Cargo.toml` `[dependencies]` has:

```toml
async-channel = { workspace = true }
```

- [ ] **Step 2: Add a pane accessor on VteController**

In the `impl VteController` block, add:

```rust
    #[cfg(feature = "browser")]
    fn browser_pane(&self, surface_id: &str) -> Option<Rc<crate::browser_pane::BrowserPaneWidget>> {
        self.browser_panes.borrow().get(surface_id).cloned()
    }
```

- [ ] **Step 3: Add nav methods to BrowserPaneWidget if missing**

SP1 wires back/forward/reload to buttons inline but may lack public methods. Add to `impl BrowserPaneWidget` (skip any that already exist):

```rust
    pub fn go_back(&self) {
        self.web_view.go_back();
    }
    pub fn go_forward(&self) {
        self.web_view.go_forward();
    }
    pub fn reload(&self) {
        self.web_view.reload();
    }
```

- [ ] **Step 4: Create the channel and attach the sender**

At `gtk_app.rs:3323`, where `state` is built, insert before the `let state = ...` line:

```rust
    #[cfg(feature = "browser")]
    let (browser_cmd_tx, browser_cmd_rx) =
        async_channel::unbounded::<forktty_core::BrowserCommand>();
```

Change the state construction to attach the sender when the feature is on:

```rust
    let state = SocketAppState::new(model.clone(), backend, shell.clone(), socket_path);
    #[cfg(feature = "browser")]
    let state = state.with_browser_cmd(browser_cmd_tx);
```

- [ ] **Step 5: Spawn the command pump on the main context**

After the controller is created and `attach_state` is called (`gtk_app.rs:~3560`), add:

```rust
    #[cfg(feature = "browser")]
    {
        let controller_for_browser = controller.clone();
        let rx = browser_cmd_rx;
        glib::spawn_future_local(async move {
            while let Ok(cmd) = rx.recv().await {
                handle_browser_command(&controller_for_browser, cmd);
            }
        });
    }
```

- [ ] **Step 6: Implement handle_browser_command**

Add these free functions (feature-gated) near the other `gtk_app.rs` free helpers:

```rust
#[cfg(feature = "browser")]
fn handle_browser_command(
    controller: &Rc<RefCell<VteController>>,
    cmd: forktty_core::BrowserCommand,
) {
    use crate::browser_pane::{click_js, fill_js};
    use forktty_core::{BrowserCmdError, BrowserOp, CmdResult};

    let pane = controller.borrow().browser_pane(&cmd.surface_id);
    let Some(pane) = pane else {
        let _ = cmd.reply.send(CmdResult::Err(BrowserCmdError::NoWebView));
        return;
    };
    let reply = cmd.reply;
    match cmd.op {
        BrowserOp::Snapshot => {
            pane.run_js("window.__forktty.snapshot()", move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Click { reference } => {
            pane.run_js(&click_js(&reference), move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Fill { reference, value } => {
            pane.run_js(&fill_js(&reference, &value), move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Eval { script } => {
            pane.run_js(&script, move |r| {
                let _ = reply.send(into_cmd_result(r));
            });
        }
        BrowserOp::Back => {
            pane.go_back();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Forward => {
            pane.go_forward();
            let _ = reply.send(CmdResult::Ok);
        }
        BrowserOp::Reload => {
            pane.reload();
            let _ = reply.send(CmdResult::Ok);
        }
    }
}

#[cfg(feature = "browser")]
fn into_cmd_result(
    r: Result<String, forktty_core::BrowserCmdError>,
) -> forktty_core::CmdResult {
    match r {
        Ok(json) => forktty_core::CmdResult::Json(json),
        Err(e) => forktty_core::CmdResult::Err(e),
    }
}
```

- [ ] **Step 7: Build both feature sets**

Run: `cargo build -p forktty-ui-gtk --features browser`
Expected: compiles.
Run: `cargo build -p forktty-ui-gtk --features gtk-vte`
Expected: compiles (all new code is `#[cfg(feature = "browser")]`).

- [ ] **Step 8: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/browser_pane.rs crates/forktty-ui-gtk/Cargo.toml Cargo.lock
git commit -m "feat(gtk): pump browser command channel to WebView

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 5: CLI subcommands

**Files:**
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs`

- [ ] **Step 1: Study the existing pattern**

Read `fn handle_browser` (`socket_cli.rs:1366`), `fn browser_open` (`:1382`), `fn browser_navigate` (`:1424`), and the test `browser_open_sends_browser_open_with_url_and_workspace` (`:5395`). Note the EXACT helper names used to: split subcommand args, parse positionals/options, send a request, and print the result. The function bodies below use placeholder helper names (`parse_options`, `required_positional`, `context.request`, `print_json`) — replace them with the real ones SP1 used.

- [ ] **Step 2: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block, mirroring the harness `browser_open_sends_...` uses (`test_context_with_response` is a placeholder — use SP1's real helper):

```rust
    #[test]
    fn browser_snapshot_sends_request() {
        let (context, requests) = test_context_with_response(json!({
            "result": {"role": "root", "children": []}
        }));
        let result = handle_browser(&context, vec!["snapshot".into(), "s9".into()]);
        assert!(result.is_ok());
        let req = requests.lock().unwrap();
        assert_eq!(req[0]["method"], "browser.snapshot");
        assert_eq!(req[0]["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_click_sends_ref() {
        let (context, requests) = test_context_with_response(json!({"result": {"ok": true}}));
        let result = handle_browser(&context, vec!["click".into(), "s9".into(), "e3".into()]);
        assert!(result.is_ok());
        let req = requests.lock().unwrap();
        assert_eq!(req[0]["method"], "browser.click");
        assert_eq!(req[0]["params"]["surface_id"], "s9");
        assert_eq!(req[0]["params"]["ref"], "e3");
    }

    #[test]
    fn browser_fill_sends_ref_and_value() {
        let (context, requests) = test_context_with_response(json!({"result": {"ok": true}}));
        let result = handle_browser(
            &context,
            vec!["fill".into(), "s9".into(), "e3".into(), "hello world".into()],
        );
        assert!(result.is_ok());
        let req = requests.lock().unwrap();
        assert_eq!(req[0]["method"], "browser.fill");
        assert_eq!(req[0]["params"]["ref"], "e3");
        assert_eq!(req[0]["params"]["value"], "hello world");
    }

    #[test]
    fn browser_eval_sends_script() {
        let (context, requests) = test_context_with_response(json!({"result": "ForkTTY"}));
        let result = handle_browser(
            &context,
            vec!["eval".into(), "s9".into(), "document.title".into()],
        );
        assert!(result.is_ok());
        let req = requests.lock().unwrap();
        assert_eq!(req[0]["method"], "browser.eval");
        assert_eq!(req[0]["params"]["script"], "document.title");
    }

    #[test]
    fn browser_back_sends_request() {
        let (context, requests) = test_context_with_response(json!({"result": {"ok": true}}));
        let result = handle_browser(&context, vec!["back".into(), "s9".into()]);
        assert!(result.is_ok());
        let req = requests.lock().unwrap();
        assert_eq!(req[0]["method"], "browser.back");
        assert_eq!(req[0]["params"]["surface_id"], "s9");
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p forktty-ui-gtk browser_snapshot_sends_request`
Expected: FAIL — the new subcommands aren't handled.

- [ ] **Step 4: Extend handle_browser**

In `fn handle_browser`, extend the inner match (which currently matches `open`/`navigate`). Using the same args-splitting convention already present (`rest` = args after the subcommand keyword):

```rust
        "snapshot" => browser_snapshot(context, rest),
        "click" => browser_click(context, rest),
        "fill" => browser_fill(context, rest),
        "eval" => browser_eval(context, rest),
        "back" => browser_nav(context, rest, "browser.back", "back"),
        "forward" => browser_nav(context, rest, "browser.forward", "forward"),
        "reload" => browser_nav(context, rest, "browser.reload", "reload"),
```

- [ ] **Step 5: Add the subcommand functions**

Add near `browser_navigate` (`:1424`). Replace placeholder helpers with SP1's real ones found in Step 1:

```rust
fn browser_snapshot(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_options(args)?;
    reject_unknown_options(&parsed.options, &[], "browser snapshot")?;
    let surface_id = required_positional(&parsed.positionals, 0, "surface-id", "browser snapshot")?;
    let result = context.request("browser.snapshot", json!({"surface_id": surface_id}))?;
    print_json(&result);
    Ok(())
}

fn browser_click(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_options(args)?;
    reject_unknown_options(&parsed.options, &[], "browser click")?;
    let surface_id = required_positional(&parsed.positionals, 0, "surface-id", "browser click")?;
    let reference = required_positional(&parsed.positionals, 1, "ref", "browser click")?;
    let result = context.request(
        "browser.click",
        json!({"surface_id": surface_id, "ref": reference}),
    )?;
    print_json(&result);
    Ok(())
}

fn browser_fill(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_options(args)?;
    reject_unknown_options(&parsed.options, &[], "browser fill")?;
    let surface_id = required_positional(&parsed.positionals, 0, "surface-id", "browser fill")?;
    let reference = required_positional(&parsed.positionals, 1, "ref", "browser fill")?;
    let value = required_positional(&parsed.positionals, 2, "value", "browser fill")?;
    let result = context.request(
        "browser.fill",
        json!({"surface_id": surface_id, "ref": reference, "value": value}),
    )?;
    print_json(&result);
    Ok(())
}

fn browser_eval(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_options(args)?;
    reject_unknown_options(&parsed.options, &[], "browser eval")?;
    let surface_id = required_positional(&parsed.positionals, 0, "surface-id", "browser eval")?;
    let script = required_positional(&parsed.positionals, 1, "script", "browser eval")?;
    let result = context.request(
        "browser.eval",
        json!({"surface_id": surface_id, "script": script}),
    )?;
    print_json(&result);
    Ok(())
}

fn browser_nav(
    context: &CliContext,
    args: Vec<String>,
    method: &str,
    label: &str,
) -> CliResult<()> {
    let parsed = parse_options(args)?;
    reject_unknown_options(&parsed.options, &[], &format!("browser {label}"))?;
    let surface_id = required_positional(
        &parsed.positionals,
        0,
        "surface-id",
        &format!("browser {label}"),
    )?;
    let result = context.request(method, json!({"surface_id": surface_id}))?;
    print_json(&result);
    Ok(())
}
```

- [ ] **Step 6: Update HELP_TEXT**

In the `HELP_TEXT` const (`:26`), under the existing `browser open`/`browser navigate` lines, add (match the surrounding column alignment exactly):

```text
  browser snapshot <surface-id>            Dump the page accessibility tree (JSON)
  browser click <surface-id> <ref>         Click the element with the given snapshot ref
  browser fill <surface-id> <ref> <value>  Set an input's value by snapshot ref
  browser eval <surface-id> <script>       Run JavaScript, print the JSON result
  browser back <surface-id>                Navigate back in history
  browser forward <surface-id>             Navigate forward in history
  browser reload <surface-id>              Reload the current page
```

- [ ] **Step 7: Run tests + build**

Run: `cargo test -p forktty-ui-gtk browser_`
Expected: PASS.
Run: `cargo build -p forktty-ui-gtk --features gtk-vte` and `--features browser`
Expected: both compile.

- [ ] **Step 8: Commit**

```bash
git add crates/forktty-ui-gtk/src/socket_cli.rs
git commit -m "feat(cli): browser snapshot/click/fill/eval/back/forward/reload

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Task 6: Docs + final verification

**Files:**
- Modify: `docs/cmux-gap-features.md`
- Modify: `ROADMAP.md`

- [ ] **Step 1: Update the gap-features status**

In `docs/cmux-gap-features.md`, feature #3 Status line (`:45`):

```text
- **Impact**: high · **Cost**: high · **Status**: **SP1+SP2 done**; SP3 P1/P2 done; SP3 P3 core/socket done; P3 CLI/GTK wiring and P4 import backlog
```

Update the ForkTTY description paragraph (`:50`) to note SP2 shipped: snapshot/click/fill/JS-evaluate verbs, socket-driven back/forward/reload, `forktty browser snapshot|click|fill|eval|back|forward|reload` CLI, behind the `browser` feature.

- [ ] **Step 2: Update ROADMAP**

In `ROADMAP.md`, find the browser SP1 entry and add an SP2 line marked done (match the existing checkbox/format used for SP1).

- [ ] **Step 3: Full workspace verification**

```bash
cargo test -p forktty-core
cargo test -p forktty-socket
cargo test -p forktty-ui-gtk
cargo test -p forktty-ui-gtk --features browser
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p forktty-ui-gtk --features browser --all-targets -- -D warnings
```

Expected: all pass; fmt clean; clippy clean on both default and `browser` feature sets.

- [ ] **Step 4: Commit**

```bash
git add docs/cmux-gap-features.md ROADMAP.md
git commit -m "docs: mark browser pane SP2 scripting verbs done

Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>"
```

---

## Manual verification (with a display, after the code tasks)

1. `cargo build -p forktty-ui-gtk --features browser`
2. Run the app; `./target/debug/forktty browser open google.com`
3. `./target/debug/forktty browser snapshot <surface-id>` → JSON tree with `ref`s.
4. Pick an input's ref → `./target/debug/forktty browser fill <sid> <ref> hello` → field fills.
5. Pick a button/link ref → `./target/debug/forktty browser click <sid> <ref>` → navigates/acts.
6. `./target/debug/forktty browser eval <sid> "document.title"` → prints the title JSON.
7. `./target/debug/forktty browser back <sid>` / `reload <sid>` → history nav works.
8. Confirm a non-browser surface id returns a not-found error for each verb.

## Out of scope (SP3)

- Cookie / history / browser-profile import.
- Multiple tabs per pane; downloads UI; devtools; screenshots; network interception.
- Per-element waiting/retry beyond `scrollIntoView` on click.
