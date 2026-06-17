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
3. The Ghostty adapter creates a Ghostty-backed terminal, applies font and color appearance from Ghostty's config plus ForkTTY scrollback settings, and spawns the configured shell. Runtime zoom shortcuts scale the open GTK panes without persisting font settings. When Ghostty shell-integration resources are available, ForkTTY injects the upstream zsh/bash/fish/elvish/nushell startup integration and packages those resources plus Ghostty terminfo in Linux release artifacts.
4. Child processes inherit:
   - `TERM=xterm-ghostty` with matching terminfo when available, otherwise `TERM=xterm-256color`
   - `COLORTERM=truecolor`
   - `TERM_PROGRAM=ForkTTY`
   - `TERM_PROGRAM_VERSION`
   - `FORKTTY_WORKSPACE_ID`
   - `FORKTTY_SURFACE_ID`
   - `FORKTTY_SOCKET_PATH`
   - `GHOSTTY_RESOURCES_DIR` and shell-specific `GHOSTTY_*` startup variables when Ghostty shell integration is active
5. Socket methods and GTK actions can send text, split, focus, close, or resize surfaces.
6. Closing a pane/workspace closes the corresponding Ghostty surface.

ForkTTY owns the child PTY. Prompt/status detection uses ForkTTY hook integration termprops, bell/child-exit signals, and a bounded visible-tail prompt fallback.

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
scrollback_lines = 20000
persistent_scrollback_lines = 0
terminal_audible_bell = true
sidebar_position = "left"
sidebar_visible = true
terminal_renderer = "auto"
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

Config files are regular-file checked and capped at 1 MiB. Malformed or invalid config content is quarantined; transient I/O errors are reported without renaming the file. Saved settings validate shell path, theme source, worktree layout, scrollback bounds, persistent scrollback bounds (max 1,000 lines), sidebar position, window mode, renderer value, PR lookup toggle, update auto-check toggle, telemetry anonymous-ping toggle, notification filters, and notification command. Legacy `font_family`, `font_size`, and `terminal_theme` keys are accepted on load for compatibility, omitted from new saves, and ignored by the GTK renderer; terminal font and color preferences come from Ghostty's config when present. The GTK runtime loads `~/.config/ghostty/config` and `config.ghostty`, follows `config-file`, resolves `theme` for dark mode, searches Ghostty theme directories, and applies font size/family/style-family entries, font features/variations, foreground/background, cursor, selection, named colors, `cell-foreground`/`cell-background` cursor and selection color references, `cursor-opacity`, DECSCUSR-backed `cursor-style`/`cursor-style-blink` defaults, `selection-clear-on-typing`, `selection-clear-on-copy`, `selection-word-chars`, `clipboard-trim-trailing-spaces`, `clipboard-codepoint-map`, `copy-on-select`, `right-click-action`, `scroll-to-bottom`, SGR faint text plus `faint-opacity`, `mouse-reporting`, `mouse-shift-capture`, `mouse-hide-while-typing`, `mouse-scroll-multiplier`, `adjust-cell-width`, `adjust-cell-height`, `adjust-font-baseline`, underline/strikethrough/overline/cursor metric adjustments, `bold-color`/`bold-is-bright`, short/full hex colors, ANSI palette entries 0-15, `scrollback-limit`, `image-storage-limit`, `unfocused-split-opacity`, and `unfocused-split-fill`. `terminal_renderer` is retained for compatibility; legacy `"vte"` input normalizes to `"auto"` and the native GTK runtime uses Ghostty.

Ghostty compatibility scope:

