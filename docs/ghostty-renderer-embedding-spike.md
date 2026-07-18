# Ghostty Renderer Embedding Spike

Inspected against `vendor/ghostty` at
`470d3174eb10d25e21d17eff69ffcefdd4f4f91c`.

> **Status:** Historical spike notes plus the landed embedding ABI record.
> Current ForkTTY terminal panes are Ghostty-only: the embedded GTK widget is the
> runtime path, and missing `ghostty-gtk-embed.so` is a spawn/package error, not
> a classic renderer fallback.

## Result

ForkTTY cannot switch directly from its GTK/Pango/Cairo renderer to Ghostty's
upstream renderer through the current public C embedding API on Linux.

The usable long-term path is a small Ghostty-side GTK embedding API, then a
ForkTTY bridge that packs that Ghostty GTK surface inside ForkTTY panes.

## Evidence

- `vendor/ghostty/include/ghostty.h` exposes a C embedding API with
  `ghostty_app_t`, `ghostty_surface_t`, `ghostty_surface_draw`,
  `ghostty_surface_set_size`, input, clipboard, selection, and read-text calls.
  The same header says the API is not general purpose yet and is only consumed
  by the macOS app.
- `vendor/ghostty/src/main_c.zig` asserts the C API is built with
  `apprt.runtime == apprt.embedded`.
- `vendor/ghostty/src/apprt/embedded.zig` only supports platform tags
  `macos` and `ios`. On Linux, those platform payloads are `void` and platform
  init returns `UnsupportedPlatform`.
- `vendor/ghostty/src/apprt/runtime.zig` defaults Linux and FreeBSD to the
  `gtk` runtime, but that is the full Ghostty application runtime, not the C
  embedded runtime.
- `vendor/ghostty/src/apprt/gtk/class/surface.zig` contains the real GTK
  widget, `GhosttySurface`, with a `gtk.GLArea` and OpenGL renderer calls.
  That is the code ForkTTY wants, but it is not exported through
  `include/ghostty.h`.
- `vendor/ghostty/src/build/SharedDeps.zig` links GTK dependencies for
  executable builds with `app_runtime = gtk`; the shared C library path is the
  embedded runtime path.

## Decision

Stop expanding ForkTTY renderer parity as the primary strategy. The classic
GTK/Pango/Cairo renderer stayed useful during the spike, but it is no longer a
runtime fallback in current ForkTTY builds.

The current fork starts the Ghostty-side API spike with:

```c
#include <stddef.h>

typedef struct _GtkWidget GtkWidget;
typedef struct ghostty_gtk_context_s ghostty_gtk_context_t;

ghostty_gtk_context_t *ghostty_gtk_context_new(void);
void ghostty_gtk_context_free(ghostty_gtk_context_t *context);
int ghostty_gtk_context_register(ghostty_gtk_context_t *context);
int ghostty_gtk_context_tick(ghostty_gtk_context_t *context);

GtkWidget *ghostty_gtk_surface_new(ghostty_gtk_context_t *context);
GtkWidget *ghostty_gtk_surface_new_with_working_directory(
    ghostty_gtk_context_t *context,
    const char *working_directory
);
GtkWidget *ghostty_gtk_surface_new_with_working_directory_and_command(
    ghostty_gtk_context_t *context,
    const char *working_directory,
    const char *const *argv,
    size_t argv_len
);
int ghostty_gtk_surface_send_text(
    GtkWidget *surface,
    const char *text,
    size_t text_len
);
void ghostty_gtk_surface_free(GtkWidget *surface);
```

The exact ABI can change during the spike. The important constraint is that
ForkTTY needs a GTK widget it can pack into an existing pane, plus lifecycle,
focus, resize, input, selection/read-text, and PTY/session hooks that do not
require Ghostty to own the whole application window.

The fork also keeps Ghostty's internal GTK application pointer separate from
the host `GApplication`, and skips Ghostty's pre-init GTK environment setup
when the embedding library is loaded after the host process has already
initialized GTK. In embedding mode it also skips Ghostty GTK startup pieces
that belong to the standalone app, including theme-manager sync, signal
handlers, application actions, global shortcuts, and config-error dialogs.
The GTK surface constructor returns a sunk full `GtkWidget*` reference so
gtk-rs and other embedders can use normal transfer-full ownership.
The embedded `GtkApp` is initialized directly inside the heap-owned embedding
context instead of a temporary stack value, keeping Ghostty's internal runtime
app pointer stable after context creation. ForkTTY's fork also exposes a
working-directory surface constructor so a packed Ghostty widget can start in
the same cwd as the ForkTTY surface, plus length-delimited text input and
plain-text read ABIs for socket send/read/capture operations.

