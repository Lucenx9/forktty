# Full Ghostty Vendor

ForkTTY pins a small Ghostty fork as a Git submodule at
`vendor/ghostty`.

- Fork: `https://github.com/Lucenx9/ghostty.git`
- Upstream base: `https://github.com/ghostty-org/ghostty.git`
- Pin: `4936a2c4ddc27075f726eebf59b95d592c3a7413`
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
the probe does not claim ForkTTY's process-global GTK application.

See [ghostty-renderer-embedding-spike.md](ghostty-renderer-embedding-spike.md)
for the current upstream embedding status and the next Ghostty-side API cut.

## Setup

```bash
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
scripts/ghostty-gtk-build-probe.sh
scripts/ghostty-gtk-lib-probe.sh
```

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