| Ghostty config area | ForkTTY status |
| --- | --- |
| Config discovery | Supported for `~/.config/ghostty/config`, `~/.config/ghostty/config.ghostty`, recursive `config-file`, and Ghostty theme directories, with ForkTTY's regular-file and size guards. |
| Terminal appearance | Supported for terminal font family/style fallbacks, font size, font features/variations, foreground/background, cursor colors, DECSCUSR-backed cursor style defaults, selection colors/clear-on-typing/clear-on-copy/word-chars/copy-on-select, clipboard trim-trailing-spaces/codepoint-map, right-click action, scroll-to-bottom, mouse reporting/shift-capture including XTSHIFTESCAPE overrides, mouse-hide-while-typing, bold, faint, ANSI palette, named colors, short/full hex colors, `theme`, cell/text-decoration/cursor metric adjustment, scrollback limit, image storage limit, mouse scroll multiplier, and inactive split dimming. |
| Runtime terminal state | Delegated to `libghostty-vt` for VT parsing, key/paste encoding, OSC 8 links, OSC 9/99 notifications, bracketed paste/focus mode mirrors, XTSHIFTESCAPE mouse-shift capture overrides, selection formatting, word selection, and Kitty image protocol storage/loading media plus PNG decode/placement geometry. Shell startup integration uses Ghostty's upstream shell scripts when resources are available. |
| ForkTTY-owned UI | Intentionally not read from Ghostty config. Window layout, tabs, splits, sidebar, socket automation, worktrees, agent controls, notifications UI, and session restore use ForkTTY config/session state. |
| Ghostty GUI/window/platform options | Ignored unless ForkTTY has the same runtime concept. Examples include Ghostty keybinds, quick terminal, window decorations, titlebar/font, shell integration UI, macOS-only options, shaders, background blur/opacity, and Linux cgroup settings. |
| Renderer parity | Partial by design: ForkTTY currently paints Ghostty cells with a GTK/Pango/Cairo renderer. Kitty PNG images are drawn from libghostty-vt placements, but shaders, GPU/background effects, and full upstream renderer parity wait on renderer work or a stable upstream Ghostty embeddable renderer/widget API that fits ForkTTY panes, splits, socket automation, and session restore. |

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
https://forktty-site.vercel.app/api/telemetry/ping
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
| System | `system.ping`, `system.capabilities`, `system.top` |
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
| Workflow | `workflow.list`, `workflow.get`, `workflow.upsert`, `workflow.plan.set`, `workflow.evidence.add`, `workflow.replay` |
| Team | `team.list`, `team.get`, `team.upsert`, `team.worker.upsert`, `team.worker.heartbeat`, `team.worker.launch`, `team.worker.health`, `team.worker.nudge`, `team.worker.shutdown`, `team.task.upsert`, `team.message.send`, `team.message.dispatch`, `team.message.ack`, `team.inbox`, `team.summary`, `team.events` |
| Topology | `topology.tree` |
| Events | `events.subscribe` |
| Browser | `browser.open`, `browser.navigate`, `browser.snapshot`, `browser.click`, `browser.fill`, `browser.back`, `browser.forward`, `browser.reload`, `browser.profile.list`, `browser.profile.create`, `browser.profile.delete`, `browser.history.list`, `browser.history.search`, `browser.history.clear`, `browser.bookmark.add`, `browser.bookmark.list`, `browser.bookmark.remove` |

