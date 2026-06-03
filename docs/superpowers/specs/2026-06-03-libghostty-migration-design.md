# Libghostty Migration Design

## Goal

Replace ForkTTY's VTE dependency with a terminal surface built on `libghostty-vt`, while keeping the GTK4/libadwaita application shell, workspace model, socket API, splits, tabs, browser feature, session restore, and notification model intact.

The migration is not a widget swap. VTE currently provides a GTK widget, PTY ownership, child spawning, terminal emulation, input handling, rendering, clipboard integration, shell-integration signals, bell handling, title updates, and child-exit signals. `libghostty-vt` provides terminal emulation state, render state, input encoders, formatters, and effects; ForkTTY must provide the PTY lifecycle, GTK widget, rendering, event translation, clipboard/selection behavior, and child-process monitoring.

## Non-Goals

- Do not replace GTK4/libadwaita with Ghostty's standalone Linux application.
- Do not embed the Ghostty executable or drive an external terminal process.
- Do not keep VTE as an alternate production backend after the migration is complete.
- Do not implement a full GPU renderer in the first working milestone. The initial renderer should be a correct GTK/Pango/Cairo renderer; GPU rendering can be evaluated after VTE has been removed.
- Do not preserve VTE-only shell-integration termprops. Prompt/status detection must move to ForkTTY hooks plus a visible-tail fallback.

## Current State

ForkTTY already has a useful backend boundary:

- `crates/forktty-terminal/src/lib.rs` defines `TerminalBackend`, `SpawnRequest`, `TerminalSurfaceState`, and the headless test backend.
- `crates/forktty-terminal/src/vte.rs` creates `vte4::Terminal`, spawns the child process with `spawn_async`, writes to the child with `feed_child`, and exposes VTE types to the GTK UI.
- `crates/forktty-ui-gtk/src/gtk_app/backend.rs` sends spawn/send/resize/close commands from the socket/runtime side onto the GTK thread.
- `crates/forktty-ui-gtk/src/gtk_app/controller.rs` stores live `VteTerminalWidget`s, builds pane chrome around them, and routes copy/paste/select/reset/focus actions.
- `crates/forktty-ui-gtk/src/gtk_app/terminal_signals.rs` wires VTE focus, title, contents-changed, shell-precmd, shell-preexec, bell, and child-exited signals into the workspace model and notifications.
- Packaging and docs currently declare GTK4, libadwaita, and VTE as runtime/build requirements.

The boundary is not yet clean enough for a backend swap because the GTK UI imports VTE extension traits and VTE-specific types directly.

## Upstream Constraints

Use `libghostty-vt` as the terminal emulation baseline. As of this design, `cargo info libghostty-vt` reports version `0.1.1`, license `MIT OR Apache-2.0`, and `rust-version = 1.93`. ForkTTY currently declares Rust `1.88`, so the migration must either:

1. raise ForkTTY's MSRV to at least Rust `1.93`, or
2. use the C FFI layer directly and keep a lower MSRV.

This design chooses option 1 because ForkTTY is already a native Linux app and the safe Rust bindings reduce FFI ownership risk. The implementation must update `Cargo.toml`, `README.md`, and `CONTRIBUTING.md` to declare the new MSRV.

The upstream `libghostty-vt` API is still in development. The migration must isolate it behind ForkTTY-owned types so later upstream breaking changes are contained in one crate/module.

## Proposed Architecture

Add a new terminal surface stack under `forktty-terminal` and keep GTK-specific wiring in `forktty-ui-gtk`.

### `forktty-terminal`

Create focused modules:

- `src/ghostty.rs`: feature-gated public adapter types and shared non-GTK API.
- `src/ghostty/pty.rs`: Unix PTY creation, child spawn, environment setup, resize, writes, and child-exit monitoring.
- `src/ghostty/core.rs`: single-threaded owner of `libghostty_vt::Terminal`, `RenderState`, formatter access, effects, and encoder option updates.
- `src/ghostty/events.rs`: terminal events emitted to the GTK app, such as title changes, bell, visible content changed, child exit, ready, and spawn failure.

`SpawnRequest::forktty_env()` remains the source of ForkTTY terminal environment variables. AppImage environment sanitization should move out of the VTE adapter into shared spawn helpers so it still applies to child shells.

### `forktty-ui-gtk`

Replace `VteController` with a backend-neutral terminal controller. The controller should store ForkTTY-owned terminal widgets, not upstream terminal types.

Create:

- `gtk_app/terminal_widget.rs`: `GhosttyTerminalWidget`, a GTK widget wrapper around `gtk::DrawingArea` plus event controllers.
- `gtk_app/terminal_renderer.rs`: Pango/Cairo rendering of the current libghostty render state.
- `gtk_app/terminal_input.rs`: GTK key/mouse/focus/paste translation into libghostty key/mouse/focus encoders.
- `gtk_app/terminal_clipboard.rs`: selection, copy, paste, bracketed paste handling, and select-all/reset behavior.
- `gtk_app/terminal_runtime.rs`: GTK-thread runtime that owns the PTY session handle, the `libghostty-vt` core, channels, and repaint scheduling.

