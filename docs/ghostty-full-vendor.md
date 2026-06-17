# Full Ghostty Vendor

ForkTTY pins a small Ghostty fork as a Git submodule at
`vendor/ghostty`.

- Fork: `https://github.com/Lucenx9/ghostty.git`
- Upstream base: `https://github.com/ghostty-org/ghostty.git`
- Pin: `39effd5c71a97608a75cdf782bd536456d7a4bba`
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
initialized embedded surface for ForkTTY socket input. It can also return
visible or full plain text from Ghostty's active screen so ForkTTY socket
`read_text` and `capture_tail` requests work in experimental embedded panes.

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
GTK widget in a pane, pass the cwd, forward socket `send_text`, and answer
socket `read_text`/`capture_tail` after Ghostty initializes the core surface.
Surface lifecycle is partially wired: ForkTTY connects to the Ghostty surface's
`notify::title`, `notify::child-exited`, and `close-request` GObject signals so
title changes, child-exit readiness/status, and clean pane teardown match the
classic panes without a new ABI symbol. Two lifecycle pieces still need the
embedding ABI extended on the fork before they reach parity:

- the exact child exit code (today's embedded exit status is the neutral
  "Closed" because the code is not exposed as a property), and
- the child PID (needed for ForkTTY's listening-port discovery).

Search and copy/selection parity also remain to be wired. Use the default
renderer path for those workflows until the remaining embedding hooks land.

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
