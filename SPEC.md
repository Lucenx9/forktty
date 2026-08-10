# ForkTTY Technical Specification

ForkTTY is a Linux-only native terminal for running multiple coding agents in parallel. The primary implementation is now Rust + GTK4/libadwaita + Ghostty, with direct Unix socket automation and git worktree isolation.

## Runtime Architecture

```text
forktty-core
  Workspace model, pane tree, config, session v2, notifications, worktree logic,
  browser profile/history stores

forktty-terminal
  TerminalBackend trait, headless test backend, Ghostty adapter

forktty-socket
  Tokio Unix socket server, newline-delimited JSON-RPC, direct dispatch,
  events stream, capabilities discovery

forktty-ui-gtk
  GTK4/libadwaita app shell, Ghostty panes, sidebar, dialogs, quake mode,
  socket CLI, agent hook installer over the socket API, and optional
  WebKitGTK6 browser panes behind the `browser` feature
```

There is no Tauri/React runtime in the primary tree. WebKitGTK6 is used
only by the optional browser-pane feature.

## Tech Stack

| Layer | Technology | Use |
| ----- | ---------- | --- |
| UI shell | GTK4 + libadwaita | Native Linux window, header, dialogs, sidebar |
| Terminal | Ghostty GTK4 | Embedded terminal widget and child PTY owner |
| Browser feature | WebKitGTK6 | Optional browser-pane surface, page scripting, and per-profile web data |
| State | Rust `WorkspaceModel` | Workspaces, panes, surfaces, metadata, notifications |
| Git | `git2` | Worktree create/attach/remove/merge/status |
| Notifications | `notify-rust` | Desktop notifications and custom notification command dispatch |
| Socket API | tokio + serde_json | Local Unix JSON-RPC automation |
| Config | TOML | `~/.config/forktty/config.toml` |
| Browser stores | `uuid`, `rusqlite`, JSON | Profile identity, history store, and bookmarks metadata |
| CLI hooks | Rust (`forktty` binary) | Native socket client and hook config merger |

## Workspace and Pane Model

Each workspace has:

- stable workspace ID;
- name and active state;
- working directory;
- optional git branch and worktree metadata;
- recursive `PaneNode` tree;
- focused surface ID;
- attention/unread state.

Each surface has:

- stable surface ID;
- workspace ID;
- cwd;
- optional terminal title;
- surface kind (`Terminal`, `Ssh { host }`, or `Browser { url, profile }`);
- unread/attention state.

Splits are represented as recursive `PaneNode::Split { axis, children, sizes }`; leaf nodes reference surface IDs. GTK renders the active workspace tree into nested `gtk::Paned` containers and reuses Ghostty widgets by surface ID.

## Terminal Lifecycle

1. A workspace or split creates a surface in `WorkspaceModel`.
2. `forktty-ui-gtk` sends a `SpawnRequest` through `TerminalBackend`.
3. The GTK controller loads `ghostty-gtk-embed.so`, creates an embedded
   Ghostty GTK surface with the ForkTTY surface cwd, and packs that widget into
   ForkTTY pane chrome. Ghostty owns the child PTY and terminal widget; ForkTTY
   owns the workspace model, pane tree, socket API, and surrounding GTK shell.
4. If the embedding library cannot load or the Ghostty surface cannot be
   created, ForkTTY records a terminal spawn failure and does not open a
   classic-renderer fallback pane.
   Restored agent terminals follow the same fail-closed rule: an invalid
   persisted session ID, resume cwd, or unsupported provider records a red
   surface status, error log, and notification and never falls back to the
   configured shell.
5. Socket methods and GTK actions send text, read visible/full/tail text,
   perform copy/paste/select-all/find, restart, split, focus, close, or resize
   surfaces through ForkTTY's model plus the Ghostty GTK embedding ABI.
6. Ghostty GObject signals and ABI calls mirror title, child-exit status, close
   requests, child PID, and scrollback snapshots into ForkTTY state.
7. Closing a pane/workspace closes the corresponding Ghostty surface.

Prompt/status detection uses ForkTTY hook integration termprops, Ghostty
bell/child-exit state, and a bounded scrollback-tail prompt fallback when the
limited text ABI is available (visible-text fallback on older libraries).

## Session Persistence

Native session file:

```text
~/.local/share/forktty/session-v2.json
```

