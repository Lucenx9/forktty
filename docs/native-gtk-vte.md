# Native GTK/VTE Runtime

ForkTTY's primary runtime is Rust + GTK4/libadwaita + VTE.

## Crates

- `forktty-core`: workspace model, pane tree, config, session v2, notifications, worktree operations, socket protocol types, and source-only browser profile/history stores.
- `forktty-terminal`: `TerminalBackend` trait, headless test backend, and VTE adapter.
- `forktty-socket`: Tokio Unix socket server with direct JSON-RPC dispatch.
- `forktty-ui-gtk`: GTK4/libadwaita UI, VTE terminal panes, sidebar, dialogs, settings, notifications, quake mode, socket CLI, hook installer, and optional source-only WebKitGTK6 browser panes behind `--features browser`.

## Build

```bash
cargo run -p forktty-ui-gtk
cargo build -p forktty-ui-gtk --release
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

For the exact terminal-only build used by release artifacts:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features gtk-vte
```

For the experimental source-only browser pane, install WebKitGTK 6 development
files and opt in:

```bash
cargo run -p forktty-ui-gtk --features browser
```

The AppImage target is the primary portable Linux package for alpha
releases. `scripts/build-appimage.sh` resolves the
`forktty` binary's `ldd` graph and bundles GTK4, libadwaita, VTE, and
their direct dependencies into `AppDir/usr/lib`. It intentionally does
not bundle glibc, GSettings schemas, GIO modules, fontconfig data, the
OpenGL/Vulkan/Mesa driver stack, or desktop session services, so the
AppImage still relies on those parts of the host system.

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
- Source-only browser panes store per-profile WebKit data under `~/.local/share/forktty/browser_profiles/<id>/`.

## Verification

The full automated check list (Rust fmt/test/clippy/build, CLI tests, desktop entry validation, Debian packaging, and AppImage packaging) lives in [release-qa.md](release-qa.md#automated-checks). Run that checklist before tagging an alpha.
