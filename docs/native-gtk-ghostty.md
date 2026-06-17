# Native GTK/Ghostty Runtime

ForkTTY's primary runtime is Rust + GTK4/libadwaita + Ghostty.

## Crates

- `forktty-core`: workspace model, pane tree, config, session v2, notifications, worktree operations, socket protocol types, and source-only browser profile/history stores.
- `forktty-terminal`: `TerminalBackend` trait, headless test backend, and Ghostty adapter.
- `forktty-socket`: Tokio Unix socket server with direct JSON-RPC dispatch.
- `forktty-ui-gtk`: GTK4/libadwaita UI, Ghostty-backed terminal panes, sidebar, dialogs, settings, notifications, quake mode, socket CLI, hook installer, and optional source-only WebKitGTK6 browser panes behind `--features browser`.

## Build

```bash
cargo run -p forktty-ui-gtk
cargo build -p forktty-ui-gtk --release
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

For the exact terminal-only build used by release artifacts:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

ForkTTY also pins a Ghostty fork at `vendor/ghostty` for the next
renderer/widget integration spike. Initialize it after cloning with:

```bash
git submodule update --init vendor/ghostty
```

That source pin is not the release renderer yet. The current Linux GTK runtime
still uses `vendor/libghostty-rs` plus ForkTTY's GTK/Pango/Cairo renderer.
The fork currently carries the experimental GTK embedding library build probe.

For the experimental source-only browser pane, install WebKitGTK 6 development
files and opt in:

```bash
cargo run -p forktty-ui-gtk --features browser
```

The AppImage target is the primary portable Linux package for alpha
releases. `scripts/build-appimage.sh` always installs the vendored
libghostty-vt into `AppDir/usr/lib`, and resolves the `forktty` binary's
`ldd` graph into `AppDir/usr/lib/bundled` as a GUI-stack fallback:
AppRun adds that directory to the library path only when the host has
no GTK4, so modern hosts run against their own GTK4/libadwaita (native
cursor themes, fontconfig, portals). Per the canonical AppImage
excludelist it never bundles glibc, fontconfig/freetype/harfbuzz,
Wayland/X11 client libraries, the OpenGL/Vulkan/Mesa driver stack,
GSettings schemas, GIO modules, or desktop session services, so the
AppImage relies on those parts of the host system.

Before tagging an alpha, run the runtime and package checklist in
[release-qa.md](release-qa.md).

The installed binary is `forktty`.

## System Dependencies

Debian/Ubuntu-style names:

- `build-essential`
- `libssl-dev`
- `libgtk-4-dev`
- `libadwaita-1-dev`
- `git zig`
- `desktop-file-utils`

Fedora-style names:

- `gcc`
- `gcc-c++`
- `openssl-devel`
- `gtk4-devel`
- `libadwaita-devel`
- `git zig`
- `desktop-file-utils`

Arch-style names:

- `base-devel`
- `openssl`
- `gtk4`
- `libadwaita`
- `git`, `zig`
- `desktop-file-utils`

ForkTTY currently requires libadwaita 1.4+ and Ghostty 0.76 or newer, matching Ubuntu 24.04 LTS and newer distro packages. `gtk4-layer-shell` is optional and only improves quake/dropdown placement on supported Wayland compositors.

## Runtime Notes

- ForkTTY owns the child PTY; ForkTTY drives terminals through Ghostty widgets rather than a separate portable-pty stream.
- Spawned shells receive `TERM=xterm-ghostty` with matching terminfo when available, otherwise `TERM=xterm-256color`, plus `COLORTERM=truecolor`, `TERM_PROGRAM=ForkTTY`, `TERM_PROGRAM_VERSION`, `FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, and `FORKTTY_SOCKET_PATH`.
- When Ghostty shell-integration resources are available, ForkTTY injects the upstream zsh/bash/fish/elvish/nushell startup integration; Linux release artifacts bundle those shell-integration resources and Ghostty terminfo.
- Prompt/metadata detection uses ForkTTY hooks and terminal events and a bounded visible-tail prompt fallback.
- Native session data is written to `~/.local/share/forktty/session-v2.json`.
- The legacy `session.json` import path exists only for migration; native saves do not overwrite that file.
- Source-only browser panes store per-profile WebKit data under `~/.local/share/forktty/browser_profiles/<id>/`.

## Verification

The full automated check list (Rust fmt/test/clippy/build, CLI tests, desktop entry validation, Debian packaging, and AppImage packaging) lives in [release-qa.md](release-qa.md#automated-checks). Run that checklist before tagging an alpha.
