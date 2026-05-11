# ForkTTY Technical Specification

ForkTTY is a Linux-only desktop terminal for running multiple coding agents in parallel. It combines Tauri v2, a Rust backend, React 19 UI, xterm.js terminals, optional git worktree isolation, prompt-aware notifications, session restore, and a local Unix socket API.

This document describes current implemented behavior unless a section is explicitly marked as backlog.

## Runtime Architecture

```text
Frontend (React 19 + TypeScript + Vite)
  Sidebar
    workspace list, worktree status, unread state, metadata, drag reorder
  Pane area
    recursive react-resizable-panels layout
    xterm.js terminal per leaf pane
  Overlays
    welcome, command palette, branch picker, settings, notifications, modals
  Stores
    Zustand workspace, config, and metadata state

Tauri v2 IPC bridge
  invoke() commands for PTY/config/session/worktree/notification operations
  Channel<PtyEvent> for PTY output, EOF, error, and scanner events
  frontend event bridge for socket API methods implemented in TypeScript

Rust backend
  pty_manager       portable-pty process lifecycle
  output_scanner    OSC 133, prompt patterns, OSC 9/99/777 notifications
  worktree          git2 worktree lifecycle and hook execution
  notification      notify-rust desktop notifications and custom command spawn
  socket_api        tokio Unix socket JSON-RPC endpoint
  config            ForkTTY TOML config and Ghostty theme parser
  session           session.json persistence, validation, quarantine, logs
```

## Tech Stack

| Layer | Technology | Current use |
| ----- | ---------- | ----------- |
| Shell | Tauri v2 | Linux desktop shell and Rust command bridge |
| Frontend | React 19 + TypeScript + Vite | Main app UI and state-driven rendering |
| Terminal | `@xterm/xterm` 5.5.0 | Terminal emulator |
| Terminal addons | Fit, Search, Canvas | Resize, find, and WebKitGTK-stable rendering |
| Split panes | `react-resizable-panels` 4.x | Recursive split layout |
| State | Zustand 5.x | Workspace, config, metadata, notifications |
| PTY | `portable-pty` 0.9 | Shell process lifecycle |
| Git | `git2` 0.20 | Branch/worktree operations |
| Notifications | `notify-rust` | XDG desktop notifications |
| Socket API | tokio + serde_json | Unix socket JSON-RPC |
| Config | toml + custom Ghostty parser | ForkTTY config and theme compatibility |

The terminal renderer is intentionally Canvas-based. WebGL is avoided because WebKitGTK/WebGL has known stability issues in Tauri Linux apps.

## UI Behavior

### Sidebar

Each workspace entry can show:

- workspace name;
- git branch label;
- working directory;
- worktree status (`clean`, `dirty`, `conflicts`, `error`) when available;
- unread count and last notification preview;
- optional metadata status pills, progress rows, and log entries from the socket API;
- reorder grip and close affordance.

The sidebar is resizable, can collapse to an icon rail, and can be positioned left or right through config.

### Overlays and Focus

Implemented overlays include:

- first-run welcome dialog, stored in `localStorage` as `forktty.welcome-seen`;
- command palette (`Ctrl+Shift+P`) with fuzzy filtering and empty state;
- branch picker (`Ctrl+Shift+N`) with loading, empty, and error handling;
- settings panel (`Ctrl+,`) with dirty-state/discard flow;
- notification panel (`Ctrl+Shift+I`) with empty state;
- inline confirm/prompt modals.

Destructive confirm modals default focus to Cancel so pressing Enter does not accidentally confirm destructive actions.

`ShortcutBar` is currently mounted but returns `null`; there is no visible shortcut bar in the current UI.

## PTY Lifecycle

1. A pane leaf is created in the workspace pane tree.
2. `TerminalPane` opens an xterm.js instance and registers it in the terminal registry.
3. The frontend invokes `pty_spawn` with cwd, workspace id, surface id, columns, and rows.
4. Rust validates the configured shell path as an absolute executable file.
5. Rust spawns the shell through `portable-pty`, sets ForkTTY environment variables, and starts a blocking reader task.
6. PTY output is scanned by `OutputScanner`, forwarded over a Tauri channel, and written into xterm.js through a bounded frontend output buffer.
7. Closing a pane or workspace kills associated PTYs when running inside Tauri.

