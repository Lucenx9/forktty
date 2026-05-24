# Browser pane — SP2: scriptable verbs + socket→GTK command channel

Date: 2026-05-24
Gap feature: #3 in `docs/cmux-gap-features.md` (Built-in browser pane, scriptable).
Builds on SP1 (`2026-05-24-browser-pane-sp1-design.md`, merged to `main`).

Status: implemented on `main` and later extended by SP3 profiles. P1/P2 and the
core P3 history/bookmark stores and CLI mirrors are now present; external browser
import and P3 GTK history wiring remain backlog.

## Goal

Add agent-browser-parity scripting to the browser pane: `snapshot` (accessibility
tree with element refs), `click`, `fill`, `eval`, plus socket-driven `back` /
`forward` / `reload`. These are request/reply operations against a live WebView, so
SP2 builds the imperative socket→GTK command channel that SP1 deliberately deferred.

Non-goals (SP2): persistence / profiles / history / import (SP3); multiple tabs
per pane; downloads UI; devtools; screenshots; network interception.

## Why a command channel (not model-state diff like SP1)

SP1 expressed navigation as model state (`browser.navigate` sets `url`, GTK observes
the diff). That works for fire-and-forget state but not for SP2: `snapshot` and `eval`
must **return data** to the socket caller, and `click` / `fill` act imperatively on
the live DOM with no natural model representation. SP2 therefore introduces a
request/reply channel.

The socket server runs on a dedicated OS thread with its own multi-thread tokio
runtime (`gtk_app.rs` spawns `runtime.block_on(serve(..))`); the WebView lives on the
GTK main thread. The channel must cross both the thread and the runtime boundary.

```text
socket handler (tokio thread)                 GTK main thread (WebView)
  cmd = BrowserCommand {                        glib::spawn_future_local loop:
    surface_id, op,                               rx.recv().await
    reply: oneshot::Sender<CmdResult> }           route surface_id -> BrowserPaneWidget
  sender.send(cmd).await        ───────────────►  webview.evaluate_javascript(js, cb)
  timeout(reply_rx).await       ◄───────────────  cb: reply.send(CmdResult { .. })
```

- `async_channel` (runtime-agnostic; usable from both tokio and the glib main context)
  carries commands main-thread-ward.
- Each command embeds a `tokio::sync::oneshot::Sender<CmdResult>` for the reply.
  `oneshot::Sender::send` is synchronous and non-blocking, so the GTK closure can
  fulfil it directly from inside the async JS callback.
- Bounded `tokio::time::timeout` on the socket side guards against a hung WebView.

## Architecture

```text
forktty-core (pure, no webkit; gains async_channel + oneshot command types)
  enum BrowserOp { Snapshot, Click { reference }, Fill { reference, value },
                   Eval { script }, Back, Forward, Reload }
  struct BrowserCommand { surface_id: String, op: BrowserOp,
                          reply: oneshot::Sender<CmdResult> }   // not Serialize (holds a Sender)
  enum CmdResult { Ok, Json(String), Err(BrowserCmdError) }
  enum BrowserCmdError { SurfaceGone, NotABrowser, NoWebView, RefNotFound,
                         JsError(String), TooLarge, Internal(String) }

forktty-socket (depends on core + async_channel; NO webkit)
  SocketAppState.browser_cmd: Option<async_channel::Sender<BrowserCommand>>  (None = unavailable)
  dispatch arms: browser.snapshot / click / fill / eval / back / forward / reload
  each: validate surface is Browser in model -> build command + oneshot ->
        send on browser_cmd -> timeout(reply) -> map CmdResult to JSON / RpcError
  METHODS gains the 7 verbs (sorted); capabilities drift test covers them

forktty-ui-gtk  [existing cargo feature: browser]
  driver.js (include_str!): window.__forktty = { snapshot, click, fill } + ref map
  browser_pane.rs: register driver.js as a persistent UserContentManager user script
                   at document-start; methods run evaluate_javascript and resolve a
                   GLib async callback
  gtk_app.rs: create async_channel, store sender in SocketAppState via builder,
              pump receiver on the main context (glib::spawn_future_local), route
              each command to the addressed BrowserPaneWidget, fulfil reply
```

## Components

### forktty-core/src/browser_cmd.rs (new, pure)

