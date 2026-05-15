# Release QA Checklist

Use this before tagging an alpha release. The goal is to catch GTK/VTE and
package regressions that unit tests cannot see.

## Automated Checks

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --features gtk-vte -- -D warnings
cargo build -p forktty-ui-gtk --features gtk-vte
node --test scripts/forktty.test.mjs
desktop-file-validate packaging/linux/forktty.desktop
bash scripts/build-deb.sh
```

## Manual Runtime Smoke

- Start from a clean config/session directory.
- Launch with `cargo run -p forktty-ui-gtk --features gtk-vte`.
- Confirm the app opens a usable terminal in the current directory.
- Split right and split down until at least three panes exist.
- Move focus between panes with keyboard shortcuts and pointer clicks.
- Copy and paste with `Ctrl+Shift+C` / `Ctrl+Shift+V`.
- Open the terminal context menu in a small split pane and use Paste.
- Close one pane and confirm focus moves to a remaining pane.
- Restart the app and confirm workspace/pane layout restores.
- Set an invalid shell path in Settings and confirm the pane shows a recovery state.
- Open Notifications, dismiss one notification, then Clear All.

## Debian Package Smoke

- Install the generated `.deb` with `sudo dpkg -i target/packaging/deb/forktty_*.deb`.
- Launch `forktty` from a terminal.
- Launch ForkTTY from the desktop/app launcher.
- Confirm the app icon and desktop name render correctly.
- Confirm `forktty --help` exits cleanly if CLI flags are supported, or document the current behavior.
- Remove the package and confirm `/usr/bin/forktty` and the desktop entry are removed.

## Suggested Matrix

- Ubuntu 24.04 or newer, GNOME Wayland.
- Ubuntu 24.04 or newer, X11 session if available.
- Debian testing/stable where VTE 0.76+ is available.
- One Arch/CachyOS or Fedora-family system for dependency-name drift.
