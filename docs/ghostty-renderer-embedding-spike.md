# Ghostty Renderer Embedding Spike

Inspected against `vendor/ghostty` at
`eba2d75a85e4b08d6c9c6b03de1b9f9c0ceaa1a9`.

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

Stop expanding ForkTTY renderer parity as the primary strategy. Keep the
current renderer only as the working fallback while we make Ghostty's Linux GTK
surface embeddable.

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
3. Keep the first packed Ghostty pane behind `FORKTTY_GHOSTTY_GTK_PANES=1`.
   Socket input/read/capture, title/status propagation, child-exit lifecycle,
   copy/paste/select-all/find, and child-PID/port discovery are now wired
   through the embedding ABI. Remaining parity work and how each behavior is
   validated live in [`ghostty-embedded-parity.md`](ghostty-embedded-parity.md).

Do not replace ForkTTY's current renderer until the parity matrix is all `pass`
and the embedding library ships in release artifacts by default.

## Build Probe

Run the upstream GTK build probe with:

```bash
scripts/ghostty-gtk-build-probe.sh
```

Or run the same command on GitHub's Ubuntu runner through the manual
`Ghostty GTK Probe` workflow.

The GTK widget probe is intentionally separate from the normal app launch:

```bash
scripts/ghostty-gtk-lib-probe.sh
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
  cargo run -p forktty-ui-gtk -- ghostty-gtk-probe
```

For a non-interactive probe smoke test, set an auto-exit delay:

```bash
FORKTTY_GHOSTTY_GTK_LIB=vendor/ghostty/zig-out/lib/ghostty-gtk-embed.so \
FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS=750 \
  cargo run -p forktty-ui-gtk -- ghostty-gtk-probe
```

On the current Arch-style local toolchain this does not reach the Ghostty API
work yet. Zig 0.15.2 attempts to link helper executables against GCC 16.1.1
startup objects containing `.sframe` relocations and fails with:

```text
fatal linker error: unhandled relocation type R_X86_64_PC64
note: in .../crt1.o:.sframe
```

Forcing `use_lld = false` on the helper executables still emits `-fno-lld` and
fails on the same relocation. Forcing `use_lld = true` changes the command to
`-flld`, but the helper compile terminates without a useful diagnostic. The
submodule was restored after this probe; no unverified Ghostty patch is kept.

If the workflow passes, the next implementation attempt is the minimal
`GtkWidget*` embedding API. If the workflow fails the same way, fix or pin the
Ghostty GTK build toolchain first.