Keep existing pane chrome, layout, workspace operations, command palette, settings dialog, and socket API semantics. Rename user-facing/internal VTE names only when the touched code requires it; avoid unrelated UI restyling.

## Runtime Data Flow

Spawn:

1. Socket/UI code creates a `SpawnRequest` as it does today.
2. `GtkTerminalBackend::spawn` records the surface state and sends `GtkTerminalCommand::Spawn`.
3. The GTK controller creates a `GhosttyTerminalWidget`.
4. `terminal_runtime` opens a PTY, forks/spawns the configured shell with sanitized environment, records the child PID, and marks the surface ready after the PTY and child are established.
5. A background reader task reads PTY bytes and forwards byte chunks to the GTK thread.
6. The GTK thread feeds bytes into `libghostty-vt`, updates render state, emits model events, and queues a widget redraw.

Input:

1. GTK event controllers receive key, mouse, focus, paste, and resize events.
2. `terminal_input` converts GTK events to libghostty encoders.
3. Encoded bytes are written to the PTY.
4. Paste uses libghostty paste safety helpers where available and falls back to bracketed paste when the terminal mode requires it.

Render:

1. `libghostty-vt` owns terminal state and render state.
2. `terminal_renderer` traverses the visible render grid and draws cells with Pango/Cairo.
3. The renderer uses the existing ForkTTY terminal themes and font settings.
4. Cursor, selection, bold/italic/inverse/underline, truecolor, indexed ANSI colors, and hyperlink styling are supported before VTE removal is considered complete.

Resize:

1. GTK allocation changes compute columns and rows from measured cell size.
2. The PTY receives `TIOCSWINSZ`.
3. `libghostty-vt` receives the new size and reflows according to its terminal state behavior.
4. `TerminalBackend::resize` remains available for socket-driven resize metadata.

Output Effects:

- Bell: map libghostty bell effects to ForkTTY notifications and audible-bell config.
- Title: map title effects to `WorkspaceModel::set_surface_title`.
- PTY responses: write device-status and other response bytes back to the PTY through the runtime writer.
- Contents changed: use render-state dirtiness to schedule redraw and visible-tail prompt checks.
- Child exit: use the PTY child watcher, not VTE signals, to mark the surface not ready and emit exit notifications.

## Prompt and Status Detection

The VTE implementation currently uses shell-precmd/shell-preexec signals when available and falls back to visible-tail prompt scanning. After migration:

- ForkTTY hook/socket events remain the preferred high-confidence source for agent status.
- Visible-tail scanning remains as the generic terminal fallback.
- Running/Ready shell status from VTE termprops is removed unless libghostty later exposes equivalent shell-integration effects.
- The notification copy should avoid claiming VTE shell integration. It should say that the terminal appears ready or that a ForkTTY hook reported readiness.

This preserves user-visible prompt notifications without depending on VTE-only behavior.

## Clipboard and Selection

VTE currently supplies copy, paste, select all, and reset/clear behavior. The Ghostty surface must implement:

- Mouse drag selection across the visible grid.
- Keyboard select all for scrollback plus visible content. If the safe Rust formatter cannot export the full buffer, implement this through the lower-level C formatter API inside the same clipboard milestone; VTE removal is blocked until select-all covers scrollback.
- Copy as plain text.
- Paste to PTY with bracketed paste mode respected.
- Context menu actions wired to the new widget methods.
- Reset and clear by clearing terminal state and sending `Ctrl+L` to the child, preserving current ForkTTY behavior.

## Configuration

Keep existing config keys:

- `appearance.font_family`
- `appearance.font_size`
- `appearance.scrollback_lines`
- `appearance.terminal_audible_bell`
- `appearance.terminal_theme`
- `appearance.terminal_renderer`

`terminal_renderer` remains a compatibility key but should no longer say that native GTK uses VTE. Docs should describe the native renderer as `libghostty-vt` backed.

## Cargo Features and Dependencies

Replace:

- `forktty-terminal/vte`
- `forktty-ui-gtk/gtk-vte`
- optional dependency `vte4`
- VTE system package requirements

With:

- `forktty-terminal/ghostty-vt`
- `forktty-ui-gtk/gtk-ghostty`
- dependency `libghostty-vt = "0.1.1"`
- Unix PTY dependency, preferably `nix` with only required features enabled
- GTK/Pango/Cairo dependencies already available through `gtk4`; add `pangocairo` only if required by the renderer

The final default feature should be `gtk-ghostty`. During migration only, both feature names may exist to keep incremental builds possible. The final cleanup removes `gtk-vte`, `forktty-terminal/vte`, and all `vte4` imports.

## Packaging and Documentation

Update:

- `README.md`
- `CONTRIBUTING.md`
- `SPEC.md`
- `SECURITY.md`
- `PRIVACY.md`
- `SUPPORT.md`
- `ROADMAP.md`
- `RELEASING.md`
- `scripts/build-deb.sh`
- `scripts/build-appimage.sh`
- Linux metainfo/desktop text if it mentions VTE

