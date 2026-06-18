# Full Ghostty Vendor

ForkTTY pins a small Ghostty fork as a Git submodule at
`vendor/ghostty`.

- Fork: `https://github.com/Lucenx9/ghostty.git`
- Upstream base: `https://github.com/ghostty-org/ghostty.git`
- Pin: `2d6400f56af4af03cc59ac5b87754de717cf6bdc`
- License: MIT, see `vendor/ghostty/LICENSE`

This mirrors the cmux direction: keep Ghostty itself available in-tree so
ForkTTY can use Ghostty's renderer/widget integration instead of expanding
GTK/Pango/Cairo parity forever.

The Linux GTK runtime still links through `vendor/libghostty-rs` for PTY/VT
support, while terminal pane rendering now requires the vendored Ghostty GTK
embedding library built from this source pin. The fork adds an `emit-gtk-lib` build
artifact and `ghostty_gtk.h`. The GTK embedding library keeps Ghostty's internal
application pointer separate from the host `GApplication` default so loading
the library does not claim ForkTTY's process-global GTK application. The embedded
artifact skips Ghostty's pre-init GTK environment setup when GTK is already
initialized by the host process, and avoids standalone-app theme/shell startup
pieces that are not needed for a packed widget. The embedded GTK app state is
initialized in the heap-owned embedding context so Ghostty's runtime app
pointer remains stable after context creation. The GTK surface ABI can also
receive a working directory override so ForkTTY's embedded pane can start
Ghostty in the surface cwd, and it can write explicit text bytes into an
initialized embedded surface for ForkTTY socket input. It can also return
visible or full plain text from Ghostty's active screen so ForkTTY socket
`read_text` and `capture_tail` requests work in embedded panes.

See [ghostty-renderer-embedding-spike.md](ghostty-renderer-embedding-spike.md)
for the current upstream embedding status and the next Ghostty-side API cut.

## Setup

```bash
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
```

After verifying the shared library, the Ghostty-rendered default pane path can
be run locally with:

```bash
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
  cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

Embedded Ghostty panes are the terminal renderer path. If the embedding library
cannot be loaded or an embedded surface fails to spawn, ForkTTY records a
terminal spawn failure instead of opening a classic-renderer pane.

This mode proves that ForkTTY can pack Ghostty's GTK widget in a pane, pass the
cwd, forward socket `send_text`, and answer socket `read_text`/`capture_tail`
after Ghostty initializes the core surface.
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
race-free. If that startup mailbox has not been observed yet, the getter falls
back to Ghostty's existing PTY foreground PID query, which exposes the startup
shell pid early enough for ForkTTY's listening-port discovery. ForkTTY polls the
getter briefly after spawn to record the pid; a library built before the symbol
skips the poll and leaves port discovery unavailable for embedded panes.

Copy/paste/select-all/find now reach parity through
`ghostty_gtk_surface_perform_action`, which performs a Ghostty keybinding action
by name on the surface. ForkTTY's `Ctrl+Shift+C/V/A` accelerators and command
palette route to the focused embedded surface, and find opens Ghostty's native
search overlay (`start_search`); mouse selection already works natively inside
the embedded widget. A library built before that symbol degrades to a logged
no-op.

Persisted scrollback restore is wired through an optional
`ghostty_gtk_surface_restore_scrollback(GtkWidget*, const char*, size_t)` symbol
that ForkTTY loads when present. It must inject already-terminal-ready bytes
(CR/LF normalized by ForkTTY) into the surface's VT stream WITHOUT writing them
to the child PTY, so restored output is not replayed as shell input. The current
pinned fork exports it: an IO-thread `inject_output` mailbox message routed to
`Termio.processOutput` keeps all terminal mutation on the IO thread (a raw
GTK-main-thread feed was rejected — it races the IO thread's PTY reader). The
design is detailed in
[ghostty-renderer-embedding-spike.md](ghostty-renderer-embedding-spike.md). The
embedded restore round-trip is verified by the **Ghostty GTK Probe** workflow:
the Ubuntu runner builds the `.so`, restarts an embedded pane, and confirms a
pre-restart marker is present in `capture_tail` after restore. A library built
before this symbol degrades to a safe no-op. The snapshot half reads the tail
through `ghostty_gtk_surface_read_text` into the session on child exit,
programmatic close/restart, and a throttled poll.

For installed builds, `FORKTTY_GHOSTTY_GTK_LIB` is only needed during local
development. `scripts/build-deb.sh` and `scripts/build-appimage.sh` call
`scripts/ghostty-gtk-lib-probe.sh --ensure --print-path` before packaging, which
reuses or builds `ghostty-gtk-embed.so` and verifies the required ABI symbols.
The packagers install the verified library into `usr/lib`, and the binary loads
it through its RUNPATH (`$ORIGIN/../lib`). The install step is required for
release packages: `scripts/build-deb.sh`, `scripts/build-appimage.sh`, and
release CI fail if the embedding library cannot be built, located, or verified.
`forktty doctor` warns about a missing library because terminal panes cannot
open without it.

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