## Next Cut

1. Keep the `emit-gtk-lib` fork branch compiling on Ubuntu CI.
2. Keep `forktty ghostty-gtk-probe` covered by the manual `Ghostty GTK Probe`
   workflow. That workflow now builds `ghostty-gtk-embed.so` and starts the
   Rust probe under Xvfb with `FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS`.
3. Keep release packaging strict: `ghostty-gtk-embed.so` is required for deb and
   AppImage builds, and missing/failed embedded startup must surface as a
   terminal spawn failure rather than silently opening the old renderer.

Embedded Ghostty panes are now the default renderer path. Socket
input/read/capture, title/status propagation, child-exit lifecycle,
copy/paste/select-all/find, zoom, reset/clear, scrollback snapshot/restore, and
child-PID/port discovery are wired through the embedding ABI. Remaining
manual-only checks and their deferred validation status live in
[`ghostty-embedded-parity.md`](ghostty-embedded-parity.md).

## Landed and probe-verified: scrollback restore ABI

Embedded panes can already *snapshot* their scrollback into the session (ForkTTY
reads the tail through `ghostty_gtk_surface_read_text_limited_with_total_lines`
and stores it on
the surface). *Restoring* that scrollback on respawn needs a Ghostty-side export
that pushes bytes into a surface's terminal state (scrollback/screen)
**without** writing them to the child PTY — otherwise old output would be
replayed as shell input.

ForkTTY's side is implemented and unit-tested: it loads an optional
`ghostty_gtk_surface_restore_scrollback` symbol, CR/LF-normalizes the persisted
text into terminal-ready bytes (same as classic panes), and seeds it on surface
init, degrading to a no-op when the symbol is absent.

The Ghostty fork now exports the symbol: the design below landed on
`Lucenx9/ghostty` at `470d3174eb10d25e21d17eff69ffcefdd4f4f91c`, and ForkTTY's
submodule pin (and `GHOSTTY_VENDOR_REV`) is bumped to it. The fork commit was
verified locally as far as the toolchain allows — `zig fmt --check`,
`zig ast-check`, and the full `zig build test -Dapp-runtime=none` core suite
(which executes the `@sizeOf(Message) == 40` assertion and compiles
`Surface.injectOutput`) all pass. The **end-to-end restore round-trip is
probe-verified** by the manual **Ghostty GTK Probe** workflow: the Ubuntu runner
builds the embedding `.so`, launches ForkTTY under Xvfb with embedded panes,
restarts a pane, and confirms a pre-restart marker is present in `capture_tail`
after restore.

### Why not a GTK-main-thread feed

The preferred shape was a main-thread call that feeds display state directly
(e.g. `Surface.dumpPlainText` runs on the GTK thread under
`renderer_state.mutex`). Writing is different: the only existing entry point,
`Termio.processOutput`, runs on Ghostty's **IO thread** and mutates the VT
parser (`terminal_stream`) plus the terminal. Calling it from the GTK main
thread races the IO thread's PTY reader (allocator and parser state), even under
the renderer mutex, and there is no exposed pre-spawn window where the IO thread
is guaranteed idle. So a raw main-thread feed is unsafe; the smallest safe
design keeps all terminal mutation on the IO thread via the existing mailbox.

### Smallest safe Ghostty-side design (landed on the fork)

Route the bytes through the IO thread the same way `writeBytes` does, but call
`processOutput` instead of `queueWrite`. Reuse the `WriteReq` data carrier so
`@sizeOf(Message)` (asserted == 40) does not grow (`WriteReq.Alloc` is smaller
than the existing `write_small`).

1. `src/termio/message.zig` — add a variant to `Message`:

   ```zig
   /// Inject bytes directly into the terminal VT stream (scrollback/screen)
   /// without writing them to the child PTY. Used to restore persisted
   /// scrollback when re-spawning an embedded surface.
   inject_output: WriteReq.Alloc,
   ```

