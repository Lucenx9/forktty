# Embedded Ghostty Pane Parity Matrix

This is the living checklist for the embedded Ghostty GTK pane renderer. It
complements [`QA.md`](QA.md) (the per-release platform grid) and
[`ghostty-full-vendor.md`](ghostty-full-vendor.md) (the fork/ABI reference).

Embedded Ghostty panes are the terminal renderer path. If
`ghostty-gtk-embed.so` is missing or fails to load, panes record a terminal
spawn failure instead of falling back to the classic GTK/Pango/Cairo renderer.
The four pointer/keyboard-only rows that could not be validated by automation
are explicitly deferred manual follow-ups.

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

- **auto (smoke)** — `scripts/gtk-ghostty-smoke.sh` run in the Ghostty GTK
  Probe workflow. Drives the app through the socket API (send-text,
  read-screen, surfaces, tab create/select/close, split, focus, live pane close,
  zoom, restart, notifications) against live embedded panes.
- **auto (unit)** — host-runnable `cargo test` that pins the ForkTTY↔ABI
  contract (no `.so` needed).
- **probe** — covered by the `forktty ghostty-gtk-probe` widget smoke (launch /
  init / no-crash), but without a behavior assertion specific to the row.
- **manual** — walked by a maintainer against a locally built `.so`, or on the
  runner via VNC/screenshot, because the result is visual (glyphs, images,
  colors) and not observable through the socket API.

## Status — legend

`pass` · `pass with notes` · `deferred` (accepted follow-up, not a default
blocker) · `fail` (file a blocker) · `pending` (not yet exercised) · `n/a`.

## Matrix

| # | Dimension | Parity target | How verified | Status |
| - | --------- | ------------- | ------------ | ------ |
| 1 | Launch | A clean app launch creates a live embedded terminal pane | probe: `forktty ghostty-gtk-probe`; auto (smoke): app launches under isolated DBus/XDG paths and `ping`/`surfaces` succeed | pass |
| 2 | Tabs | New tab, tab selection, and tab close keep embedded surfaces and model focus consistent | auto (smoke): `new-tab`, `select-tab`, `close-surface`, focus/readback assertions | pass |
| 3 | Split / focus | Split panes can be created by GTK action and socket API; focus moves between embedded panes | auto (smoke): action split, socket split, `focus-next-pane`, `focus-previous-pane`, focused-surface readback | pass |
| 4 | Resize | Cols/rows track pane size and zoom; reflow matches classic | auto (smoke): zoom-in/out/reset asserts `cols`/`rows` change and restore | pass |
| 5 | Input | Keystrokes and socket `send_text` reach the child PTY | auto (smoke): `send-text` then `read-screen` readback of echoed markers | pass |
| 6 | Close | Closing a live embedded surface removes it without stale model/widget state; child exit marks closed | auto (smoke): live split `close-surface` removes the pane; child `exit` marks the pane non-writable with `Closed` | pass |
| 7 | Restart / scrollback restore | Restart re-spawns the embedded pane; persisted scrollback restores on respawn | auto (unit): `read_text(ALL)`/tail snapshot derivation, snapshot + restore decision logic; auto (smoke): restart then `capture-tail` confirms a pre-restart marker was restored | pass |
| 8 | Session restore | Saved workspace/pane layout reopens embedded panes on app restart | manual: local manual 2026-06-18 confirmed session restore after GUI relaunch | pass |
| 9 | OSC 8 hyperlinks | Hyperlinks render and are clickable | manual (visual; Ghostty renders natively); rendering confirmed 2026-06-18, click deferred | deferred |
| 10 | Right click | Terminal context/right-click behavior reaches Ghostty or ForkTTY action as configured | manual: requires trusted pointer input and clipboard/menu observation | deferred |
| 11 | Images (Kitty/iTerm) | Inline images render in the embedded surface | manual (visual; Ghostty renders natively); local manual 2026-06-18 confirmed Kitty graphics | pass with notes |
| 12 | Selection | Mouse drag selects; selection survives soft-wrap | manual (native to the embedded widget) | deferred |
| 13 | Copy / paste | `Ctrl+Shift+C/V` + command palette copy/paste the selection | auto (unit): `perform_action` grammar pins `copy_to_clipboard`/`paste_from_clipboard`; **manual**: clipboard round-trip | deferred |
| 14 | Search | `Ctrl+Shift+F` opens Ghostty's native search overlay | auto (unit): `start_search` grammar pinned; **manual**: overlay + navigate | deferred |
| 15 | Socket API | `read_text`, `capture_tail`, `send_text`, and `surfaces` listing/focus behavior work on embedded panes | auto (smoke): surfaces/read-screen/capture-tail/send-text plus tab, action split, socket split, close, and focus checks | pass |
| 16 | Port discovery / child PID | Embedded panes populate child PID in socket `surfaces` so listening-port discovery reaches classic-pane parity | auto (unit): child-pid symbol; auto (smoke): Probe requires a positive `surfaces` PID for the initial embedded pane | pass |

