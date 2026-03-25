# ForkTTY Roadmap

## Phase 1 — Skeleton (MVP Terminal)

**Goal**: A Tauri app that opens, shows a single terminal pane, and you can type in it.

### Tasks

- [x] 1.1 — Scaffold Tauri v2 project with React + TypeScript + Vite frontend
- [x] 1.2 — Add `portable-pty` to Rust backend, implement `pty_spawn`, `pty_write`, `pty_resize` Tauri commands
- [x] 1.3 — Frontend: mount single xterm.js instance, connect to backend via Tauri Channel for output streaming
- [x] 1.4 — Wire bidirectional data: keystrokes → invoke('pty_write'), PTY output → Channel.onmessage → xterm.js
- [x] 1.5 — Implement terminal resize (FitAddon + ResizeObserver → invoke('pty_resize'))
- [x] 1.6 — Load Canvas addon (default), try WebGL with catch-and-fallback (WebKitGTK has known WebGL bugs)
- [x] 1.7 — Basic window chrome: titlebar, min/max/close

### Acceptance
- Launch app, get a working bash shell in the terminal
- Resize window, terminal reflows correctly
- `htop`, `vim`, `less` all render correctly

---

## Phase 2 — Multi-Pane + Tabs

**Goal**: Split terminals horizontally and vertically, navigate between them.

### Tasks

- [x] 2.1 — Install react-resizable-panels, implement recursive `PaneTree` component
- [x] 2.2 — Tauri commands: `pty_spawn` returns a `surface_id`, manage multiple PTYs
- [x] 2.3 — Split right (Ctrl+D): add leaf to current pane's parent as horizontal split
- [x] 2.4 — Split down (Ctrl+Shift+D): add leaf as vertical split
- [x] 2.5 — Pane focus management: Alt+Arrow to navigate, visual focus indicator (border highlight)
- [x] 2.6 — Close pane (Ctrl+W): kill PTY, remove from tree, rebalance layout
- [x] 2.7 — Canvas renderer on all panes (WebGL optional/experimental, try-catch per pane)

### Acceptance
- Split into 4 panes (2x2), each running independent shell
- Navigate between panes with Alt+Arrow
- Close a pane, remaining panes fill the space
- No WebGL context errors with 6+ panes

---

## Phase 3 — Sidebar + Workspaces

**Goal**: Left sidebar listing workspaces, each workspace is a named group of panes.

### Tasks

- [x] 3.1 — Sidebar component: list of workspaces, click to switch
- [x] 3.2 — Workspace state management (Zustand): create, switch, close, rename
- [x] 3.3 — New workspace (Ctrl+N): spawns fresh pane, user names it
- [x] 3.4 — Workspace metadata display: name, current git branch (read via `git rev-parse`), working directory
- [x] 3.5 — Workspace status indicator: colored dot (idle=gray, running=green, waiting=yellow, error=red)
- [x] 3.6 — Close workspace (Ctrl+Shift+W): kill all PTYs in workspace, confirm if multiple
- [x] 3.7 — Ctrl+1..9 to jump to workspace by position
- [x] 3.8 — Sidebar resizable (drag handle)

### Acceptance
- Create 3 workspaces, switch between them, each preserves its own pane layout
- Sidebar shows branch name and working directory per workspace
- Close a workspace, its PTYs are killed

---

## Phase 4 — Git Worktree Integration

**Goal**: Each workspace optionally backed by an isolated git worktree.

### Tasks

- [x] 4.1 — Add `git2` crate to backend, implement worktree create/list/remove
- [x] 4.2 — `forktty new <name>`: creates worktree at `.worktrees/<name>`, creates branch, spawns workspace with cwd in worktree
- [x] 4.3 — `forktty merge [name]`: merge worktree branch into current branch of main checkout
- [x] 4.4 — `forktty rm [name]`: remove worktree + delete branch + close workspace
- [x] 4.5 — Setup hook support: if `.forktty/setup` exists in repo, run it after worktree creation
- [x] 4.6 — Teardown hook: if `.forktty/teardown` exists, run before worktree removal
- [x] 4.7 — Worktree layout config: nested (`.worktrees/`), sibling, outer-nested
- [x] 4.8 — Sidebar shows worktree status (clean/dirty/conflicts)

### Acceptance
- `forktty new feature-x` creates worktree and opens workspace inside it
- Work in worktree, commit, then `forktty merge feature-x` merges into main
- `forktty rm feature-x` cleans up everything
- Setup hook runs `npm install` (or equivalent) on worktree creation

---

## Phase 5 — Notification System

**Goal**: Know when an agent needs your attention without watching every pane.

### Tasks

- [x] 5.1 — Output scanner in Rust: intercept PTY output, parse OSC 133 sequences (A/B/C/D)
- [x] 5.2 — Pattern matcher: regex scan last terminal line for Claude Code prompt patterns
- [x] 5.3 — Idle detector: timer resets on each PTY output, fires after threshold
- [x] 5.4 — Notification engine: when trigger fires and workspace is unfocused, create notification
- [x] 5.5 — In-app: blue dot on sidebar workspace entry, unread count badge
- [x] 5.6 — Desktop notification via notify-rust (XDG/D-Bus)
- [x] 5.7 — Notification panel (Ctrl+Shift+I): list of all notifications, click to jump to workspace
- [x] 5.8 — Jump to latest unread (Ctrl+Shift+U)
- [x] 5.9 — Mark as read when workspace is focused
- [x] 5.10 — Custom notification command support (config.toml `notification_command`)

