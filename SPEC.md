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
5. Socket methods and GTK actions send text, read visible/full/tail text,
   perform copy/paste/select-all/find, restart, split, focus, close, or resize
   surfaces through ForkTTY's model plus the Ghostty GTK embedding ABI.
6. Ghostty GObject signals and ABI calls mirror title, child-exit status, close
   requests, child PID, and scrollback snapshots into ForkTTY state.
7. Closing a pane/workspace closes the corresponding Ghostty surface.

Prompt/status detection uses ForkTTY hook integration termprops, Ghostty
bell/child-exit state, and a bounded visible-tail prompt fallback.

## Session Persistence

Native session file:

```text
~/.local/share/forktty/session-v2.json
```

The native session includes workspace order, active workspace, pane tree, focused surface, cwd, branch, worktree metadata, and opt-in bounded plain-text scrollback tails when `appearance.persistent_scrollback_lines` is greater than zero. It excludes running PTY process handles.

Browser panes persist their surface URL and profile ID in the same session
model. WebKit processes, in-memory page state, and terminal PTY state are
not restored.

ForkTTY can import the legacy `session.json` format but saves native sessions as v2. Session load validates file type, size, version, pane-tree depth/shape, and focused leaf index. Invalid files are quarantined instead of crashing startup.

## Config

Config file:

```text
~/.config/forktty/config.toml
```