## Local manual validation — 2026-06-18

Run against the bundled embedded build
`target/packaging/appimage/forktty-0.2.0-alpha.13-x86_64-ghostty-opengl.AppImage`
(which ships `usr/lib/ghostty-gtk-embed.so`, so embedded panes are real here even
though the `.so` cannot be linked from the local source toolchain). The running
GUI process had
`ghostty-gtk-embed.so` mapped, and the focused pane reported a child PID, so the
panes exercised below are the embedded renderer, not the classic fallback.

Observation method in this environment: pane content was driven through the
socket API (`forktty send-text --surface-id <id> --text …`, newline appended to
execute), screen rendering was captured with `spectacle -b -n -a -o <png>` and
read back, and surface/exit state was read with `forktty surfaces [--json]`.
Follow-up readback on the same date also confirmed the shipped embedded
AppImage maps `ghostty-gtk-embed.so`, exposes a positive child PID through
`surfaces --json`, and serves embedded `read-screen` / `capture-tail` through the
socket CLI once those documented commands are routed by the top-level CLI.

**Validated:**

- **Row 11 (Images) → pass with notes.** A generated 168×98 PNG was emitted with
  the Kitty graphics protocol (`\e_Gf=100,a=T,m=…;<base64>\e\\`, chunked) and the
  iTerm2 protocol (`OSC 1337;File=inline=1;…:<base64>\a`) from one script. The
  Kitty image rendered correctly in the embedded surface; the iTerm2 image did
  **not** render. This is expected: in the pinned Ghostty fork, OSC 1337 `File`
  is in the unimplemented set (`src/terminal/osc/parsers/iterm2.zig`), so Ghostty
  supports Kitty graphics but not the iTerm2 inline-image protocol. The classic
  renderer draws no inline images at all, so Kitty support already meets/exceeds
  classic parity; the iTerm2 gap is an upstream-Ghostty limitation, tracked as
  the follow-up note for this row.
- **Row 8 (Session restore) → pass.** Sending `exit 3` to the
  focused embedded pane flipped the workspace to an `EXITED` badge reading
  `Terminal: Exited (3)` (exit-code mapping), raised an abnormal-exit
  notification (notification badge incremented), and re-spawned a fresh shell in
  the pane. `SIGTERM` to the GUI followed by relaunch restored all three
  workspaces and their embedded panes (re-spawn confirmed visually and via
  `surfaces`). Scrollback *content* is not persisted across restart because
  `persistent_scrollback_lines` is unset in the local config — that path is
  Row 7, already covered by the Probe.
- **Row 9 (OSC 8 hyperlinks) — rendering confirmed, click deferred.** Two
  OSC 8 hyperlinks were emitted and their anchor text rendered correctly in the
  embedded surface. The hover-underline and click-to-open behavior could not be
  exercised (see blocker below), so this row is deferred rather than marked
  `pass`.

