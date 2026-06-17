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
| 1 | Resize | Cols/rows track pane size and zoom; reflow matches classic | auto (smoke): zoom-in/out/reset asserts `cols`/`rows` change and restore | pass |
| 2 | Input | Keystrokes and socket `send_text` reach the child PTY | auto (smoke): `send-text` then `read-screen` readback of an echoed marker | pass |
| 3 | Scrollback | Full history is readable; persisted scrollback restores on respawn | auto (unit): `read_text(ALL)`/tail snapshot derivation, snapshot + restore decision logic; auto (smoke): restart then `capture-tail` confirms a pre-restart marker was restored | pass |
| 4 | OSC 8 hyperlinks | Hyperlinks render and are clickable | manual (visual; Ghostty renders natively) | pending |
| 5 | Images (Kitty/iTerm) | Inline images render in the embedded surface | manual (visual; Ghostty renders natively); local manual 2026-06-18 confirmed Kitty graphics | pass with notes |
| 6 | Selection | Mouse drag selects; selection survives soft-wrap | manual (native to the embedded widget) | pending |
| 7 | Copy / paste | `Ctrl+Shift+C/V` + command palette copy/paste the selection | auto (unit): `perform_action` grammar pins `copy_to_clipboard`/`paste_from_clipboard`; **manual**: clipboard round-trip | pending |
| 8 | Search | `Ctrl+Shift+F` opens Ghostty's native search overlay | auto (unit): `start_search` grammar pinned; **manual**: overlay + navigate | pending |
| 9 | Exit / restart | Child exit flips readiness, sets status, raises abnormal-exit notification; session restore re-spawns | auto (unit): `embedded_child_exit_status` mapping; auto (smoke): child exit marks surface not writable and sets `Closed`; **manual**: session restore after app restart (local manual 2026-06-18) | pass |
| 10 | Socket API | `read_text`, `capture_tail`, `send_text`, and `surfaces` listing/focus behavior work on embedded panes | auto (smoke): surfaces/read-screen/capture-tail/send-text plus action and socket splits | pass |
| 11 | Port discovery / child PID | Embedded panes populate child PID in socket `surfaces` so listening-port discovery reaches classic-pane parity | auto (unit): child-pid symbol; auto (smoke): Probe requires a positive `surfaces` PID for the initial embedded pane | pass |

## Local manual validation — 2026-06-18

Run against the bundled embedded build
`target/packaging/appimage/forktty-0.2.0-alpha.13-x86_64-ghostty-opengl.AppImage`
(which ships `usr/lib/ghostty-gtk-embed.so`, so embedded panes are real here even
though the `.so` cannot be linked from the local source toolchain). Launched with
`FORKTTY_GHOSTTY_GTK_PANES=1`; the running GUI process had
`ghostty-gtk-embed.so` mapped, and the focused pane reported a child PID, so the
panes exercised below are the embedded renderer, not the classic fallback.

Observation method in this environment: pane content was driven through the
socket API (`forktty send-text --surface-id <id> --text …`, newline appended to
execute), screen rendering was captured with `spectacle -b -n -a -o <png>` and
read back, and surface/exit state was read with `forktty surfaces [--json]`.

**Validated:**

- **Row 5 (Images) → pass with notes.** A generated 168×98 PNG was emitted with
  the Kitty graphics protocol (`\e_Gf=100,a=T,m=…;<base64>\e\\`, chunked) and the
  iTerm2 protocol (`OSC 1337;File=inline=1;…:<base64>\a`) from one script. The
  Kitty image rendered correctly in the embedded surface; the iTerm2 image did
  **not** render. This is expected: in the pinned Ghostty fork, OSC 1337 `File`
  is in the unimplemented set (`src/terminal/osc/parsers/iterm2.zig`), so Ghostty
  supports Kitty graphics but not the iTerm2 inline-image protocol. The classic
  renderer draws no inline images at all, so Kitty support already meets/exceeds
  classic parity; the iTerm2 gap is an upstream-Ghostty limitation, tracked as
  the follow-up note for this row.
- **Row 9 (Exit / restart / session restore) → pass.** Sending `exit 3` to the
  focused embedded pane flipped the workspace to an `EXITED` badge reading
  `Terminal: Exited (3)` (exit-code mapping), raised an abnormal-exit
  notification (notification badge incremented), and re-spawned a fresh shell in
  the pane. `SIGTERM` to the GUI followed by relaunch restored all three
  workspaces and their embedded panes (re-spawn confirmed visually and via
  `surfaces`). Scrollback *content* is not persisted across restart because
  `persistent_scrollback_lines` is unset in the local config — that path is
  Row 3, already covered by the Probe.
- **Row 4 (OSC 8 hyperlinks) — rendering confirmed, click not exercised.** Two
  OSC 8 hyperlinks were emitted and their anchor text rendered correctly in the
  embedded surface. The hover-underline and click-to-open behavior could not be
  exercised (see blocker below), so this row stays `pending` rather than `pass`.