The native session includes workspace order, active workspace, pane tree, focused surface, each local terminal pane's last live cwd, branch, worktree metadata, generated workspace/surface id high-water marks, and opt-in bounded plain-text tails when `appearance.persistent_scrollback_lines` is greater than zero. Embedded Ghostty panes source those tails from the bounded end of full scrollback when `ghostty_gtk_surface_read_text_limited` is available, and fall back to recent visible text with older embedding libraries. The pinned release library provides the optional `ghostty_gtk_surface_read_text_limited_with_total_lines` extension, so socket snapshots report `total_lines` for the complete selected source before byte truncation; older compatible libraries without it report the bounded fragment's line count. It does not serialize running PTY process handles; the session file records only durable identifiers (surface ids and agent resume metadata). Process survival across a UI restart is a separate, opt-in mechanism (`general.persist_terminal_processes`) described under [PTY process persistence](#pty-process-persistence), keyed by the persisted surface id rather than by any serialized handle.

Live `/proc/<pid>/cwd` values enter the model only when they are valid UTF-8;
non-UTF-8 values are ignored so the last valid cwd remains serializable through
the JSON socket API and native session persistence.

Browser panes persist their surface URL and profile ID in the same session
model. WebKit processes, in-memory page state, and terminal PTY state are
not restored.

ForkTTY can import the legacy `session.json` format but saves native sessions as v2. Session load validates file type, size, version, pane-tree depth/shape, and focused leaf index. Invalid files are quarantined instead of crashing startup.

### PTY process persistence

Embedded Ghostty surfaces own their child PTY for the lifetime of the GTK
process; the embedding ABI exposes no way to detach a running PTY from one
surface and re-attach it to a surface created after a UI restart. So by default
a UI restart re-spawns fresh shells and replays saved scrollback, but the
previous processes are gone.

When `general.persist_terminal_processes` is enabled (default off) and a
detach/reattach broker (`dtach`) is found on an absolute `PATH` entry, ForkTTY
launches plain interactive terminal surfaces under the broker in
attach-or-create mode (`dtach -A <socket> -E -z <program> <args>`). The broker
keeps the real program and its descendants under its own PTY in a detached
daemon; the embedded surface runs only the broker client, which dies with the
GTK process. On relaunch ForkTTY spawns a fresh client that re-attaches to the
surviving daemon, so the shell, dev servers, REPLs, editors, and long-running
commands continue. AppImage-launched broker commands are first passed through
ForkTTY's internal child-exec helper so inherited AppImage runtime file
descriptors are closed before `dtach` starts and detached brokers do not keep a
FUSE AppImage mount alive. The reattach is keyed by a per-surface socket path
derived from the persisted surface id, so no extra session state is serialized;
if no daemon survived (the program exited), the broker creates a fresh session.

Scope and boundaries: persistence applies only to spawns explicitly marked as
plain interactive `Terminal` shells. Agent panes persist through provider
resume, SSH surfaces are already remote, browser surfaces are not terminals,
and project actions are delegated command executions, so none of them are wrapped. The
behavior is unchanged when the flag is off or no broker is installed. Broker
sockets live under `$XDG_RUNTIME_DIR/forktty-pty/` with owner-only (`0700`)
directory permissions, the surface id is validated as a safe filename component
(no path separators or traversal) before it is used in the socket path, and
the complete socket path is capped below Linux's Unix-domain `sun_path` limit.
ForkTTY never wraps a `sh -c` command — the no-`sh -c` argv policy holds across
the broker boundary. The broker program itself is resolved only from absolute
`PATH` entries, matching terminal child-program resolution. Explicit surface
close/restart terminates the matching ForkTTY-managed broker process tree and
removes the per-surface broker socket so a future reused surface id cannot
attach to stale detached state. Disabling the setting live cleans stale
detached sessions while preserving currently visible surfaces until they close;
closing the GTK window with the flag off cleans visible managed broker sessions
too. Startup with the flag off cleans old managed sessions before restore.
Normal UI process exit with the flag on does not take that cleanup path,
preserving the intended restart/relaunch reattach behavior.

## Config

Config file:

```text
~/.config/forktty/config.toml
```

```toml
[general]
shell = "/bin/bash"
worktree_layout = "nested"
enable_pr_lookup = false
notification_command = ""
persist_terminal_processes = false

[appearance]
persistent_scrollback_lines = 0
sidebar_position = "left"
sidebar_visible = true
window_mode = "normal"

[notifications]
desktop = true
sound = true
blocked_terminal_apps = []
blocked_terminal_types = []

[updates]
auto_check = true

[telemetry]
anonymous_ping = true
```

Config files are regular-file checked and capped at 1 MiB. Malformed or
invalid content is quarantined; transient I/O errors are reported without
renaming the file. Load validates and normalizes `general.shell` from manual
TOML edits, while the Settings dialog intentionally does not expose a shell
editor. Saved settings validate worktree layout, terminal-process persistence,
persistent scrollback bounds (max 1,000 lines), sidebar position, window mode,
PR lookup, update checks, telemetry, notification filters, and notification
commands. Settings > Worktrees exposes `general.persist_terminal_processes` and
reports whether `dtach` is detectable on the ForkTTY process `PATH`.

The GTK workspace sidebar is a collapsed libadwaita overlay, so showing or
hiding it does not resize the terminal layout. `appearance.sidebar_position`
chooses the left or right edge and `appearance.sidebar_visible` remains the
persisted startup preference used by Settings and the Ctrl+B/F9 action.

Legacy `general.theme_source`, `font_family`, `font_size`, `scrollback_lines`,
`terminal_audible_bell`, `terminal_renderer`, `terminal_theme`, and the temporary
alpha `embedded_ghostty` switch are accepted on load for compatibility, omitted
from new saves, and ignored by the GTK runtime. Unknown removed orchestration
and team keys are ignored by serde and are omitted the next time ForkTTY saves
configuration.

Terminal font, colors, cursor, selection, bell, mouse, `scrollback-limit`, and
`scrollbar` preferences come from Ghostty configuration when present; no system
Ghostty install is required. Embedded panes use Ghostty's bounded default
scrollback budget (10,000,000 bytes per surface) when no limit is configured.
The GTK runtime reads `~/.config/ghostty/config.ghostty` and the legacy
`~/.config/ghostty/config`, follows `config-file`, and resolves Ghostty themes
under the existing size and regular-file guards.

Ghostty compatibility scope:

| Ghostty config area | ForkTTY status |
| --- | --- |
| Config discovery | Supported for `~/.config/ghostty/config`, `~/.config/ghostty/config.ghostty`, recursive `config-file`, and Ghostty theme directories, with ForkTTY's regular-file and size guards. |
| Terminal appearance | Supported for terminal font family/style/synthetic-style fallbacks, font size, font features/variations, foreground/background, cursor colors, DECSCUSR-backed cursor style defaults, selection colors/clear-on-typing/clear-on-copy/word-chars/copy-on-select, clipboard trim-trailing-spaces/codepoint-map, right-click action, scroll-to-bottom, scrollbar policy, mouse reporting/shift-capture including XTSHIFTESCAPE overrides, mouse-hide-while-typing, bold, faint, ANSI palette, named colors, short/full hex colors, `theme`, cell/text-decoration/cursor metric adjustment, image storage limit, mouse scroll multiplier, and inactive split dimming. Embedded panes follow Ghostty's bounded `scrollback-limit` budget and pack the surface in a GTK scrolled window so retained history is reachable from the UI. |
| Runtime terminal state | Delegated to `libghostty-vt` for VT parsing, key/paste encoding, OSC 8 links, OSC 9/99 notifications, bracketed paste/focus mode mirrors, XTSHIFTESCAPE mouse-shift capture overrides, selection formatting, word selection, and Kitty image protocol storage/loading media plus PNG decode/placement geometry. ForkTTY snapshots Kitty placements across the GTK boundary as RGBA buffers bounded by the rendered pixel footprint, not the stored source image size. Shell startup integration uses Ghostty's upstream shell scripts when resources are available. |
| ForkTTY-owned UI | Intentionally not read from Ghostty config. Window layout, tabs, splits, sidebar, socket automation, worktrees, agent controls, notifications UI, and session restore use ForkTTY config/session state. |
| Ghostty GUI/window/platform options | Ignored unless ForkTTY has the same runtime concept. Examples include Ghostty keybinds, quick terminal, window decorations, titlebar/font, shell integration UI, macOS-only options, shaders, background blur/opacity, and Linux cgroup settings. |
| Renderer parity | Terminal panes use the embedded Ghostty GTK widget and require `ghostty-gtk-embed.so`; if the library cannot load or an embedded surface cannot spawn, ForkTTY records a terminal spawn failure instead of falling back to the classic GTK/Pango/Cairo renderer. The full upstream Ghostty source is pinned at `vendor/ghostty` for the cmux-style renderer/widget integration. Upstream's current public C embedding API is macOS/iOS-only for surfaces, while the Linux `GhosttySurface` GTK widget is internal to Ghostty's GTK app runtime. ForkTTY's Ghostty fork now carries a GTK widget embedding ABI with cwd, direct command spawn, socket text-input, and visible/full text-read hooks. Embedded panes pass the requested argv plus per-surface `FORKTTY_*` environment through `ghostty_gtk_surface_new_with_working_directory_and_command` when available, avoiding typed bootstrap text in the child shell; older libraries start Ghostty's default shell in the requested cwd without ForkTTY environment injection. ForkTTY-managed embedded panes force Ghostty's `wait-after-command` behavior so clean child exits remain inspectable as `Closed` panes with restart/scrollback parity instead of closing the split immediately. Embedded panes also mirror Ghostty surface title, child-exit readiness/status (with the real exit code via `ghostty_gtk_surface_exit_code`, falling back to a neutral "Closed" on older libraries), and abnormal-exit notifications via GObject signals. A `close-request` emitted by the Ghostty widget opens ForkTTY's Close Pane confirmation; explicit socket/API close remains noninteractive. Embedded panes reach copy/paste/select-all/find parity through `ghostty_gtk_surface_perform_action`, which performs a Ghostty keybinding action by name on the focused surface; mouse selection works natively inside the surface, and libraries lacking the symbol degrade to a logged no-op. Embedded panes also expose their child PID via `ghostty_gtk_surface_child_pid`, so listening-port discovery and the socket `surfaces` PID field reach parity. The deb and AppImage packagers invoke `scripts/ghostty-gtk-lib-probe.sh --ensure --print-path` before packaging; every invocation enters Zig's incremental build graph, verifies every mandatory ABI symbol in the resulting `ghostty-gtk-embed.so`, and installs it into `usr/lib`. Installed builds load it via the binary RUNPATH (`$ORIGIN/../lib`) without `FORKTTY_GHOSTTY_GTK_LIB`. When `appearance.persistent_scrollback_lines` is greater than zero, embedded panes snapshot a bounded full-scrollback tail into the session and restore it through `ghostty_gtk_surface_restore_scrollback`, which feeds Ghostty's VT stream rather than the child PTY. Libraries lacking `ghostty_gtk_surface_read_text_limited` retain the safe visible-text fallback. `forktty doctor` warns about a missing embedding library because terminal panes cannot open without it. Rows 9/10/12/13/14 in `docs/ghostty-embedded-parity.md` remain deferred manual validation items. |

## Updates

When `updates.auto_check = true`, GTK startup checks GitHub Releases at most
once every 24 hours by fetching:

```text
https://api.github.com/repos/Lucenx9/forktty/releases?per_page=10
```

The request uses HTTPS, a `ForkTTY/<version>` user agent, the GitHub JSON
media type, and `X-GitHub-Api-Version`. The local stamp is updated for both
success and failure; 403/429 responses honor `Retry-After` or
`X-RateLimit-Reset` before the next attempt.

Stable builds ignore prerelease releases. Prerelease builds consider newer
prereleases and stable releases. Release assets are selected only from
GitHub-provided asset names and `browser_download_url`; ForkTTY does not
construct release download URLs.

If the app is running from a writable, regular AppImage path exposed by
`APPIMAGE` and not from `APPIMAGE_EXTRACT_AND_RUN=1`, ForkTTY can update in
place after explicit user confirmation. It downloads the AppImage and
`SHA256SUMS` into the target directory, verifies SHA256, chmods the temp file,
fsyncs file and directory, then renames over the current AppImage and offers a
restart. The old file remains intact until the final rename. Non-AppImage,
read-only, extracted, or otherwise unsafe launches open the GitHub release page
instead. External AppImage managers such as Gear Lever can continue to launch
the same path; they may rescan their own metadata separately.

Packaged AppImages set `LD_LIBRARY_PATH` to ForkTTY's private `usr/lib`
runtime first. In `auto` mode AppRun performs an eager loader compatibility
probe with the effective loader environment, preloading
`ghostty-gtk-embed.so` with immediate binding while starting the GTK-linked
ForkTTY binary. Host GTK/libadwaita is selected only when both the binary and
embedding library load; otherwise AppRun adds the bundled GTK/libadwaita
fallback. Embedded terminal commands first enter the already-loaded
`appimage-child-exec` helper, which applies the intended environment delta and
removes AppImage loader/runtime entries immediately before executing the real
command. Ghostty's packaged shell integration and `TERM=xterm-ghostty` remain
present in that intended child environment. The runtime choice can be forced
for troubleshooting with `FORKTTY_APPIMAGE_GTK_RUNTIME=bundled`, `host`, or
`auto`.

## Telemetry

When `telemetry.anonymous_ping = true`, GTK startup sends at most one anonymous
usage ping per UTC day to:

```text
https://forktty.dev/api/telemetry/ping
```

The request is a best-effort HTTPS POST with this JSON shape:

```json
{
  "schema": 1,
  "kind": "daily_ping",
  "app": "forktty",
  "version": "0.2.0-alpha.12",
  "date": "2026-06-13"
}
```

The payload contains no install id, rotating id, username, hostname, cwd,
repository path, branch, shell, agent metadata, terminal buffer, socket payload,
or crash data. CLI invocations and agent hooks do not send telemetry pings.
The local stamp is stored under `$XDG_STATE_HOME/forktty/telemetry-ping.json`
or the platform data fallback. Set `telemetry.anonymous_ping = false` to
disable the ping.

On the first launch a one-time welcome dialog presents the (default-on)
telemetry toggle before any ping is sent; the startup ping is deferred until
the dialog is dismissed, then sent only if the toggle is still enabled.
Dismissing the dialog records `$XDG_STATE_HOME/forktty/welcome-seen.json` (or
the platform data fallback) so it is not shown again, and the update check is
skipped on that first launch.

## Socket API

ForkTTY exposes a newline-delimited JSON-RPC-like protocol over a user-local
Unix socket. The default path is `$XDG_RUNTIME_DIR/forktty.sock`, with an
owner-only fallback at `/tmp/forktty-<uid>/forktty.sock`; `FORKTTY_SOCKET_PATH`
can select another absolute path.

```json
{"id":"1","method":"workspace.list","params":{}}
```

```json
{"id":"1","ok":true,"result":[{"id":"...","name":"..."}]}
```

Request lines are capped at 1 MiB. Official clients accept response lines up to
64 MiB, including the terminating newline. Before writing a normal JSON-RPC
response, the server measures its compact encoded form; an oversized response
is replaced with a compact `response_too_large` error carrying the same request
id. The server validates that the socket and its parent are owned by the current
user (rejecting a symlinked parent directory, validated on an opened
`O_NOFOLLOW` descriptor), refuses unsafe existing paths, verifies each accepted
connection's `SO_PEERCRED` uid against the server's effective uid (root is also
accepted; anything else is dropped before dispatch), and applies bounded reads
to terminal text and event replay. Official clients likewise verify that the
connected server's `SO_PEERCRED` uid matches the client's effective uid before
writing any request data. One request yields one response, except `events.subscribe`,
which upgrades the connection to a replay-plus-live event stream.

Official socket clients and existing-socket inspection use the same
deadline-bounded nonblocking AF_UNIX connector. Linux `EAGAIN` from a full
accept backlog is retried on a newly created descriptor until the deadline;
the resulting timeout classifies an existing socket as occupied/foreign rather
than stale, so its inode is never removed. Successful streams are restored to
blocking mode before request or probe I/O.

GTK shutdown is cooperative and ordered. The first close request keeps the UI
alive, stops new socket dispatch, and waits without a fixed deadline for requests
that already entered dispatch and for the socket runtime to drop. Finalization
then snapshots bounded embedded-terminal scrollback, synchronizes live surface
working directories, waits for process-local worktree and surface-set
transactions to finish, saves the session, performs configured PTY-persistence
cleanup, marks the UI dead, and issues the final window close. A live-directory
sync failure is logged and does not silently suppress that final snapshot.

Method stability tiers are tracked in [docs/socket-api.md](docs/socket-api.md).
The core contract is deliberately process-neutral; agent methods are a thin
lifecycle adapter over sessions discovered through optional hooks.

| Category | Methods |
| --- | --- |
| System | `system.ping`, `system.capabilities`, `system.identify`, `system.top` |
| Context | `context.snapshot` |
| Agent lifecycle | `agent.list`, `agent.health`, `agent.reclaim.plan`, `agent.hibernate`, `agent.reclaim`, `agent.resume` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.create_ssh`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.read_text`, `surface.capture_tail`, `surface.split`, `surface.send_text`, `surface.focus`, `surface.close` |
| Remote | `remote.list`, `remote.status` |
| Pane tabs | `pane.new_tab`, `pane.select_tab` |
| Notifications | `notification.create`, `notification.list`, `notification.clear` |
| Worktrees | `worktree.list`, `worktree.status`, `worktree.create`, `worktree.attach`, `worktree.remove`, `worktree.merge` |
| Project actions | `project.action.list`, `project.action.run` |
| Metadata | `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.list_progress`, `metadata.clear_progress`, `metadata.log`, `metadata.list_logs`, `metadata.clear_logs` |
| Status | `status.summary` |
| Topology | `topology.tree` |
| Events | `events.subscribe` |
| Browser (source-only feature) | `browser.open`, `browser.navigate`, `browser.snapshot`, `browser.click`, `browser.fill`, `browser.back`, `browser.forward`, `browser.reload`, `browser.profile.list`, `browser.profile.create`, `browser.profile.delete`, `browser.history.list`, `browser.history.search`, `browser.history.clear`, `browser.bookmark.add`, `browser.bookmark.list`, `browser.bookmark.remove` |

Surface topology mutations are process-locally serialized. In particular,
`surface.split` and `pane.new_tab` retain the surface-set guard after a GTK
backend accepts the request, through either embedded terminal materialization
or complete rollback of the modeled pane tree, focus, and saved session. Socket
acceptance alone is therefore not the transaction commit point.

The human-readable `forktty logs` formatter treats socket-provided log levels
and messages as untrusted terminal text. Newline, carriage return, tab, ESC,
BEL, and other control characters are rendered as visible escapes before the
CLI writes them, so metadata cannot inject terminal control sequences. JSON
mode preserves the protocol response unchanged.

The built-in contract does not include task routing, provider-neutral team or
workflow stores, approval feeds, orchestration cleanup, an MCP stdio bridge, or
managed agent skills. External MCP servers and agent processes remain ordinary
terminal workloads and may call the socket CLI when they need a ForkTTY
primitive.

The removal does not mutate external agent configuration automatically. Older
ForkTTY versions marked managed MCP registrations with
`FORKTTY_MCP_MANAGED = "forktty"` and managed skill files with
`<!-- forktty-managed-agent-skill -->`; migration must remove only entries with
those markers. The former skill remover retained up to three sibling
`forktty-agent-orchestration.bak-*` directories, so marker-safe migration also
checks those backups without following symlinks. The exact backup, dry-run, and
manual cleanup paths are in the
[README upgrade guide](README.md#upgrading-from-orchestration-builds).

`workspace.create` and `workspace.create_ssh` default an omitted name to the
allocated workspace id. Workspace selectors accept an id, name, or linked
worktree name where documented; surface selectors override workspace focus.
`system.identify` resolves explicit selectors first and otherwise reports active
focus. `system.capabilities` returns only methods dispatchable by the current
build plus PTY-persistence diagnostics; browser methods appear only in browser
builds.

`surface.read_text` supports visible or full retained text with byte bounds.
`surface.capture_tail` returns a bounded recent tail. `surface.send_text`
rejects oversized input and targets an explicit or focused surface. These reads
are observations of untrusted terminal content, not instructions.

`context.snapshot` combines the selected workspace, pane topology, surface
inventory, metadata, notifications, agent lifecycle rows, remotes, optional
bounded terminal tails, and risk flags. It intentionally contains no hidden
team, workflow, routing, or feed state. `status.summary` is a compact projection
of the same generic status and attention primitives. Notifications are scoped
to the selected workspace (plus untargeted global notifications); an unread
prompt notification adds `notification_needs_input` to `risk_flags`.
Risk evaluation covers the complete matching notification set, while the
response projects only the newest 100 notifications and omits binary
`terminal_metadata.icon_data` so untrusted OSC icon payloads cannot inflate the
snapshot beyond the official client response limit.

Agent hook events may bind a provider session to a surface, update lifecycle and
attention, and retain provider-native resume metadata. `agent.hibernate` and
`agent.reclaim` act only on idle, restorable sessions; `agent.resume` uses the
provider's recorded native resume command. Hooks are optional, installed only by
an explicit `forktty hooks setup` action, and are never installed or updated
automatically at GTK startup. Hook setup writes atomically, preserves unrelated
entries, and supports dry-run and targeted removal.

Hook session target resolution completes before a serialized mutation or its
event-order watermark commits. An explicit live surface wins. If the surface id
is omitted or stale, ForkTTY may reuse that session's live learned target or a
unique canonical-cwd match; an explicit workspace id constrains both fallbacks
and is never overwritten by a surface in another workspace. A primary
`agent:<provider>` hook status without a resolved live surface fails closed, so
it cannot leave sidebar-invisible workspace metadata and a corrected retry at
the same event order remains applicable. Workspace-scoped hook logs,
notifications, and non-primary metadata retain their existing surface-optional
behavior.

The managed event sets contain 10 Codex events, 28 Claude events (25 in the
default lifecycle profile and 28 with `--full`), 3 Antigravity events, and 11
OpenCode plugin events. The Claude lifecycle profile excludes only
`PreToolUse`, `PostToolUse`, and `PostToolUseFailure`; `PostToolBatch` remains
installed for prompt-result correlation. Claude `SessionStart` workspace
enrichment is atomic: it requires nonblank workspace and surface IDs plus an
absolute socket path from the same ForkTTY child environment. If any provenance
component is absent or invalid, the hook returns the exact continue response
without reading stdin or issuing a socket request.

Codex may execute hooks from a shared app-server process that does not inherit
the originating terminal pane's `FORKTTY_*` environment. A managed Codex hook
whose local `session_meta` identifies a `codex-tui` originator, provider
session id, and absolute hook cwd may therefore contact the default owner-only
ForkTTY socket. When no explicit surface is present, the socket inventories
same-user Codex TUI processes in the canonical cwd and binds the session only
if exactly one unclaimed process belongs to exactly one eligible ForkTTY
surface. Processes already claimed by other live ForkTTY sessions are excluded;
ended sessions release their claim. Any other unclaimed Codex TUI in the cwd,
including one outside ForkTTY, makes the request fail closed instead of
selecting active focus. Learned exact-session targets remain authoritative for
later events.

Hook notifications that do not request attention are logged without publishing
an `agent:<key>` status update. An informational notification received after
`Stop` therefore preserves the persisted idle lifecycle instead of changing the
workspace badge back to running.

Persisted `Suspended` is a lifecycle tombstone. Hook events arriving after
hibernate are accepted as inert: they do not mutate lifecycle, attention,
metadata, prompt state, or the per-session event-order watermark. Only an
explicit resume can replace the suspended state. Prompt requests are correlated
privately by provider, session, kind, prompt ID, target, and event order.
Accepted results keep only the matching in-app notification as read history,
close its desktop notification, and leave stale or unrelated prompts untouched;
session end, target remap, and surface/workspace removal retire only affected
correlations.

`forktty hooks doctor <agent>` remains a local, read-only version-1 report. Its
additive `installationCheck` regenerates the provider's expected managed assets
through the canonical setup planner and verifies complete config/plugin content,
one usable recorded launcher, and every Antigravity wrapper's exact content,
regular-file type, and executable bit. The top-level `ok` requires
`installationCheck.ok`; wrapper-only, missing-group, malformed, partial,
modified, or non-executable installations are unhealthy.

`remote.list`, `remote.status`, and the `context.snapshot` remote rows report
`connected: true` only when the terminal backend says the SSH surface is ready.
A runtime inventory entry that is still starting or no longer ready reports
`connected: false`; its last available `pid`, dimensions, and shell metadata may
remain present for diagnosis. This is local terminal-I/O readiness, not an
independent SSH heartbeat, network probe, or authentication check.

`forktty remote-helper hello` is a no-socket stdio handshake for SSH discovery.
`forktty remote-helper pty -- <program> [args...]` starts the argv command in a
PTY and relays raw stdin/stdout; it does not open a listener, reconnect, resize,
or persist remote ownership.

## Browser Pane Feature

The `browser` cargo feature builds WebKitGTK6 panes alongside Ghostty panes.
Browser surfaces are part of `WorkspaceModel` and carry a URL plus a
`ProfileId`. The socket can open/navigate browser surfaces and, when a
GTK browser command channel is available, run snapshot/click/fill and
back/forward/reload operations against the live WebView. JavaScript-backed
automation runs in ForkTTY's isolated WebKit script world rather than the
page's default script world, so visited pages cannot observe or replace the
automation driver global.

Browser profile metadata is stored in:

```text
~/.local/share/forktty/browser_profiles/profiles.json
```

Each profile has a directory under `browser_profiles/<id>/` containing WebKit
data/cache/cookies. `forktty-core` also contains pure per-profile
`HistoryStore` and `BookmarkStore` implementations. The socket and CLI expose
history/bookmark list/search/clear/add/remove methods over those stores. Visit
recording and GTK address-bar completion are not wired yet.
Profile metadata and bookmark JSON stores reject symlinked inputs before
parsing, as does the direct Chromium `Bookmarks` import path; their existing
per-file byte limits are enforced again on the opened descriptor.

## Worktree Behavior

Worktree operations use `git2` and avoid shelling out to git.

Implemented operations:

- list worktrees;
- create worktree and branch; if the requested branch already has a linked worktree in one of ForkTTY's supported layouts, `worktree.create` returns that existing worktree so a retry can recover after a crash between Git registration and ForkTTY session persistence; if that worktree's directory was deleted out from under git, the stale registration is pruned and the worktree recreated in place (adopting the existing branch);
- attach existing branch/worktree (the same stale-registration recovery applies);
- remove worktree after dirty-state and metadata validation;
- merge worktree branch with dirty-target/conflict checks and abort incomplete merges before returning failure;
- run `.forktty/setup` after open/create as advisory setup; failures are reported but do not hide an already-created worktree;
- run `.forktty/teardown` before removal; failures block removal, and dirty state is rechecked after the hook before deleting files.

Worktree and hook paths are canonicalized. Hook execution is limited to `.forktty/setup` and `.forktty/teardown` inside verified worktrees.

The modeled worktree identity is the exact `(worktree_name,
canonical_worktree_path)` pair. After Create or Attach returns a verified
canonical path, ForkTTY selects and refreshes an existing exact match instead
of allocating another workspace or surface. A repeated Create, or
Create followed by Attach for the same identity, therefore returns the same
workspace ID while preserving its user-facing display name. Equal worktree
names at different canonical paths remain distinct. Session repair collapses
only exact, resolvable duplicate identities (preferring the active record,
otherwise the earliest ordered record); unresolved paths are not deduplicated.

One running ForkTTY process coordinates GTK and socket worktree operations
through a shared reader/writer transaction boundary: discovery, List, and
Status may overlap, while Create, Attach, Remove, and Merge serialize through
commit or complete rollback. This guarantee is process-local; it is not a
cross-process or distributed lock against another application process or an
independent Git command.

Removal uses its prepared verified target path to match the exact modeled
workspace. Before terminal close, every target surface ID is registered as
auto-spawn suppressed, so GTK reconciliation cannot recreate a terminal while
the model still describes the workspace. Suppression remains active through
either a filesystem removal that has started plus model commit or the complete
rollback restoration attempt. A partial close or pre-destructive filesystem
failure attempts to restore the prior active selection and any surfaces already
closed. Once verified target deletion starts, even a later removal error is
treated as irreversible: model removal commits and ForkTTY does not respawn
terminals into a potentially partial checkout. Removing the final workspace
stages a replacement before destructive close and attempts to remove that
replacement again on rollback. If a rollback spawn fails, ForkTTY records a
terminal error status before releasing suppression; that status continues to
block automatic respawn.

## Notifications

Notification sources:

- explicit socket/hook `notification.create`;
- ForkTTY hook `precmd`, `preexec`, and `postexec` termprops;
- ForkTTY progress termprops;
- Ghostty OSC 9 and basic OSC 99 terminal notifications, rate-limited per surface;
- OSC 99 same-id update/close controls, desktop notification replacement/closing, in-app activation/close reports, and basic same-id button reports;
- OSC 99 icon names for in-app/desktop notification icons, application-name fallback when no icon name is provided, plus bounded same-process `p=icon` data caching by `g` and binary image rendering for decodable in-app data / desktop notification servers that support image paths;
- OSC 99 application name (`f`) and notification type (`t`) metadata retained on notifications, filtered by `notifications.blocked_terminal_apps` / `blocked_terminal_types`, and exposed to `notification_command` as `FORKTTY_NOTIFICATION_TERMINAL_APP` and `FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON`;
- OSC 99 occasion filtering (`o=always`, `o=unfocused`, `o=invisible`) using ForkTTY's active workspace, focused surface, and active pane-tab model;
- OSC 99 urgency (`u`) and base64 sound (`s`) metadata as desktop notification hints where the notification server supports them;
- OSC 99 auto-expiry (`w`) as a desktop notification timeout hint where the notification server supports it and as an in-app auto-dismiss timer for positive values;
- OSC 99 `p=?` support replies and `p=alive` live-id replies for tracked same-surface notifications;
- Ghostty bell;
- Ghostty child exit;
- bounded full-scrollback-tail prompt fallback for common agent prompts when
  the limited text ABI is available, with visible-text fallback on older
  embedding libraries.

Notifications update in-app unread state and may dispatch through `notify-rust` and `notification_command`. Workspace/surface-targeted desktop notifications register a best-effort default `Open` action, using the freedesktop `default` action key, that argv-executes the current ForkTTY binary to focus the target surface or workspace; global notifications remain passive. Custom commands are argv-executed, not `sh -c`; title/body are passed through environment variables, and terminal-originated OSC 99 `f`/`t` metadata is passed as `FORKTTY_NOTIFICATION_TERMINAL_APP` plus JSON array `FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON`. `blocked_terminal_apps` and `blocked_terminal_types` are exact string match filters for terminal-originated OSC 99 `f`/`t` metadata before the notification is stored or dispatched. OSC 99 notification identifiers are tracked and echoed only when they use the protocol identifier character set (`A-Z`, `a-z`, `0-9`, `_`, `-`, `+`, `.`); unsafe identifiers are treated as untracked notification payloads or ignored for reply-only actions. Unknown OSC 99 payload types are ignored so future protocol extensions do not surface as terminal status noise. OSC 99 binary icons are rendered in-app when GTK can decode them, after `n=` icon names and `f=` application-name icon fallback; desktop binary icons are materialized as bounded files under `$XDG_RUNTIME_DIR/forktty-notification-icons` and removed when the tracked desktop notification is replaced, closed, or evicted.

`notification.list` returns one retained-history page in oldest-to-newest order.
`limit` is optional, defaults to 200, and must be an integer from 1 through 200.
Without `before_id`, the page contains the newest retained items; `before_id`
is an exclusive cursor and returns the older page immediately before that
notification. Unknown cursors are rejected. Updating an existing notification
moves it to the newest position without changing its id, so paging follows
recency rather than original insertion position. `notification.create`,
`notification.list`, and `context.snapshot` all use the same socket projection:
terminal metadata remains available, but binary `terminal_metadata.icon_data`
is omitted. Hook prompt correlation is internal runtime state kept outside the
public `NotificationItem` projection. Provider result hooks resolve only an
unread prompt with the same provider, session, kind, target, and correlation id;
providers without a result correlation id resolve only the newest compatible
prompt older than the accepted result event. Ignored stale events do not change
notification or desktop state. Accepted session cleanup, target remap, and target
removal mark affected retained prompt history read, clear its correlation, close
the matching desktop notification id, and recompute attention from remaining
notifications and terminal output. The CLI exposes one-page access as `forktty notifications
[--limit <n>] [--before-id <id>]` and never aggregates pages automatically.

## Security Constraints

- Local Linux desktop threat model; same-user processes are not treated as hostile isolation boundaries.
- No crash-reporting or product event-tracking network calls. The default
  anonymous daily usage ping can be disabled with
  `telemetry.anonymous_ping = false`; optional update checks query GitHub
  Releases at most once per day and can be disabled. Optional browser panes
  and optional PR lookup can make user-directed network requests.
- Owner-only Unix socket permissions and private runtime directory validation;
  the socket parent must not be a symlink, and accepted connections must carry
  the server's effective uid (or root) in their `SO_PEERCRED` credentials.
  Official clients send requests only when the server peer carries the client's
  effective uid.
- 1 MiB bounds for socket requests, config, and session files.
- Hook session-to-surface routing and prompt-correlation state is local process
  memory only. The routing cache is capped at 256 entries; prompt correlations
  live in a private map keyed by retained notification id. Both are evicted as
  applicable on accepted session-end, target remap, or surface/workspace removal. Per-surface agent
  session ids and hook cwd values learned from hooks are persisted as resume
  metadata for explicit and restore-time provider resume. Codex cwd fallback
  reads only local `session_meta` JSONL records under `$CODEX_HOME/sessions` or
  `~/.codex/sessions`, requires originator `codex-tui`, and requires the
  referenced cwd to still be a directory. Unscoped Codex hooks also require a
  unique unclaimed live `codex` process in the canonical cwd and in the
  candidate surface's Linux `/proc` descendant tree; ambiguity is rejected.
  Session metadata and `/proc` correlation are same-user local provenance, not
  authentication or a security boundary; a same-user process that deliberately
  forges both can impersonate this best-effort lifecycle signal.
- Shell and notification executables must be absolute executable files.
- Hooks are limited to verified worktree-local paths.
- Worktree removal rejects dirty/tampered targets.
- Browser profile IDs are validated before becoming directory names.
- Browser automation still runs ForkTTY-authored JavaScript in the addressed
  WebKit page.

Residual risks:

- User-authored hooks and notification commands run with user privileges.
- A same-user process can interact with user-owned runtime resources.
- Persisted agent session ids and resume cwd values do not preserve PTY
  processes. Restore-time auto-resume and
  `agent.resume` can only ask the installed provider CLI to resume;
  provider-side expiry, deletion, incompatible ids, or missing project
  directories still fail inside that CLI.
- `agent.hibernate` and `agent.reclaim` close local terminal processes only
  after local resume readiness says the provider command can be rebuilt; they
  still cannot prove the provider-side session has not expired or been deleted.
- Advanced OSC 99 compatibility remains partial; title/body base64 payloads and same-id title/body chunks are decoded with multipart title/body kept separate, same-id update/close controls update the ForkTTY notification model and desktop notification id, in-app activation/close/button reports plus `p=?`/`p=alive` replies are sent back to the source terminal, and icon names, application/type filtering metadata, occasion filtering, urgency/expiry/sound handling, plus bounded `p=icon` caches with decodable in-app/desktop binary image rendering are tracked.

## Test Strategy

Current automated coverage:

- Rust unit tests for config validation, session validation/quarantine, workspace/pane model, socket protocol, terminal backend, notification metadata, and worktree hardening.
- Rust tests for browser command types, profile metadata, and history/bookmark stores.
- Rust tests in `crates/forktty-ui-gtk/src/socket_cli.rs` for CLI parameter building, hook config merging, notification formatting, and socket-target fallbacks.
- CI for Rust fmt/test/clippy/build, repository consistency (`cargo run -p xtask -- check`), desktop entry validation, `.deb` packaging, dependency review, and cargo audit.
- The Ghostty GTK Probe workflow builds `ghostty-gtk-embed.so`, runs `forktty ghostty-gtk-probe`, and runs `scripts/gtk-ghostty-smoke.sh` against live embedded panes.

Backlog validation:

- manual package QA across supported Linux environments;
- expanded pointer/clipboard/search checks for embedded Ghostty panes that still need trusted real-input validation.
