# Ghostty Renderer Embedding Spike

Inspected against `vendor/ghostty` at
`e8e7fea103ab8bff5384673a60e04b59939738dd`.

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

Minimum Ghostty-side API to spike:

```c
typedef void* ghostty_gtk_surface_t;

ghostty_gtk_surface_t ghostty_gtk_surface_new(ghostty_config_t config);
void* ghostty_gtk_surface_widget(ghostty_gtk_surface_t surface);
void ghostty_gtk_surface_free(ghostty_gtk_surface_t surface);
```

The exact ABI can change during the spike. The important constraint is that
ForkTTY needs a GTK widget it can pack into an existing pane, plus lifecycle,
focus, resize, input, selection/read-text, and PTY/session hooks that do not
require Ghostty to own the whole application window.

## Next Cut

1. Add a Ghostty vendor patch or fork branch that builds a Linux shared library
   with GTK linked.
2. Export a minimal GTK surface constructor returning a `GtkWidget*`.
3. Add a ForkTTY feature-gated probe that creates the widget in isolation.
4. Only after the probe renders, replace one ForkTTY terminal pane behind a
   feature flag.

Do not replace ForkTTY's current renderer until step 4 is proven locally.

## Build Probe

Run the upstream GTK build probe with:

```bash
scripts/ghostty-gtk-build-probe.sh
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

The next implementation attempt should run the same script on CI/Ubuntu or a
Zig/toolchain combination that can link the GTK helper executables, then add
the minimal `GtkWidget*` embedding API there.