**Could not validate in this environment (left `pending`):**

The validating agent cannot synthesize pointer or keyboard input into the GUI:
the session has no running `ydotoold` and the account is not in the `input`
group, so `/dev/uinput` (`root:input`) is inaccessible; the socket/CLI exposes no
clicked-selection, clipboard, or Ghostty-action verb (`action-run` is for
*project* commands, not terminal keybinding actions), and socket `send_text`
reaches only the child PTY, not GTK accelerators. Therefore these rows require a
maintainer at a real pointer/keyboard (or the runner Probe):

- **Row 4 (OSC 8)** — pointer hover/click on the rendered link (render already
  confirmed above).
- **Row 6 (Selection)** — mouse drag-select, including across soft-wrapped and
  wide-character lines.
- **Row 7 (Copy / paste)** — `Ctrl+Shift+C/V` and command-palette clipboard
  round-trip (the `perform_action` grammar half is already `auto (unit)`).
- **Row 8 (Search)** — `Ctrl+Shift+F` to open Ghostty's native search overlay and
  navigate (the `start_search` grammar half is already `auto (unit)`).

## Wiring already in place (so the rows above can pass)

- **Lifecycle** — title, child-exit readiness/status, abnormal-exit
  notification, and close-request teardown via the embedded `Surface` GObject
  signals; real exit code via `ghostty_gtk_surface_exit_code`.
- **Copy/paste/select-all/find** — `ghostty_gtk_surface_perform_action`
  (keybinding action by name); ForkTTY routes the `Ctrl+Shift+C/V/A/F`
  accelerators, clear-screen command, and command palette to the focused
  embedded surface. `clear_screen` is Ghostty's native clear action; a full
  VT-state reset remains blocked on a dedicated Ghostty action.
- **Child PID** — `ghostty_gtk_surface_child_pid` fed by a `pid_available`
  surface mailbox message; ForkTTY records it for listening-port discovery and
  the socket `surfaces` PID field, and the Probe requires the initial embedded
  pane to expose a positive PID.
- **Socket and agent text** — `send_text` / `read_text` (visible + full) ABIs
  back the socket `send_text`, `read_text`, and `capture_tail` requests, plus
  Agent HUD tail reads and inline agent replies.
- **Scrollback snapshot** — when `appearance.persistent_scrollback_lines > 0`,
  embedded panes snapshot their scrollback tail into
  `surface.persisted_scrollback` on child exit, on programmatic close/restart,
  and via a throttled poll (`read_text_snapshot(Tail)` +
  `set_surface_persisted_scrollback`), so a later session save keeps recent
  embedded output. The ABI read never holds the model lock, and an unchanged
  tail skips the model write.
- **Scrollback restore (gated)** — on respawn ForkTTY computes terminal-ready
  bytes from `persisted_scrollback` (same CR/LF normalization as classic panes)
  and seeds them through the optional `ghostty_gtk_surface_restore_scrollback`
  ABI, which feeds Ghostty's VT stream and never the child PTY. The pinned fork
  now exports the symbol (IO-thread `inject_output` → `Termio.processOutput`); a
  library built before it still degrades to a safe no-op. The Ghostty GTK Probe
  verifies the restart round-trip under Xvfb.

## Verified — scrollback restore ABI

The Ghostty fork now ships the `ghostty_gtk_surface_restore_scrollback` export
(pin `2d6400f56af4af03cc59ac5b87754de717cf6bdc`): an IO-thread `inject_output`
mailbox message routed to `Termio.processOutput` injects bytes into the surface's
VT stream without writing them to the child PTY. A raw GTK-main-thread feed into
`processOutput` was rejected because it races the IO thread's PTY reader; the
message route keeps all terminal mutation on the IO thread. The design is
documented in
[`ghostty-renderer-embedding-spike.md`](ghostty-renderer-embedding-spike.md).

The fork commit was verified locally as far as the toolchain allows
(`zig fmt --check`, `zig ast-check`, and the `zig build test -Dapp-runtime=none`
core suite — which runs the `@sizeOf(Message) == 40` assertion and compiles
`Surface.injectOutput`). The runtime restore round-trip is verified by the
manual **Ghostty GTK Probe** workflow: the Ubuntu runner builds the embedding
`.so`, the smoke restarts an embedded pane, and `capture-tail` confirms a
pre-restart marker survived in restored scrollback. The ForkTTY side snapshots
on close/restart before removing the embedded widget, so immediate restarts do
not depend on the throttled snapshot poll.

## Promotion gate

Move roadmap item 4 (default switch) only when:

1. Every matrix row is `pass` (or `pass with notes` with a tracked follow-up).
2. The embedding `.so` ships in the deb/AppImage and its release-CI build is
   promoted from best-effort to required (item 3 "Slice C": release CI already
   runs `scripts/ghostty-gtk-lib-probe.sh` before packaging, non-fatal for now).
3. A clear runtime fallback to the classic renderer remains (e.g. when the
   library is absent or fails to load).
