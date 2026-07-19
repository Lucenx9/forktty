# ForkTTY Roadmap

ForkTTY's current direction is a focused Linux workspace terminal: GTK,
embedded Ghostty, workspaces/panes/tabs, notifications, worktrees, and a small
local automation boundary. Task routing and provider-neutral agent
orchestration are intentionally outside the product core.

## Implemented

- Native GTK4/libadwaita shell with embedded Ghostty surfaces.
- Recursive split panes, pane tabs, keyboard navigation, drag-and-drop, command
  palette, quake mode, and session/layout restore.
- Workspace sidebar with cwd, branch, unread state, worktree, listening-port,
  and optional PR hints.
- OSC 9/99 notifications, desktop/in-app attention, status/progress/log
  metadata, and terminal bell/child-exit handling.
- Owner-only JSON-RPC Unix socket and CLI for workspace, pane, surface,
  terminal-text, notification, metadata, topology, remotes, worktrees, and
  project actions.
- Optional agent lifecycle hooks plus explicit health/resume/hibernate/reclaim
  primitives; providers retain ownership of task planning and coordination.
- Native `git2` worktree create/attach/status/merge/remove with validated local
  setup/teardown hooks.
- Opt-in bounded scrollback persistence and opt-in `dtach` process persistence
  for plain terminal panes.
- Source-only experimental WebKit browser panes, excluded from release
  artifacts.
- AppImage and Debian packaging with vendored Ghostty GTK embedding.

## Maintenance backlog

- Complete manual Ghostty embedding parity checks and upstream the smallest
  viable Linux GTK embedding surface where practical.
- Improve terminal/pane recovery diagnostics without adding a second renderer.
- Expand distribution QA across Debian/Ubuntu, Fedora-family, and Arch-family
  desktops on Wayland and X11.
- Continue command-palette, branch-picker, notification-inbox, accessibility,
  and keyboard-focus polish.
- Deepen SSH/remote PTY reliability only where it remains a terminal primitive.
- Keep the browser feature compiling while evaluating it separately from the
  shipped terminal product.

## Non-goals

- A ForkTTY-owned task router or execution strategy engine.
- Provider-neutral team, task, mailbox, workflow, loop, evidence, or approval
  stores.
- A built-in MCP server or managed agent-skill distribution.
- Hidden background schedulers or autonomous agent execution.
- macOS or Windows support.
- Treating the local Unix socket as isolation from every same-user process.
