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
- surface kind (`Terminal` or `Browser { url, profile }`);
- unread/attention state.

Splits are represented as recursive `PaneNode::Split { axis, children, sizes }`; leaf nodes reference surface IDs. GTK renders the active workspace tree into nested `gtk::Paned` containers and reuses Ghostty widgets by surface ID.

## Terminal Lifecycle

1. A workspace or split creates a surface in `WorkspaceModel`.
2. `forktty-ui-gtk` sends a `SpawnRequest` through `TerminalBackend`.
3. The Ghostty adapter creates a Ghostty-backed terminal, applies appearance (font, colors, scrollback) settings, and spawns the configured shell.
4. Child processes inherit:
   - `TERM=xterm-256color`
   - `COLORTERM=truecolor`
   - `TERM_PROGRAM=ForkTTY`
   - `TERM_PROGRAM_VERSION`
   - `FORKTTY_WORKSPACE_ID`
   - `FORKTTY_SURFACE_ID`
   - `FORKTTY_SOCKET_PATH`
5. Socket methods and GTK actions can send text, split, focus, close, or resize surfaces.
6. Closing a pane/workspace closes the corresponding Ghostty surface.

ForkTTY owns the child PTY. Prompt/status detection uses ForkTTY hook integration termprops, bell/child-exit signals, and a bounded visible-tail prompt fallback.

## Session Persistence

Native session file:

```text
~/.local/share/forktty/session-v2.json
```

The native session includes workspace order, active workspace, pane tree, focused surface, cwd, branch, and worktree metadata. It excludes running PTY process handles and scrollback.

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
font_family = ""
font_size = 14
scrollback_lines = 20000
terminal_audible_bell = true
sidebar_position = "left"
sidebar_visible = true
terminal_renderer = "auto"
terminal_theme = "system"
window_mode = "normal"

