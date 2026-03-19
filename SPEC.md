# ForkTTY — Multi-Agent Terminal for Linux

A lightweight, GPU-accelerated terminal designed for running multiple AI coding agents in parallel. Each agent gets its own isolated git worktree and terminal pane with smart notifications when attention is needed.

**Inspired by** [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) (macOS-only, Swift/AppKit). ForkTTY is a from-scratch Linux implementation using Tauri v2 + React + xterm.js.

## UI Reference

See `docs/assets/cmux-reference.png` for the visual target (screenshot of manaflow-ai/cmux on macOS).

### Layout

```
┌──────────────┬─────────────────────────────────────────────┐
│              │  Tab strip (surface tabs within workspace)   │
│   SIDEBAR    ├─────────────────────┬───────────────────────┤
│              │                     │                       │
│  ┌────────┐  │   Terminal pane 1   │   Terminal pane 3     │
│  │▸ ws-1  │  │   (Claude Code)     │   (Claude Code)       │
│  │  main  │  │                     │                       │
│  │  ~/proj│  ├─────────────────────┤                       │
│  └────────┘  │                     │                       │
│  ┌────────┐  │   Terminal pane 2   │                       │
│  │  ws-2  │  │   (Claude Code)     │                       │
│  │  feat-x│  │                     │                       │
│  │  ~/..  │  │                     │                       │
│  └────────┘  │                     │                       │
│              │                     │                       │
│  [+ New]     │                     │                       │
└──────────────┴─────────────────────┴───────────────────────┘
```

### Sidebar Workspace Entry

Each entry in the sidebar displays:
- **Workspace name** (bold, highlighted if selected with teal/blue background)
- **Git branch** name below the title
- **Working directory** path (truncated)
- **Status badge**: colored dot (idle=gray, running=green, waiting=amber, error=red)
- **Unread notification indicator**: blue dot + count when agent needs attention
- **Compact preview**: last few chars of latest agent output (optional)

### Visual Style
- Dark theme by default (Ghostty-compatible colors)
- Dense information layout, IDE-like feel
- Monospace font throughout (JetBrains Mono / Ghostty config)
- Focused pane has subtle highlighted border
- Sidebar entries have rounded corners, subtle hover states
- Minimal chrome: no unnecessary borders or decorations

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Frontend (React 19 + TypeScript + Vite)            │
│                                                     │
│  ┌─────────┐  ┌─────────────────────────────────┐   │
│  │ Sidebar  │  │  Pane Area (Allotment splits)   │   │
│  │          │  │  ┌──────────┐ ┌──────────┐      │   │
│  │ workspace│  │  │ xterm.js │ │ xterm.js │      │   │
│  │ list     │  │  │ (WebGL)  │ │ (canvas) │      │   │
│  │          │  │  └──────────┘ └──────────┘      │   │
│  │ + status │  │  ┌──────────┐                   │   │
│  │ + branch │  │  │ xterm.js │                   │   │
│  │ + notifs │  │  │ (canvas) │                   │   │
│  └─────────┘  │  └──────────┘                   │   │
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
| Terminal | @xterm/xterm 6.x + addons (fit, canvas, search, web-links) | Industry standard, used by VS Code |
| Terminal (alt) | ghostty-web (evaluate) | Ghostty VT100 parser as WASM, MIT, drop-in xterm.js API, ~400KB |
| Split panes | react-resizable-panels | 2M+ downloads/week, React 19 native, used by shadcn/ui |
| PTY | portable-pty 0.9 (Rust) | Battle-tested (powers WezTerm), sync API via spawn_blocking |
| PTY→Frontend | Tauri v2 Channels (`Channel<String>`) | Push-based streaming, ordered delivery, built for this use case |
| Git | git2-rs | Native git operations without shelling out |
| Notifications | notify-rust | XDG Desktop Notifications via D-Bus |
| State | Zustand 5.x | Lightweight, React 19 compatible, subscription-based |
| CLI/API | clap (CLI) + tokio + serde_json (socket) | Standard Rust ecosystem |
| Theme compat | Custom parser | Ghostty `key = value` format → xterm.js ITheme |

### Renderer Strategy

