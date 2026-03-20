# ForkTTY — Multi-Agent Terminal for Linux

A lightweight Linux terminal multiplexer designed for running multiple AI coding agents in parallel. Each agent gets its own workspace and terminal panes, with optional git worktree isolation and smart notifications when attention is needed.

**Inspired by** [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) (macOS-only, Swift/AppKit). ForkTTY is a from-scratch Linux implementation using Tauri v2 + React + xterm.js.

## UI Reference

See `docs/assets/cmux-reference.png` for the visual target (screenshot of manaflow-ai/cmux on macOS).

### Layout

``` 
┌──────────────────┬──────────────────────────────────────────┐
│                  │  Pane toolbar                            │
│   SIDEBAR        ├──────────────────────┬───────────────────┤
│                  │                      │                   │
│  Workspace 1     │   Terminal pane 1    │   Terminal pane 3 │
│  main            │                      │                   │
│  ~/project       ├──────────────────────┤                   │
│                  │   Terminal pane 2    │                   │
│  Workspace 2     │                      │                   │
│  feature-x       │                      │                   │
│  ~/project/.wt   │                      │                   │
│                  │                      │                   │
│  Help / Shortcuts│                      │                   │
└──────────────────┴──────────────────────┴───────────────────┘
```

### Sidebar Workspace Entry

Each entry in the sidebar displays:
- **Workspace name** (bold, highlighted when selected)
- **Git branch** name below the title
- **Working directory** path (truncated)
- **Worktree status badge** when the workspace is backed by a git worktree
- **Unread notification indicator**: badge/count when agent needs attention
- **Compact preview** of the latest notification text for inactive workspaces
- **Workspace metadata**: optional status pills, progress rows, and recent log snippets
- **Reorder grip + close affordance** revealed on hover/active state

### Visual Style
- Dark theme by default with Ghostty-compatible colors and Catppuccin Mocha fallback
- Dense information layout, IDE-like feel
- UI chrome uses a proportional UI font; terminal content remains monospace
- Focused panes use restrained borders and tonal shifts; inactive panes are slightly dimmed
- Sidebar entries use an active rail, subtle hover states, and low-noise controls
- Minimal chrome: overlays and menus share one elevated desktop material instead of heavy decorative effects

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Frontend (React 19 + TypeScript + Vite)            │
│                                                     │
│  ┌─────────┐  ┌─────────────────────────────────┐   │
│  │ Sidebar │  │ Pane Area (react-resizable-     │   │
│  │         │  │ panels recursive splits)        │   │
│  │ ws list │  │  ┌──────────┐ ┌──────────┐      │   │
│  │ + meta  │  │  │ xterm.js │ │ xterm.js │      │   │
│  │ + badge │  │  │ (canvas) │ │ (canvas) │      │   │
│  │ + help  │  │  └──────────┘ └──────────┘      │   │
│  └─────────┘  │  ┌──────────┐                   │   │
│               │  │ overlays │                   │   │
│               │  │ palette  │                   │   │
│               │  │ settings │                   │   │
│               │  └──────────┘                   │   │
│               └─────────────────────────────────┘   │
├─────────────────────────────────────────────────────┤
│  Tauri IPC Bridge                                   │
├─────────────────────────────────────────────────────┤
│  Backend (Rust)                                     │
│  ├── pty_manager     — portable-pty, spawn/resize   │
│  ├── output_scanner  — OSC 133 + pattern matching   │
│  ├── worktree_mgr    — git2-rs worktree lifecycle   │
│  ├── notification    — notify-rust + D-Bus          │
│  ├── socket_api      — Unix socket, JSON-RPC        │
│  ├── config          — Ghostty theme parser         │
│  └── cli             — clap-based CLI client        │
└─────────────────────────────────────────────────────┘
```

## Tech Stack

| Layer | Technology | Why |
|-------|-----------|-----|
| Shell | Tauri v2 | ~30MB RAM, native Linux packaging, Rust backend |
| Frontend | React 19 + TypeScript + Vite | Fast iteration, huge ecosystem |
| Terminal | @xterm/xterm 5.x + addons (fit, canvas, search) | Industry-standard terminal component with a stable canvas renderer |
| Split panes | react-resizable-panels | 2M+ downloads/week, React 19 native, used by shadcn/ui |
| PTY | portable-pty 0.9 (Rust) | Battle-tested (powers WezTerm), sync API via spawn_blocking |
| PTY→Frontend | Tauri v2 Channels (`Channel<String>`) | Push-based streaming, ordered delivery, built for this use case |
| Git | git2-rs | Native git operations without shelling out |
| Notifications | notify-rust | XDG Desktop Notifications via D-Bus |
| State | Zustand 5.x | Lightweight, React 19 compatible, subscription-based |
| CLI/API | clap (CLI) + tokio + serde_json (socket) | Standard Rust ecosystem |
| Theme compat | Custom parser | Ghostty `key = value` format → xterm.js ITheme |

### Renderer Strategy

WebGL inside WebKitGTK has **known upstream bugs** (context lost, freezes — Tauri issues #6559, #8498), so the current build standardizes on canvas rendering.

- **Default**: Canvas renderer (`@xterm/addon-canvas`) — reliable on WebKitGTK and faster than DOM
- **Fallback**: DOM renderer — built-in and available if needed in the future

### PTY Data Flow

```
[Shell/Agent] → PTY → portable-pty reader (blocking, spawn_blocking)
                         → output_scanner (OSC 133, patterns)
                         → Channel<String>.send()
                         → Tauri IPC
                         → Channel.onmessage → xterm.js term.write()