```toml
[general]
theme_source = "dark"
shell = "/bin/bash"
worktree_layout = "nested"
enable_pr_lookup = false
notification_command = ""

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

Config files are regular-file checked and capped at 1 MiB. Malformed or invalid config content is quarantined; transient I/O errors are reported without renaming the file. Config load validates and normalizes `general.shell` from manual TOML edits, but the Settings dialog does not expose a shell editor. Saved settings validate theme source, worktree layout, persistent scrollback bounds (max 1,000 lines), sidebar position, window mode, PR lookup toggle, update auto-check toggle, telemetry anonymous-ping toggle, notification filters, and notification command. Legacy `font_family`, `font_size`, `scrollback_lines`, `terminal_audible_bell`, `terminal_renderer`, `terminal_theme`, and the temporary alpha `embedded_ghostty` switch are accepted on load for compatibility, omitted from new saves, and ignored by the GTK runtime; terminal panes always require the embedded Ghostty GTK renderer. Terminal font, color, bell, `scrollback-limit`, and `scrollbar` preferences come from Ghostty's config when present; no system Ghostty install is required. Embedded panes fall back to Ghostty's bounded default scrollback budget (10,000,000 bytes per surface) when Ghostty config does not set `scrollback-limit`; legacy ForkTTY `scrollback_lines` does not change retained embedded history. The GTK runtime loads `~/.config/ghostty/config.ghostty` and the legacy `~/.config/ghostty/config`, follows `config-file`, resolves `theme` for dark mode, searches Ghostty theme directories, and applies font size/family/style-family/style/synthetic-style entries, font features/variations, foreground/background, cursor, selection, named colors, `cell-foreground`/`cell-background` cursor and selection color references, `cursor-opacity`, DECSCUSR-backed `cursor-style`/`cursor-style-blink` defaults, `selection-clear-on-typing`, `selection-clear-on-copy`, `selection-word-chars`, `clipboard-trim-trailing-spaces`, `clipboard-codepoint-map`, `copy-on-select`, `right-click-action`, `scroll-to-bottom`, `scrollbar`, SGR faint text plus `faint-opacity`, `mouse-reporting`, `mouse-shift-capture`, `mouse-hide-while-typing`, `bell-features`, `bell-audio-path`, `bell-audio-volume`, `mouse-scroll-multiplier`, `adjust-cell-width`, `adjust-cell-height`, `adjust-font-baseline`, underline/strikethrough/overline/cursor metric adjustments, `bold-color`/`bold-is-bright`, short/full hex colors, ANSI palette entries 0-15, `image-storage-limit`, `unfocused-split-opacity`, and `unfocused-split-fill`. `terminal_renderer` is retained on load for compatibility; legacy `"vte"` input normalizes to `"auto"` and the native GTK runtime uses Ghostty.

Ghostty compatibility scope:

| Ghostty config area | ForkTTY status |
| --- | --- |
| Config discovery | Supported for `~/.config/ghostty/config`, `~/.config/ghostty/config.ghostty`, recursive `config-file`, and Ghostty theme directories, with ForkTTY's regular-file and size guards. |
| Terminal appearance | Supported for terminal font family/style/synthetic-style fallbacks, font size, font features/variations, foreground/background, cursor colors, DECSCUSR-backed cursor style defaults, selection colors/clear-on-typing/clear-on-copy/word-chars/copy-on-select, clipboard trim-trailing-spaces/codepoint-map, right-click action, scroll-to-bottom, scrollbar policy, mouse reporting/shift-capture including XTSHIFTESCAPE overrides, mouse-hide-while-typing, bold, faint, ANSI palette, named colors, short/full hex colors, `theme`, cell/text-decoration/cursor metric adjustment, image storage limit, mouse scroll multiplier, and inactive split dimming. Embedded panes follow Ghostty's bounded `scrollback-limit` budget and pack the surface in a GTK scrolled window so retained history is reachable from the UI. |
| Runtime terminal state | Delegated to `libghostty-vt` for VT parsing, key/paste encoding, OSC 8 links, OSC 9/99 notifications, bracketed paste/focus mode mirrors, XTSHIFTESCAPE mouse-shift capture overrides, selection formatting, word selection, and Kitty image protocol storage/loading media plus PNG decode/placement geometry. ForkTTY snapshots Kitty placements across the GTK boundary as RGBA buffers bounded by the rendered pixel footprint, not the stored source image size. Shell startup integration uses Ghostty's upstream shell scripts when resources are available. |
| ForkTTY-owned UI | Intentionally not read from Ghostty config. Window layout, tabs, splits, sidebar, socket automation, worktrees, agent controls, notifications UI, and session restore use ForkTTY config/session state. |
| Ghostty GUI/window/platform options | Ignored unless ForkTTY has the same runtime concept. Examples include Ghostty keybinds, quick terminal, window decorations, titlebar/font, shell integration UI, macOS-only options, shaders, background blur/opacity, and Linux cgroup settings. |
| Renderer parity | Terminal panes use the embedded Ghostty GTK widget and require `ghostty-gtk-embed.so`; if the library cannot load or an embedded surface cannot spawn, ForkTTY records a terminal spawn failure instead of falling back to the classic GTK/Pango/Cairo renderer. The full upstream Ghostty source is pinned at `vendor/ghostty` for the cmux-style renderer/widget integration. Upstream's current public C embedding API is macOS/iOS-only for surfaces, while the Linux `GhosttySurface` GTK widget is internal to Ghostty's GTK app runtime. ForkTTY's Ghostty fork now carries a GTK widget embedding ABI with cwd, direct command spawn, socket text-input, and visible/full text-read hooks. Embedded panes pass the requested argv plus per-surface `FORKTTY_*` environment through `ghostty_gtk_surface_new_with_working_directory_and_command` when available, avoiding typed bootstrap text in the child shell; older libraries start Ghostty's default shell in the requested cwd without ForkTTY environment injection. ForkTTY-managed embedded panes force Ghostty's `wait-after-command` behavior so clean child exits remain inspectable as `Closed` panes with restart/scrollback parity instead of closing the split immediately. Embedded panes also mirror Ghostty surface title, child-exit readiness/status (with the real exit code via `ghostty_gtk_surface_exit_code`, falling back to a neutral "Closed" on older libraries), abnormal-exit notifications, and close-request teardown into the model via GObject signals. Embedded panes reach copy/paste/select-all/find parity through `ghostty_gtk_surface_perform_action`, which performs a Ghostty keybinding action by name on the focused surface; mouse selection works natively inside the surface, and libraries lacking the symbol degrade to a logged no-op. Embedded panes also expose their child PID via `ghostty_gtk_surface_child_pid`, so listening-port discovery and the socket `surfaces` PID field reach parity. The deb and AppImage packagers invoke `scripts/ghostty-gtk-lib-probe.sh --ensure --print-path` before packaging, require the probe to build or locate a `ghostty-gtk-embed.so` with the required ABI symbols, and install it into `usr/lib`; installed builds load it via the binary RUNPATH (`$ORIGIN/../lib`) without `FORKTTY_GHOSTTY_GTK_LIB`. When `appearance.persistent_scrollback_lines` is greater than zero, embedded panes snapshot their scrollback tail into the session and restore it through `ghostty_gtk_surface_restore_scrollback`, which feeds Ghostty's VT stream rather than the child PTY. `forktty doctor` warns about a missing embedding library because terminal panes cannot open without it. Rows 9/10/12/13/14 in `docs/ghostty-embedded-parity.md` remain deferred manual validation items. |

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

Socket path:

```text
$XDG_RUNTIME_DIR/forktty.sock
```

Fallback:

```text
/tmp/forktty-<uid>/forktty.sock
```

Override:

```text
FORKTTY_SOCKET_PATH=/path/to/socket
```

The protocol is newline-delimited JSON-RPC-like messages:

```json
{"id":"1","method":"workspace.list","params":{}}
```

```json
{"id":"1","ok":true,"result":[{"id":"...","name":"..."}]}
```

Implemented categories:

| Category | Methods |
| -------- | ------- |
| System | `system.ping`, `system.capabilities`, `system.identify`, `system.top` |
| Context | `context.snapshot` |
| Agent | `agent.list`, `agent.health`, `agent.reclaim.plan`, `agent.hibernate`, `agent.reclaim`, `agent.resume` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.create_ssh`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.read_text`, `surface.capture_tail`, `surface.split`, `surface.send_text`, `surface.focus`, `surface.close` |
| Remote | `remote.list`, `remote.status` |
| Pane | `pane.new_tab`, `pane.select_tab` |
| Notification | `notification.create`, `notification.list`, `notification.clear` |
| Worktree | `worktree.list`, `worktree.status`, `worktree.create`, `worktree.attach`, `worktree.remove`, `worktree.merge` |
| Project Actions | `project.action.list`, `project.action.run` |
| Metadata | `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.list_progress`, `metadata.clear_progress`, `metadata.log`, `metadata.list_logs`, `metadata.clear_logs` |
| Status | `status.summary` |
| Feed | `feed.list`, `feed.approval.respond` |
| Workflow | `workflow.list`, `workflow.get`, `workflow.upsert`, `workflow.plan.set`, `workflow.loop.set`, `workflow.evidence.add`, `workflow.replay` |
| Team | `team.list`, `team.get`, `team.upsert`, `team.finish`, `team.worker.upsert`, `team.worker.heartbeat`, `team.worker.launch`, `team.worker.health`, `team.worker.nudge`, `team.worker.shutdown`, `team.task.upsert`, `team.message.send`, `team.message.dispatch`, `team.message.ack`, `team.inbox`, `team.summary`, `team.events` |
| Topology | `topology.tree` |
| Events | `events.subscribe` |
| Browser | `browser.open`, `browser.navigate`, `browser.snapshot`, `browser.click`, `browser.fill`, `browser.back`, `browser.forward`, `browser.reload`, `browser.profile.list`, `browser.profile.create`, `browser.profile.delete`, `browser.history.list`, `browser.history.search`, `browser.history.clear`, `browser.bookmark.add`, `browser.bookmark.list`, `browser.bookmark.remove` |

`workspace.create` and `workspace.create_ssh` default omitted names to the
allocated `workspace-N` id name, keeping the visible workspace label aligned
with the real workspace id.

`team.finish` verifies and finalizes one team record. It accepts required
`team_id` plus optional `dry_run`, `close_workers`, and `force`; dry-run returns
the planned actions, blockers, and cleanup errors without mutation or
precondition rejection. Non-dry-run finalization without force rejects open
tasks, pending messages, or live-looking worker final states. With
`close_workers`, it only closes disposable worker surfaces created by
`team.worker.launch` in the current ForkTTY runtime; missing worker surfaces
are normalized to closed before the team is marked done.
`team.summary` and `context.snapshot` team summaries flag active teams with no
active workers, open tasks, or pending messages as `active_without_open_work`.

`workflow.loop.set` records bounded closed-loop progress on an existing
workflow: optional recipe, stage, iteration, maximum iterations, stop reason,
and up to 64 verification gates. The call is state-only; it does not start a
scheduler, run commands, send terminal input, push, merge, or grant approval.
When a request advances to a different iteration without supplying replacement
gates or a replacement stop reason, ForkTTY clears the previous gate rows and
stop reason so stale failed checks do not describe the new pass.
Agents use it to make a visible loop such as discover/plan/execute/verify
auditable across context compaction. `context.snapshot` exposes compact
`loop_summaries` by default, including recipe, stage, gate counts, iteration
budget, stale surface-binding detection, and loop risk flags such as
`loop_gate_failed`, `loop_needs_human`, `loop_blocked`,
`loop_budget_exhausted`, and `loop_stale_binding`. Loop summaries deliberately
omit full workflow goals, memory, evidence, and gate notes; use
`workflow.get` or `include_workflow_details` when those details are needed.


`system.identify` is a compact read-only context call for agents and scripts that need the canonical current target before acting: it accepts the same workspace selectors plus optional `surface_id`, treats wrapper-provided `FORKTTY_SURFACE_ID`/`FORKTTY_WORKSPACE_ID` as caller validation context, uses a known caller surface as the default target when no explicit target selector is supplied, and falls back to the active workspace focus when that caller surface is stale or absent. It returns the target workspace, target surface with `effective_project_cwd`, current agent binding when present, and caller id validation booleans. The CLI `forktty wait agent-status` is a bounded client-side lifecycle poll built from repeated short `context.snapshot` calls with `tail_lines: 0`; it accepts a required `--status` (`running`, `working`, `idle`, `done`, `needs_input`, `blocked`, `suspended`, `ended`, `closed`, or `unknown`), optional workspace/surface/agent filters, `--timeout-ms` capped at 120000, and `--interval-ms` capped at 5000. The wait wrapper never reads terminal text, sends input, closes panes, or holds one socket request open while waiting; timeout exits nonzero with code `timeout`.

Agent rows from `agent.list`, `agent.health`, `status.summary`, and `context.snapshot` include `lifecycle_evidence` as diagnostic metadata, not as a second source of lifecycle truth. The block repeats the persisted lifecycle source, last activity, observation time, nullable age, matching workspace/provider status key/value/source/scope when present, and permission mode when present. `status_scope: "workspace_provider"` means the status row is shared by same-provider sessions in that workspace and is not per-session live proof. `agent.health` additionally includes `ready` and `readiness_reason` in that block so clients can explain stale-looking or non-resumable rows without joining separate response fields.

Claude Code `team.worker.launch` calls add documented permission-mode defaults unless the caller already supplied Claude permission args: review roles start with `--permission-mode dontAsk` plus pre-approved built-in read tools (`Read`, `Grep`, and `Glob`), while other Claude workers start with `--permission-mode auto`.

When `team.worker.launch` receives an explicit `worktree_name`, the worker tab is created in the matching already-open worktree workspace and inherits that workspace directory; without `worktree_name`, launch falls back to the team leader, team workspace focus, or active workspace. Provider launch argv validation treats BusyBox shell applets such as `busybox sh -c ...` as shell trampolines, matching direct shell and `env` wrappers.

Provider-scoped status and progress keys are automatically cleared when the last same-provider session in a workspace ends, is suspended/hibernated, is forgotten, or its surface is closed; per-surface status/progress keys are cleared when their surface is closed.

`team.message.dispatch` foregrounds the recipient worker surface by selecting its workspace and tab before writing. If the embedded terminal surface is not socket-ready yet, dispatch waits up to 10 seconds before returning `not_ready`.

Embedded Ghostty panes currently service visible and tail captures from Ghostty's visible-text ABI to avoid unbounded host allocations; explicit `surface.read_text` with `scope: "all"` remains the only full-scrollback read until a native bounded-tail embedding ABI exists.

`team.worker.shutdown` with `close_surface: true` is immediate disposable-pane cleanup after the shutdown text is accepted by the terminal; it is not proof that the worker processed a graceful shutdown request. Use it only for worker panes launched by the current ForkTTY runtime that can be discarded; stale persisted launch records are not sufficient close authorization after restart.

Hook-reported permission-mode status entries update the persisted agent session only when they target an already-known provider/session/surface. The exact mode `bypassPermissions` is preserved for supported Codex and Claude Code resumes by adding the providers' documented argv flags (`codex --dangerously-bypass-approvals-and-sandbox resume ...` and `claude --dangerously-skip-permissions --resume ...`) to `agent.health`, `agent.resume`, and restore-time auto-resume. Other permission mode strings remain metadata and are never copied into argv.

Worktree and branch names are trimmed and rejected if empty, too long, flag-like, control-character bearing, backslash-containing, or path-traversing.

Error responses include a structured `code` field so clients can branch on outcome instead of parsing message text:

| Code | Cause |
| ---- | ----- |
| `method_not_found` | Unknown method name. |
| `missing_param` | A required parameter is absent or has the wrong type. |
| `not_found` | The referenced workspace, surface, worktree, or metadata entry does not exist. |
| `payload_too_large` | The request line exceeds 1 MiB, `surface.send_text` text exceeds 256 KiB, or metadata text exceeds its method-specific limit. |
| `conflict` | The operation is valid but blocked by current state, such as dirty worktrees or in-use browser profiles. |
| `precondition_failed` | The request needs setup the caller can perform first: the worktree open-workspace boundary returns this, naming the remedy (`forktty create-workspace` / the `workspace_create` MCP tool). |

`forktty --json hooks doctor <agent>` and `forktty --json hooks test <agent>` emit a stable machine-readable report: a `version` field (currently 1) with additive-only evolution, an overall `ok` boolean, and — for `hooks test` — per-method `{method, ok, error?}` entries from a real socket round-trip that always includes `notification.create`. Both commands exit 0 when every check passes and 1 otherwise, so CI can gate on the exit code alone; the human-readable output is rendered from the same report.
| `already_exists` | The requested worktree or resource already exists. |
| `not_ready` | A target exists but is not ready to accept the operation. |
| `invalid_param` | A supplied parameter has an invalid value. |
| `error` | Catch-all for other failures (carries a `message`). |

`forktty remote-helper hello` is a no-socket stdio handshake intended to run
through SSH as `ssh <host> forktty remote-helper hello`. It emits one JSON
object with `schema: 1`, `protocol: "forktty-remote-stdio"`,
`protocol_version: 1`, the ForkTTY version, remote cwd, hostname, platform, and
the currently implemented helper capabilities (`["hello", "pty"]`). It does
not open a network listener, reconnect sessions, or require a local ForkTTY
socket.

`forktty remote-helper pty -- <program> [args...]` starts the argv command in a
PTY and relays raw stdin bytes into the PTY plus raw PTY output to stdout. It
uses a fixed initial 80x24 PTY size and exits with the child process status.
It does not frame messages, resize, reconnect, or persist remote session
ownership.

### MCP stdio bridge

`forktty mcp` runs a local Model Context Protocol server over stdio. It does
not listen on a network port; each MCP tool call is validated and then bridged
to the same owner-only Unix socket described above. Oversized, invalid JSON,
and invalid UTF-8 stdio messages return JSON-RPC `-32700` parse errors and do
not end the stdio session. The server exposes
`identify`, `workspace_list`, `surface_list`, `context_snapshot`, `topology_tree`, `remote_list`, `remote_status`, `surface_read_text`, `surface_capture_tail`, `agent_list`, `agent_health`, `agent_reclaim_plan`, `agent_hibernate`, `agent_reclaim`, `agent_resume`, `status_summary`, `workflow_list`, `workflow_get`, `workflow_upsert`, `workflow_plan_set`, `workflow_evidence_add`, `workflow_replay`, `team_list`, `team_get`, `team_upsert`, `team_finish`, `team_worker_upsert`, `team_worker_heartbeat`, `team_worker_launch`, `team_worker_health`, `team_worker_nudge`, `team_worker_shutdown`, `team_task_upsert`, `team_message_send`, `team_message_dispatch`, `team_message_ack`, `team_inbox`, `team_summary`, `team_events`, `surface_split`, `surface_send_text`,
`surface_focus`, `worktree_list`, `worktree_status`, `worktree_create`,
`worktree_attach`, `worktree_remove`, `worktree_merge`,
`notification_create`, and `status_set`. `FORKTTY_SOCKET_PATH` chooses the
socket, and `FORKTTY_WORKSPACE_ID`/`FORKTTY_SURFACE_ID` are used as default
targets when a tool omits an explicit target, except `identify`, which treats
them as caller validation context and falls back to active focus when the caller
surface is stale. `team_upsert` prefers `FORKTTY_SURFACE_ID` as
`leader_surface_id`; it falls back to `FORKTTY_WORKSPACE_ID` only when no leader
surface is available.

The MCP server also declares `resources` and `prompts` capabilities. It exposes
a read-only `forktty://agent/operating-guide` text resource and a
`forktty_operating_guide` prompt with the same content. The guide tells agents
to use ForkTTY tools for pane/workspace coordination, SSH remote inventory,
agent session discovery or resume, worktree management, terminal read/capture, visible
status/notifications, and sending text to a different surface; ordinary code
edits in the current repository should proceed without ForkTTY tool calls. The
server's `initialize` instructions include the
same short policy and point at the resource/prompt for the full guide.

