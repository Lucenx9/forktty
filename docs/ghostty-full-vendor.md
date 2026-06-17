# Full Ghostty Vendor

ForkTTY pins a small Ghostty fork as a Git submodule at
`vendor/ghostty`.

- Fork: `https://github.com/Lucenx9/ghostty.git`
- Upstream base: `https://github.com/ghostty-org/ghostty.git`
- Pin: `eba2d75a85e4b08d6c9c6b03de1b9f9c0ceaa1a9`
- License: MIT, see `vendor/ghostty/LICENSE`

This mirrors the cmux direction: keep Ghostty itself available in-tree so
ForkTTY can test an upstream renderer/widget integration instead of expanding
GTK/Pango/Cairo parity forever.

The current Linux GTK runtime still links through `vendor/libghostty-rs` and
draws with ForkTTY's GTK renderer. `vendor/ghostty` is the source pin for the
next renderer bridge spike, not a release-runtime dependency yet. The fork adds
an experimental `emit-gtk-lib` build artifact and `ghostty_gtk.h`; it does not
replace ForkTTY panes yet. The GTK embedding library keeps Ghostty's internal
application pointer separate from the host `GApplication` default so loading
the probe does not claim ForkTTY's process-global GTK application. The embedded
artifact skips Ghostty's pre-init GTK environment setup when GTK is already
initialized by the host process, and avoids standalone-app theme/shell startup
pieces that are not needed for a packed widget. The embedded GTK app state is
initialized in the heap-owned embedding context so Ghostty's runtime app
pointer remains stable after context creation. The GTK surface ABI can also
receive a working directory override so ForkTTY's experimental pane can start
Ghostty in the surface cwd, and it can write explicit text bytes into an
initialized embedded surface for ForkTTY socket input.

See [ghostty-renderer-embedding-spike.md](ghostty-renderer-embedding-spike.md)
for the current upstream embedding status and the next Ghostty-side API cut.

## Setup

```bash
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
scripts/ghostty-gtk-build-probe.sh
scripts/ghostty-gtk-lib-probe.sh
```

After building the shared library, an experimental Ghostty-rendered pane can be
enabled for local testing with:

```bash
FORKTTY_GHOSTTY_GTK_PANES=1 \
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
  cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

This mode is intentionally incomplete: it proves that ForkTTY can pack Ghostty's
GTK widget in a pane, pass the cwd, and forward socket `send_text` after Ghostty
initializes the core surface. Socket `read_text`, agent capture, search, and
title/status plumbing are not available in embedded pane mode yet. Use the
default renderer path for those workflows until Ghostty exposes the needed
embedding hooks.

`xtask check` fails if the submodule is missing, points at the wrong fork,
or is checked out at a different revision.

## Update

```bash
git -C vendor/ghostty fetch fork
git -C vendor/ghostty checkout <new-sha>
```

Then update `GHOSTTY_VENDOR_REV` in `xtask/src/main.rs` and this document in
the same commit.

Do not patch files inside `vendor/ghostty` for normal ForkTTY work. If the GTK
bridge needs Ghostty-side changes, make them on the `Lucenx9/ghostty` fork
first, push that commit, then update the submodule pin here.