**Could not validate in this environment (deferred):**

The validating agent cannot synthesize trusted pointer or keyboard input into
the GUI. `wtype` fails because the compositor does not expose the
virtual-keyboard protocol. `/dev/uinput` has an ACL for this user, but
`ydotoold` is not usable in this session: with its default mouse setup it exits
after an Xwayland virtual-device error, and in keyboard-only/no-display mode the
server process still exits and leaves a stale socket that refuses client
connections. Activating the exported GTK actions over DBus is not a valid
substitute for the accelerator path: app-level copy/paste/select-all actions
intentionally require real GTK focus inside the terminal, while DBus activation
only focuses the application window. The socket/CLI also exposes no
clicked-selection, clipboard, or Ghostty-action verb (`action-run` is for
*project* commands, not terminal keybinding actions), and socket `send_text`
reaches only the child PTY, not GTK accelerators. Therefore these rows require a
maintainer at a real pointer/keyboard, a working input-injection daemon, or a
  Probe extension that can drive GTK input directly. These rows are accepted
  follow-ups and no longer block the default renderer switch:

- **Row 9 (OSC 8)** — pointer hover/click on the rendered link (render already
  confirmed above).
- **Row 10 (Right click)** — terminal context menu/right-click action behavior.
- **Row 12 (Selection)** — mouse drag-select, including across soft-wrapped and
  wide-character lines.
- **Row 13 (Copy / paste)** — `Ctrl+Shift+C/V` and command-palette clipboard
  round-trip (the `perform_action` grammar half is already `auto (unit)`).
- **Row 14 (Search)** — `Ctrl+Shift+F` to open Ghostty's native search overlay and
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
- **Socket and agent text** — `send_text` / bounded `read_text` (visible + full)
  ABIs back the socket `send_text`, `read_text`, `capture_tail`, and inline
  agent replies. The current embedding library exports
  `ghostty_gtk_surface_read_text_limited`, so Ghostty streams the requested text
  into a bounded buffer before ForkTTY copies the FFI payload. Explicit
  `read_text(all)` and full-scrollback tails may still scan scrollback, but they
  no longer materialize more than the requested byte budget plus one
  truncation-detection byte in either process.
- **Scrollback snapshot** — when `appearance.persistent_scrollback_lines > 0`,
  embedded panes snapshot their scrollback tail into
  `surface.persisted_scrollback` on child exit, on programmatic close/restart,
  and via a throttled poll (`read_text_snapshot(Tail)` +
  `set_surface_persisted_scrollback`), so a later session save keeps recent
  visible embedded output. The ABI read never holds the model lock, never asks
  Ghostty for full scrollback on the polling path, and an unchanged tail skips
  the model write.
- **Scrollback restore (gated)** — on respawn ForkTTY computes terminal-ready
  bytes from `persisted_scrollback` (same CR/LF normalization as classic panes)
  and seeds them through the optional `ghostty_gtk_surface_restore_scrollback`
  ABI, which feeds Ghostty's VT stream and never the child PTY. The pinned fork
  now exports the symbol (IO-thread `inject_output` → `Termio.processOutput`); a
  library built before it still degrades to a safe no-op. The Ghostty GTK Probe
  verifies the restart round-trip under Xvfb.

## Verified — scrollback restore ABI

The Ghostty fork now ships the `ghostty_gtk_surface_restore_scrollback` export
(pin `c26320d93448c42b78c5315a660e6d9359fcd26a`): an IO-thread `inject_output`
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

## Default renderer gate

Embedded Ghostty is accepted with rows 9/10/12/13/14 deferred. The remaining release
guard is:

1. The embedding `.so` ships in the deb/AppImage and its release-CI build is
   required.
2. Missing or failed embedded startup records an explicit terminal spawn
   failure; it must not silently open a classic-renderer pane.
3. Deferred rows 9/10/12/13/14 stay tracked here until a maintainer validates them
   with a real pointer/keyboard or a CI input driver.
