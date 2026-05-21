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

Before tagging an alpha, run the runtime and package checklist in
[release-qa.md](release-qa.md).

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

ForkTTY currently requires libadwaita 1.4+ and VTE 0.76 or newer, matching Ubuntu 24.04 LTS and newer distro packages. `gtk4-layer-shell` is optional and only improves quake/dropdown placement on supported Wayland compositors.

## Runtime Notes

- VTE owns the child PTY; ForkTTY drives terminals through VTE widgets rather than a separate portable-pty stream.
- Spawned shells receive `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=ForkTTY`, `TERM_PROGRAM_VERSION`, `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, and `FORKTTY_SOCKET_PATH`.
- Prompt/metadata detection uses VTE shell integration signals and a bounded visible-tail prompt fallback.
- Native session data is written to `~/.local/share/forktty/session-v2.json`.
- The legacy `session.json` import path exists only for migration; native saves do not overwrite that file.

## Verification

The full automated check list (Rust fmt/test/clippy/build, CLI tests, desktop entry validation, Debian packaging) lives in [release-qa.md](release-qa.md#automated-checks). Run that checklist before tagging an alpha.
