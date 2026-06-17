# ForkTTY Roadmap

This roadmap tracks the native GTK/Ghostty implementation that replaced the old Tauri/React path.

## Near-Term Direction

- [ ] Close the practical cmux gap through ForkTTY's control plane first, not by
  copying every panel: after `top`/health inspection, continue with a durable
  feed/approval bridge, repo-local `forktty.json` actions, and real agent
  hibernate/reclaim.
- [ ] Keep richer sidebars, remote daemon depth, multi-window routing, and plugin
  surfaces behind those primitives; they need real state/events before extra UI
  pays for itself.

## Implemented

### Native Runtime

- [x] Rust workspace split into `forktty-core`, `forktty-terminal`, `forktty-socket`, and `forktty-ui-gtk`.
- [x] GTK4/libadwaita application shell.
- [x] Ghostty-backed terminal panes with configured shell, font, colors, send-text, resize, close, bell, and child-exit handling.
- [x] Direct Unix socket JSON-RPC dispatch without a frontend bridge.
- [x] Native socket CLI and agent hook installer in the `forktty` binary over the socket API.
- [x] `events.subscribe` NDJSON change stream and `system.capabilities` discovery, with `forktty events`/`forktty capabilities` CLI.
- [x] Read-only `system.top` / `forktty top` health snapshot for workspaces,
  surfaces, runtime size/PID, unread state, agents, status, and progress.
- [x] Browser pane SP1: WebKitGTK6 pane kind + `browser.open`/`browser.navigate` + in-pane address bar (behind the `browser` cargo feature).
- [x] Browser pane SP2: scriptable verbs (`browser.snapshot`/`click`/`fill`/`eval`) + socket-driven `back`/`forward`/`reload` via socket→GTK command channel + `forktty browser snapshot|click|fill|eval|back|forward|reload` CLI (behind the `browser` cargo feature).
- [x] Browser pane SP3 P1/P2: persistent per-profile WebKit sessions, `ProfileId` on browser surfaces, profile metadata store, `browser.profile.*` socket methods, and `forktty browser profile ...` CLI.
- [x] Browser pane SP3 P3 core/socket/CLI storage: per-profile `HistoryStore` and `BookmarkStore` in `forktty-core`, `browser.history.*` / `browser.bookmark.*` socket verbs, and `forktty browser history|bookmark` CLI mirrors.
- [x] Browser import: `forktty-import` crate reading Firefox/Chromium-family profiles, `browser.import.discover`/`preview`/`run` socket verbs, `forktty browser import discover|preview|run` CLI, and a Settings "Import Browser Data" dialog with profile rollback on failure.

### Workspaces and Panes

- [x] Rust workspace model with active workspace, pane tree, surfaces, focus, unread state, metadata, and notifications.
- [x] Recursive split-pane rendering through GTK paned containers.
- [x] Surface split, focus, close, workspace select, workspace create, and workspace close.
- [x] Per-pane tabs: `pane.new_tab`/`pane.select_tab` socket methods, `forktty new-tab`/`select-tab` CLI, and pane-chrome/command-palette tab controls.
- [x] SSH remote workspaces: `SurfaceKind::Ssh` panes spawned as `ssh <host>`, `workspace.create_ssh` socket method, `forktty ssh` CLI, sidebar `ssh:<host>` hints, and respawn on session restore.
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
- [x] Agent hook session ids, resume cwd, and lifecycle persisted per surface as resume metadata.
- [x] Agent session inventory through `agent.list`, `forktty agents`, and MCP `agent_list`.
- [x] Agent resume readiness checks through `agent.health`, `forktty agent-health`, and MCP `agent_health`.
- [x] Explicit provider resume into a new tab through `agent.resume`, `forktty resume-agent`, and MCP `agent_resume`.
- [x] Restore-time provider resume for persisted supported agent terminal surfaces.
- [x] Agent last-activity tracking and read-only reclaim candidate planning through `agent.reclaim.plan`, `forktty agent-reclaim-plan`, and MCP `agent_reclaim_plan`.
- [x] Compact status/HUD export through `status.summary`, `forktty statusline`, and MCP `status_summary`.
- [x] GTK Agent HUD for persisted agent lifecycle, last activity, needs-input attention, focus, and resume.
- [x] Prompt notifications from ForkTTY hook termprops and bounded visible-tail fallback.
- [x] OSC 9 and basic OSC 99 title/body terminal notifications parsed from the Ghostty-owned PTY stream and rate-limited per surface, including same-id update/close, desktop replacement/closing, in-app report replies, support/alive query replies, basic same-id buttons, icon names, application/type metadata filtering, occasion filtering, urgency/expiry/sound hints, bounded `p=icon` data caching, and in-app/desktop binary icon rendering where GTK/notification servers can decode the image.
- [x] Ghostty bell and child-exit notifications.
- [x] Explicit notification `kind` support.

### Appearance

