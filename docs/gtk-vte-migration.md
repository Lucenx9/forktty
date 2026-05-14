# GTK/VTE Migration Notes

This branch introduces the new Linux-native path without deleting the existing
Tauri/React implementation.

## Crates

- `forktty-core`: workspace model, pane tree, config, session v2, notification
  state, worktree service, and JSON-RPC request/response types.
- `forktty-terminal`: terminal backend trait plus a default headless backend for
  tests and a VTE adapter behind the `vte` feature.
- `forktty-socket`: Unix socket JSON-RPC line server that dispatches directly to
  the Rust model and terminal backend.
- `forktty-ui-gtk`: GTK4/libadwaita shell. The VTE UI is behind
  `--features gtk-vte` so dependency-light checks can still run on systems that
  do not have VTE development files installed.

## Implemented Migration Surface

- `system.ping`
- `workspace.create`
- `workspace.list`
- `workspace.select`
- `workspace.close`
- `surface.list`
- `surface.send_text`
- `surface.split`
- `surface.focus`
- `surface.close`
- `worktree.list`
- `worktree.status`
- `worktree.create`
- `worktree.attach`
- `worktree.remove`
- `worktree.merge`
- `notification.create`
- `notification.list`
- `notification.clear`
- `metadata.set_status`
- `metadata.list_status`
- `metadata.clear_status`
- `metadata.set_progress`
- `metadata.list_progress`
- `metadata.clear_progress`
- `metadata.log`
- `metadata.list_logs`
- `metadata.clear_logs`
- `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, and `FORKTTY_SOCKET_PATH`
  injection through the terminal backend adapter.
- Worktree create, attach, list, remove, merge, status, and hook execution in
  `forktty-core`, with direct socket dispatch that opens/closes matching
  worktree-backed workspaces.
- CLI surface commands: `surfaces`, `split-surface`, `focus-surface`, and
  `close-surface`.
- CLI worktree commands: `worktree-list`, `worktree-status`,
  `worktree-create`, `worktree-attach`, `worktree-remove`, and
  `worktree-merge`.
- CLI metadata commands: `set-status`, `list-status`, `clear-status`,
  `set-progress`, `list-progress`, `clear-progress`, `log`, `logs`, and
  `clear-logs`.
- GTK app startup now binds the local Unix socket and routes terminal backend
  commands to VTE widgets on the GTK main thread. `surface.send_text` reaches
  VTE through this adapter, and backend resize requests call VTE's terminal
  resize API for the addressed surface.
- GTK split panes are rendered from the core `PaneNode` tree. UI split buttons
  and socket `surface.split` both create real VTE-backed surfaces and rebuild
  the GTK `Paned` layout from the model.
- VTE focus events update the core focused surface, clear that surface's unread
  marker, and apply a focused terminal CSS indicator for split-pane workflows.
- GTK command palette, notification panel, and settings dialog have native
  implementations. The notification panel can clear unread/attention state, and
  settings persist shell, font size, notification command, worktree layout,
  window mode, and notification preferences through `forktty-core` config.
- The GTK command palette can open a native worktree dialog that creates or
  attaches a branch worktree from the active workspace, opens a matching
  worktree-backed workspace, spawns a VTE terminal there, and runs `.forktty/setup`.
- GTK quake/dropdown mode reads `appearance.window_mode = "quake"`, starts with
  an undecorated monitor-sized dropdown-style window, and uses
  `gtk4-layer-shell` at runtime when available on Wayland to anchor the window
  to the top edge. It also attempts to register a global F12 shortcut on
  desktops supported by `global-hotkey` and degrades to the in-app accelerator
  otherwise. Main GTK accelerators are wired for split, palette, notifications,
  and settings.
- Terminal backends now expose `close(surface_id)`. Socket `surface.close` and
  the GTK close-pane action remove VTE widgets and keep at least one focused
  surface alive.
- The GTK sidebar refreshes from `WorkspaceModel`, including active workspace,
  branch/worktree metadata, agent status metadata, and unread/attention state.
  Activating a sidebar row selects that workspace and rebuilds the VTE split
  layout from the active workspace's pane tree.
- VTE `bell` and `child-exited` signals now update the core model, creating
  notifications that drive unread/attention state.
- VTE termprop events are wired for `xterm.title`, `vte.shell.precmd`,
  `vte.shell.preexec`, `vte.shell.postexec`, `vte.progress.value`, and
  `vte.progress.hint`. These drive terminal titles, prompt notifications,
  terminal running/done status, command-finished logs, and sidebar progress
  metadata without taking PTY ownership away from VTE.
- Notification dispatch now runs through the GTK/core path as well: socket,
  local GTK, and VTE signal notifications can emit desktop notifications and
  invoke `general.notification_command` with `FORKTTY_NOTIFICATION_*`
  environment variables.
- Session restore can load the current Tauri v1 `session.json` shape and migrate
  workspace order, active workspace, pane tree, focused pane, cwd, branch, and
  worktree metadata into the new v2 core schema. The GTK app restores that
  session on startup, spawns VTE terminals for restored surfaces, and writes the
  v2 session on window close.

## VTE Owning PTY Gap

The GTK path starts from VTE owning the PTY. The previous `output_scanner.rs`
was coupled to the portable-pty read loop, so it is not directly reusable when
VTE owns the child process. The migrated path keeps notification/unread state
available via socket `notification.create`, VTE bell/child-exit signals, VTE
shell-integration termprops, VTE legacy OSC 777-to-termprop translation, and
visible prompt scanning for simple prompt patterns such as `>`, `❯`, `(Y/n)`,
and `Do you want to proceed`.

Byte-level OSC 9/99 notification parsing is still not fully ported because the
GTK app does not receive the raw PTY byte stream when VTE owns the PTY. Legacy
OSC 777 is enabled through VTE's translation path, but only the termprops that
VTE surfaces are visible to the GTK app. Hidden control sequences that VTE does
not surface still need either VTE API support or shell integration events.

## Native Dependencies

Fedora-style names:

- `gtk4-devel`
- `libadwaita-devel`
- `vte291-gtk4-devel`

Debian/Ubuntu-style names:

- `libgtk-4-dev`
- `libadwaita-1-dev`
- `libvte-2.91-gtk4-dev`

Arch-style names:

- `gtk4`
- `libadwaita`
- `vte4`

Optional quake top-edge anchoring dependency:

- Debian/Ubuntu: `libgtk4-layer-shell-dev` / runtime library package
- Fedora: `gtk4-layer-shell-devel`
- Arch: `gtk4-layer-shell`

Build commands:

```sh
cargo build -p forktty-ui-gtk
cargo build -p forktty-ui-gtk --features gtk-vte
bash scripts/gtk-build-deb.sh
```

The first command builds the dependency-safe headless binary. The second command
builds the GTK/VTE app once the VTE development package is installed. On the
current development machine, `pkg-config --modversion vte-2.91-gtk4` returns
`0.84.0` and the GTK/VTE feature build passes. The GTK/VTE feature currently
requires VTE 0.80 or newer for shell-integration and progress termprops.

`scripts/gtk-build-deb.sh` creates a preview package named `forktty-gtk` so it
does not replace the existing Tauri package while the migration is still in
progress. The CI workflow builds that package on pull requests and pushes, and
uploads `target/packaging/deb/*.deb` to GitHub releases when a release is
published.

No GTK AppImage is produced in this branch. The native `.deb` is the supported
preview artifact for now; AppImage remains deferred until the GTK/VTE runtime
dependency story is stable enough to bundle without hiding system integration
problems.

## Known Limits

- Byte-level OSC 9/99 parsing remains limited by VTE-owned PTY access, and OSC
  777 support depends on the termprops surfaced by VTE's legacy translation, as
  described above.
- Quake top-edge anchoring requires Wayland plus the optional
  `gtk4-layer-shell` runtime library. Other desktops fall back to the normal
  undecorated GTK quake window.
