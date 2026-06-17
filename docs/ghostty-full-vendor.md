# Full Ghostty Vendor

ForkTTY pins the full upstream Ghostty source as a Git submodule at
`vendor/ghostty`.

- Upstream: `https://github.com/ghostty-org/ghostty.git`
- Pin: `e8e7fea103ab8bff5384673a60e04b59939738dd`
- License: MIT, see `vendor/ghostty/LICENSE`

This mirrors the cmux direction: keep Ghostty itself available in-tree so
ForkTTY can test an upstream renderer/widget integration instead of expanding
GTK/Pango/Cairo parity forever.

The current Linux GTK runtime still links through `vendor/libghostty-rs` and
draws with ForkTTY's GTK renderer. `vendor/ghostty` is the source pin for the
next renderer bridge spike, not a release-runtime dependency yet.

See [ghostty-renderer-embedding-spike.md](ghostty-renderer-embedding-spike.md)
for the current upstream embedding status and the next Ghostty-side API cut.

## Setup

```bash
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
```

`xtask check` fails if the submodule is missing, points at the wrong upstream,
or is checked out at a different revision.

## Update

```bash
git -C vendor/ghostty fetch origin
git -C vendor/ghostty checkout <new-sha>
```

Then update `GHOSTTY_VENDOR_REV` in `xtask/src/main.rs` and this document in
the same commit.

Do not patch files inside `vendor/ghostty` for normal ForkTTY work. If the GTK
bridge needs Ghostty-side changes, make that an explicit fork/upstream-patch
decision first.