Remove VTE build/runtime package requirements. Add any new build requirements introduced by PTY/rendering dependencies. If `libghostty-vt-sys` builds Zig source or links native artifacts, packaging must document the required Zig/toolchain or vendor strategy. If the crates.io package builds without external Zig in release CI, document that instead.

## Testing Strategy

Unit tests:

- Preserve existing `HeadlessTerminalBackend` tests.
- Add PTY spawn tests around argv/env construction without launching arbitrary user shells.
- Add resize tests that assert PTY size and terminal state receive the same dimensions.
- Add terminal core tests that feed VT bytes and assert title, bell, text formatting, and PTY response effects.
- Add input encoder tests for representative key combinations used by ForkTTY shortcuts and terminal input.
- Add prompt fallback tests using formatted visible text.

GTK tests:

- Test that spawn creates a widget, marks the surface ready, and records the child PID.
- Test close removes widget/runtime state and does not leave orphan backend surfaces.
- Test copy/paste/select/reset actions call the new widget methods.
- Test context menu actions target the focused Ghostty widget.

Manual/runtime smoke tests:

- Launch a shell.
- Type text and run commands.
- Split panes and switch focus.
- Resize panes and confirm reflow.
- Copy/paste text.
- Run full-screen TUIs such as `vim`, `less`, and `htop`.
- Verify bell/title/exit notifications.
- Verify socket `surface.send_text`.
- Verify session restore respawns terminals.
- Build Debian and AppImage artifacts without VTE packages installed.

## Milestones

### M1: Backend Boundary Cleanup

Rename VTE-specific controller/backend names to terminal-neutral names where they cross module boundaries. Add a small `TerminalWidget` interface in the GTK app for copy, paste, select-all, reset, focus, resize, and child widget access. No behavior changes.

Success: current VTE implementation still builds and tests pass, but VTE types are contained in fewer files.

### M2: Ghostty Core and PTY Prototype

Add `libghostty-vt`, raise MSRV, implement PTY spawn/read/write/resize/exit handling, and feed PTY output into a `libghostty-vt` terminal core in tests. Do not render GTK yet.

Success: tests can spawn a controlled command through a PTY, feed output into the Ghostty core, and format visible text.

### M3: Minimal GTK Renderer

Create `GhosttyTerminalWidget` using `gtk::DrawingArea` and Pango/Cairo. Render plain text, ANSI colors, cursor, and basic scrollback.

Success: `cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty` opens an interactive shell pane without linking VTE.

### M4: Input, Resize, and Socket Parity

Implement key/mouse/focus/paste translation, PTY resize, and `surface.send_text` behavior.

Success: typing, common shortcuts, socket sends, split-pane focus, and pane resize work in the Ghostty build.

### M5: Effects, Notifications, and Session Lifecycle

Implement title, bell, child-exit, prompt fallback, readiness, restart pane, close pane, and orphan cleanup.

Success: existing notification/session/socket lifecycle tests pass against the Ghostty build.

### M6: Clipboard, Selection, and Context Menu

Implement selection, copy, paste, select-all, reset/clear, and terminal context menu parity.

Success: user-facing terminal actions work without VTE.

### M7: Remove VTE

Delete `crates/forktty-terminal/src/vte.rs`, remove `vte4` dependencies/features/imports, update packaging and docs, and rename release QA commands to `gtk-ghostty`.

Success: `cargo tree --workspace` contains no `vte4` or `vte4-sys`; release builds work without VTE development packages.

### M8: Hardening and QA

Run terminal torture tests, TUI smoke tests, packaging tests, AppImage runtime tests, and security review for PTY/environment handling.

Success: the libghostty build becomes the only supported native terminal runtime.

## Risks and Mitigations

- `libghostty-vt` API instability: isolate all direct use in `forktty-terminal::ghostty` and `gtk_app/terminal_runtime`.
- MSRV increase: document the Rust 1.93+ requirement before implementation and update CI/release docs in the same milestone that adds the dependency.
- Rendering complexity: start with Pango/Cairo to remove VTE first; defer GPU rendering until correctness is proven.
- PTY security regressions: keep shell path validation, reserved env protection, AppImage environment sanitization, payload limits, and socket targeting checks.
- Shortcut conflicts: GTK app shortcuts must still be handled by app actions, while terminal input must receive normal terminal keys.
- Prompt regression: rely on ForkTTY hooks and visible-tail fallback rather than trying to emulate VTE shell termprops.
- Packaging surprises: test Debian/AppImage builds in an environment without VTE packages before declaring VTE removed.

## Acceptance Criteria

The migration is complete when:

- No source file imports `vte4`.
- `Cargo.lock` contains no `vte4` or `vte4-sys`.
- Build scripts and package metadata no longer require VTE system packages.
- `cargo test --workspace --all-targets --no-default-features --features gtk-ghostty` passes.
- `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings` passes.
- `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty --release` passes.
- A manual smoke test confirms shell launch, typing, split panes, resize, copy/paste, socket send-text, title/bell/exit notifications, restart pane, close pane, and session restore.