- `enum BrowserOp { Snapshot, Click { reference: String }, Fill { reference: String,
  value: String }, Eval { script: String }, Back, Forward, Reload }` —
  `Debug, Clone, PartialEq`. Field is named `reference` (not `ref`, a Rust keyword).
- `struct BrowserCommand { pub surface_id: String, pub op: BrowserOp, pub reply:
  tokio::sync::oneshot::Sender<CmdResult> }`. Not `Clone`/`Serialize` (owns a Sender).
- `enum CmdResult { Ok, Json(String), Err(BrowserCmdError) }` — `Debug`.
- `enum BrowserCmdError { SurfaceGone, NotABrowser, NoWebView, RefNotFound,
  JsError(String), TooLarge, Internal(String) }` — `Debug, Clone, PartialEq`;
  `Display` impl produces the human message used in socket errors.
- `const MAX_BROWSER_RESULT_BYTES: usize = 256 * 1024;` — cap on a JSON result.
- `const MAX_BROWSER_SCRIPT_BYTES: usize = 64 * 1024;` — cap on an `eval` script
  (below the global 1 MiB request cap; eval payloads are small in practice).
- Unit tests: `BrowserOp` equality; `BrowserCmdError` `Display` strings stable.

`async_channel` and `tokio` (oneshot only, `features = ["sync"]`) become core deps.
Neither pulls webkit; both are runtime-agnostic data plumbing.

### forktty-socket/src/lib.rs

- `SocketAppState` gains `pub browser_cmd: Option<async_channel::Sender<BrowserCommand>>`,
  defaulting `None` in `new`. Add builder `with_browser_cmd(sender) -> Self`.
- `const BROWSER_CMD_TIMEOUT: Duration = Duration::from_secs(10);`
- New async helper `dispatch_browser_cmd(state, surface_id, op) -> Result<CmdResult, DispatchError>`:
  1. lock model, confirm `surface_id` exists and is `SurfaceKind::Browser` → else
     `NotFound`;
  2. `let Some(sender) = &state.browser_cmd` → else error
     `"browser automation unavailable"`;
  3. build `(reply_tx, reply_rx) = oneshot::channel()`; `sender.send(BrowserCommand
     { surface_id, op, reply: reply_tx }).await` (channel closed → unavailable);
  4. `tokio::time::timeout(BROWSER_CMD_TIMEOUT, reply_rx).await` → `Timeout` error on
     elapse, `Internal` on `RecvError` (GTK dropped the reply).
- Dispatch arms (the four scripting verbs validate/cap params first):
  - `browser.snapshot { surface_id }` → `Snapshot` → `CmdResult::Json` returned as
    raw JSON in the result body.
  - `browser.click { surface_id, ref }` → require non-empty `ref` → `Click`.
  - `browser.fill { surface_id, ref, value }` → require `ref`; `value` may be empty →
    `Fill`.
  - `browser.eval { surface_id, script }` → require non-empty `script`, cap at
    `MAX_BROWSER_SCRIPT_BYTES` (else `PayloadTooLarge`) → `Eval`.
  - `browser.back` / `browser.forward` / `browser.reload { surface_id }` → the
    matching op; reply `CmdResult::Ok` → `{"ok":true}`.
- `CmdResult` → response mapping: `Ok` → `{"ok":true}`; `Json(s)` → the parsed JSON
  value; `Err(e)` → `RpcError` (`RefNotFound`/`SurfaceGone`/`NotABrowser`/`NoWebView`
  → NotFound class, `TooLarge` → PayloadTooLarge, `JsError`/`Internal` → Other).
- Add the 7 method names to the sorted `METHODS` const (drift test enforces match).

### forktty-ui-gtk/src/driver.js (new, embedded via include_str!)

A self-contained IIFE installing `window.__forktty`:

- `let refMap = new Map(); let counter = 0;`
- `snapshot()`: reset `refMap`/`counter`; walk the document from `document.body`,
  visiting elements; for each element with a meaningful accessible role/name (ARIA
  `role` or implicit role from tag, accessible name from `aria-label` /
  `aria-labelledby` / text / `placeholder` / `value`), assign `ref = "e" + (++counter)`,
  `refMap.set(ref, el)`, and emit a node `{ ref, role, name, value, children: [...] }`.
  Skip hidden elements (`hidden`, `display:none`, `visibility:hidden`,
  `aria-hidden="true"`). Return the root node. (Pragmatic ARIA walk, not full AT-SPI.)
