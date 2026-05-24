# ForkTTY Roadmap

This roadmap tracks the native GTK/VTE implementation that replaced the old Tauri/React path.

## Implemented

### Native Runtime

- [x] Rust workspace split into `forktty-core`, `forktty-terminal`, `forktty-socket`, and `forktty-ui-gtk`.
- [x] GTK4/libadwaita application shell.
- [x] VTE terminal panes with configured shell, font, colors, send-text, resize, close, bell, and child-exit handling.
- [x] Direct Unix socket JSON-RPC dispatch without a frontend bridge.
- [x] Native socket CLI and agent hook installer in the `forktty` binary over the socket API.
- [x] `events.subscribe` NDJSON change stream and `system.capabilities` discovery, with `forktty events`/`forktty capabilities` CLI.

### Workspaces and Panes

- [x] Rust workspace model with active workspace, pane tree, surfaces, focus, unread state, metadata, and notifications.
- [x] Recursive split-pane rendering through GTK paned containers.
- [x] Surface split, focus, close, workspace select, workspace create, and workspace close.
- [x] Sidebar refresh from the Rust model with active/unread/worktree/metadata state.
- [x] Listening-TCP-port hints in the sidebar, auto-detected from each workspace's pane child-process tree via `/proc`.
- [x] Linked-PR hints in the sidebar (`#number state`), resolved per branch via `gh` on a background thread.
- [x] Command palette, notification panel, settings dialog, and worktree dialog.

### Git Worktrees

- [x] Native `git2` worktree list/status/create/attach/remove/merge support.
- [x] Worktree layout config: `nested`, `sibling`, `outer-nested`.
- [x] Dirty-state and linked-worktree validation before removal.
- [x] Dirty-target and conflict checks before merge.
- [x] `.forktty/setup` and `.forktty/teardown` hooks inside verified worktrees.
- [x] Closing terminals/workspaces for removed worktrees.

### Notifications and Metadata

- [x] Desktop notifications through `notify-rust`.
- [x] Custom `notification_command` with argv execution and title/body environment variables.
- [x] Agent status, progress, and log metadata through socket API.
- [x] Prompt notifications from VTE shell termprops and bounded visible-tail fallback.
- [x] VTE bell and child-exit notifications.
- [x] Explicit notification `kind` support.

### Appearance

- [x] Auto/Light/Dark theme-source selection in Settings.
- [x] Per-pane VTE scrollback and audible-bell controls.
- [x] Sidebar position (`left`/`right`) and visibility persistence.

### Session, Config, and Security

- [x] Native `session-v2.json` restore/save.
- [x] Legacy session import path.
- [x] Session size, regular-file, depth, and pane-tree validation with quarantine.
- [x] Config size and regular-file validation.
- [x] Shell, notification command, sidebar position, renderer, window mode, layout, and font-size validation.
- [x] Socket bind hardening against live multi-instance takeover.
- [x] Owner-only socket permissions and 1 MiB request line cap.

### Packaging and CI

- [x] Native `.deb` package installing `forktty`.
- [x] Desktop entry and icon under `packaging/linux`.
- [x] CI for Rust fmt/test/clippy/build, repository consistency (`cargo run -p xtask -- check`), desktop entry validation, `.deb` packaging, dependency review, and cargo audit.
- [x] GitHub prerelease workflow that uploads the Debian package artifact.

## Backlog

- [ ] Port or replace byte-level OSC 9/99 parsing now that VTE owns the PTY.
- [ ] Runtime GTK/VTE smoke tests for keyboard, split, notification, and socket workflows.
- [ ] Manual QA matrix for `.deb` across Debian/Ubuntu, Fedora-family, Arch/CachyOS.
- [ ] Persistent scrollback, opt-in and bounded.
- [ ] More complete command palette search/filter parity.
- [ ] Rich branch picker UI with query highlighting.
- [ ] Better notification inbox grouping and actions.
- [ ] Full theme customization.
- [ ] Ghostty theme import for the native VTE color palette.
- [ ] Multi-window support.
- [ ] Built-in browser pane.
- [ ] SSH remote workspaces.
- [ ] MCP/server integration.
- [ ] Plugin system.
- [ ] Auto-update mechanism.

## Known Non-Goals for Current Releases

- macOS and Windows support.
- Persisting running PTY processes across app restarts.
- Treating the local Unix socket as a security boundary against all same-user processes.
- Executing shell pipelines in `notification_command`.