### Acceptance
- Start Claude Code in workspace 1, switch to workspace 2
- When Claude Code shows prompt waiting for input, blue dot appears on workspace 1
- Desktop notification pops up
- Click notification → workspace 1 is focused

---

## Phase 6 — Socket API + CLI

**Goal**: Scriptable control from outside the app.

### Tasks

- [x] 6.1 — Unix domain socket server in Rust (tokio), JSON-RPC protocol
- [x] 6.2 — Implement MVP methods: system.ping, workspace.*, surface.*, notification.*
- [x] 6.3 — CLI binary (`forktty`): clap-based, connects to socket, sends JSON-RPC
- [x] 6.4 — Set env vars in spawned shells: `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, `FORKTTY_SOCKET_PATH`
- [x] 6.5 — `forktty send <surface> "text"`: send keystrokes to a specific terminal
- [x] 6.6 — `forktty read-screen [surface]`: dump current terminal buffer content

### Acceptance
- `forktty ls` from another terminal lists workspaces
- `forktty new test -p "fix the bug"` creates workspace and sends prompt to Claude
- `forktty notify --title "Done"` triggers notification in app
- Scripts can orchestrate multiple agents via CLI

---

## Phase 7 — Theming + Config

**Goal**: Look good, respect user's existing Ghostty config.

### Tasks

- [x] 7.1 — Ghostty config parser: read `~/.config/ghostty/config`, extract colors/font/theme
- [x] 7.2 — Ghostty theme file parser: read `~/.config/ghostty/themes/<name>`
- [x] 7.3 — Map Ghostty palette to xterm.js ITheme
- [x] 7.4 — Config file: `~/.config/forktty/config.toml` with TOML parser (toml crate)
- [x] 7.5 — Settings UI (Ctrl+,): appearance, notifications, shell, worktree layout
- [x] 7.6 — Dark/light mode support, follow system preference
- [x] 7.7 — Sidebar theming (respect background/foreground from theme)

### Acceptance
- User with Ghostty Catppuccin theme installed: ForkTTY automatically picks up same colors
- Change font size in config → terminals update
- Settings panel allows changing notification preferences without editing TOML

---

## Phase 8 — Polish + Release

**Goal**: Stable enough for daily use.

### Tasks

- [x] 8.1 — Session persistence: save workspace layout + restore on restart (no scrollback, just structure)
- [x] 8.2 — Command palette (Ctrl+Shift+P): fuzzy search all commands
- [x] 8.3 — Find in terminal (Ctrl+F) using xterm.js SearchAddon
- [x] 8.4 — Copy mode (Shift+click select, Ctrl+Shift+C to copy)
- [x] 8.5 — Proper error handling: PTY spawn failure, git errors, socket errors
- [x] 8.6 — Logging: structured logs to `~/.local/share/forktty/logs/`
- [x] 8.7 — Package as .deb and AppImage via Tauri bundler
- [x] 8.8 — README with install instructions, screenshots, usage guide
- [x] 8.9 — License: switch project to AGPL-3.0

### Acceptance
- Install from .deb on Debian 13
- Use daily for 1 week with Claude Code agents
- No crashes, no memory leaks, notifications work reliably

---

## Phase 9 — Feature Parity (cmux)

**Goal**: Match cmux functionality with additional notification and UI features.

### Tasks

- [x] 9.1 — OSC 9/99/777 notification parsing in output scanner
- [x] 9.2 — Notification ring: blue glowing border on panes with unread notifications
- [x] 9.3 — Sidebar notification preview: latest notification text inline
- [x] 9.4 — Window title badge: unread count in document.title
- [x] 9.5 — Auto-reorder: move workspace to top of sidebar on notification
- [x] 9.6 — `forktty read-screen`: dump terminal buffer via socket API + CLI
- [x] 9.7 — Focus flash: brief blue flash on pane focus change
- [x] 9.8 — Workspace drag-and-drop reorder in sidebar
- [x] 9.9 — System tray icon with unread count tooltip

### Acceptance
- `echo -e '\033]9;Hello\007'` triggers notification with blue pane ring
- Drag workspace entries in sidebar to reorder
- Tray icon shows unread count, click opens window
- `forktty read-screen` returns terminal content

---

## Future (Post-MVP)

These are explicitly **out of scope** for the initial build:

- [ ] Built-in browser pane (WebKitGTK embed)
- [ ] SSH remote workspaces
- [ ] MCP server integration
- [ ] Multi-window support
- [ ] Tab strip within panes (multiple surfaces per pane)
- [ ] Agent orchestration API (primary agent spawning sub-agents)
- [ ] Scrollback persistence across restarts
- [ ] Plugin system
- [ ] Auto-update mechanism