Spawned shells receive:

| Variable | Description |
| -------- | ----------- |
| `FORKTTY_WORKSPACE_ID` | Current workspace UUID |
| `FORKTTY_SURFACE_ID` | Current surface/pane UUID |
| `FORKTTY_SOCKET_PATH` | Active ForkTTY socket path |
| `TERM` | `xterm-256color` |

The frontend bounds pending PTY output (`MAX_PENDING_OUTPUT_BYTES`) and drains in batches. This avoids unbounded frontend memory growth, but it is not full end-to-end PTY flow control.

## Workspace and Pane State

```typescript
interface Workspace {
  id: string;
  name: string;
  root: PaneTree;
  surfaces: Record<string, Surface>;
  focusedPaneId: string;
  workingDir: string;
  gitBranch: string;
  worktreeDir: string;
  worktreeName: string;
  worktreeStatus: string;
  unreadCount: number;
  lastNotificationText: string;
  createdAt: string;
}
```

```typescript
type PaneTree =
  | { type: "leaf"; id: string; surfaceId: string }
  | { type: "horizontal" | "vertical"; id: string; children: PaneTree[]; sizes: number[] };
```

`PaneTree` is recursively validated during session restore. Split nodes must have at least two children, `sizes.length` must match child count, and sizes must be finite positive numbers.

## Session Persistence

Rust session file:

```text
~/.local/share/forktty/session.json
```

The stored format includes:

- `version`;
- workspace snapshots;
- active workspace index;
- workspace name, cwd, branch, worktree path/name;
- pane tree shape;
- focused leaf index.

The stored format intentionally excludes:

- running PTY process handles;
- scrollback buffers;
- unread notification state;
- in-memory metadata and activity timestamps.

### Save Path

`startWorkspaceEffects()` subscribes to the workspace store and saves a debounced session payload through the Tauri `save_session` command. It also performs a best-effort flush on `beforeunload`, but browser unload does not guarantee async IPC completion.

Rust writes the session atomically through a `.tmp` file and rename.

### Restore Path

On startup, `load_session` reads `session.json`, validates the session version and pane tree structure, and returns `None` for missing files. Corrupt JSON, unsupported versions, or invalid structures are renamed to `session.json.bad-<timestamp>` and ignored.

The frontend rebuilds fresh workspace IDs, pane IDs, and surface records from the validated snapshot. Restore sets `lastWorkspaceSwitchTime` so prompt detection from newly spawned inactive shells is suppressed during the initial render window.

## Notification System

Notification events originate in the Rust output scanner and are dispatched in the frontend.

### Scanner Events

Supported prompt and notification signals:

- OSC 133 `A`, `C`, and `D` shell integration events;
- Claude-style prompt patterns such as `>`, `? ... (Y/n)`, `? ...:`, and `Do you want to proceed`;
- OSC 9 notifications;
- OSC 99 kitty notifications;
- OSC 777 `notify` notifications.

OSC buffers and prompt line buffers are bounded in Rust.

### Delivery

When a notification is dispatched:

1. The workspace store records it in-app and updates unread state.
2. The inactive workspace can be moved to the top of the sidebar.
3. The target pane can receive an unread ring when the pane is not focused.
4. Desktop notification is sent through `notify-rust` when enabled.
5. `notification_command` is run when configured.

Custom notification command behavior:

- split with `shell_words`;
- first token must be an absolute executable file;
- remaining tokens are passed as argv;
- no `sh -c`;
- title/body are delivered via `FORKTTY_NOTIFICATION_TITLE` and `FORKTTY_NOTIFICATION_BODY`.

### Noise Control

- Switching to a workspace marks its unread notifications as read.
- Prompt notifications are suppressed for four seconds after workspace switch or restore.
- Explicit notification events have a short per-pane debounce.
- Identical workspace/title/body notifications are deduplicated for a short window.
- Prompt notifications are skipped while the workspace or pane is already unread.

Idle-time notification detection is not implemented.

## Worktree Behavior

Worktree operations use `git2` and never shell out to git.

Implemented operations:

