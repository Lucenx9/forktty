# ForkTTY Technical Specification

ForkTTY is a Linux-only native terminal for running multiple coding agents in parallel. The primary implementation is now Rust + GTK4/libadwaita + VTE, with direct Unix socket automation and git worktree isolation.

## Runtime Architecture

```text
forktty-core
  Workspace model, pane tree, config, session v2, notifications, worktree logic

forktty-terminal
  TerminalBackend trait, headless test backend, VTE adapter

forktty-socket
  Tokio Unix socket server, newline-delimited JSON-RPC, direct dispatch

forktty-ui-gtk
  GTK4/libadwaita app shell, VTE panes, sidebar, dialogs, quake mode

scripts/forktty.mjs
  Node CLI and agent hook installer over the socket API
```

There is no Tauri/WebKit/React runtime in the primary tree.

## Tech Stack

| Layer | Technology | Use |
| ----- | ---------- | --- |
| UI shell | GTK4 + libadwaita | Native Linux window, header, dialogs, sidebar |
| Terminal | VTE GTK4 | Embedded terminal widget and child PTY owner |
| State | Rust `WorkspaceModel` | Workspaces, panes, surfaces, metadata, notifications |
| Git | `git2` | Worktree create/attach/remove/merge/status |
| Notifications | `notify-rust` | Desktop notifications and custom notification command dispatch |
| Socket API | tokio + serde_json | Local Unix JSON-RPC automation |
| Config | TOML | `~/.config/forktty/config.toml` |
| CLI hooks | Node.js built-ins | Repo-local socket client and hook config merger |

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
- unread/attention state.

Splits are represented as recursive `PaneNode::Split { axis, children, sizes }`; leaf nodes reference surface IDs. GTK renders the active workspace tree into nested `gtk::Paned` containers and reuses VTE widgets by surface ID.

## Terminal Lifecycle

1. A workspace or split creates a surface in `WorkspaceModel`.
2. `forktty-ui-gtk` sends a `SpawnRequest` through `TerminalBackend`.
3. The VTE adapter creates a VTE terminal, applies appearance (font, colors, scrollback) settings, and spawns the configured shell.
4. Child processes inherit:
   - `TERM=xterm-256color`
   - `COLORTERM=truecolor`
   - `TERM_PROGRAM=ForkTTY`
   - `TERM_PROGRAM_VERSION`
   - `FORKTTY_WORKSPACE_ID`
   - `FORKTTY_SURFACE_ID`
   - `FORKTTY_SOCKET_PATH`
5. Socket methods and GTK actions can send text, split, focus, close, or resize surfaces.
6. Closing a pane/workspace closes the corresponding VTE surface.

VTE owns the child PTY. Prompt/status detection uses VTE shell integration termprops, bell/child-exit signals, and a bounded visible-tail prompt fallback.

## Session Persistence

Native session file:

```text
~/.local/share/forktty/session-v2.json
```

The native session includes workspace order, active workspace, pane tree, focused surface, cwd, branch, and worktree metadata. It excludes running PTY process handles and scrollback.

ForkTTY can import the legacy `session.json` format but saves native sessions as v2. Session load validates file type, size, version, pane-tree depth/shape, and focused leaf index. Invalid files are quarantined instead of crashing startup.

## Config

Config file:

```text
~/.config/forktty/config.toml
```

```toml
[general]
theme_source = "auto"
shell = "/bin/bash"
worktree_layout = "nested"
notification_command = ""

[appearance]
font_family = ""
font_size = 14
scrollback_lines = 20000
terminal_audible_bell = true
sidebar_position = "left"
sidebar_visible = true
terminal_renderer = "vte"
window_mode = "normal"

[notifications]
desktop = true
sound = true
```

Config files are regular-file checked and capped at 1 MiB. Saved settings validate shell path, worktree layout, font size, scrollback bounds, sidebar position, window mode, renderer value, and notification command. `terminal_renderer` is retained for compatibility; the native GTK runtime uses VTE.

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
| System | `system.ping` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.split`, `surface.send_text`, `surface.focus`, `surface.close` |
| Notification | `notification.create`, `notification.list`, `notification.clear` |
| Worktree | `worktree.list`, `worktree.status`, `worktree.create`, `worktree.attach`, `worktree.remove`, `worktree.merge` |
| Metadata | `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.list_progress`, `metadata.clear_progress`, `metadata.log`, `metadata.list_logs`, `metadata.clear_logs` |

Request lines are capped at 1 MiB. `surface.send_text` additionally rejects `text` payloads larger than 256 KiB so a wedged VTE pipe cannot block the dispatch task. Surface-targeted writes, notification targets, and explicit metadata workspace selectors are validated against the current workspace model, so stale workspace or surface ids return `not_found` instead of dispatching to dead panes. Socket paths are owner-private by default, stale sockets are removed only after probing, and an existing live ForkTTY socket prevents a second instance from taking over the path.

Error responses include a structured `code` field so clients can branch on outcome instead of parsing message text:

| Code | Cause |
| ---- | ----- |
| `method_not_found` | Unknown method name. |
| `missing_param` | A required parameter is absent or has the wrong type. |
| `not_found` | The referenced workspace, surface, worktree, or metadata entry does not exist. |
| `payload_too_large` | The request line exceeds 1 MiB, or `surface.send_text` text exceeds 256 KiB. |
| `error` | Catch-all for other failures (carries a `message`). |

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
- VTE shell `precmd`, `preexec`, and `postexec` termprops;
- VTE progress termprops;
- VTE bell;
- VTE child exit;
- bounded visible-tail prompt fallback for common agent prompts.

Notifications update in-app unread state and may dispatch through `notify-rust` and `notification_command`. Custom commands are argv-executed, not `sh -c`; title/body are passed through environment variables.

## Security Constraints

- Local Linux desktop threat model; same-user processes are not treated as hostile isolation boundaries.
- No remote network calls, telemetry, or update checks.
- Owner-only Unix socket permissions and private runtime directory validation.
- 1 MiB bounds for socket requests, config, and session files.
- Shell and notification executables must be absolute executable files.
- Hooks are limited to verified worktree-local paths.
- Worktree removal rejects dirty/tampered targets.

Residual risks:

- User-authored hooks and notification commands run with user privileges.
- A same-user process can interact with user-owned runtime resources.
- VTE owns the PTY, so byte-level OSC 9/99 parsing from the legacy PTY-owner path is not fully ported.

## Test Strategy

Current automated coverage:

- Rust unit tests for config validation, session validation/quarantine, workspace/pane model, socket protocol, terminal backend, notification metadata, and worktree hardening.
- Node built-in tests for CLI parameter building, hook config merging, notification formatting, and socket-target fallbacks.
- CI for Rust fmt/test/clippy/build, CLI tests, desktop entry validation, `.deb` packaging, dependency review, and cargo audit.

Backlog validation:

- runtime smoke tests for GTK/VTE interactions;
- manual package QA across supported Linux environments;
- persistent scrollback tests once scrollback persistence exists.