- `click(ref)`: `const el = refMap.get(ref)`; missing → `throw "ref-not-found"`;
  else `el.scrollIntoView(); el.click(); return true`.
- `fill(ref, value)`: missing → `throw "ref-not-found"`; set `el.value = value` and
  dispatch `input` + `change` events; return `true`.
- `eval` has no driver entry — `browser.eval` runs the caller's script directly.

The script is idempotent (guards against double-install) so it can register at
document-start on every navigation without clobbering state mid-page.

### forktty-ui-gtk/src/browser_pane.rs

- In `BrowserPaneWidget::new`, obtain the WebView's `UserContentManager` and
  `add_script` a `webkit6::UserScript` built from `driver.js`
  (`InjectionTime::Start`, all frames, main world). This makes `window.__forktty`
  available before page scripts run.
- New async-style methods returning results via a callback (GLib async):
  - `run_js(&self, js: &str, on_done: impl FnOnce(Result<String, BrowserCmdError>))` —
    calls `evaluate_javascript`; in the GIO callback, serialize the `JSCValue` to a
    JSON string (`to_json(0)`), enforce `MAX_BROWSER_RESULT_BYTES` (→ `TooLarge`), map
    a JS exception to `JsError`, and invoke `on_done`.
  - `go_forward`, `reload` already exist from SP1 (`go_back` too); reused for nav ops.
- `BrowserOp::Click`/`Fill` build the JS call string by JSON-encoding the ref/value
  (`window.__forktty.click(<json ref>)`) so quoting is safe.

### forktty-ui-gtk/src/gtk_app.rs

- Before launching the socket thread, create `let (cmd_tx, cmd_rx) =
  async_channel::unbounded::<BrowserCommand>();` and attach `cmd_tx` to the state via
  `SocketAppState::with_browser_cmd` (feature `browser` only; without the feature the
  sender is never set and verbs report unavailable).
- Pump the receiver on the GTK main context with `glib::spawn_future_local(async move
  { while let Ok(cmd) = cmd_rx.recv().await { handle_browser_command(cmd, ..) } })`.
- `handle_browser_command(cmd, browser_panes, model)`:
  - look up `browser_panes[&cmd.surface_id]` → missing → `cmd.reply.send(Err(NoWebView))`.
  - match `cmd.op`:
    - `Snapshot` → `pane.run_js("window.__forktty.snapshot()", move |r| reply.send(r.map(CmdResult::Json).unwrap_or_else(|e| CmdResult::Err(e))))`.
    - `Click { reference }` → `run_js("window.__forktty.click(<json>)", ..)` mapping a
      `ref-not-found` JsError to `CmdResult::Err(RefNotFound)`.
    - `Fill { reference, value }` → `run_js("window.__forktty.fill(<json>,<json>)", ..)`.
    - `Eval { script }` → `run_js(&script, ..)`.
    - `Back`/`Forward`/`Reload` → call the WebView nav method, `reply.send(Ok)`.
  - The reply Sender is moved into the async JS callback so it fulfils once the JS
    settles. (If a callback can fire after the pane is dropped, the moved Sender simply
    errors on `send`; the socket side maps the dropped reply to `Internal`.)

## Data flow

```text
browser.snapshot(surface_id):
  socket: validate Browser surface -> send BrowserCommand{Snapshot, reply}
       -> GTK pump -> pane.run_js("window.__forktty.snapshot()")
       -> JS walk -> JSON tree -> evaluate_javascript cb -> reply.send(Json(tree))
       -> socket timeout-wait resolves -> response body = tree

browser.click(surface_id, ref):
  same path, op=Click; JS looks up refMap; ok -> Ok / missing -> Err(RefNotFound)

browser.back(surface_id):
  socket -> Back -> GTK pump -> pane.go_back() -> reply.send(Ok) -> {"ok":true}
```

## Error handling

- Surface missing / not a browser → `NotFound` (checked under the model lock before
  any command is sent).