`forktty mcp setup` registers this stdio server in verified user-scope MCP
config locations for Codex (`$CODEX_HOME/config.toml` or
`~/.codex/config.toml`), Claude Code (`~/.claude.json`), and Antigravity
(`~/.gemini/config/mcp_config.json`). Codex TOML setup preserves comments and
formatting and uses the MCP config size budget, so large hand-edited Codex
configs are not constrained by the smaller hook-template limit. Registration
writes a ForkTTY-managed server named `forktty`, preserves foreign MCP servers,
writes atomically, and creates a `.bak-*` backup when content changes.
`forktty mcp remove` removes only that managed server entry. `forktty mcp
remove gemini` is kept only to clean legacy ForkTTY-managed server entries
from `~/.gemini/settings.json`; Gemini MCP setup remains unsupported.

### Agent skills

`forktty skills setup` installs a ForkTTY-managed Agent Skill named
`forktty-agent-orchestration`. The skill is instruction-only and tells coding
agents when to inspect ForkTTY context, how to treat terminal tails as
untrusted input, how to treat fetched public docs as documentation-only
evidence, when to use team/workflow/status MCP tools, how to account for
`provider_capabilities`, compact `team_summaries`, persisted agent
source/age/`lifecycle_evidence` metadata, and dispatch submit/Enter semantics,
how to run durable team preflight with workflow/task records, how to assign
explicit worker roles, how to prefer
already-open worktree workspaces for mutating parallel workers, how to avoid
cross-pane writes before reading the target surface, and how to prefer isolated
temporary config roots for integration QA without redirecting the live ForkTTY
socket path when validating the currently running instance. For hook, MCP, and
skill setup debugging, it points agents at local doctor diagnostics and setup
dry runs before configuration changes. The default setup writes
the same managed skill to the interoperable Agent Skills user location
(`~/.agents/skills/forktty-agent-orchestration`) and to Claude Code's personal
skills location (`~/.claude/skills/forktty-agent-orchestration`, or
`$CLAUDE_CONFIG_DIR/skills/forktty-agent-orchestration` when set). The
`codex` and `pi` are accepted as aliases for the interoperable `agents` target;
`claude` targets Claude Code's skill directory.
`forktty skills remove` removes only skill directories containing ForkTTY's
managed marker and moves the directory to a `.bak-*` backup. Setup refuses to
overwrite an existing skill directory with the same name unless its `SKILL.md`
contains the ForkTTY-managed marker. `forktty skills setup --dry-run` and
`forktty --json doctor` report each managed skill target's status
(`missing`, `up_to_date`, `update_available`, `unmanaged`, or `invalid`),
source and installed checksums, and `forktty skills setup <target>` as the
repair command when the managed copy is missing, stale, or invalid with a
verified ForkTTY-managed marker.
Doctor skill inspection reads only bounded regular files and reports symlinked,
non-regular, or oversized managed skill components (`SKILL.md`, `agents/`, or
`agents/openai.yaml`) as `invalid`.
`forktty skills setup <target>` treats invalid managed copies as repairable
only after verifying the marker, backs up the existing skill directory, and
reinstalls regular managed files; when the marker cannot be verified, such as a
symlinked skill directory or `SKILL.md`, setup refuses to overwrite the path.

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
- bounded visible-tail prompt fallback for common agent prompts.