- [x] Auto/Light/Dark theme-source selection in Settings.
- [x] Per-pane Ghostty scrollback and audible-bell controls.
- [x] Ghostty config appearance import for font, colors, `theme`, `config-file`, named colors, ANSI palette entries, `scrollback-limit`, cursor/faint opacity, mouse scroll multiplier, cell size adjustments, and inactive split dimming.
- [x] Sidebar position (`left`/`right`) and visibility persistence.

### Session, Config, and Security

- [x] Native `session-v2.json` restore/save.
- [x] Legacy session import path.
- [x] Session size, regular-file, depth, and pane-tree validation with quarantine.
- [x] Config size and regular-file validation.
- [x] Shell, notification command, sidebar position, renderer, window mode, layout, and font-size validation.
- [x] Socket bind hardening against live multi-instance takeover.
- [x] Owner-only socket permissions and 1 MiB request line cap.
- [x] Browser profile IDs and metadata writes validated before touching profile storage paths.

### Packaging and CI

- [x] Native `.deb` package installing `forktty`.
- [x] Desktop entry and icon under `packaging/linux`.
- [x] CI for Rust fmt/test/clippy/build, repository consistency (`cargo run -p xtask -- check`), desktop entry validation, `.deb` packaging, dependency review, and cargo audit.
- [x] Runtime GTK/Ghostty smoke script for isolated socket boot, terminal input/readback, runtime zoom reflow/reset, GTK action split/focus behavior, socket split readback, and notification CLI create/list/clear.
- [x] GitHub prerelease workflow that uploads the Debian package, experimental AppImage, and shared `SHA256SUMS`.

## Backlog

- [x] Agent hibernation/reclaim control plane: `agent.hibernate` and
  `agent.reclaim` close idle, locally resumable terminal processes, persist an
  explicit `suspended` lifecycle, and resume through the existing provider
  argv path. Remaining parity scope: richer GTK suspend/restore UI and
  provider-side stale-session validation beyond local command/PATH readiness.
- [ ] Workflow control plane: per-session/per-mode state files, durable goal/plan/evidence artifacts, session search/replay, and compaction-resistant project memory.
- [ ] Team orchestration runtime: leader/worker state, task DAGs, mailbox/dispatch, worker heartbeats, status/summary, and optional worktree-backed workers.
- [ ] Feed/approval bridge: minimal read-only `feed.list` / `forktty feed`
  snapshot is available; durable history and actionable permission events are
  still pending.
- [ ] Expanded HUD/statusline export: active-mode, worker, token, health, and notification fields for provider statusline integrations; workspace/status/progress/session summary is available through `status.summary`/`forktty statusline`/MCP and GTK now includes the persisted-agent HUD.
- [ ] Remote daemon/SSH depth: persistent remote ForkTTY helper, reconnect/disconnect semantics, CLI relay, and remote PTY/session ownership beyond plain `ssh <host>`.
- [ ] Project actions: repo-local validated `forktty.json` actions exposed in the command palette and socket.
- [ ] Right-sidebar/Dock ecosystem: files/find/vault/session/feed style panels, panel persistence, and optional custom sidebar contributors.
- [ ] Workspace organization: groups, pin/collapse/reorder, and saved layout intent.
- [ ] Expanded socket topology and tmux-compatible verbs: send key, move/reorder/join/swap/split-off, buffers, and pipe/wait primitives. Initial read-only `tree`, `read-screen`, and `capture-tail` primitives are available through socket/CLI/MCP; `top` is available through socket/CLI.
- [ ] Prompt composer/TextBox surface for reusable prompt drafting and dispatch.
- [ ] Agent/skill catalog: installable prompt packs, provider-specific workflow skills, and project-scoped reusable guidance.
- [ ] File/project panels: file explorer, markdown preview, diff/comment review flows.
- [ ] Manual QA matrix for `.deb` across Debian/Ubuntu, Fedora-family, Arch/CachyOS.
- [x] Persistent scrollback, opt-in and bounded.
- [ ] More complete command palette search/filter parity.
- [ ] Rich branch picker UI with query highlighting.
- [ ] Better notification inbox grouping and actions.
- [ ] Full theme customization beyond Ghostty-imported terminal appearance.
- [ ] Broader Ghostty option compatibility where the GTK/libghostty-vt runtime exposes a real matching knob.
- [ ] Replace the custom GTK/Pango/Cairo terminal renderer with an upstream Ghostty embeddable renderer/widget if Ghostty exposes a stable Linux API that fits ForkTTY panes, splits, socket automation, and session restore.
- [ ] Multi-window support.
- [ ] Browser visit recording and history/bookmark address-bar completion.
- [ ] Remote-aware MCP/server integration.
- [ ] Plugin system.
- [ ] Auto-update mechanism.

## Known Non-Goals for Current Releases

- macOS and Windows support.
- Persisting running PTY processes across app restarts.
- Treating the local Unix socket as a security boundary against all same-user processes.
- Executing shell pipelines in `notification_command`.
