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
- `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, and `FORKTTY_SOCKET_PATH`
  injection through the terminal backend adapter.
- Worktree create, attach, list, remove, merge, status, and hook execution in
  `forktty-core`, with direct socket dispatch that opens/closes matching
  worktree-backed workspaces.
- CLI worktree commands: `worktree-list`, `worktree-create`, `worktree-attach`,
  `worktree-remove`, and `worktree-merge`.
- GTK app startup now binds the local Unix socket and routes terminal backend
  commands to VTE widgets on the GTK main thread. `surface.send_text` reaches
  VTE through this adapter.
- Session restore can load the current Tauri v1 `session.json` shape and migrate
  workspace order, active workspace, pane tree, focused pane, cwd, branch, and
  worktree metadata into the new v2 core schema.

## VTE Owning PTY Gap

The GTK path starts from VTE owning the PTY. The previous `output_scanner.rs`
was coupled to the portable-pty read loop, so it is not directly reusable when
VTE owns the child process. The minimum migrated path keeps notification/unread
state available via socket `notification.create`; the next VTE-specific step is
to attach scanning to VTE output signals or shell integration events where VTE
exposes them.

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

Build commands:

```sh
cargo build -p forktty-ui-gtk
cargo build -p forktty-ui-gtk --features gtk-vte
bash scripts/gtk-build-deb.sh
```

The first command builds the dependency-safe headless binary. The second command
builds the GTK/VTE app once the VTE development package is installed. On the
current development machine, `pkg-config --modversion vte-2.91-gtk4` returns
`0.84.0` and the GTK/VTE feature build passes.

`scripts/gtk-build-deb.sh` creates a preview package named `forktty-gtk` so it
does not replace the existing Tauri package while the migration is still in
progress.

## Remaining Parity Work

- Replace GTK split placeholders with model-driven VTE pane creation.
- Port command palette, settings, notification panel, and quake window behavior
  from the Tauri path.
- Harden `.deb` metadata and add CI/release wiring for the GTK package.
