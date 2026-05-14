<div align="center">

<img src="packaging/linux/icons/forktty.png" alt="ForkTTY" width="80" />

# ForkTTY

**Linux-native multi-agent terminal with a programmable local socket API, first-class git worktrees, and prompt-aware notifications.**

ForkTTY runs coding agents in isolated workspaces, exposes a user-local Unix socket for automation, and can place long-running tasks in dedicated git worktrees without tying the UI to one agent vendor.

[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/Lucenx9/forktty/ci.yml?branch=main)](https://github.com/Lucenx9/forktty/actions)
[![Release](https://img.shields.io/github/v/release/Lucenx9/forktty)](https://github.com/Lucenx9/forktty/releases/latest)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://rustup.rs/)
[![GTK4](https://img.shields.io/badge/GTK4%20%2B%20VTE-native-blue.svg)](docs/native-gtk-vte.md)

[Download Latest Release](https://github.com/Lucenx9/forktty/releases/latest)

</div>

> **Status**: Early development (v0.1.2). ForkTTY is Linux-only and the GTK/VTE runtime is now the primary implementation.

## Why ForkTTY

- **Agent-agnostic automation**: the same socket API and CLI flow work for Codex, Claude Code, Gemini CLI, shell scripts, and custom tools.
- **First-class worktree workflows**: create, attach, remove, and merge isolated worktree workspaces through native `git2` operations and optional `.forktty/setup` / `.forktty/teardown` hooks.
- **Native Linux terminal stack**: GTK4/libadwaita shell with embedded VTE terminals, split panes, session restore, notifications, command palette, settings, and quake mode.
- **Local-first posture**: no telemetry, no update checks, no external service dependency, owner-only Unix socket permissions, bounded request/session/config files, and argv-based command execution.

## Quick Start

### Requirements

- Linux
- [Rust 1.88+](https://rustup.rs/)
- Node.js 20+ only for the repo-local CLI helper/tests
- GTK4, libadwaita, VTE GTK4 development libraries

Debian / Ubuntu:

```bash
sudo apt install build-essential libssl-dev libgtk-4-dev libadwaita-1-dev libvte-2.91-gtk4-dev desktop-file-utils
```

Fedora:

```bash
sudo dnf install gcc gcc-c++ openssl-devel gtk4-devel libadwaita-devel vte291-gtk4-devel desktop-file-utils
```

Arch / CachyOS:

```bash
sudo pacman -S base-devel openssl gtk4 libadwaita vte4 desktop-file-utils
```

ForkTTY currently builds with VTE 0.76 or newer, matching Ubuntu 24.04 LTS and newer distro packages. For compositor-anchored quake/dropdown placement on Wayland, install `gtk4-layer-shell` as an optional runtime dependency.

### Build and Run

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
cargo run -p forktty-ui-gtk --features gtk-vte
```

Build the Debian package:

```bash
bash scripts/build-deb.sh
sudo dpkg -i target/packaging/deb/forktty_*.deb
```

The old Tauri/React/WebKit implementation has been removed from the primary tree. The supported desktop app is the native GTK4/libadwaita/VTE binary installed as `forktty`.

### First Run Basics

ForkTTY opens the current directory as the `main` workspace. Use the command palette for most navigation and pane actions:

- `Ctrl+Shift+P`: command palette
- `Ctrl+Shift+N`: new workspace
- `Ctrl+Shift+O`: open workspace
- `Ctrl+Shift+H`: split pane right
- `Ctrl+Shift+E`: split pane down
- `Ctrl+Alt+Left` / `Ctrl+Alt+Right`: focus previous/next pane
- `Ctrl+Shift+W`: close pane
- `Ctrl+B` or `F9`: toggle workspace sidebar
- `Ctrl+Shift+M`: notifications
- `F1`: keyboard shortcuts
- `Ctrl+,`: settings

### Container Development

If you do not want to install the build dependencies on the host, use the Ubuntu container wrapper:

```bash
./scripts/ubuntu-dev.sh
```

Useful one-off commands:

```bash
./scripts/ubuntu-dev.sh cargo test --workspace
./scripts/ubuntu-dev.sh cargo build -p forktty-ui-gtk --features gtk-vte
./scripts/ubuntu-build-deb.sh
```

## Socket CLI

ForkTTY ships a repo-local CLI wrapper over the Unix socket API:

```bash
./scripts/forktty.mjs ping
./scripts/forktty.mjs list
./scripts/forktty.mjs focus "Workspace 2"
./scripts/forktty.mjs surfaces
./scripts/forktty.mjs split-surface --axis vertical
./scripts/forktty.mjs send-text "cargo test\n"
./scripts/forktty.mjs worktree-status
./scripts/forktty.mjs notify --title "Input needed" --kind prompt "Blocked on test fixture"
./scripts/forktty.mjs set-status --key agent:codex --label Codex --value Running --color blue
./scripts/forktty.mjs set-progress --key build --label Build --value 42 --total 100
./scripts/forktty.mjs log --level warn "Waiting for reviewer input"
./scripts/forktty.mjs notifications
```

Spawned shells receive:

- `FORKTTY_WORKSPACE_ID`
- `FORKTTY_SURFACE_ID`
- `FORKTTY_SOCKET_PATH`

That lets hooks and scripts target the current workspace without extra flags.

## Agent Hooks

Install hook templates for Codex, Claude Code, and Gemini CLI:

```bash
./scripts/forktty.mjs hooks setup
./scripts/forktty.mjs hooks setup codex claude gemini
```

The installer merges commands into:

- Codex: `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json`
- Claude Code: `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json`
- Gemini CLI: `~/.gemini/settings.json`

Hooks report status, progress, logs, and prompt notifications through the same local socket pipeline.

## Features

- Native GTK4/libadwaita desktop shell with embedded VTE terminals.
- Recursive split panes, pane focus/close, command palette, settings dialog, notification panel, and workspace sidebar.
- Quake/dropdown mode through config and F12 where global shortcuts are supported.
- Direct Unix socket JSON-RPC server for workspace, surface, notification, worktree, and metadata automation.
- Git worktree create/attach/remove/merge/status with dirty-state protection and hook execution inside verified worktrees. Setup hooks are advisory; teardown hook failures or teardown-created dirty state block removal.
- Session restore for workspace order, active workspace, pane tree, focused surface, cwd, branch, and worktree metadata.
- Prompt-aware notifications from VTE shell integration signals, bounded visible prompt fallback, VTE bell, and hook/socket events.
- Bounded config/session/socket handling and local-only privacy defaults.

## Configuration

Config file: `~/.config/forktty/config.toml`. All fields are optional.

```toml
[general]
theme_source = "auto"
shell = "/bin/bash"
worktree_layout = "nested" # "nested", "sibling", or "outer-nested"
notification_command = ""

[appearance]
font_family = ""
font_size = 14
sidebar_position = "left" # "left" or "right"
sidebar_visible = true
terminal_renderer = "vte"
window_mode = "normal" # "normal" or "quake"

[notifications]
desktop = true
sound = true
```

`notification_command` is split with `shell_words`; ForkTTY does not use `sh -c`. The first token must be an absolute executable path, and notification title/body are passed through `FORKTTY_NOTIFICATION_TITLE` and `FORKTTY_NOTIFICATION_BODY`.

`terminal_renderer` is kept for config compatibility; the native GTK app uses VTE.

## Session Restore

GTK/VTE sessions are stored as:

```text
~/.local/share/forktty/session-v2.json
```

ForkTTY imports legacy `session.json` when present, but saves the native runtime as v2. Restore does not preserve running PTY processes or scrollback; new VTE terminals are spawned for restored panes. Corrupt or structurally invalid session files are quarantined.

## Security Summary

- Local Linux desktop threat model; same-user processes remain part of the local trust boundary.
- Unix socket defaults to `$XDG_RUNTIME_DIR/forktty.sock` with `/tmp/forktty-<uid>/forktty.sock` fallback and owner-only permissions.
- Socket request lines, config files, and session files are size bounded.
- Shell paths, hooks, and custom notification commands use validated argv execution, not shell pipelines.
- Worktree names, socket-provided repo paths, and hook locations are validated before mutation or execution.
- ForkTTY makes no telemetry, update-check, or external network calls.

See [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md).

## Known Limitations

- Linux only. There are no supported macOS or Windows builds.
- VTE 0.76+ is currently required by the native terminal integration.
- PTYs and scrollback are not persisted across restart.
- Byte-level OSC 9/99 parsing from the old PTY-owner path is not fully ported because VTE owns the child PTY.
- Quake global shortcuts and layer-shell placement depend on desktop/compositor support.
- Full theme customization, multi-window, persistent scrollback, and browser panes are backlog items.

## Contributing

Useful commands:

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --features gtk-vte -- -D warnings
cargo build -p forktty-ui-gtk --features gtk-vte
node --test scripts/forktty.test.mjs
desktop-file-validate packaging/linux/forktty.desktop
bash scripts/build-deb.sh
```

See [SPEC.md](SPEC.md), [ROADMAP.md](ROADMAP.md), and [docs/native-gtk-vte.md](docs/native-gtk-vte.md).

## Inspiration

Built from scratch for Linux, inspired by [cmux](https://github.com/manaflow-ai/cmux) and other multi-agent terminal workflows.

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`)
