# Full Ghostty Vendor

ForkTTY pins a small Ghostty fork as a Git submodule at
`vendor/ghostty`.

- Fork: `https://github.com/Lucenx9/ghostty.git`
- Upstream base: `https://github.com/ghostty-org/ghostty.git`
- Pin: `e8a3802fb3fa73381e8e664d3da7fa3b6b469f95`
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
Surface lifecycle is wired through the Ghostty surface's `notify::title`,
`notify::child-exited`, and `close-request` GObject signals so title changes,
child-exit readiness/status, and clean pane teardown match the classic panes.
The embedding ABI adds `ghostty_gtk_surface_exit_code` so the child-exit handler
can report the real exit status (`Closed` / `Exited (n)` plus an abnormal-exit
notification); a library built before that symbol degrades gracefully to a
neutral "Closed".

The child PID (needed for ForkTTY's listening-port discovery and the socket
`surfaces` PID field) is exposed through `ghostty_gtk_surface_child_pid`. The pid
is set on Ghostty's io thread, so the fork plumbs it across via a new
`pid_available` surface mailbox message: the io thread pushes the pid after a
successful spawn, the surface mailbox is drained on the apprt main thread and
caches it on the core surface, and the getter reads that main-thread-owned field
race-free. ForkTTY polls the getter briefly after spawn to record the pid; a
library built before the symbol skips the poll and leaves port discovery
unavailable for embedded panes.

Copy/paste/select-all/find now reach parity through
`ghostty_gtk_surface_perform_action`, which performs a Ghostty keybinding action
by name on the surface. ForkTTY's `Ctrl+Shift+C/V/A` accelerators and command
palette route to the focused embedded surface, and find opens Ghostty's native
search overlay (`start_search`); mouse selection already works natively inside
the embedded widget. A library built before that symbol degrades to a logged
no-op.

For installed builds, `FORKTTY_GHOSTTY_GTK_LIB` is only needed during local
development. When `scripts/ghostty-gtk-lib-probe.sh` has produced
`vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so`, `scripts/build-deb.sh` and
`scripts/build-appimage.sh` install it into `usr/lib`, and the binary loads it
through its RUNPATH (`$ORIGIN/../lib`). The install step is best-effort:
packaging still succeeds when the library is absent, and `forktty doctor` warns
about the missing library only when `FORKTTY_GHOSTTY_GTK_PANES` opts the user
into embedded panes. Release CI does not run the probe before packaging yet, so
stable release artifacts still ship without the library until the renderer
becomes the default.

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
