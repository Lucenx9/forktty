# Socket `events` stream + `capabilities`

Date: 2026-05-24
Gap feature: #2 in `docs/cmux-gap-features.md`.

## Goal

Give external automation (editors, MCP servers, scripts) two things the
request/response socket cannot offer today:

1. **`events.subscribe`** — a long-lived connection that streams newline-delimited
   JSON (NDJSON) describing changes to the workspace model: workspaces, surfaces,
   focus, status, progress, notifications, listening ports, and linked PR state.
2. **`capabilities`** — a one-shot verb returning the protocol version and the list
   of supported methods, so clients can feature-detect instead of guessing.

Non-goals: persistence/replay of past events, per-client filtering, authentication
beyond the existing owner-only socket permissions, `cmux top`-style resource usage.

## Why diff-based emission

The model (`Arc<Mutex<WorkspaceModel>>`) is mutated from two places: the tokio
socket server (hooks/CLI/automation) and the GTK main loop (user clicks, the 3s
ports timer, the 30s PR timer). Instrumenting every `set_*`/`create_*`/`close_*`
call site would mean touching dozens of points across two crates and silently
missing any that are added later.

Instead, a single background task snapshots the model on a fixed interval, diffs the
new snapshot against the previous one, and emits one `ModelEvent` per change. This is
source-agnostic (catches GTK- and socket-origin mutations identically), needs zero
instrumentation of mutation sites, and the diff is a pure function that is trivially
unit-tested. The cost is latency bounded by the tick interval (250 ms) — acceptable
for a local automation feed.

## Components

### `forktty-core/src/events.rs` (pure, testable)

Field names below match the real model (`Workspace.name`, keyed `StatusEntry`/
`ProgressEntry`, `NotificationItem.id`, per-workspace `focused_surface_id`):

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ModelEvent {
    WorkspaceAdded { id: String, name: String },
    WorkspaceRemoved { id: String },
    WorkspaceSelected { id: Option<String> },          // None = nothing active
    SurfaceAdded { id: String, workspace_id: String },
    SurfaceRemoved { id: String },
    SurfaceFocused { workspace_id: String, surface_id: String }, // per-workspace focus
    SurfaceTitleChanged { id: String, title: String },
    // Status/progress are keyed multi-entries per workspace; value None = key cleared.
    StatusChanged { workspace_id: String, key: String, value: Option<String> },
    ProgressChanged { workspace_id: String, key: String, value: Option<f64>, total: Option<f64> },
    // Edge-triggered on a notification id not seen in the previous snapshot.
    NotificationAdded { id: String, workspace_id: Option<String>, title: String, body: String },
    PortsChanged { workspace_id: String, ports: Vec<u16> },
    PrChanged { workspace_id: String, pr: Option<String> },   // pr = PrInfo::summary()
}
```

- `struct Snapshot` — a compact, owned copy of just the fields the events above
  depend on, keyed by id for cheap diffing:
  - workspaces: `BTreeMap<id, WsSnap { name, focused_surface_id, status: BTreeMap<key,value>, progress: BTreeMap<key,(value,total)>, ports: Vec<u16>, pr: Option<String> }>`
  - surfaces: `BTreeMap<id, SurfSnap { workspace_id, title }>`
  - `active_workspace_id: Option<String>`
  - notifications: `BTreeMap<id, NotifSnap { workspace_id, title, body }>` (ids only ever grow within a session; cleared notifications produce no event).

  It deliberately does NOT hold the full model so the diff stays cheap and the tick
  lock is short.
- `pub fn snapshot(model: &WorkspaceModel) -> Snapshot` — built under the model lock,
  via `list_workspaces`, `list_surfaces`, `active_workspace_id`, `list_status`,
  `list_progress`, `list_notifications`.
- `pub fn diff(prev: &Snapshot, next: &Snapshot) -> Vec<ModelEvent>` — pure;
  deterministic order: workspace removes, workspace adds, surface removes, surface
  adds, then per-workspace field changes (selected, focus, title, status, progress,
  ports, pr), then new notifications.

Notifications are edge-triggered on new ids: an id present in `next` but not `prev`
yields one `NotificationAdded`. A notification cleared (id gone) yields nothing —
clients track liveness via workspace/surface events, not notification removal.

### `forktty-socket/src/lib.rs`

- `SocketAppState` gains `events: broadcast::Sender<ModelEvent>` (tokio
  `broadcast::channel(EVENTS_CHANNEL_CAPACITY)`, capacity 256). `SocketAppState::new`
  creates the channel; the sender is cloned into both the tick task and each
  subscriber's receiver via `subscribe()`.
- `serve()` spawns the **diff-tick task** before the accept loop: every 250 ms, lock
  model → `snapshot()` → `diff(prev, next)` → `send` each event (ignore `SendError`
  when no subscribers) → store `next` as `prev`. The task ends when `serve` returns.
- `handle_connection` special-cases `method == "events.subscribe"`: it does NOT go
  through `dispatch`. Instead it:
  1. writes an initial NDJSON line `{"event":"subscribed"}` (handshake / probe-safe),
  2. optionally replays the current state as `*_added`/`*_changed` events built from a
     fresh snapshot diffed against an empty one, so a new subscriber sees the whole
     world (controlled by request param `"replay": true`, default true),
  3. loops on `receiver.recv()`, writing each event as one NDJSON line, until the
     peer disconnects (write error) or the server stops. On `RecvError::Lagged(n)` it
     writes `{"event":"lagged","dropped":n}` and continues. On `RecvError::Closed` it
     ends.
- `capabilities` verb (normal sync dispatch): returns
  `{"version": <crate version>, "methods": [ ... sorted verb names ... ]}`. The method
  list is a single source-of-truth `const METHODS: &[&str]` that the `dispatch` match
  is checked against by a test (so the list can't drift from the match arms).

### CLI: `forktty events`

New subcommand in `socket_cli.rs`. Unlike every other verb it streams: connect, send
`events.subscribe` (passing `--no-replay` → `"replay": false`), then read lines from
the socket and write each to stdout until the connection closes or the user
interrupts. No read timeout (the stream is idle-by-design). Reconnection is the
caller's job (re-run the command), matching cmux's reconnectable contract.

`forktty capabilities` subcommand: one-shot, prints the `capabilities` result.

## Data flow

```
GTK mutations ─┐
               ├─► Arc<Mutex<WorkspaceModel>> ─► [250ms tick] snapshot+diff ─► broadcast::Sender