2. `src/termio/Thread.zig` — handle it in `drainMailbox` (alongside the
   `write_*` cases), calling `processOutput` (it takes `renderer_state.mutex`
   internally) instead of `queueWrite`:

   ```zig
   .inject_output => |v| {
       defer v.alloc.free(v.data);
       io.processOutput(v.data);
   },
   ```

3. `src/Surface.zig` — add a method mirroring `writeBytes` plumbing. `queueIo`'s
   readonly guard only filters `write_*`, so inject correctly passes through
   even in readonly mode (it is display state, not a PTY write):

   ```zig
   /// Inject bytes into the terminal VT stream (scrollback/screen) WITHOUT
   /// writing them to the child PTY. Used to restore persisted scrollback on
   /// respawn. Mirrors writeBytes plumbing but routes to processOutput.
   pub fn injectOutput(self: *Surface, data: []const u8) !void {
       if (data.len == 0) return;
       const buf = try self.alloc.dupe(u8, data);
       errdefer self.alloc.free(buf);
       self.queueIo(.{ .inject_output = .{
           .alloc = self.alloc,
           .data = buf,
       } }, .unlocked);
   }
   ```

4. `src/main_gtk_c.zig` — export the C ABI (mirrors `ghostty_gtk_surface_send_text`):

   ```zig
   pub export fn ghostty_gtk_surface_restore_scrollback(
       surface_: ?*gtk.Widget,
       text_: ?[*]const u8,
       text_len: usize,
   ) c_int {
       if (text_len == 0) return 1;
       const surface_widget = surface_ orelse return 0;
       const text_ptr = text_ orelse return 0;
       const surface = gobject.ext.cast(Surface, surface_widget) orelse return 0;
       const core_surface = surface.core() orelse return 0;
       core_surface.injectOutput(text_ptr[0..text_len]) catch |err| {
           std.log.warn("failed to restore scrollback to Ghostty GTK surface: {}", .{err});
           return 0;
       };
       return 1;
   }
   ```

5. `include/ghostty_gtk.h` — declare it:

   ```c
   // Injects already terminal-ready bytes (CR/LF normalized by the caller) into
   // the surface's terminal VT stream (scrollback/screen) WITHOUT writing them to
   // the child PTY. Used to restore persisted scrollback on respawn. Returns 1 on
   // success, 0 if the surface is invalid or not yet initialized.
   int ghostty_gtk_surface_restore_scrollback(
       GtkWidget *surface,
       const char *text,
       size_t text_len
   );
   ```

This change landed on `Lucenx9/ghostty` (branch `forktty-gtk-embed`,
`470d3174eb10d25e21d17eff69ffcefdd4f4f91c`), and the submodule pin +
`GHOSTTY_VENDOR_REV` + the pin in `ghostty-full-vendor.md` are bumped to it.
ForkTTY's loader picks up the symbol automatically with no further Rust changes.
The end-to-end restore is covered by the Ghostty GTK Probe workflow and can also
be smoke-tested locally with the probe command below.

## Build Probe

Run the upstream GTK build probe with:

```bash
scripts/ghostty-gtk-build-probe.sh
```

Or run the same command on GitHub's Ubuntu runner through the manual
`Ghostty GTK Probe` workflow.

The GTK widget probe is intentionally separate from the normal app launch:

```bash
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
  cargo run -p forktty-ui-gtk -- ghostty-gtk-probe
```

For a non-interactive probe smoke test, set an auto-exit delay:

```bash
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS=750 \
  cargo run -p forktty-ui-gtk -- ghostty-gtk-probe
```

On Arch-style toolchains with GCC 16 startup objects, Zig 0.15.2 can fail to
link Ghostty's small GTK Blueprint helper when that helper uses Zig's default
ELF linker:

```text
fatal linker error: unhandled relocation type R_X86_64_PC64
note: in .../crt1.o:.sframe
```

ForkTTY's pinned Ghostty fork now builds that helper with Zig's LLVM backend and
`scripts/ghostty-gtk-lib-probe.sh` builds the embedded GTK library with
`-Doptimize=ReleaseSafe`. That combination avoids the local `.sframe` linker
failure and the `ReleaseFast` startup crash observed in
`ghostty_gtk_context_new`, while preserving the same embedding ABI.

The GitHub Ghostty GTK Probe remains the release source of truth for this
embedding path, but the local probe should now build and run on the same
Arch-style development machine.
