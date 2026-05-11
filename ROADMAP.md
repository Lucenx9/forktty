# ForkTTY Roadmap

This roadmap separates implemented behavior from active backlog. It should track code that exists in the repository, not aspirational product copy.

## Implemented

### Phase 1 - MVP Terminal

- [x] Tauri v2 project with React 19, TypeScript, and Vite.
- [x] Rust PTY backend using `portable-pty`.
- [x] xterm.js terminal connected to backend output through Tauri channels.
- [x] Bidirectional input/output and resize handling with FitAddon.
- [x] Canvas renderer for stable WebKitGTK behavior.
- [x] Basic Linux desktop window integration.

### Phase 2 - Multi-Pane Layout

- [x] Recursive pane tree rendered through `react-resizable-panels`.
- [x] Multiple PTYs tracked by backend IDs.
- [x] Split right (`Ctrl+D`) and split down (`Ctrl+Shift+D`).
- [x] Pane focus management and `Alt+Arrow` navigation.
- [x] Close pane (`Ctrl+W`) with PTY disposal.
- [x] Focus flash and unread pane ring.

### Phase 3 - Workspaces and Sidebar

- [x] Zustand workspace state for create, switch, close, rename, focus, and reorder.
- [x] Sidebar entries with workspace name, branch, directory, unread state, and worktree status.
- [x] Workspace creation (`Ctrl+N`) and close (`Ctrl+Shift+W`).
- [x] Workspace jump shortcuts (`Ctrl+1`..`Ctrl+9`).
- [x] Resizable, collapsible, left/right sidebar.
- [x] Workspace drag-and-drop reorder.
- [x] Auto-reorder of inactive workspaces when they receive notifications.
- [x] Workspace metadata display through socket API status/progress/log records.

### Phase 4 - Git Worktree Integration

- [x] Native `git2` worktree create/list/remove/merge support.
- [x] Branch picker for creating a new branch from `HEAD` or attaching an existing branch.
- [x] Worktree layout config: `nested`, `sibling`, `outer-nested`.
- [x] Worktree status badges: clean, dirty, conflicts, error.
- [x] `.forktty/setup` hook after worktree creation.
- [x] `.forktty/teardown` hook before worktree removal.
- [x] Last worktree-backed workspace removal fallback to a valid repository root.
- [x] Socket `cwd` validation accepts subdirectories/linked worktrees in an open repo and rejects unrelated repos.

### Phase 5 - Notifications

- [x] Rust output scanner for OSC 133 shell integration.
- [x] Prompt pattern matching for Claude-style prompts.
- [x] OSC 9, OSC 99, and OSC 777 notification parsing.
- [x] In-app notification list, unread badges, sidebar preview, and pane unread state.
- [x] Desktop notifications through `notify-rust`.
- [x] Notification panel (`Ctrl+Shift+I`) and jump to unread (`Ctrl+Shift+U`).
- [x] Custom `notification_command` support.
- [x] No `sh -c`; notification title/body passed through environment variables.
- [x] Switch/restore suppression, dedupe, and repeated prompt-notification suppression.
- [ ] Idle detector based on inactivity threshold.

### Phase 6 - Socket API

- [x] Unix domain socket server with newline-delimited JSON-RPC.
- [x] Owner-only socket permissions and 1 MiB request limit.
- [x] Methods for `system`, `workspace`, `surface`, `notification`, `worktree`, and `metadata`.
- [x] ForkTTY environment variables in spawned shells for socket discovery.
- [x] `surface.read_screen` support through the terminal registry.

### Phase 7 - Theming and Config

- [x] Ghostty config parser for colors, font family, font size, palette, and theme files.
- [x] ForkTTY TOML config at `~/.config/forktty/config.toml`.
- [x] Settings panel (`Ctrl+,`) for appearance, shell, worktree layout, and notifications.
- [x] Catppuccin Mocha fallback theme.
- [x] Sidebar position config.
- [x] ForkTTY config read hardening: regular file validation and 1 MiB size bound.
- [x] Shell executable path validation.
- [x] Notification command executable path validation.
- [ ] Full light-mode toggle.

### Phase 8 - Polish, Packaging, and Reliability

- [x] Session persistence with debounced saves.
- [x] Session restore of workspace list, pane tree, focused leaf, cwd, branch, and worktree metadata.
- [x] Session validation and quarantine for corrupt/invalid `session.json`.
- [x] Prompt notification suppression after restore/session switch.
- [x] Command palette (`Ctrl+Shift+P`) with fuzzy filtering and empty state.
- [x] Find in terminal (`Ctrl+F`) using xterm.js SearchAddon.
- [x] Copy selection (`Ctrl+Shift+C`).
- [x] User-visible spawn/settings/git/socket error handling.
- [x] Welcome/onboarding overlay.
- [x] UI polish for sidebar, pane chrome, settings, branch picker, command palette, notifications, menus, and find bar.
- [x] Safer focus defaults for destructive modals.
- [x] System tray icon with unread-count tooltip and click-to-focus behavior when supported by the desktop environment.
- [x] Structured logs under `~/.local/share/forktty/logs/`.
- [x] `.deb` and AppImage packaging through Tauri bundler.
- [x] AppImage icon path traversal mitigation and absolute root symlink refusal.
- [x] AppImage WebKitGTK runtime environment patching.
- [x] License set to AGPL-3.0-only.

### Phase 9 - Test Coverage and Validation

- [x] Frontend unit tests for pane tree, workspace store, selectors, workspace effects, socket handler, notification dispatch, terminal registry, output buffering, terminal fonts, session persistence, and Ghostty theme parsing.
- [x] Rust unit tests for config validation, session validation/quarantine, socket line limits and cwd validation, output scanner, PTY cwd fallback, worktree validation, and AppImage-adjacent path behavior.
- [x] CI coverage for npm build/lint/test, Rust fmt/clippy/test, dependency audit, CodeQL, and Tauri validation/build paths.
- [ ] Full Tauri GUI smoke tests.
- [ ] Manual runtime QA matrix for release artifacts.

## Backlog

- [ ] Code splitting review beyond the current lazy-loaded panels.
- [ ] Stricter `notification_command` policy that disallows extra argv arguments.
- [ ] Better release automation and artifact verification.
- [ ] Full Tauri GUI smoke tests with realistic keyboard/window flows.
- [ ] Runtime manual QA for `.deb` and AppImage across supported Linux environments.
- [ ] Documentation screenshot refresh after UI polish.
- [ ] End-to-end PTY backpressure or flow-control strategy.
- [ ] Idle notification detector if a clear product need emerges.
- [ ] Light mode and explicit theme switching.
- [ ] Multi-window support.
- [ ] Scrollback persistence.
- [ ] Built-in browser pane.
- [ ] SSH remote workspaces.
- [ ] MCP/server integration.
- [ ] Plugin system.
- [ ] Auto-update mechanism.

## Known Non-Goals for Current Releases

- macOS and Windows support.
- Persisting running PTY processes across app restarts.
- Executing shell pipelines in `notification_command`.
- Treating the local Unix socket as a boundary against all same-user processes.