- create worktree and branch;
- attach to an existing branch from the branch picker;
- list branches for the branch picker;
- merge worktree branch;
- prepare and execute remove;
- run `.forktty/setup` after creation and `.forktty/teardown` before removal.

Validation:

- worktree names reject empty names, `/`, `\`, `..`, and NUL;
- worktree and hook paths are canonicalized;
- hook execution is limited to `.forktty/setup` and `.forktty/teardown` inside the worktree;
- socket-driven `cwd` values must belong to the same Git common directory as an open frontend workspace;
- subdirectories and linked worktrees in that open repository are accepted;
- unrelated repositories are rejected.

When removing the last worktree-backed workspace through the socket path, the frontend ensures at least one replacement plain workspace remains.

## Configuration

ForkTTY config:

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
sidebar_position = "left"

[notifications]
desktop = true
sound = true
```

Validation and hardening:

- ForkTTY config must be a regular file and no larger than 1 MiB.
- The settings save path validates `general.shell`, `general.worktree_layout`, `appearance.sidebar_position`, `appearance.font_size`, and `general.notification_command`.
- `general.shell` must be an absolute path to an executable file when saved and is checked again before PTY spawn.
- `general.worktree_layout` must be `nested`, `sibling`, or `outer-nested`.
- `appearance.sidebar_position` must be `left` or `right`.
- `appearance.font_size` must be between 8 and 64.
- `general.notification_command` may be empty; otherwise its first token must be an absolute executable file. Custom command execution validates this again at runtime.

Ghostty compatibility parses `~/.config/ghostty/config` and theme files under `~/.config/ghostty/themes/`. Theme names are allowlisted to prevent path traversal.

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

The socket server uses newline-delimited JSON. Parent directories are created with private permissions when needed and validated for owner-only access when using the default runtime path. The socket itself is owner-only and each request line is limited to 1 MiB.

Request/response shape:

```json
{"id":"1","method":"workspace.list","params":{}}
```

```json
{"id":"1","ok":true,"result":[{"id":"...","name":"..."}]}
```

```json
{"id":"1","ok":false,"error":{"code":"error","message":"..."}}
```

Implemented methods:

| Category | Methods |
| -------- | ------- |
| System | `system.ping` |
| Workspace | `workspace.list`, `workspace.create`, `workspace.select`, `workspace.close` |
| Surface | `surface.list`, `surface.split`, `surface.send_text`, `surface.read_screen`, `surface.close` |
| Notification | `notification.create`, `notification.list`, `notification.clear` |
| Worktree | `worktree.create`, `worktree.merge`, `worktree.remove` |
| Metadata | `metadata.set_status`, `metadata.list_status`, `metadata.clear_status`, `metadata.set_progress`, `metadata.clear_progress`, `metadata.log` |

Most workspace selectors prefer stable IDs, then worktree names or workspace names where supported. Renameable names can be ambiguous.

## Security Constraints

- Local desktop app; same-user processes are part of the local trust boundary.
- No remote network calls are made by ForkTTY itself.
- No `sh -c` for hooks or notification commands.
- Shell and notification executables must be absolute executable files.
- Socket request size is capped at 1 MiB.
- AppImage packaging rejects unsafe root icon values and refuses absolute root symlinks.
- CSP limits the WebView to local app content.
- Logs sanitize newlines before writing.

Residual risks:

- A same-user process can access local files and may be able to interact with user-owned runtime resources.
- User-authored shell hooks and notification commands execute with the user's privileges.
- A valid custom notification command can receive sensitive prompt text through environment variables.

## Test Strategy

Current automated coverage includes:

- Rust unit tests for config validation, session validation/quarantine, socket line limits and cwd validation, output scanner behavior, PTY cwd fallback, and worktree validation.
- Vitest tests for workspace store behavior, pane tree validation, session persistence payloads, socket handler behavior, notification dispatch, terminal registry, output buffering, terminal fonts, and Ghostty theme parsing.
- CI jobs for npm build/lint/test, Rust format/clippy/test, dependency audits, CodeQL, and Tauri validation/build surfaces.

Backlog validation:

- full Tauri GUI smoke tests;
- routine runtime/manual QA matrix for .deb and AppImage;
- refreshed screenshots after UI changes.
