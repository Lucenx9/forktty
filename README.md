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

## Why ForkTTY?

Running 5+ AI agents on the same repo means juggling terminals, worrying about file conflicts, and constantly checking which agent needs input. tmux splits screens but doesn't isolate code or notify you when an agent is waiting.

ForkTTY gives each agent its own git worktree, watches for prompts, and tells you when to act. **One window, zero conflicts.**

## Features

- **Split panes** — horizontal/vertical splits, resize with drag, navigate with `Alt+Arrow`
- **Workspaces** — named sessions with their own pane layouts, visible in a sidebar
- **Git worktree isolation** — each workspace gets an isolated worktree and branch, no conflicts between agents
- **Smart notifications** — detects when an agent waits for input (OSC 133, prompt patterns, OSC 9/99/777) and alerts via sidebar badge + desktop notification
- **Ghostty theme compatible** — reads `~/.config/ghostty/config` for colors and fonts automatically
- **Scriptable** — Unix socket API (JSON-RPC) and CLI for automation
- **Command palette** — `Ctrl+Shift+P` to fuzzy-search all actions
- **Session persistence** — workspace layout restored on restart
- **Lightweight** — Tauri v2, not Electron. ~30MB RAM, ~10MB binary
- **Privacy-first** — zero telemetry, zero network connections, all data stays local ([PRIVACY.md](PRIVACY.md))

## Quick Start

### Dependencies

<details>
<summary><strong>Debian / Ubuntu</strong></summary>

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential libxdo-dev \
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
cargo tauri dev
```

> Requires [Rust 1.88+](https://rustup.rs/) and [Node.js 20+](https://nodejs.org/).

### Install from release

```bash
cargo tauri build
sudo dpkg -i src-tauri/target/release/bundle/deb/forktty_*.deb
# Or use the AppImage directly
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

## CLI

Control ForkTTY from scripts or other terminals:

```bash
forktty-cli ls                      # List workspaces
forktty-cli new feature-x           # New worktree workspace
forktty-cli select feature-x        # Focus workspace
forktty-cli split right             # Split current pane
forktty-cli send <pty_id> "text"    # Send text to terminal
forktty-cli read-screen             # Read terminal content
forktty-cli merge feature-x         # Merge worktree branch
forktty-cli rm feature-x            # Remove worktree + close workspace
forktty-cli notify --title "Done"   # Send notification
```

## Configuration

Config file: `~/.config/forktty/config.toml`

```toml
[general]
theme = "ghostty"              # "ghostty" auto-detects, or "builtin"
shell = "/bin/bash"
worktree_layout = "nested"     # "nested", "sibling", or "outer-nested"

[appearance]
font_family = "JetBrains Mono"
font_size = 14
sidebar_position = "left"      # "left" or "right"

[notifications]
desktop = true
```

If you have a Ghostty config with a theme, ForkTTY picks up your colors and fonts automatically.

## Architecture

```
Frontend (React 19 + TypeScript + Vite)
  ├── @xterm/xterm (canvas)    Terminal rendering
  ├── react-resizable-panels   Split pane layout
  └── Zustand                  State management

Tauri v2 IPC (Channels for PTY streaming, invoke for commands)

Backend (Rust)
  ├── portable-pty             PTY management
  ├── git2                     Worktree lifecycle
  ├── output_scanner           OSC 133/9/99/777 parsing
  ├── notify-rust              Desktop notifications (D-Bus)
  ├── tokio                    Socket API server
  └── clap                     CLI client
```

## Security

- Unix socket restricted to owner (`0600`), 1 MiB request limit
- Shell and notification commands must be absolute paths
- All worktree paths canonicalized and verified within git working directory
- CSP restricts WebView to local content only
- No `sh -c` anywhere — all external commands use argv splitting

See [SECURITY.md](SECURITY.md) for vulnerability reporting and the full security model.

## Contributing

ForkTTY is in active early development. Issues, feature requests, and PRs are welcome.

```bash
cargo tauri dev                           # Dev mode
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