```

Input: `xterm.js term.onData → invoke('pty_write') → portable-pty writer`
Resize: `FitAddon + ResizeObserver → invoke('pty_resize') → master.resize()`

### Competitive Landscape

| Project | Stack | License | Notes |
|---------|-------|---------|-------|
| [cmux](https://github.com/manaflow-ai/cmux) | Swift/AppKit | AGPL-3.0 | macOS only, our UI reference |
| [Coder Mux](https://github.com/coder/mux) | Electron + ghostty-web | AGPL-3.0 | Cross-platform, closest competitor |
| [BridgeSpace](https://bridgemind.ai) | Tauri v2 + React | Proprietary | Closed source, proves Tauri arch works |
| [agtx](https://github.com/fynnfluegge/agtx) | Rust CLI + tmux | Open source | Kanban + spec-driven workflows |
| [agent-deck](https://github.com/asheshgoplani/agent-deck) | Go + Bubble Tea TUI | MIT | Conductor pattern, MCP pooling |
| [dmux](https://github.com/standardagents/dmux) | Bash + tmux | MIT | Simple, composable, 11+ agent CLIs |

## Data Model

### Hierarchy

```
App
  └── Window (OS window)
        └── Workspace (sidebar entry, optionally worktree-backed)
              └── Pane (split region)
                    └── Surface (terminal instance)
```

### Workspace State

```typescript
interface Workspace {
  id: string;                   // UUID
  name: string;                 // User-editable label
  root: PaneTree;               // Recursive split tree
  surfaces: Record<string, Surface>;
  focusedPaneId: string;
  workingDir: string;           // Current cwd for the workspace
  gitBranch: string;            // Current branch name
  worktreeDir: string;          // Empty string for plain workspaces
  worktreeName: string;         // Empty string for plain workspaces
  worktreeStatus: string;       // clean / dirty / conflicts / error
  unreadCount: number;
  lastNotificationText: string;
  createdAt: string;            // ISO timestamp
}
```

### Pane Tree (recursive splits)

```typescript
type PaneTree =
  | { type: 'leaf'; surfaceId: string }
  | { type: 'horizontal'; children: PaneTree[]; sizes: number[] }
  | { type: 'vertical'; children: PaneTree[]; sizes: number[] };
```

### Surface (terminal instance)

```typescript
interface Surface {
  id: string;              // UUID
  ptyId: number | null;    // Backend PTY handle
  title: string;           // From OSC 0/2 or shell
  hasUnreadNotification: boolean;
}
```

Workspace activity timestamps are tracked outside Zustand to avoid re-render churn in the main UI store.

## Environment Variables

Set in every spawned shell:

| Variable | Description |
|----------|-------------|
| `FORKTTY_WORKSPACE_ID` | Current workspace UUID |
| `FORKTTY_SURFACE_ID` | Current surface UUID |
| `FORKTTY_SOCKET_PATH` | Path to control socket |
| `TERM` | `xterm-256color` |

## Notification System

Notification signals come from the Rust backend scanner and are surfaced in the frontend workspace store.

### 1. Prompt detection (OSC 133 + prompt patterns)

The shell emits escape sequences at prompt lifecycle boundaries:
- `OSC 133 ; A` — Prompt displayed (shell waiting for input)
- `OSC 133 ; B` — User typing command
- `OSC 133 ; C` — Command executed (Enter pressed)
- `OSC 133 ; D ; <exit_code>` — Command finished

The backend also scans terminal output for known Claude-style prompt patterns:
```
/^>\s*$/                     — Claude Code ">" prompt
/^❯\s*$/                    — Unicode prompt variant
/\? .+\(Y\/n\)/             — Confirmation prompt
/\? .+:/                    — Input prompt
/Do you want to proceed/    — Permission prompt
```

When a prompt is detected and the workspace is not focused, the frontend creates a `Prompt waiting` notification.

### 2. Explicit OSC notifications

The backend also forwards explicit terminal notification sequences:
- `OSC 9`
- `OSC 99`
- `OSC 777;notify;...`

### Notification Delivery

1. **In-app**: unread badge on the sidebar workspace entry, inline preview text, notification panel (`Ctrl+Shift+I`), unread pane ring
2. **Desktop**: XDG notification via D-Bus (notify-rust), when enabled
3. **Custom command**: user-configurable external command via `notification_command`

### Noise Control

- Switching to a workspace marks it read
- Prompt notifications are suppressed briefly after a workspace switch to avoid resize/redraw false positives
- Identical notifications are deduplicated for a short window
- Repeated prompt notifications are skipped while a workspace is already unread

## Socket API (JSON-RPC over Unix Domain Socket)

Socket path: `/tmp/forktty.sock` (override: `FORKTTY_SOCKET_PATH`)

Protocol: newline-delimited JSON

```jsonc
// Request
{"id": "1", "method": "workspace.list", "params": {}}