WebGL inside WebKitGTK has **known upstream bugs** (context lost, freezes — Tauri issues #6559, #8498).

- **Default**: Canvas renderer (`@xterm/addon-canvas`) — reliable on WebKitGTK, 2-3x faster than DOM
- **Experimental**: WebGL renderer — try on startup, catch failure, auto-fallback to canvas
- **Fallback**: DOM renderer — built-in, always works, slowest

```typescript
try {
  term.loadAddon(new WebglAddon());
} catch {
  term.loadAddon(new CanvasAddon()); // safe default
}
```

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
        └── Workspace (sidebar entry, 1:1 with git worktree)
              └── Pane (split region)
                    └── Surface (terminal instance)
```

### Workspace State

```typescript
interface Workspace {
  id: string;              // UUID
  name: string;            // User-editable label
  gitBranch: string;       // Current branch name
  workingDir: string;      // Worktree path
  worktreeDir: string;     // .worktrees/<name> path
  status: 'idle' | 'running' | 'waiting' | 'error';
  unreadNotifications: number;
  panes: PaneTree;         // Recursive split tree
  createdAt: string;       // ISO timestamp
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
  ptyId: number;           // Backend PTY handle
  title: string;           // From OSC 0/2 or shell
  shellState: 'idle' | 'typing' | 'executing';  // OSC 133
  lastActivity: string;    // ISO timestamp
}
```

## Environment Variables

Set in every spawned shell:

| Variable | Description |
|----------|-------------|
| `FORKTTY_WORKSPACE_ID` | Current workspace UUID |
| `FORKTTY_SURFACE_ID` | Current surface UUID |
| `FORKTTY_SOCKET_PATH` | Path to control socket |
| `TERM` | `xterm-256color` |

## Notification System

Three complementary detection methods, all running in the Rust backend:

### 1. OSC 133 Shell Integration (primary)

The shell emits escape sequences at prompt lifecycle boundaries:
- `OSC 133 ; A` — Prompt displayed (shell waiting for input)
- `OSC 133 ; B` — User typing command
- `OSC 133 ; C` — Command executed (Enter pressed)
- `OSC 133 ; D ; <exit_code>` — Command finished

When `A` fires and the workspace is not focused → notify.

### 2. Pattern Matching (Claude Code specific)

Scan last terminal line for known prompt patterns:
```
/^>\s*$/                     — Claude Code ">" prompt
/^❯\s*$/                    — Unicode prompt variant
/\? .+\(Y\/n\)/             — Confirmation prompt
/\? .+:/                    — Input prompt
/Do you want to proceed/    — Permission prompt
```

### 3. Idle Detection (fallback)

If no output for 2 seconds and workspace is unfocused → check if prompt is visible → notify.

### Notification Delivery

1. **In-app**: Blue dot on sidebar workspace entry, notification panel (Ctrl+Shift+I)
2. **Desktop**: XDG notification via D-Bus (notify-rust)
3. **Custom command**: User-configurable shell command with env vars `$FORKTTY_NOTIFICATION_TITLE`, `$FORKTTY_NOTIFICATION_BODY`

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
| Workspace | `workspace.create` | New workspace + worktree |
| Workspace | `workspace.select` | Focus a workspace |
| Workspace | `workspace.close` | Close + optionally remove worktree |
| Surface | `surface.list` | List surfaces in workspace |
| Surface | `surface.split` | Split pane horizontally or vertically |
| Surface | `surface.send_text` | Send text to a terminal |
| Surface | `surface.close` | Close a surface |
| Notification | `notification.create` | Create notification for a workspace |
| Notification | `notification.list` | List pending notifications |
| Notification | `notification.clear` | Clear notifications |

## CLI

```bash
forktty new <name>              # New workspace + worktree + launch agent
forktty ls                      # List workspaces
forktty select <name>           # Focus workspace
forktty split [right|down]      # Split current pane
forktty send <surface> "text"   # Send text to surface
forktty notify --title "X"      # Send notification
forktty merge [name]            # Merge worktree branch
forktty rm [name]               # Remove workspace + worktree
forktty config                  # View/set config
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
| Close workspace | Ctrl+Shift+W |
| Jump to workspace 1-9 | Ctrl+1..9 |
| Split right | Ctrl+D |
| Split down | Ctrl+Shift+D |
| Navigate panes | Alt+Arrow |
| New surface (tab) | Ctrl+T |
| Close surface | Ctrl+W |
| Notification panel | Ctrl+Shift+I |
| Jump to unread | Ctrl+Shift+U |
| Find in terminal | Ctrl+F |
| Command palette | Ctrl+Shift+P |
| Settings | Ctrl+, |

## Performance Guidelines

- **WebGL renderer** only on the focused terminal pane; canvas renderer on visible-but-unfocused panes. Browser limit: ~8-16 WebGL contexts.
- **ResizeObserver** on each terminal container → `fitAddon.fit()`
- **Output scanning** happens in Rust backend before forwarding to frontend — zero overhead on the render thread.
- **Flow control**: Watermark-based backpressure when PTY output exceeds threshold (prevents UI freeze on `cat /dev/urandom`).

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
sidebar_width = 250
sidebar_position = "left"            # "left" or "right"

[notifications]
desktop = true                       # XDG desktop notifications
sound = true                         # Notification sound
idle_threshold_ms = 2000             # Idle detection timeout
```