[notifications]
desktop = true
sound = true
```

Config files are regular-file checked and capped at 1 MiB. Saved settings validate shell path, theme source, worktree layout, font size, scrollback bounds, sidebar position, terminal theme, window mode, renderer value, PR lookup toggle, and notification command. `terminal_theme = "system"` uses ForkTTY's neutral dark palette; named values are fixed dark palettes (`catppuccin-mocha`, `rose-pine`, `tokyo-night`, `dracula`, `gruvbox-dark`). `terminal_renderer` is retained for compatibility; legacy `"vte"` input normalizes to `"auto"` and the native GTK runtime uses Ghostty.

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
| System | `system.ping`, `system.capabilities` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.create_ssh`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.split`, `surface.send_text`, `surface.focus`, `surface.close` |
| Pane | `pane.new_tab`, `pane.select_tab` |
| Notification | `notification.create`, `notification.list`, `notification.clear` |
| Worktree | `worktree.list`, `worktree.status`, `worktree.create`, `worktree.attach`, `worktree.remove`, `worktree.merge` |
| Metadata | `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.list_progress`, `metadata.clear_progress`, `metadata.log`, `metadata.list_logs`, `metadata.clear_logs` |
| Events | `events.subscribe` |
| Browser | `browser.open`, `browser.navigate`, `browser.snapshot`, `browser.click`, `browser.fill`, `browser.eval`, `browser.back`, `browser.forward`, `browser.reload`, `browser.profile.list`, `browser.profile.create`, `browser.profile.delete`, `browser.history.list`, `browser.history.search`, `browser.history.clear`, `browser.bookmark.add`, `browser.bookmark.list`, `browser.bookmark.remove`, `browser.import.discover`, `browser.import.preview`, `browser.import.run` |

Request lines are capped at 1 MiB. `surface.send_text` additionally rejects `text` payloads larger than 256 KiB so a wedged PTY pipe cannot block the dispatch task. Surface-targeted writes, notification targets, and explicit metadata workspace selectors are validated against the current workspace model, so stale workspace or surface ids return `not_found` instead of dispatching to dead panes. Hook-originated metadata/notification requests that carry `hook_session_id` can use a bounded server-side session-to-surface cache when later hook requests lose their explicit `workspace_id`/`surface_id`; explicit targets always win, and stale cached surfaces are discarded instead of reviving closed panes. Socket paths are owner-private by default, stale sockets are removed only after probing, and an existing live ForkTTY socket prevents a second instance from taking over the path.

Error responses include a structured `code` field so clients can branch on outcome instead of parsing message text:

| Code | Cause |
| ---- | ----- |
| `method_not_found` | Unknown method name. |
| `missing_param` | A required parameter is absent or has the wrong type. |
| `not_found` | The referenced workspace, surface, worktree, or metadata entry does not exist. |
| `payload_too_large` | The request line exceeds 1 MiB, or `surface.send_text` text exceeds 256 KiB. |
| `conflict` | The operation is valid but blocked by current state, such as dirty worktrees or in-use browser profiles. |
| `already_exists` | The requested worktree or resource already exists. |
| `not_ready` | A target exists but is not ready to accept the operation. |
| `invalid_param` | A supplied parameter has an invalid value. |
| `error` | Catch-all for other failures (carries a `message`). |

### MCP stdio bridge

`forktty mcp` runs a local Model Context Protocol server over stdio. It does
not listen on a network port; each MCP tool call is validated and then bridged
to the same owner-only Unix socket described above. The server exposes
`workspace_list`, `surface_list`, `surface_split`, `surface_send_text`,
`surface_focus`, `worktree_list`, `worktree_status`, `worktree_create`,
`worktree_attach`, `worktree_remove`, `worktree_merge`,
`notification_create`, and `status_set`. `FORKTTY_SOCKET_PATH` chooses the
socket, and `FORKTTY_WORKSPACE_ID`/`FORKTTY_SURFACE_ID` are used as default
targets when a tool omits an explicit target.

`forktty mcp setup` registers this stdio server in verified user-scope MCP
config locations for Codex (`$CODEX_HOME/config.toml` or
`~/.codex/config.toml`), Claude Code (`~/.claude.json`), Gemini CLI
(`~/.gemini/settings.json`), and Antigravity (`~/.gemini/config/mcp_config.json`).
Registration writes a ForkTTY-managed server named `forktty`, preserves
foreign MCP servers, writes atomically, and creates a `.bak-*` backup when
content changes. `forktty mcp remove` removes only that managed server entry.

## Browser Pane Feature

The `browser` cargo feature builds WebKitGTK6 panes alongside Ghostty panes.
Browser surfaces are part of `WorkspaceModel` and carry a URL plus a
`ProfileId`. The socket can open/navigate browser surfaces and, when a
GTK browser command channel is available, run snapshot/click/fill/eval and
back/forward/reload operations against the live WebView.

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
- create worktree and branch;
- attach existing branch/worktree;
- remove worktree after dirty-state and metadata validation;
- merge worktree branch with dirty-target/conflict checks;
- run `.forktty/setup` after open/create as advisory setup; failures are reported but do not hide an already-created worktree;
- run `.forktty/teardown` before removal; failures block removal, and dirty state is rechecked after the hook before deleting files.

Worktree and hook paths are canonicalized. Hook execution is limited to `.forktty/setup` and `.forktty/teardown` inside verified worktrees.

## Notifications

Notification sources:

- explicit socket/hook `notification.create`;
- ForkTTY hook `precmd`, `preexec`, and `postexec` termprops;
- ForkTTY progress termprops;
- Ghostty OSC 9 and basic OSC 99 terminal notifications;
- Ghostty bell;
- Ghostty child exit;
- bounded visible-tail prompt fallback for common agent prompts.

Notifications update in-app unread state and may dispatch through `notify-rust` and `notification_command`. Custom commands are argv-executed, not `sh -c`; title/body are passed through environment variables.

## Security Constraints

- Local Linux desktop threat model; same-user processes are not treated as hostile isolation boundaries.
- No telemetry, update checks, or product-service network calls. Optional
  browser panes and optional PR lookup can make user-directed network requests.
- Owner-only Unix socket permissions and private runtime directory validation.
- `forktty mcp` is a local stdio bridge only; it opens no network listener and
  enforces the same Unix socket ownership boundary as the CLI.
- 1 MiB bounds for socket requests, config, and session files.
- Hook session-to-surface correlation is local process memory only, capped at
  256 entries, and evicted on session-end or surface close.
- Shell and notification executables must be absolute executable files.
- Hooks are limited to verified worktree-local paths.
- Worktree removal rejects dirty/tampered targets.
- Browser profile IDs are validated before becoming directory names.
- Browser JavaScript evaluation is available only through the local socket and
  runs in the addressed WebKit page.

Residual risks:

- User-authored hooks and notification commands run with user privileges.
- A same-user process can interact with user-owned runtime resources.
- Advanced OSC 99 compatibility remains partial; base64 payloads, notification update/close controls, activation reports, buttons, and full chunk aggregation are not yet implemented.

## Test Strategy

Current automated coverage:

- Rust unit tests for config validation, session validation/quarantine, workspace/pane model, socket protocol, terminal backend, notification metadata, and worktree hardening.
- Rust tests for browser command types, profile metadata, and history/bookmark stores.
- Rust tests in `crates/forktty-ui-gtk/src/socket_cli.rs` for CLI parameter building, hook config merging, notification formatting, and socket-target fallbacks.
- CI for Rust fmt/test/clippy/build, repository consistency (`cargo run -p xtask -- check`), desktop entry validation, `.deb` packaging, dependency review, and cargo audit.

Backlog validation:

- runtime smoke tests for GTK/Ghostty interactions;
- manual package QA across supported Linux environments;
- persistent scrollback tests once scrollback persistence exists.