// Success
{"id": "1", "ok": true, "result": [{"id": "...", "name": "...", ...}]}

// Error
{"id": "1", "ok": false, "error": {"code": "not_found", "message": "..."}}
```

### MVP Methods

| Category | Method | Description |
|----------|--------|-------------|
| System | `system.ping` | Health check |
| Workspace | `workspace.list` | List all workspaces |
| Workspace | `workspace.create` | Create a plain workspace, or a worktree-backed one when worktree fields are provided |
| Workspace | `workspace.select` | Focus a workspace |
| Workspace | `workspace.close` | Close a workspace |
| Surface | `surface.list` | List surfaces in workspace |
| Surface | `surface.split` | Split pane horizontally or vertically |
| Surface | `surface.send_text` | Send text to a terminal or PTY |
| Surface | `surface.read_screen` | Read terminal screen contents |
| Surface | `surface.close` | Close a surface |
| Notification | `notification.create` | Create notification for a workspace |
| Notification | `notification.list` | List pending notifications |
| Notification | `notification.clear` | Clear notifications |
| Worktree | `worktree.create` | Create a git worktree and matching workspace |
| Worktree | `worktree.merge` | Merge a worktree branch |
| Worktree | `worktree.remove` | Remove a worktree and close its workspace |
| Metadata | `metadata.set_status` | Add/update a sidebar status pill |
| Metadata | `metadata.list_status` | List status pills |
| Metadata | `metadata.clear_status` | Clear one or all status pills |
| Metadata | `metadata.set_progress` | Add/update a sidebar progress row |
| Metadata | `metadata.clear_progress` | Clear one or all progress rows |
| Metadata | `metadata.log` | Append a sidebar log entry |

## CLI

```bash
forktty-cli new                 # New plain workspace
forktty-cli new feature-x       # New worktree-backed workspace
forktty-cli ls                  # List workspaces
forktty-cli select <name>       # Focus workspace
forktty-cli split [right|down]  # Split current pane
forktty-cli send <pty_id> "x"   # Send text to PTY
forktty-cli notify --title "X"  # Send notification
forktty-cli notifications       # List notifications
forktty-cli clear-notifications # Clear notifications
forktty-cli read-screen         # Read focused screen
forktty-cli merge feature-x     # Merge worktree branch
forktty-cli rm feature-x        # Remove worktree + close workspace
```

The CLI is a thin JSON-RPC client that connects to the socket.

## Ghostty Theme Compatibility

Parse `~/.config/ghostty/config` and `~/.config/ghostty/themes/`:

```
# Ghostty format         →  xterm.js mapping
background = #303446     →  theme.background
foreground = #c6d0f5     →  theme.foreground
cursor-color = #f2d5cf   →  theme.cursor
palette = 0=#51576d      →  theme.black
palette = 1=#e78284      →  theme.red
...
font-family = JetBrains Mono  →  terminal.options.fontFamily
font-size = 14           →  terminal.options.fontSize
```

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| New workspace | Ctrl+N |
| New worktree workspace | Ctrl+Shift+N |
| Close workspace | Ctrl+Shift+W |
| Jump to workspace 1-9 | Ctrl+1..9 |
| Split right | Ctrl+D |
| Split down | Ctrl+Shift+D |
| Navigate panes | Alt+Arrow |
| Close pane | Ctrl+W |
| Notification panel | Ctrl+Shift+I |
| Jump to unread | Ctrl+Shift+U |
| Find in terminal | Ctrl+F |
| Copy selection | Ctrl+Shift+C |
| Command palette | Ctrl+Shift+P |
| Settings | Ctrl+, |

## Performance Guidelines

- **Canvas renderer** is the default for all terminal panes in the current build.
- **ResizeObserver** on each terminal container → `fitAddon.fit()`
- **Output scanning** happens in Rust backend before forwarding to frontend — zero overhead on the render thread.
- **Inactive workspaces stay mounted but hidden** so PTYs and terminal buffers remain alive across workspace switches.

## Configuration

File: `~/.config/forktty/config.toml`

```toml
[general]
theme = "ghostty"                    # "ghostty" reads Ghostty config, or custom theme name
shell = "/bin/bash"                  # Default shell
worktree_layout = "nested"           # "nested" (.worktrees/), "sibling", "outer-nested"
notification_command = ""            # Custom command, empty = disabled

[appearance]
font_family = "JetBrains Mono"      # Override, or read from Ghostty
font_size = 14
sidebar_position = "left"            # "left" or "right"

[notifications]
desktop = true                       # XDG desktop notifications
sound = true                         # Notification sound
idle_threshold_ms = 2000             # Reserved for notification heuristics
```