Notifications update in-app unread state and may dispatch through `notify-rust` and `notification_command`. Workspace/surface-targeted desktop notifications register a best-effort default `Open` action, using the freedesktop `default` action key, that argv-executes the current ForkTTY binary to focus the target surface or workspace; global notifications remain passive. Custom commands are argv-executed, not `sh -c`; title/body are passed through environment variables, and terminal-originated OSC 99 `f`/`t` metadata is passed as `FORKTTY_NOTIFICATION_TERMINAL_APP` plus JSON array `FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON`. `blocked_terminal_apps` and `blocked_terminal_types` are exact string match filters for terminal-originated OSC 99 `f`/`t` metadata before the notification is stored or dispatched. OSC 99 notification identifiers are tracked and echoed only when they use the protocol identifier character set (`A-Z`, `a-z`, `0-9`, `_`, `-`, `+`, `.`); unsafe identifiers are treated as untracked notification payloads or ignored for reply-only actions. Unknown OSC 99 payload types are ignored so future protocol extensions do not surface as terminal status noise. OSC 99 binary icons are rendered in-app when GTK can decode them, after `n=` icon names and `f=` application-name icon fallback; desktop binary icons are materialized as bounded files under `$XDG_RUNTIME_DIR/forktty-notification-icons` and removed when the tracked desktop notification is replaced, closed, or evicted.

