# Embedded Ghostty Pane Parity Matrix

This is the living checklist that gates switching ForkTTY's default renderer
from the GTK/Pango/Cairo path to embedded Ghostty GTK panes
(`FORKTTY_GHOSTTY_GTK_PANES=1`). It complements [`QA.md`](QA.md) (the
per-release platform grid, which walks the *classic* renderer) and
[`ghostty-full-vendor.md`](ghostty-full-vendor.md) (the fork/ABI reference).

Each row is a behavior that must reach parity with classic panes before the
embedded renderer can become the default. The embedding library now ships in
release artifacts (roadmap item 3, "Slice C": release CI builds it best-effort
before packaging). The renderer switch (roadmap item 4) stays blocked until
every row is `pass` and that best-effort build is promoted to required.

## Verification constraint

The embedding library `ghostty-gtk-embed.so` cannot be built on the current
local toolchain (Zig 0.15.2 hits `fatal linker error: unhandled relocation type
R_X86_64_PC64` at `gtk_blueprint_compiler`; see
[`ghostty-renderer-embedding-spike.md`](ghostty-renderer-embedding-spike.md)).
So every *runtime* row is verified on GitHub's Ubuntu runner through the manual
**Ghostty GTK Probe** workflow, which builds the `.so` and runs the embedded
smoke under Xvfb. Contract-level rows (ABI symbol names, action grammar,
exit-status mapping) are pinned by host-runnable Rust unit tests.

## How verified — legend

- **auto (smoke)** — `scripts/gtk-ghostty-smoke.sh` run with
  `FORKTTY_GHOSTTY_GTK_PANES=1` in the Ghostty GTK Probe workflow. Drives the
  app through the socket API (send-text, read-screen, surfaces, split, focus,
  zoom, notifications) against live embedded panes.
- **auto (unit)** — host-runnable `cargo test` that pins the ForkTTY↔ABI
  contract (no `.so` needed).
- **probe** — covered by the `forktty ghostty-gtk-probe` widget smoke (launch /
  init / no-crash), but without a behavior assertion specific to the row.
- **manual** — walked by a maintainer against a locally built `.so`, or on the
  runner via VNC/screenshot, because the result is visual (glyphs, images,
  colors) and not observable through the socket API.

## Status — legend

`pass` · `pass with notes` · `fail` (file a blocker) · `pending` (not yet
exercised) · `n/a`.

## Matrix

| # | Dimension | Parity target | How verified | Status |
| - | --------- | ------------- | ------------ | ------ |
| 1 | Resize | Cols/rows track pane size and zoom; reflow matches classic | auto (smoke): zoom-in/out/reset asserts `cols`/`rows` change and restore | pending |
| 2 | Input | Keystrokes and socket `send_text` reach the child PTY | auto (smoke): `send-text` then `read-screen` readback of an echoed marker | pending |
| 3 | Scrollback | Full history is readable; persisted scrollback restores on respawn | auto (unit): `read_text(ALL)`/tail snapshot derivation; **manual**: scrollback persistence across restart | pending |
| 4 | OSC 8 hyperlinks | Hyperlinks render and are clickable | manual (visual; Ghostty renders natively) | pending |
| 5 | Images (Kitty/iTerm) | Inline images render in the embedded surface | manual (visual; Ghostty renders natively) | pending |
| 6 | Selection | Mouse drag selects; selection survives soft-wrap | manual (native to the embedded widget) | pending |
| 7 | Copy / paste | `Ctrl+Shift+C/V` + command palette copy/paste the selection | auto (unit): `perform_action` grammar pins `copy_to_clipboard`/`paste_from_clipboard`; **manual**: clipboard round-trip | pending |
| 8 | Search | `Ctrl+Shift+F` opens Ghostty's native search overlay | auto (unit): `start_search` grammar pinned; **manual**: overlay + navigate | pending |
| 9 | Exit / restart | Child exit flips readiness, sets status, raises abnormal-exit notification; session restore re-spawns | auto (unit): `embedded_child_exit_status` mapping; auto (smoke): surface lifecycle; **manual**: session restore after app restart | pending |
| 10 | Socket API | `surfaces` (incl. child PID), `read_text`, `capture_tail`, `send_text` behave as on classic panes | auto (smoke): surfaces/read-screen/send-text; auto (unit): capture_tail tail derivation, child-pid symbol | pending |

## Wiring already in place (so the rows above can pass)

- **Lifecycle** — title, child-exit readiness/status, abnormal-exit
  notification, and close-request teardown via the embedded `Surface` GObject
  signals; real exit code via `ghostty_gtk_surface_exit_code`.
- **Copy/paste/select-all/find** — `ghostty_gtk_surface_perform_action`
  (keybinding action by name); ForkTTY routes the `Ctrl+Shift+C/V/A/F`
  accelerators and command palette to the focused embedded surface.
- **Child PID** — `ghostty_gtk_surface_child_pid` fed by a `pid_available`
  surface mailbox message; ForkTTY records it for listening-port discovery and
  the socket `surfaces` PID field.
- **Socket text** — `send_text` / `read_text` (visible + full) ABIs back the
  socket `send_text`, `read_text`, and `capture_tail` requests.

## Promotion gate

Move roadmap item 4 (default switch) only when:

1. Every matrix row is `pass` (or `pass with notes` with a tracked follow-up).
2. The embedding `.so` ships in the deb/AppImage and its release-CI build is
   promoted from best-effort to required (item 3 "Slice C": release CI already
   runs `scripts/ghostty-gtk-lib-probe.sh` before packaging, non-fatal for now).
3. A clear runtime fallback to the classic renderer remains (e.g. when the
   library is absent or fails to load).
