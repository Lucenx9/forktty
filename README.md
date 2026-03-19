<div align="center">

# ForkTTY

**A multi-agent terminal for Linux — split panes, isolated worktrees, smart notifications.**

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/Lucenx9/forktty/ci.yml?branch=main)](https://github.com/Lucenx9/forktty/actions)

<!-- TODO: Replace with actual screenshot/GIF after first visual test -->
<!-- ![ForkTTY screenshot](docs/assets/screenshot.png) -->

</div>

---

Run multiple AI coding agents in parallel without losing track. Each agent gets its own git worktree and terminal pane. When one needs your attention, you'll know.

## Features

- **Split panes** — horizontal and vertical splits, resize with drag, navigate with `Alt+Arrow`
- **Workspaces** — each workspace is a named session with its own pane layout, visible in the sidebar
- **Git worktree isolation** — every workspace gets an isolated worktree and branch, no conflicts between agents
- **Smart notifications** — detects when an agent is waiting for input (OSC 133, pattern matching, idle detection) and alerts you via sidebar badge + desktop notification
- **Ghostty theme compatible** — reads your existing `~/.config/ghostty/config` for colors and fonts
- **Scriptable** — Unix socket API (JSON-RPC) and CLI for automation
- **Lightweight** — Tauri v2, not Electron. ~30MB RAM, ~10MB binary

## Quick Start

### Dependencies (Fedora)

```bash
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel
```

### Dependencies (Debian/Ubuntu)

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential libxdo-dev \
  libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

### Build & Run

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
npm install
cargo tauri dev
```

> Requires [Rust](https://rustup.rs/) and [Node.js 20+](https://nodejs.org/).

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| New workspace | `Ctrl+N` |
| Close workspace | `Ctrl+Shift+W` |
| Jump to workspace 1-9 | `Ctrl+1..9` |
| Split right | `Ctrl+D` |
| Split down | `Ctrl+Shift+D` |
| Navigate panes | `Alt+Arrow` |
| Close pane | `Ctrl+W` |

## Why ForkTTY?

Running 5+ AI agents on the same repo means juggling terminals, worrying about file conflicts, and constantly checking which agent needs input. Existing terminal multiplexers weren't built for this — they split screens but don't isolate code or notify you when an agent is waiting.

ForkTTY gives each agent its own git worktree, watches for prompts, and tells you when to act. One window, zero conflicts.

## Architecture

```
Frontend (React + TypeScript + Vite)
  ├── react-resizable-panels    Split pane layout
  ├── @xterm/xterm              Terminal rendering (canvas)
  └── Zustand                   State management

Tauri v2 IPC (Channels for streaming, invoke for commands)

Backend (Rust)
  ├── portable-pty              PTY management
  ├── git2                      Worktree lifecycle
  ├── notify-rust               Desktop notifications
  └── tokio                     Socket API server
```

## Roadmap

| Phase | Feature | Status |
|-------|---------|--------|
| 1 | Terminal + PTY | Done |
| 2 | Multi-pane splits | Done |
| 3 | Sidebar + workspaces | Done |
| 4 | Git worktree isolation | In progress |
| 5 | Smart notifications | Planned |
| 6 | Socket API + CLI | Planned |
| 7 | Theming + config | Planned |
| 8 | Polish + packaging | Planned |

See [ROADMAP.md](ROADMAP.md) for detailed task lists and acceptance criteria.

## CLI (coming in Phase 6)

```bash
forktty new feature-x           # New workspace + worktree + agent
forktty ls                      # List workspaces
forktty merge feature-x         # Merge worktree branch
forktty rm feature-x            # Cleanup
forktty notify --title "Done"   # Send notification
```

## Inspiration

Built from scratch for Linux, inspired by [cmux](https://github.com/manaflow-ai/cmux) (macOS-only, Swift/AppKit). See [SPEC.md](SPEC.md) for the full technical specification.

## Contributing

ForkTTY is in active early development. Issues, feature requests, and PRs are welcome.

```bash
# Development workflow
cargo tauri dev          # Run in dev mode
cargo clippy             # Lint Rust
npm run build            # Check TypeScript
npx prettier --check src # Check formatting
```

## License

MIT