socket dispatch┘                                                                      │
                                                                                      ▼
                                                          per-subscriber broadcast::Receiver
                                                                                      │
                                                          events.subscribe connection │─► NDJSON lines ─► client
```

## Error handling

- Broadcast lag (slow client): channel drops oldest; subscriber gets
  `RecvError::Lagged(n)` → emits `{"event":"lagged","dropped":n}` so the client knows
  to resync (reconnect with replay).
- Client disconnect mid-stream: write fails → subscriber loop ends, receiver dropped.
- No subscribers: tick task's `send` returns `Err` → ignored.
- Connection cap: `events.subscribe` holds a connection for its whole lifetime and so
  consumes one of the `MAX_SOCKET_CONNECTIONS` permits — documented; a runaway client
  count is bounded by the existing semaphore.

## Testing

- `events.rs`: unit tests for `diff` covering each event variant (add/remove/select/
  focus/title/status/progress/notification/ports/pr), the empty→full case (replay),
  and the no-change case (empty diff). Pure, no I/O.
- `forktty-socket`: a test that subscribes, mutates the model via existing dispatch
  verbs, and asserts the expected NDJSON events arrive on the receiver; a test that
  `capabilities` lists exactly the dispatchable methods (guards match/list drift); a
  test that a lagged receiver yields the lagged notice.
- Manual: `forktty events` in one terminal, drive changes from another, observe stream.

## Build sequence

1. `events.rs` + unit tests (core). Verify: `cargo test -p forktty-core`.
2. Channel in `SocketAppState` + tick task in `serve` + `capabilities` verb + method
   const + drift test. Verify: `cargo test -p forktty-socket`.
3. `events.subscribe` streaming branch in `handle_connection` + subscribe/lag tests.
4. CLI `events` + `capabilities` subcommands + help text.
5. Update `docs/cmux-gap-features.md` (#2 → done) and `ROADMAP.md`.