Prompt notification feed rows expose `approval_state` as `pending`, `approved`, `denied`, `dismissed`, or `stale`. `notification.clear` and the GTK notification panel's dismiss/clear actions mark still-pending prompt approvals as `dismissed`, close any matching desktop notification, and send OSC 99 close reports when the originating terminal requested close reporting. `feed.list` and `context.snapshot` normalize still-pending approvals whose target workspace or surface no longer exists to `stale`; only `pending` approvals raise the snapshot `pending_approval` risk flag.

## Security Constraints

- Local Linux desktop threat model; same-user processes are not treated as hostile isolation boundaries.
- No crash-reporting or product event-tracking network calls. The default
  anonymous daily usage ping can be disabled with
  `telemetry.anonymous_ping = false`; optional update checks query GitHub
  Releases at most once per day and can be disabled. Optional browser panes
  and optional PR lookup can make user-directed network requests.
- Owner-only Unix socket permissions and private runtime directory validation.
- `forktty mcp` is a local stdio bridge only; it opens no network listener and
  enforces the same Unix socket ownership boundary as the CLI.
- 1 MiB bounds for socket requests, config, and session files.
- Hook session-to-surface routing cache is local process memory only, capped at
  256 entries, and evicted on session-end or surface close. Per-surface agent
  session ids and hook cwd values learned from hooks are persisted as resume
  metadata for explicit and restore-time provider resume. Codex cwd fallback
  reads only local `session_meta` JSONL records under `$CODEX_HOME/sessions` or
  `~/.codex/sessions` and requires the referenced cwd to still be a directory.
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