Request lines are capped at 1 MiB. Browser import is an in-app workflow only; `browser.import.*` methods are not advertised or accepted over the Unix-socket automation boundary because they read external browser profile data. `surface.send_text` additionally rejects `text` payloads larger than 256 KiB so a wedged PTY pipe cannot block the dispatch task. `surface.read_text` returns bounded terminal text for one surface (`scope: "visible"` by default, or `"all"` for full scrollback) and `surface.capture_tail` returns the last `lines` terminal lines (default 80, max 5000); both cap returned UTF-8 text to at most 512 KiB by default/upper bound and set `truncated` when text is byte-trimmed. `topology.tree` returns read-only workspace entries with pane trees and nested surface metadata, accepting the same workspace selectors as `surface.list`. `system.top` returns a read-only health snapshot with workspace totals, surface focus/unread/kind/cwd/runtime size, PID when the GTK backend knows it, persisted agent lifecycle, status, and progress fields; it accepts the same workspace selectors as `surface.list`. `remote.list` and `remote.status` return read-only SSH workspace rows derived from `SurfaceKind::Ssh` plus current terminal backend state: host, workspace/surface identity, title/cwd, active/focused flags, `connected`, and runtime PID/size/shell when the backend still owns a live surface. They do not run remote commands, reconnect sessions, or start a remote helper. `feed.list` normalizes notification, prompt approval, status, and progress events into feed rows, accepts the same workspace selectors plus optional `limit` (default 50, max 200), and reads bounded history from `$XDG_STATE_HOME/forktty/feed.json` (or the platform data fallback). Without an available feed store, it falls back to the current in-memory snapshot. `feed.approval.respond` accepts `{id, decision}` with `decision` as `approve`/`approved` or `deny`/`denied` and records the approval row state as `approved` or `denied`; it does not yet send provider-specific replies back into an agent permission protocol. `project.action.list` and `project.action.run` read repo-local `forktty.json` files from git worktrees already open in ForkTTY. The file format is `{"actions":[{"id":"test","label":"Run tests","argv":["cargo","test"],"cwd":"."}]}`; `description` and `cwd` are optional, `argv` must be an array, shell command strings such as `bash -c ...` are rejected, action cwd values must stay inside the project root, and `run` opens a new terminal tab in the matching workspace. `workflow.*` persists provider-neutral workflow state in `$XDG_STATE_HOME/forktty/workflow-v1.json` (falling back to the local data dir): `workflow.upsert` creates or updates workspace/surface/session/mode bindings with status, goal, and compaction-resistant memory; `workflow.plan.set` replaces bounded plan steps; `workflow.evidence.add` appends bounded text/path evidence; `workflow.list`, `workflow.get`, and `workflow.replay` support session search and event replay. The store rejects symlinks, non-regular files, oversized data, duplicate ids, control characters, and entries beyond its workflow/plan/evidence/event caps. `team.*` methods provide a provider-neutral team orchestration control plane persisted in `$XDG_STATE_HOME/forktty/team-v1.json` (or the platform state/data fallback) with owner-private directory/file permissions. The store is versioned and capped at 1 MiB, with per-store/team caps for teams, workers, tasks, messages, and events. `team.upsert` records leader/workspace metadata, `team.worker.*` records worker references, heartbeats, launch/nudge/shutdown timestamps, and health snapshots, `team.task.upsert` stores task status and validates dependencies as a DAG, `team.message.*` stores bounded mailbox messages, `team.message.dispatch` sends queued text to an attached worker pane and acknowledges only after delivery succeeds, and `team.list`/`team.get`/`team.inbox`/`team.summary`/`team.events` expose read-only views. `team.worker.launch` opens a new tab next to the team leader or team workspace focus, starts a supported provider (`codex`, `claude`, `gemini`, `opencode`, or `antigravity`) through argv only, and records the new `surface_id` on the worker; extra provider args are bounded UTF-8 argv entries and shell trampolines are rejected. `team.worker.nudge` and `team.worker.shutdown` send explicit text to the worker's attached pane and update state only after `surface.send_text` succeeds; shutdown is a safe request, not a process kill. Team surface ids and workspace selectors are validated against the current model; worktree names use the same validation as worktree commands. Team methods do not add GTK parity UI. All `worktree.*` and `project.action.*` methods validate their target path against the git repositories of currently open workspaces: this is a deliberate security boundary — a socket client (hook, MCP server, CLI) cannot drive git worktree operations or project actions on a repository the user never opened in ForkTTY, and a shell's mutable cwd is never treated as authorization. The rejection names the open workspace roots and the `forktty create-workspace` remedy. Surface-targeted writes, reads, notification targets, explicit workflow surface/workspace bindings, and explicit metadata workspace selectors are validated against the current workspace model, so stale workspace or surface ids return `not_found` instead of dispatching to dead panes. Hook-originated metadata/notification requests that carry `hook_session_id` can use a bounded server-side session-to-surface cache when later hook requests lose their explicit `workspace_id`/`surface_id`; `hook_session_id` uses the same 16 KiB metadata text limit before it can be cached, explicit targets always win, and stale cached surfaces are discarded instead of reviving closed panes. When a hook status targets a primary `agent:<provider>` key and carries `hook_session_id`, ForkTTY also persists `{agent, session_id, resume_cwd, lifecycle, last_activity_ms}` on that surface in `session-v2.json`; `resume_cwd` is the provider session cwd when `hook_session_cwd` names an existing absolute directory, with Antigravity deriving that value from the hook payload's `workspacePaths` because its wrapper scripts run from `~/.gemini/config`. Lifecycle is derived conservatively from hook events and normalized status values as `running`, `idle`, `needs_input`, `ended`, or `unknown`, plus `suspended` when ForkTTY intentionally hibernates a resumable agent terminal, and `last_activity_ms` is the Unix epoch millisecond when ForkTTY accepted the hook status or hibernation. `agent.list` exposes those persisted bindings with workspace/surface/title/cwd context and accepts the same workspace selectors as `surface.list`. `agent.health` returns those bindings plus local resume readiness fields (`ready`, `reason`, `program`, `executable`, `argv`): it validates the provider/session pair with the same argv-only command builder used by resume and checks whether the provider executable is discoverable on the ForkTTY process PATH, without launching the provider or proving the remote/session id still exists. When a Codex binding has no persisted `resume_cwd`, health/resume/restore may infer one from Codex's local `$CODEX_HOME/sessions` or `~/.codex/sessions` `session_meta` JSONL if the matching session file and cwd still exist. `agent.reclaim.plan` is read-only and accepts the same workspace selectors plus optional `min_idle_ms` (default 600000). It returns `{policy, candidates, protected}`: candidates are persisted sessions whose lifecycle is `idle`, last activity is known and older than `min_idle_ms`, and local resume readiness is `ready`; protected rows include `protect_reason` such as `running`, `needs_input`, `suspended`, `ended`, `unknown_lifecycle`, `unknown_activity`, `recent_activity`, or `not_ready:<reason>`. The plan does not close panes, kill processes, or mark sessions suspended. `agent.hibernate` is an explicit operation on one `surface_id`: it requires an idle persisted terminal agent session with known activity, optional `min_idle_ms` satisfied, and local resume readiness `ready`; then it closes the terminal process if present, marks the persisted session `suspended`, clears unread, and sets a per-surface `Suspended` status that prevents automatic respawn. `agent.reclaim` applies that hibernation operation to current `agent.reclaim.plan` candidates for an optional workspace selector, `min_idle_ms`, and capped `limit`, returning `hibernated`, `protected`, and `failed` rows. `agent.resume` is an explicit operation that takes a source `surface_id` with a persisted agent session, creates a new tab, and spawns the provider through argv only (`codex resume -C <resume_cwd> <id>` when Codex has a persisted or inferred `resume_cwd`, otherwise `codex resume <id>`, `claude --resume <id>`, `gemini --resume <id>`, `opencode --session <id>`, or `agy --conversation <id>`). When `resume_cwd` is available, ForkTTY also uses it as the provider process cwd, which keeps providers without a cwd flag such as Claude Code in the recorded session directory. Restored terminal surfaces with a supported persisted agent session use the same argv-only resume command during session restore instead of spawning a plain shell, except suspended sessions which stay closed until explicit resume. Session ids used for resume are trimmed and rejected if empty, too long, flag-like, or control-character bearing; unsupported custom agents return `precondition_failed` from resume and `unsupported_agent` health rows. `status.summary` returns a derived read-only workspace summary containing workspace identity, persisted agent sessions, current status entries, and progress entries for statusline/HUD consumers. Socket paths are owner-private by default, stale sockets are removed only after probing, and an existing live ForkTTY socket prevents a second instance from taking over the path.
Hook-reported permission-mode status entries are display-only metadata and do not add high-risk provider flags such as Claude Code `--dangerously-skip-permissions` or Codex `--dangerously-bypass-approvals-and-sandbox` to health, resume, or restore argv.

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
`workspace_list`, `surface_list`, `topology_tree`, `remote_list`, `remote_status`, `surface_read_text`, `surface_capture_tail`, `agent_list`, `agent_health`, `agent_reclaim_plan`, `agent_hibernate`, `agent_reclaim`, `agent_resume`, `status_summary`, `workflow_list`, `workflow_get`, `workflow_upsert`, `workflow_plan_set`, `workflow_evidence_add`, `workflow_replay`, `team_list`, `team_get`, `team_upsert`, `team_worker_upsert`, `team_worker_heartbeat`, `team_worker_launch`, `team_worker_health`, `team_worker_nudge`, `team_worker_shutdown`, `team_task_upsert`, `team_message_send`, `team_message_dispatch`, `team_message_ack`, `team_inbox`, `team_summary`, `team_events`, `surface_split`, `surface_send_text`,
`surface_focus`, `worktree_list`, `worktree_status`, `worktree_create`,
`worktree_attach`, `worktree_remove`, `worktree_merge`,
`notification_create`, and `status_set`. `FORKTTY_SOCKET_PATH` chooses the
socket, and `FORKTTY_WORKSPACE_ID`/`FORKTTY_SURFACE_ID` are used as default
targets when a tool omits an explicit target.

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
(`~/.gemini/config/mcp_config.json`). Gemini CLI is legacy opt-in:
`forktty mcp setup gemini` still registers the same managed server in
`~/.gemini/settings.json` for existing Gemini users. Registration writes a
ForkTTY-managed server named `forktty`, preserves foreign MCP servers, writes
atomically, and creates a `.bak-*` backup when content changes.
`forktty mcp remove` removes only that managed server entry.

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

