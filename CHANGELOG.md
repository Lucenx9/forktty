# Changelog

All notable changes to ForkTTY are documented here.

## [Unreleased]

### Architecture
- Replaced the old Tauri/React/WebKit runtime with the native GTK4/libadwaita/VTE implementation as the primary app.
- Removed the legacy frontend, Tauri backend, Vite/TypeScript build, and npm dependency tree from the main code path.
- Installed the native binary and Debian package as `forktty` instead of `forktty-gtk`.

### UI
- Added a first native GTK visual pass: dark libadwaita chrome, themed VTE colors, styled workspace sidebar cards, active/unread indicators, and focused terminal outline.

### Reliability
- Fixed `workspace.close` to close by the resolved workspace ID so surface cleanup and model mutation cannot diverge on ambiguous selectors.
- Limited VTE prompt fallback scanning to a bounded visible tail instead of copying the full terminal text on every contents-changed signal.

### Tooling
- Replaced Vitest/Vite frontend checks with Node built-in CLI tests.
- Updated CI, dependency review, security audit, Docker dev image, desktop entry validation, and Debian packaging for the Rust GTK/VTE stack.

## [0.1.2] - 2026-05-11

### Documentation
- Updated README, SPEC, ROADMAP, SECURITY, and PRIVACY to match current UI polish, session restore, config validation, notification, worktree, AppImage, and test coverage behavior
- Clarified that `notification_command` still supports static argv arguments after the required absolute executable path; a no-arguments policy remains a future hardening item

### UI Polish
- Refined WelcomeScreen, modal focus behavior, and empty/loading/error states across key frontend surfaces
- Added safer focus defaults for destructive modals

### Reliability & Security
- Session restore now validates persisted pane trees and quarantines corrupt or invalid session files instead of failing startup
- Restored sessions suppress spurious prompt notifications during startup
- Config loading for ForkTTY's TOML config is bounded to regular files up to 1 MiB
- Ghostty config and theme loading now ignores missing, non-regular, oversized, or unreadable files instead of reading them unbounded
- Shell and notification command configuration now validate executable paths more defensively
- AppImage packaging normalizes root desktop/icon symlinks, rejects unsafe icon values, and refuses absolute root symlinks
- Socket request reading now enforces the 1 MiB line limit without relying on `BufReader::lines()`

### Tests & Tooling
- Added frontend and Rust coverage for restore, notification, config, and packaging hardening paths
- Refreshed dependency and tooling versions where relevant

## [0.1.1] - 2026-04-23

### UI Polish
- Refined sidebar, pane chrome, command palette, branch picker, notifications, settings, menus, and find bar with a more consistent dark desktop visual language
- Split UI typography from terminal typography: proportional font for chrome, monospace for terminal content, shortcuts, and badges
- Added explicit inactive-pane dimming and more restrained focus/unread states
- Added extra breathing room around terminal surfaces without changing PTY behavior
- Replaced placeholder text controls with shared SVG iconography
- Added `prefers-contrast` and `prefers-reduced-motion` polish for dark-theme accessibility

### Interaction Fixes
- Help & Shortcuts menu now renders above the sidebar correctly instead of appearing behind other UI
- Workspace switching from the sidebar triggers earlier and feels more immediate
- Workspace name hover now shows the text cursor only over the actual name, not across the full row
- Workspace reordering now uses a dedicated drag handle instead of making the whole row draggable
- Reduced duplicate prompt notifications with stronger switch-time suppression and short-window deduplication
- Avoid repeated `Prompt waiting` notifications while a workspace is already unread

### Socket & Worktree Hardening
- Fixed socket-driven `worktree.create` prompts being written twice to the target PTY
- Fixed removal of the last worktree-backed workspace so the replacement workspace falls back to a valid repository root instead of a deleted directory
- Relaxed socket `cwd` validation to accept subdirectories and linked worktrees from the same open repository while preserving repo-boundary checks

## [0.1.0] - 2026-03-19

### Phase 1 — MVP Terminal
- Tauri v2 + React 19 + TypeScript scaffold
- portable-pty PTY management with Tauri Channel streaming
- xterm.js terminal with Canvas renderer (WebGL fallback disabled due to WebKitGTK bugs)
- Full TUI support (htop, vim, less all render correctly)
- Terminal resize via ResizeObserver + FitAddon

### Phase 2 — Multi-Pane Splits
- react-resizable-panels recursive split layout (horizontal/vertical)
- Zustand store tracking PaneTree structure and focus
- Keyboard: Ctrl+D (split right), Ctrl+Shift+D (split down), Alt+Arrow (navigate), Ctrl+W (close)

### Phase 3 — Sidebar + Workspaces
- Sidebar showing workspace list with metadata (branch, directory, status)
- Workspace creation (Ctrl+N), switching (Ctrl+1..9), closing (Ctrl+Shift+W)
- Git branch detection via git2

### Phase 4 — Git Worktree Integration
- git2 crate for native worktree create/merge/remove
- Setup/teardown hook support (.forktty/setup, .forktty/teardown)
- Worktree layout config (nested/sibling/outer-nested)
- Sidebar worktree status badges (clean/dirty/conflicts)

### Phase 5 — Notification System
- OSC 133 shell integration parsing in Rust backend
- Pattern matching for Claude Code prompt detection
- In-app blue dot + unread count on sidebar
- Desktop notifications via notify-rust (XDG/D-Bus)
- Notification panel (Ctrl+Shift+I), jump to unread (Ctrl+Shift+U)

### Phase 6 — Socket API
- Unix domain socket JSON-RPC server (tokio)
- 22 methods: system.ping, workspace.*, surface.*, notification.*, worktree.*, metadata.*
- Environment variables set in spawned shells (`FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, `FORKTTY_SOCKET_PATH`)

### Phase 7 — Theming + Config
- Ghostty config parser with theme file and palette support
- TOML config at ~/.config/forktty/config.toml
- Settings panel (Ctrl+,) for in-app config editing
- Catppuccin Mocha as default fallback theme
- Configurable sidebar position (left/right)

### Phase 8 — Polish + Release
- Session persistence (auto-save and restore on startup)
- Command palette (Ctrl+Shift+P) with keyboard navigation and inline filtering
- Find in terminal (Ctrl+F) via xterm.js SearchAddon
- Copy selection (Ctrl+Shift+C)
- ErrorToast component for user-visible error feedback
- Structured logging to ~/.local/share/forktty/logs/
- .deb and AppImage bundle targets
- License: AGPL-3.0

### Security Hardening
- Socket: owner-only permissions (0o600), XDG_RUNTIME_DIR default path, 1 MiB request size limit
- Notifications: argv splitting instead of sh -c (no command injection)
- Worktree: path traversal protection via canonicalize + git-workdir boundary check
- Worktree names: reject /, \, .., \0
- Shell path: must be absolute and point to an executable file
- CSP: strict Content Security Policy in tauri.conf.json
- Config: Ghostty theme path traversal guard
- Logging: newline injection sanitization

### Known Limitations
- `beforeunload` session save is fire-and-forget (async IPC may not complete)
- No idle detection (`idle_threshold_ms` config field reserved but not active)
- No dark/light mode toggle (dark theme only; CSS has a minimal system-preference fallback)
- No flow control / backpressure on PTY output
