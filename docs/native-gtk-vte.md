# Native GTK/VTE Runtime

ForkTTY's primary runtime is Rust + GTK4/libadwaita + VTE.

## Crates

- `forktty-core`: workspace model, pane tree, config, session v2, notifications, worktree operations, socket protocol types.
- `forktty-terminal`: `TerminalBackend` trait, headless test backend, and VTE adapter.
- `forktty-socket`: Tokio Unix socket server with direct JSON-RPC dispatch.
- `forktty-ui-gtk`: GTK4/libadwaita UI, VTE terminal panes, sidebar, dialogs, settings, notifications, and quake mode.

## Build

```bash
cargo run -p forktty-ui-gtk --features gtk-vte
cargo build -p forktty-ui-gtk --features gtk-vte --release
bash scripts/build-deb.sh
```

The installed binary is `forktty`.

## System Dependencies

Debian/Ubuntu-style names:

- `build-essential`
- `libssl-dev`
- `libgtk-4-dev`
- `libadwaita-1-dev`
- `libvte-2.91-gtk4-dev`
- `desktop-file-utils`

Fedora-style names:

- `gcc`
- `gcc-c++`
- `openssl-devel`
- `gtk4-devel`
- `libadwaita-devel`
- `vte291-gtk4-devel`
- `desktop-file-utils`

Arch-style names:

- `base-devel`
- `openssl`
- `gtk4`
- `libadwaita`
- `vte4`
- `desktop-file-utils`

ForkTTY currently requires VTE 0.76 or newer, matching Ubuntu 24.04 LTS and newer distro packages. `gtk4-layer-shell` is optional and only improves quake/dropdown placement on supported Wayland compositors.

## Runtime Notes

- VTE owns the child PTY; ForkTTY drives terminals through VTE widgets rather than a separate portable-pty stream.
- Spawned shells receive `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, and `FORKTTY_SOCKET_PATH`.
- Prompt/metadata detection uses VTE shell integration signals and a bounded visible-tail prompt fallback.
- Native session data is written to `~/.local/share/forktty/session-v2.json`.
- The legacy `session.json` import path exists only for migration; native saves do not overwrite that file.

## Verification

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --features gtk-vte -- -D warnings
cargo build -p forktty-ui-gtk --features gtk-vte
node --test scripts/forktty.test.mjs
desktop-file-validate packaging/linux/forktty.desktop
bash scripts/build-deb.sh
```
