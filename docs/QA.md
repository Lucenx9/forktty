# ForkTTY QA Matrix

This is the platform and feature grid maintainers walk before tagging an
alpha release. It complements [`release-qa.md`](release-qa.md) (the
detailed runtime checklist) and [`../RELEASING.md`](../RELEASING.md)
(the end-to-end release flow).

ForkTTY is Linux-only. The supported runtime baseline is libadwaita 1.4+,
which matches Ubuntu 24.04 LTS, Debian 13/Trixie, and newer distro packages.
Terminal panes use the vendored embedded Ghostty GTK library; a system Ghostty
package is not a release prerequisite.

## Supported platforms

| Distro family            | Versions covered                       | Package manager |
| ------------------------ | -------------------------------------- | --------------- |
| Ubuntu / Debian          | Ubuntu 24.04 LTS+, Debian 13/Trixie+   | `apt`           |
| Fedora                   | Current stable                         | `dnf`           |
| Arch / CachyOS / Manjaro | Rolling                                | `pacman`        |

Other distros are best-effort: they should work if libadwaita 1.4+ is
available and the packaged `ghostty-gtk-embed.so` loads, but they are not part
of the release gate.

## Display server coverage

| Session   | Notes                                                                                 |
| --------- | ------------------------------------------------------------------------------------- |
| Wayland   | Primary target. Quake/dropdown anchoring uses `gtk4-layer-shell` when installed.      |
| X11       | Must launch and pass the runtime smoke. Layer-shell features fall back to GTK defaults.|

## Matrix to walk per release

For each distro and each display server, walk the rows below. A cell
should be one of:

- `pass`
- `pass with notes`
- `fail` (file a release-blocker issue and link from the changelog)
- `n/a` (only when the feature physically cannot exist on that platform)

| Area                                  | Ubuntu / Debian (Wayland) | Ubuntu / Debian (X11) | Fedora (Wayland) | Arch / CachyOS (Wayland) |
| ------------------------------------- | ------------------------- | --------------------- | ---------------- | ------------------------ |
| Install dependencies (per README)     |                           |                       |                  |                          |
| `scripts/ghostty-gtk-lib-probe.sh --ensure --print-path` |          |                       |                  |                          |
| `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty` | |                 |                  |                          |
| Source-only browser build: `cargo build -p forktty-ui-gtk --no-default-features --features browser` | |             |                  |                          |
| `bash scripts/build-deb.sh`           |                           |                       |                  | n/a                      |
| `scripts/check-deb-piuparts.sh` (Debian 13/Trixie install/purge) |     |                       | n/a              | n/a                      |
| `bash scripts/build-appimage.sh`      |                           |                       |                  |                          |
| `dpkg -i target/packaging/deb/forktty_*.deb` |                    |                       | n/a              | n/a                      |
| Run `target/packaging/appimage/*.AppImage` |                       |                       |                  |                          |
| Launch from terminal (`forktty`)      |                           |                       |                  |                          |
| Launch from desktop launcher          |                           |                       |                  |                          |
| `forktty --version` / `forktty --help`|                           |                       |                  |                          |
| `forktty doctor` reports a clean env  |                           |                       |                  |                          |
| Default workspace opens               |                           |                       |                  |                          |
| Split panes (right, down, three+)     |                           |                       |                  |                          |
| Keyboard focus between panes          |                           |                       |                  |                          |
| Copy/paste (`Ctrl+Shift+C/V`)         |                           |                       |                  |                          |
| Command palette (`Ctrl+Shift+P`)      |                           |                       |                  |                          |
| Notifications appear (desktop + panel)|                           |                       |                  |                          |
| Quake mode toggle                     |                           |                       |                  |                          |
| Socket: `forktty ping`                |                           |                       |                  |                          |
| Socket: hooks setup/remove (codex/claude/gemini/opencode) |        |                       |                  |                          |
| Worktree: create / attach / status    |                           |                       |                  |                          |
| Worktree: merge / remove              |                           |                       |                  |                          |
| Session restore after restart         |                           |                       |                  |                          |
| Corrupted config quarantined          |                           |                       |                  |                          |
| Corrupted session quarantined         |                           |                       |                  |                          |
| Settings dialog: scrollback, sidebar, privacy |                  |                       |                  |                          |
| Source-only browser pane smoke (`--features browser`) |             |                       |                  |                          |

## Negative / hardening checks

These do not need to be repeated per distro — once per release on a
single distro is enough. They are the rows most likely to reveal a
security regression.

- Invalid shell path in `config.toml` → pane shows recovery state.
- `notification_command` pointing at a non-executable file → command
  silently ignored, dispatch falls back to the desktop notification.
- Hand-edited session with multiple `active: true` workspaces → file is
  quarantined, app starts fresh.
- `FORKTTY_SOCKET_PATH=" "` → app still binds the default socket.
- `FORKTTY_SOCKET_PATH=relative.sock` → app and CLI ignore the env value and
  use the default socket.
- Stub socket at the default path returning a foreign id on
  `system.ping` → ForkTTY refuses to replace it.
- `cargo audit` and (optionally) `cargo deny check` clean.

## Recording results

Per-release results live in the GitHub release notes, not in this file.
This document is the template; the release writer fills in a copy in the
release body or in a tracking issue if the matrix is large enough to
warrant one.