Notifications update in-app unread state and may dispatch through `notify-rust` and `notification_command`. Custom commands are argv-executed, not `sh -c`; title/body are passed through environment variables, and terminal-originated OSC 99 `f`/`t` metadata is passed as `FORKTTY_NOTIFICATION_TERMINAL_APP` plus JSON array `FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON`. `blocked_terminal_apps` and `blocked_terminal_types` are exact string match filters for terminal-originated OSC 99 `f`/`t` metadata before the notification is stored or dispatched. OSC 99 notification identifiers are tracked and echoed only when they use the protocol identifier character set (`A-Z`, `a-z`, `0-9`, `_`, `-`, `+`, `.`); unsafe identifiers are treated as untracked notification payloads or ignored for reply-only actions. Unknown OSC 99 payload types are ignored so future protocol extensions do not surface as terminal status noise. OSC 99 binary icons are rendered in-app when GTK can decode them, after `n=` icon names and `f=` application-name icon fallback; desktop binary icons are materialized as bounded files under `$XDG_RUNTIME_DIR/forktty-notification-icons` and removed when the tracked desktop notification is replaced, closed, or evicted.

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

Backlog validation:

- runtime smoke tests for GTK/Ghostty interactions;
- manual package QA across supported Linux environments;
- persistent scrollback tests for the opt-in plain-text tail.