- `browser_cmd` sender `None` (no `browser` feature, or headless) or channel closed →
  `"browser automation unavailable"` error.
- No live WebView for the surface (pane not realized) → reply `Err(NoWebView)` →
  NotFound-class socket error.
- Bad ref → JS throws `ref-not-found` → `Err(RefNotFound)`.
- JS runtime error → `Err(JsError(msg))` → Other-class socket error with the message.
- Result JSON over `MAX_BROWSER_RESULT_BYTES` → `Err(TooLarge)` → `PayloadTooLarge`.
- `eval` script over `MAX_BROWSER_SCRIPT_BYTES` → `PayloadTooLarge` (rejected socket-side).
- WebView wedged / callback never fires → `BROWSER_CMD_TIMEOUT` elapses → `Timeout`.

## Testing

- **core** (`browser_cmd.rs`, pure): `BrowserOp` equality; `BrowserCmdError` `Display`
  strings; size constants present. No I/O.
- **socket**:
  - each verb returns `"browser automation unavailable"` when `browser_cmd` is `None`.
  - with a **stub sender** (a test installs an `async_channel` receiver that answers
    commands): `browser.snapshot` returns the stub's JSON; `browser.eval` round-trips;
    `browser.back` returns `{"ok":true}`; an unanswered command yields `Timeout`
    (use a short test-only timeout via a constructor seam, or drop the reply to force
    the `RecvError`→`Internal` path).
  - surface validation: non-browser / missing surface → `NotFound` before send.
  - `browser.eval` over the script cap → `PayloadTooLarge`.
  - capabilities/METHODS drift test automatically covers the 7 new verbs.
- **gtk** (`browser` feature, compile-gated): `driver.js` is non-empty and parses as a
  script the UserContentManager accepts; `run_js` wiring compiles; the snapshot/click
  JSON-call builders quote correctly (unit-testable string builders extracted from the
  GLib calls).
- **Manual**: build `--features browser`; `forktty browser open google.com`;
  `forktty browser snapshot <sid>` → inspect tree; `forktty browser fill <sid> <ref>
  hello` + `click`; `forktty browser eval <sid> "document.title"`; `back`/`reload`.

## Build sequence

1. core: `browser_cmd.rs` (`BrowserOp`, `BrowserCommand`, `CmdResult`,
   `BrowserCmdError`, size consts) + deps (`async_channel`, `tokio` sync) + unit
   tests + re-export from `lib.rs`. Verify `cargo test -p forktty-core`.
2. socket: `browser_cmd` field + `with_browser_cmd` + `BROWSER_CMD_TIMEOUT` +
   `dispatch_browser_cmd` + 7 dispatch arms + METHODS entries + stub-sender tests.
   Verify `cargo test -p forktty-socket`.
3. gtk: `driver.js` + UserContentManager install + `run_js` + nav reuse in
   `browser_pane.rs`. Verify `cargo build -p forktty-ui-gtk --features browser`.
4. gtk: async_channel creation + `with_browser_cmd` wiring + main-context pump +
   `handle_browser_command` routing in `gtk_app.rs`. Verify
   `--features browser` and `--features gtk-vte` (no browser) both compile.
5. CLI: `forktty browser snapshot|click|fill|eval|back|forward|reload` subcommands +
   help text + arg validation tests in `socket_cli.rs`.
6. Docs: `docs/cmux-gap-features.md` (#3 → SP2 done, SP3 backlog) + `ROADMAP.md`.

## Concurrency note

The GTK command pump processes commands sequentially but does not await each
`run_js` callback before pulling the next command, so two in-flight commands'
JS callbacks may interleave. Rust-side state is unaffected (each command owns its
reply channel; the pane map is read-and-cloned). The one shared resource is the
per-page JS `refMap`, which `snapshot` resets — so concurrent callers issuing
`snapshot` + `click` against the same pane can invalidate each other's refs.
SP2 targets a single serial agent (snapshot → act on a ref → snapshot); callers
that drive a pane concurrently must serialize their own snapshot/act cycles.

## Out of scope (SP2, see SP3)

- Cookie / history / browser-profile import.
- Multiple tabs within a single browser pane; downloads UI; devtools; screenshots.
- Network request interception; per-element waiting/retry; auto-scroll heuristics
  beyond `scrollIntoView` on click.
