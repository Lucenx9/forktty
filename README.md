<div align="center">

<img src="packaging/linux/icons/forktty.svg" alt="ForkTTY" width="80" />

# ForkTTY

**Linux-native workspace terminal with embedded Ghostty, a programmable local
socket API, first-class git worktrees, and prompt-aware notifications.**

ForkTTY runs shells and coding agents in isolated workspaces, keeps terminal
attention visible, exposes a user-local Unix socket for automation, and can
place work in dedicated git worktrees without tying the UI to one agent vendor.

[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue.svg)](LICENSE)
[![Build](https://img.shields.io/github/actions/workflow/status/Lucenx9/forktty/ci.yml?branch=main)](https://github.com/Lucenx9/forktty/actions)
[![Release](https://img.shields.io/github/v/release/Lucenx9/forktty?include_prereleases)](https://github.com/Lucenx9/forktty/releases/tag/v0.2.0-alpha.18)
[![Rust](https://img.shields.io/badge/rust-1.96%2B-orange.svg)](https://rustup.rs/)
[![GTK4](https://img.shields.io/badge/GTK4%20%2B%20Ghostty-native-blue.svg)](docs/native-gtk-ghostty.md)

[Website](https://forktty.dev/) ·
[Docs](https://forktty.dev/docs) ·
[Agent context](https://forktty.dev/llms.txt) ·
[Download v0.2.0-alpha.18 AppImage](https://github.com/Lucenx9/forktty/releases/download/v0.2.0-alpha.18/forktty-0.2.0-alpha.18-x86_64.AppImage)

</div>

> **Status**: Early alpha (v0.2.0-alpha.18). ForkTTY is Linux-only and the GTK/Ghostty runtime is now the primary implementation. The AppImage is the primary Linux download for this alpha; the Debian package remains available for Debian/Ubuntu users.

For the fastest local walkthrough, read [GETTING_STARTED.md](GETTING_STARTED.md).
For local quality targets, read [METRICS.md](METRICS.md).
For the complete user guide, read [forktty.dev/docs](https://forktty.dev/docs).
For agent-oriented retrieval, start with
[llms.txt](https://forktty.dev/llms.txt) or the single-file
[llms-full.txt](https://forktty.dev/llms-full.txt). This README stays focused
on the project overview, install paths, quick start, and contributor commands.

## Why ForkTTY

- **Process-agnostic automation**: the same socket and CLI primitives work for
  shells, editors, Codex, Claude Code, Pi, Antigravity CLI, OpenCode, and custom
  tools.
- **Attention without orchestration policy**: OSC and hook notifications,
  unread state, status/progress metadata, and the Agent HUD make terminal work
  visible while the process inside each pane owns its own coordination.
- **First-class worktree workflows**: create, attach, remove, and merge isolated worktree workspaces through native `git2` operations and optional `.forktty/setup` / `.forktty/teardown` hooks.
- **Native Linux terminal stack**: GTK4/libadwaita shell with embedded Ghostty-backed terminals, split panes, session restore, notifications, command palette, settings, and quake mode.
- **Local-first posture**: no crash reporting or product event tracking, an anonymous daily usage ping that can be disabled, owner-only Unix socket permissions, bounded request/session/config files, and argv-based command execution. Optional update checks hit GitHub Releases at most once per day and can also be disabled.

## Install

The fastest paths are the prebuilt artifacts from the
[v0.2.0-alpha.18 release](https://github.com/Lucenx9/forktty/releases/tag/v0.2.0-alpha.18).
Each release ships:

- `forktty-0.2.0-alpha.18-x86_64.AppImage` — recommended portable Linux package.
- `forktty-0.2.0-alpha.18-x86_64.AppImage.zsync` — AppImage delta-update metadata for external AppImage managers.
- `forktty_0.2.0.alpha.18_amd64.deb` — Debian/Ubuntu package.
- `SHA256SUMS` — checksums for release artifacts.

After downloading, verify checksums:

```bash
sha256sum -c SHA256SUMS
```

### AppImage

```bash
chmod +x forktty-0.2.0-alpha.18-x86_64.AppImage
./forktty-0.2.0-alpha.18-x86_64.AppImage
```

The AppImage always ships the vendored libghostty-vt library and prefers the
host's GUI stack: when the host provides GTK4, ForkTTY runs against the
system GTK4/libadwaita for native cursor themes, fontconfig, and portal
integration, and the bundled GTK copy is used only as a fallback on hosts
without GTK4. Set `FORKTTY_APPIMAGE_GTK_RUNTIME=bundled`, `host`, or `auto`
to force or debug that runtime choice. It depends on the host system for
glibc, the GSettings/GIO data tree, Wayland/X11 session services, fontconfig,
the
OpenGL/Vulkan/Mesa driver stack, and desktop notification services.
It is the primary downloadable artifact for alpha releases and works on
most modern distros that ship a recent glibc, but it should
still be tested on the target distro/desktop environment before being
relied on.

ForkTTY checks GitHub Releases for updates at most once per day by default.
AppImage installs can update in place after confirmation: ForkTTY downloads
the new AppImage and `SHA256SUMS`, verifies SHA256, then atomically replaces
the current file. Non-AppImage installs open the release page instead. AppImage
managers such as Gear Lever continue to work when they launch ForkTTY from a
stable writable AppImage path; if ForkTTY sees an extracted, read-only, or
otherwise unsafe path, it falls back to the release page and leaves the manager
in control.

ForkTTY defaults to the OpenGL GTK renderer (`GSK_RENDERER=ngl`). If the
AppImage launches but the GTK interface renders incorrectly, override it with a
different renderer from a terminal:

```bash
GSK_RENDERER=gl ./forktty-0.2.0-alpha.18-x86_64.AppImage
```

Use `GSK_RENDERER=cairo` only as a last resort: the software renderer is slower
and retains far more memory under heavy terminal redraws.

### Debian / Ubuntu (.deb)

The `.deb` targets Debian 13/Trixie or newer and Ubuntu 24.04 LTS or newer.
Debian 12/Bookworm is below the package baseline because it does not provide
libadwaita 1.4+.

```bash
sudo apt install ./forktty_0.2.0.alpha.18_amd64.deb
# or, if apt cannot read the file path directly:
sudo dpkg -i forktty_0.2.0.alpha.18_amd64.deb
sudo apt -f install
```

The package installs the `forktty` binary, the desktop entry, and the
icon. Removing it (`sudo apt remove forktty`) cleans up
`/usr/bin/forktty` and the desktop integration.

### Build from source

Requirements:

- Linux
- [Rust 1.96+](https://rustup.rs/)
- GTK4 and libadwaita development files
- `git`, Zig, and the full Ghostty source submodule for the vendored Ghostty
  terminal libraries
- No system Ghostty package is required; source and packaged builds use the
  pinned vendored Ghostty libraries

Developer repository checks also expect the full Ghostty source submodule:

```bash
git submodule update --init vendor/ghostty
```

Debian / Ubuntu:

```bash
sudo apt install build-essential libssl-dev libgtk-4-dev libadwaita-1-dev git zig desktop-file-utils
```

Fedora:

```bash
sudo dnf install gcc gcc-c++ openssl-devel gtk4-devel libadwaita-devel git zig desktop-file-utils
```

Arch / CachyOS:

```bash
sudo pacman -S base-devel openssl gtk4 libadwaita git zig desktop-file-utils
```

Source builds require libadwaita 1.4+, matching Debian 13/Trixie, Ubuntu
24.04 LTS, and newer distro packages. Release AppImages bundle GTK4,
libadwaita, and gtk4-layer-shell so terminal panes do not depend on those host
packages.

Clone and run:

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
git submodule update --init vendor/ghostty
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
cargo run -p forktty-ui-gtk
```

Packaged builds are GTK/Ghostty-only. The experimental WebKitGTK browser pane
remains in source behind the opt-in `browser` feature; it is not shipped in
the AppImage or `.deb` for this alpha.

For the explicit terminal-only build used by release artifacts:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

For the source-only browser experiment, install WebKitGTK 6 development files
and opt in:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features browser
```

Build the Debian package locally:

```bash
bash scripts/build-deb.sh
sudo dpkg -i target/packaging/deb/forktty_*.deb
```

`scripts/build-deb.sh` and `scripts/build-appimage.sh` call
`scripts/ghostty-gtk-lib-probe.sh --ensure --print-path` before packaging and
fail if `ghostty-gtk-embed.so` cannot be built, located, or verified. The
verified library is installed into `usr/lib` beside the `forktty` binary so
installed terminal panes can load it through the binary RUNPATH without
`FORKTTY_GHOSTTY_GTK_LIB`.
Debian packages also install ForkTTY copyright/license information and
third-party notices under `/usr/share/doc/forktty/`.

Build the AppImage locally (requires `appimagetool` on
`PATH`, or `APPIMAGETOOL=/path/to/appimagetool`):

```bash
bash scripts/build-appimage.sh
./target/packaging/appimage/forktty-*-x86_64.AppImage
```

Set `APPIMAGE_UPDATE_INFO=1` when building release-style AppImages with
embedded update metadata; this requires `zsyncmake` on `PATH` and emits a
matching `.zsync` file.
The AppImage includes the same ForkTTY copyright/license information and
third-party notices under `usr/share/doc/forktty/` inside the AppDir/AppImage.

## First Run

### Check the environment

After install, confirm the runtime looks healthy:

```bash
forktty --version
forktty doctor
```

`forktty doctor` is a local-only inspector. It reports the resolved
config, session, socket, hook config paths, and known recovery behaviors,
and exits 0 on a clean environment or 2 with explicit warnings. Use
`forktty --json doctor` when you also need the socket doctor report with
environment, executable, and hook config paths.

### Default workspace and shortcuts

ForkTTY opens the current directory as the `main` workspace. Use the
command palette for most navigation and pane actions:

- `Ctrl+Shift+P`: command palette
- `Ctrl+Shift+N`: new workspace
- `Ctrl+Shift+O`: open workspace
- `Ctrl+Shift+H`: split pane right
- `Ctrl+Shift+E`: split pane down
- `Ctrl+Shift+T`: new tab in the focused pane
- `Ctrl+Shift+W`: close pane
- `Ctrl++`/`Ctrl+=`, `Ctrl+-`, `Ctrl+0`: zoom terminal panes
- `Ctrl+B` or `F9`: toggle workspace sidebar
- Agents: titlebar button or command palette
- `Ctrl+Shift+M`: notifications
- `Ctrl+?` (`F1` also works): keyboard shortcuts
- `Ctrl+,`: settings
- `F10`: main menu when focus is outside terminal content; terminal panes keep
  plain `F10` for TUI apps

## Socket CLI

The same `forktty` binary exposes a user-local Unix socket and CLI for terminal
workspace automation. The socket is intentionally limited to primitives that
are useful for shells, editors, scripts, and coding agents alike: workspaces,
panes, terminal text, notifications, status/progress/log metadata, worktrees,
project actions, remotes, and a thin agent-session lifecycle adapter.

Diagnostic commands work even when the GTK app is not running:

```bash
forktty --help
forktty --version
forktty doctor
forktty worktree-doctor --cwd "$PWD" --json
```

Common live-socket commands:

```bash
forktty ping
forktty list
forktty focus main
forktty surfaces --workspace-name main
forktty split-surface --axis horizontal
forktty read-screen --scope visible
forktty capture-tail --lines 40
forktty notify "Build complete" --title Build
forktty context-snapshot --workspace-name main --json
forktty worktree-list --cwd "$PWD"
```

Use `forktty capabilities --json` for the runtime method list and
`forktty examples` for a compact set of agent-status examples. The socket is
not an execution planner: ForkTTY no longer owns task routing, provider-neutral
team/workflow state, approval feeds, a built-in MCP server, or managed agent
skills. External MCP servers and agent CLIs continue to run normally inside
terminal panes.

Socket requests are one JSON-RPC object per line over the owner-only Unix
socket at `$XDG_RUNTIME_DIR/forktty.sock` (with an owner-only fallback under
`/tmp/forktty-<uid>/`). See [SPEC.md](SPEC.md#socket-api) for the protocol and
[docs/socket-api.md](docs/socket-api.md) for stability tiers.

### Upgrading from orchestration builds

Releases that included the MCP bridge and managed agent skill may have written
client configuration that survives the ForkTTY upgrade. The current binary
does not mutate or remove those files automatically.

Before replacing an older binary, the safest cleanup is to use that same older
binary and inspect both removal plans before applying them:

```bash
forktty mcp remove --dry-run
forktty mcp remove gemini --dry-run       # legacy Gemini registration
forktty skills remove --dry-run
forktty mcp remove
forktty mcp remove gemini
forktty skills remove
```

The older skill remover preserves removed installs as sibling
`forktty-agent-orchestration.bak-*` directories. After applying the commands,
inspect those backups and delete only real directories whose `SKILL.md` still
contains `<!-- forktty-managed-agent-skill -->`; do not follow symlinks.

If the older binary is no longer available, first back up the affected files,
then remove only entries carrying ForkTTY's ownership marker:

- Codex: remove `[mcp_servers.forktty]` from
  `$CODEX_HOME/config.toml` or `~/.codex/config.toml` only when
  `env.FORKTTY_MCP_MANAGED = "forktty"` is present in that table.
- Claude Code: remove `mcpServers.forktty` from `~/.claude.json` only when its
  `env.FORKTTY_MCP_MANAGED` value is `forktty`.
- Antigravity: apply the same JSON check to
  `~/.gemini/config/mcp_config.json`.
- Legacy Gemini: apply the same JSON check to `~/.gemini/settings.json`.
- Agent Skills and Claude Code: remove these active directories and any sibling
  `forktty-agent-orchestration.bak-*` directories beside them:
  `~/.agents/skills/forktty-agent-orchestration` and
  `${CLAUDE_CONFIG_DIR:-~/.claude}/skills/forktty-agent-orchestration`. Remove
  each candidate only when it is a real directory, not a symlink, and its
  `SKILL.md` contains `<!-- forktty-managed-agent-skill -->`.

Leave unmarked entries and directories untouched; they are user-managed.

## Agent Hooks

Install hook templates for Codex, Claude Code, Antigravity CLI, and OpenCode:

```bash
forktty hooks setup                       # install default agents
forktty hooks setup codex                 # install just one
forktty hooks setup codex claude --dry-run
forktty hooks setup --full claude         # include Claude per-tool hooks
forktty hooks remove opencode             # remove ForkTTY-managed hooks/plugin
forktty hooks remove gemini               # cleanup legacy Gemini config only
```

`--dry-run` prints the would-be diff without touching disk. When setup records
an AppImage launcher, generated hook commands set `APPIMAGE_EXTRACT_AND_RUN=1`
for the ForkTTY CLI child so short hooks do not keep a FUSE AppImage mount
alive. Claude Code setup uses the lifecycle profile by default, avoiding
blocking per-tool hooks on every tool call; pass `--full` to include
`PreToolUse`, `PostToolUse`, `PostToolUseFailure`, and `PostToolBatch`.
Re-running setup migrates Claude to
the lifecycle profile unless `--full` is passed. `hooks remove` removes only
ForkTTY-managed entries/plugins and leaves unrelated agent hooks in place.
Codex ties non-managed hook approval to the hook definition's current hash:
after `hooks setup` changes Codex entries, open `/hooks` inside Codex to review
them. ForkTTY reports whether trust records exist but cannot verify that their
stored hashes match the current definitions.
`hooks remove gemini` is kept only to clean legacy ForkTTY-managed
`~/.gemini/settings.json` entries from older releases; Gemini setup remains
unsupported.

The installer merges commands into the agent's own config file:

| Agent       | Destination                                                       |
| ----------- | ----------------------------------------------------------------- |
| Codex       | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json`                 |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json`   |
| Antigravity CLI | `~/.gemini/config/hooks.json` plus wrapper scripts in `~/.gemini/config/forktty-hooks.generated/` |
| OpenCode    | `$OPENCODE_CONFIG_DIR/plugins/forktty.generated.js` or `~/.config/opencode/plugins/forktty.generated.js` |

When `HOME` is overridden, the `~` defaults are resolved under that home
directory. Existing configs are written atomically (tmp + rename) and a
timestamped `.bak-*` backup is created when content changes.

ForkTTY does not create startup reminders for missing or stale hooks. Optional
setup remains available from the first-run welcome flow, Settings > Agents, and
the `forktty hooks setup` CLI.

Diagnose and exercise installed hooks:

```bash
forktty hooks doctor codex     # inspect socket, launcher, env, hook config
forktty hooks test codex       # round-trip a status update through the socket
```

Each agent's hook commands honor a per-agent disable variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_ANTIGRAVITY_HOOKS_DISABLED=1`
- `FORKTTY_OPENCODE_HOOKS_DISABLED=1`

Hooks report status, progress, logs, and prompt notifications through
the same local socket pipeline. Manual hook-event commands can pass
`--socket <path>` when they run outside a ForkTTY-spawned shell.
For Codex and Claude Code, `SubagentStop` keeps the parent session running
because only the nested subagent ended. Claude Code `TeammateIdle` publishes
the teammate as ready/idle.

## Features

- Native GTK4/libadwaita desktop shell with embedded Ghostty-backed terminals.
- Recursive split panes, pane focus/close, command palette, settings dialog, notification panel, and workspace sidebar.
- Quake/dropdown mode through config and F12 where global shortcuts are supported.
- Direct Unix socket JSON-RPC server for workspace (including SSH remote workspaces), surface, terminal read/capture, topology tree/top health inspection, pane-tab, notification, worktree, metadata, persisted agent-session inventory/resume, compact status summaries, context identify, event-stream, and capabilities; CLI wrappers add bounded lifecycle waits over read-only socket calls.
- Agent HUD in the GTK titlebar for lifecycle, last activity, attention, focus, and resume across workspaces.
- Git worktree create/attach/remove/merge/status with dirty-state protection and hook execution inside verified worktrees. Setup hooks are advisory; teardown hook failures or teardown-created dirty state block removal.
- Session restore for workspace order, active workspace, pane tree, focused surface, each local terminal pane's last live cwd, branch, and worktree metadata.
- Prompt-aware notifications from ForkTTY hooks and terminal events, bounded visible prompt fallback, Ghostty bell, and hook/socket events. The notification panel groups prompts/current-workspace/history and its latest-target action prioritizes unread prompts before lower-urgency history.
- Source-only experimental WebKitGTK6 browser panes (behind the `browser` feature) with scriptable snapshot/click/fill/eval verbs, per-profile persistent WebKit sessions, profile CRUD, history/bookmark socket plus CLI access, and history/bookmark import from local Firefox/Chromium-family profiles.
- Bounded config/session/socket handling and local-only privacy defaults.

## Configuration

Config file: `~/.config/forktty/config.toml`. All fields are optional. The
Settings dialog intentionally does not edit `general.shell`; change the shell by
editing this file or your login shell environment.

```toml
[general]
shell = "/bin/bash"
worktree_layout = "nested" # "nested", "sibling", or "outer-nested"
enable_pr_lookup = false
notification_command = ""
persist_terminal_processes = false

[appearance]
persistent_scrollback_lines = 0
sidebar_position = "left" # "left" or "right"
sidebar_visible = true
window_mode = "normal" # "normal" or "quake"

[notifications]
desktop = true
sound = true
blocked_terminal_apps = []
blocked_terminal_types = []

[updates]
auto_check = true

[telemetry]
anonymous_ping = true
```

`notification_command` is split with `shell_words`; ForkTTY does not use `sh -c`. The first token must be an absolute executable path. Notification title/body are passed through `FORKTTY_NOTIFICATION_TITLE` and `FORKTTY_NOTIFICATION_BODY`; OSC 99 `f`/`t` metadata is exposed as `FORKTTY_NOTIFICATION_TERMINAL_APP` and `FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON`. `blocked_terminal_apps` and `blocked_terminal_types` suppress terminal-originated OSC 99 notifications whose exact `f` application or `t` type matches one of the listed strings.

`persistent_scrollback_lines` is off by default; when set above `0`, ForkTTY stores a bounded plain-text tail per surface in `session-v2.json` and restores it before the fresh shell starts. Embedded Ghostty panes use the limited text ABI to read that tail from the end of full scrollback without materializing an unbounded buffer; older embedding libraries without the limited ABI fall back to recent visible text. `persist_terminal_processes` is also off by default and can be toggled from Settings > Worktrees; when set to `true` and `dtach` is available on an absolute `PATH` entry, plain terminal panes run under a detach/reattach broker so generic shells, dev servers, REPLs, editors, and long-running commands can survive a GTK UI restart and re-attach on relaunch. AppImage-launched brokers close inherited AppImage runtime file descriptors before `dtach` starts, so surviving brokers do not keep the FUSE AppImage mount alive after the GTK window closes. Explicit pane close/restart terminates the matching ForkTTY-managed broker process tree and removes the per-surface broker socket, so a later reused surface id starts fresh instead of attaching to stale detached state. Disabling the setting cleans stale detached sessions while preserving currently visible panes until they close; closing the GTK window with the setting disabled cleans visible managed brokers too. Starting with the setting disabled cleans old managed sessions before restore. Agent panes continue to use provider resume, and SSH, browser, and project-action panes are not wrapped. If `dtach` is missing, ForkTTY falls back to normal ephemeral terminal spawning. Live embedded panes follow Ghostty's `scrollback-limit` budget (10 MB by default) and `scrollbar = system|never` preference, so mouse-wheel scrollback and the vertical scrollbar come from Ghostty config while legacy ForkTTY `scrollback_lines` is treated as a compatibility key and omitted from new saves. Terminal font, colors, cursor/faint opacity, bell behavior, mouse scroll multiplier, cell size adjustments, and inactive split dimming come from Ghostty's config (`~/.config/ghostty/config.ghostty` or the legacy `~/.config/ghostty/config`) when present, including `config-file`, `theme`, named colors, 16-color palette entries, `cursor-opacity`, `faint-opacity`, `bell-features`, `mouse-scroll-multiplier`, `adjust-cell-width`, `adjust-cell-height`, `unfocused-split-opacity`, and `unfocused-split-fill`; no system Ghostty install is required. Legacy ForkTTY font/theme/scrollback/bell/renderer keys are kept only for config compatibility and omitted from new saves. Terminal panes require the embedded Ghostty GTK widget; if `ghostty-gtk-embed.so` is missing or fails to load, panes report a spawn failure rather than opening with the old renderer.

`updates.auto_check = true` checks GitHub Releases no more than once every 24 hours. The stamp is written on both success and failure so offline machines are not probed on every launch.

`telemetry.anonymous_ping = true` sends at most one GTK-startup ping per UTC day to `https://forktty.dev/api/telemetry/ping`. The JSON body is limited to `schema`, `kind`, `app`, `version`, and `date`; it contains no install id, username, hostname, cwd, repository path, branch, shell, agent metadata, terminal buffer, socket payload, or crash data. Set it to `false` to disable the ping.

See [SPEC.md](SPEC.md#config) for the full list of validated fields and their bounds.

## Session Restore

GTK/Ghostty sessions are stored as:

```text
~/.local/state/forktty/session-v2.json
```

ForkTTY imports legacy `session.json` when present, but saves the native runtime as v2. By default restore re-spawns fresh Ghostty-backed terminals; scrollback restore is limited to the opt-in plain-text tail controlled by `persistent_scrollback_lines`, and embedded Ghostty panes currently source that tail from visible text rather than off-screen scrollback. With `general.persist_terminal_processes = true` and `dtach` available, plain terminal process trees survive through the broker and restored panes re-attach by persisted surface id. If the setting is false on startup, old ForkTTY-managed broker sessions are cleaned before restore. Corrupt or structurally invalid session files are quarantined.

## Security Summary

- Local Linux desktop threat model; same-user processes remain part of the local trust boundary.
- Unix socket defaults to `$XDG_RUNTIME_DIR/forktty.sock` with `/tmp/forktty-<uid>/forktty.sock` fallback and owner-only permissions.
- Socket request lines, config files, and session files are size bounded.
- Shell paths, hooks, and custom notification commands use validated argv execution, not shell pipelines.
- Worktree names, socket-provided repo paths, and hook locations are validated before mutation or execution.
- ForkTTY makes no crash-reporting or product event-tracking network calls. With `telemetry.anonymous_ping = true`, the GTK app sends one anonymous daily usage ping; set it to `false` to disable it. With `updates.auto_check = true`, the GTK app checks GitHub Releases at most once per day; browser panes and PR lookup remain optional/user-directed network paths. The shipped AppImage and `.deb` do not embed a browser runtime.

See [SECURITY.md](SECURITY.md) and [PRIVACY.md](PRIVACY.md).

## Known Limitations

- Linux only. There are no supported macOS or Windows builds.
- libadwaita 1.4+ is required by the native terminal integration.
- The AppImage ships a bundled GTK4/libadwaita fallback plus Ghostty and gtk4-layer-shell, but prefers a host GTK stack when available and still relies on the host's glibc, fontconfig, OpenGL/Vulkan/Mesa driver stack, display-server libraries, and desktop session services. Test it on the target distro/desktop environment; prefer the `.deb` on Debian/Ubuntu when package-manager integration matters.
- PTY/process persistence is opt-in for plain terminal panes through `general.persist_terminal_processes` and requires `dtach`; by default restored sessions spawn fresh shells. Scrollback persistence is opt-in, plain-text only, and bounded.
- OSC 9 and basic OSC 99 terminal notifications are parsed from the Ghostty-owned PTY stream and rate-limited per surface; OSC 99 title/body base64 payloads and same-id title/body chunks are decoded with multipart title/body kept separate, same-id update/close controls affect ForkTTY's notification model, and in-app Open/Dismiss/Clear All plus basic same-id buttons can send OSC 99 reports. Targeted desktop notifications expose a best-effort Open action, notification dismiss/clear closes matching desktop and OSC 99 tracked notifications, icon names, application-name icon fallback, application/type filtering metadata, occasion filtering, urgency, expiry, and sound metadata inform notification handling, positive `w` expiry values dismiss in-app notifications, and bounded `p=icon` data can be cached by `g`; broader chunk lifecycle behavior remains partial.
- Quake global shortcuts and layer-shell placement depend on desktop/compositor support.
- Agent hibernation/suspend UI, provider-side session existence checks, full theme customization, multi-window, and browser history/bookmark GTK address-bar integration are backlog items.
- Browser panes are source-only and experimental in this alpha; use `--features browser` only when intentionally testing that path.

## Troubleshooting

- `forktty doctor` is the first stop: it explains config, session, socket, and hook config problems before they trigger a launch failure. Use `forktty --json doctor` for socket, environment, executable, and hook diagnostics.
- If terminal panes report `ghostty-gtk-embed.so` as missing in a source tree,
  run `git submodule update --init vendor/ghostty` and
  `scripts/ghostty-gtk-lib-probe.sh --ensure --print-path`. For packaged
  builds, rebuild or reinstall with `bash scripts/build-deb.sh` or
  `bash scripts/build-appimage.sh`; the packagers install the verified library
  into `usr/lib`.
- If the GTK app refuses to start, run it from a terminal to see GLib/GTK error output, then re-run `forktty doctor`.
- The local socket lives at `$XDG_RUNTIME_DIR/forktty.sock` (or `/tmp/forktty-<uid>/forktty.sock`). Stale or foreign sockets are refused on startup; remove them by hand only after confirming no other ForkTTY instance owns them.
- A corrupt `~/.config/forktty/config.toml` or `~/.local/state/forktty/session-v2.json` is renamed aside as `*.bad-<timestamp>` so the app can start with defaults; the rename reason is logged to stderr.
- Local logs live under `~/.local/share/forktty/logs/`.

## Support

For usage questions and bug reports, start with [SUPPORT.md](SUPPORT.md)
and include the output of `forktty doctor`, your distro/desktop
environment, install method, and the exact command or workflow that
failed. Security reports should follow [SECURITY.md](SECURITY.md).

## Contributing

Useful commands:

```bash
cargo fmt --all --check
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
scripts/gtk-ghostty-smoke.sh
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser
desktop-file-validate packaging/linux/dev.forktty.forktty.desktop
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

See [SPEC.md](SPEC.md), [ROADMAP.md](ROADMAP.md), and [docs/native-gtk-ghostty.md](docs/native-gtk-ghostty.md).
Use [docs/release-qa.md](docs/release-qa.md) before tagging alpha releases.

## Inspiration

Built from scratch for Linux, inspired by [cmux](https://github.com/manaflow-ai/cmux) and other multi-agent terminal workflows.

## License

[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`)
