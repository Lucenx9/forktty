<div align="center">

<img src="src-tauri/icons/128x128.png" alt="ForkTTY" width="80" />

# ForkTTY

**Multi-agent terminal for Linux — split panes, isolated worktrees, smart notifications.**

Run multiple AI coding agents in parallel. Each gets its own git worktree. When one needs your attention, you'll know.

[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/Lucenx9/forktty/ci.yml?branch=main)](https://github.com/Lucenx9/forktty/actions)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://rustup.rs/)
[![Tauri](https://img.shields.io/badge/tauri-v2-blue.svg)](https://v2.tauri.app/)

<!-- TODO: Add screenshot/GIF after first visual test -->
<!-- ![ForkTTY screenshot](docs/assets/screenshot.png) -->

</div>

> **Status**: Early development (v0.1.0). Usable for daily work on Linux, but expect rough edges. Not yet packaged for distribution — build from source.

## Why ForkTTY?

Running 5+ AI agents on the same repo means juggling terminals, worrying about file conflicts, and constantly checking which agent needs input. tmux splits screens but doesn't isolate code or notify you when an agent is waiting.

ForkTTY gives each agent its own git worktree, watches for prompts, and tells you when to act. **One window, zero conflicts.**

## Features

- **Split panes** — horizontal/vertical splits, resize with drag, navigate with `Alt+Arrow`
- **Workspaces** — named sessions with their own pane layouts, visible in a resizable sidebar
- **Git worktree isolation** — each workspace gets an isolated worktree and branch, no conflicts between agents
- **Smart notifications** — detects when an agent waits for input (OSC 133, prompt patterns, OSC 9/99/777) and alerts via sidebar badge + desktop notification
- **Ghostty theme compatible** — reads `~/.config/ghostty/config` for colors and fonts automatically
- **Scriptable** — Unix socket API (JSON-RPC) at `$XDG_RUNTIME_DIR/forktty.sock`
- **Command palette** — `Ctrl+Shift+P` to fuzzy-search all actions
- **Session persistence** — workspace layout restored on restart
- **System tray** — unread count tooltip, click to focus window
- **Find in terminal** — `Ctrl+F` via xterm.js SearchAddon
- **Workspace metadata** — status pills, progress bars, and log entries via CLI/API
- **Privacy-first** — zero telemetry, zero network connections, all data stays local ([PRIVACY.md](PRIVACY.md))

## Quick Start

### Prerequisites

- [Rust 1.88+](https://rustup.rs/)
- [Node.js 20+](https://nodejs.org/)
- System libraries (see below)

<details>
<summary><strong>Debian / Ubuntu</strong></summary>

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential \
  libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

</details>

<details>
<summary><strong>Fedora</strong></summary>

```bash
sudo dnf install webkit2gtk4.1-devel gtk3-devel libappindicator-gtk3-devel librsvg2-devel
```

</details>

### Build & Run

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
npm install
npm run tauri:dev
```

### Install from release build

```bash
npm run tauri:build
sudo dpkg -i src-tauri/target/release/bundle/deb/ForkTTY_*.deb
# Or use the AppImage directly from src-tauri/target/release/bundle/appimage/
```

## Keyboard Shortcuts

| Action | Shortcut |
|--------|----------|
| New workspace | `Ctrl+N` |
| New worktree workspace | `Ctrl+Shift+N` |
| Close workspace | `Ctrl+Shift+W` |
| Jump to workspace 1-9 | `Ctrl+1..9` |
| Split right | `Ctrl+D` |
| Split down | `Ctrl+Shift+D` |
| Navigate panes | `Alt+Arrow` |
| Close pane | `Ctrl+W` |
| Find in terminal | `Ctrl+F` |
| Copy selection | `Ctrl+Shift+C` |
| Command palette | `Ctrl+Shift+P` |
| Notification panel | `Ctrl+Shift+I` |
| Jump to unread | `Ctrl+Shift+U` |
| Settings | `Ctrl+,` |
| Zoom in/out/reset | `Ctrl+=` / `Ctrl+-` / `Ctrl+0` |

## Configuration

Config file: `~/.config/forktty/config.toml` — all fields are optional with sensible defaults.

```toml
# All values below are defaults — only add what you want to change.

[general]
# theme_source = "auto"       # "auto" detects from Ghostty; "builtin" uses Catppuccin Mocha
# shell = "/bin/zsh"          # default: $SHELL
# worktree_layout = "nested"  # "nested", "sibling", or "outer-nested"
# notification_command = "/usr/bin/notify-send"  # must be absolute path, empty = disabled

[appearance]
# font_family = "Fira Code"   # overrides Ghostty font if set
# font_size = 16              # overrides Ghostty size if set (default: 14)
# sidebar_position = "right"  # "left" (default) or "right"

[notifications]
# desktop = true              # enable/disable desktop notifications
# sound = true                # enable/disable notification sound
```

Ghostty users: ForkTTY reads `~/.config/ghostty/config` automatically for colors, fonts, and palette. Explicit `[appearance]` values override Ghostty.

If `notification_command` is set, ForkTTY exports `FORKTTY_NOTIFICATION_TITLE` and `FORKTTY_NOTIFICATION_BODY` as environment variables to that command.

## Architecture

```
Frontend (React 19 + TypeScript + Vite)
  ├── @xterm/xterm 6.x         Terminal rendering (built-in canvas renderer)
  ├── react-resizable-panels    Split pane layout
  └── Zustand 5.x              State management

Tauri v2 IPC (Channels for PTY streaming, invoke for commands)

Backend (Rust)
  ├── portable-pty              PTY management
  ├── git2                      Worktree lifecycle
  ├── output_scanner            OSC 133/9/99/777 parsing
  ├── notify-rust               Desktop notifications (D-Bus)
  ├── tokio                     Socket API server
  └── clap                      CLI client
```

## Security

- Unix socket restricted to owner (`0600`), 1 MiB request limit
- Shell and notification commands must be absolute paths
- All worktree paths canonicalized and verified within git working directory
- CSP restricts WebView to local content only
- No `sh -c` anywhere — all external commands use argv splitting

See [SECURITY.md](SECURITY.md) for vulnerability reporting and the full security model.

## Known Limitations

- Dark theme only (no light mode toggle; CSS has a minimal system-preference fallback)
- No idle detection for notifications (config field reserved but not active)
- `beforeunload` session save is fire-and-forget (async IPC may not complete)
- No flow control / backpressure on PTY output
- Linux only — no macOS or Windows support

## Contributing

ForkTTY is in active early development. Issues, feature requests, and PRs are welcome.

```bash
npm run tauri:dev                         # Dev mode
npm run tauri:build                       # Production build
npm run tauri:info                        # Check Tauri environment
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm run build && npx prettier --check src/
```

See [SPEC.md](SPEC.md) for architecture details and [ROADMAP.md](ROADMAP.md) for the implementation plan.

## Inspiration

Built from scratch for Linux, inspired by [cmux](https://github.com/manaflow-ai/cmux) (macOS-only, Swift/AppKit).

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`)
