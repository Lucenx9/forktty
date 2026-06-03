# Libghostty Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace ForkTTY's VTE terminal runtime with a GTK4 surface backed by `libghostty-vt`, with no final VTE backend, dependency, import, package requirement, or feature gate.

**Architecture:** Keep the existing socket/workspace/backend boundary, but move all terminal emulation to ForkTTY-owned Ghostty modules. `forktty-terminal` owns spawn environment helpers, PTY lifecycle, Ghostty core state, formatter helpers, input/output event types, and OSC 9/99 metadata parsing; `forktty-ui-gtk` owns the GTK drawing widget, Pango/Cairo renderer, GTK event translation, clipboard/selection, and lifecycle wiring on the GTK main thread.

**Tech Stack:** Rust 1.93+, GTK4/libadwaita, `libghostty-vt = 0.1.1`, `nix` PTY/process APIs, Pango/Cairo through GTK, Tokio/socket stack already present, vendored `libghostty-vt-sys` build via git + Zig.

---

## File Structure

- Modify `Cargo.toml`: raise workspace MSRV to `1.93`; add workspace dependencies for `libghostty-vt`, `nix`, and any GTK renderer helper crate needed after compile discovery.
- Modify `crates/forktty-terminal/Cargo.toml`: replace `vte` with `ghostty-vt`; add `libghostty-vt`, `nix`, and `libc`; update description.
- Modify `crates/forktty-terminal/src/lib.rs`: expose `ghostty`; move shared child environment/AppImage sanitization out of the old VTE adapter; keep `TerminalBackend` and headless tests.
- Create `crates/forktty-terminal/src/spawn.rs`: shared argv/cwd/env helpers and AppImage runtime env sanitization.
- Create `crates/forktty-terminal/src/ghostty.rs`: public Ghostty adapter module with re-exports and tests.
- Create `crates/forktty-terminal/src/ghostty/events.rs`: `GhosttyEvent`, `ChildExit`, `TerminalMetadataEvent`.
- Create `crates/forktty-terminal/src/ghostty/metadata.rs`: streaming OSC 9/99 parser for ForkTTY metadata.
- Create `crates/forktty-terminal/src/ghostty/core.rs`: single-threaded `GhosttyCore`, `RenderFrame`, visible-text formatting, title/bell/PTY-response effects, resize/reset/paste helpers.
- Create `crates/forktty-terminal/src/ghostty/pty.rs`: PTY open/spawn/read/write/resize/child-exit helpers and tests with controlled commands.
- Delete `crates/forktty-terminal/src/vte.rs` in M7.
- Modify `crates/forktty-ui-gtk/Cargo.toml`: replace `gtk-vte` with `gtk-ghostty`; retarget `browser` to `gtk-ghostty`; remove `vte4`.
- Modify `crates/forktty-ui-gtk/src/main.rs`: gate app launch on `gtk-ghostty` and update disabled-feature message.
- Modify `crates/forktty-ui-gtk/src/socket_cli.rs`: update hook/doctor text and cfg names from `gtk-vte` to `gtk-ghostty`.
- Modify `crates/forktty-ui-gtk/src/cli.rs`: rename doctor field to `feature_gtk_ghostty`, update AppImage runtime library checks to include `libghostty-vt.so`, and remove VTE wording.
- Modify `crates/forktty-ui-gtk/src/gtk_app.rs`: remove `forktty_terminal::vte` imports; import new terminal modules.
- Modify `crates/forktty-ui-gtk/src/gtk_app/backend.rs`: rename `GtkVteBackend` to `GtkTerminalBackend` without changing backend semantics.
- Modify `crates/forktty-ui-gtk/src/gtk_app/controller.rs`: rename `VteController` to `TerminalController`; store ForkTTY-owned `GhosttyTerminalWidget`s; route commands to runtime methods.
- Create `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`: GTK wrapper around `gtk::DrawingArea`, event controllers, focus, context hooks, and public copy/paste/select/reset methods.
- Create `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`: GTK-thread runtime owning `GhosttyCore`, PTY writer/reader channels, child PID, repaint scheduling, event drain, and readiness.
- Create `crates/forktty-ui-gtk/src/gtk_app/terminal_renderer.rs`: Pango/Cairo renderer for `RenderFrame`.
- Create `crates/forktty-ui-gtk/src/gtk_app/terminal_input.rs`: GTK key/mouse/focus/paste translation into Ghostty encoders.
- Create `crates/forktty-ui-gtk/src/gtk_app/terminal_clipboard.rs`: selection model, copy, paste, select-all, and reset helpers.
- Modify `crates/forktty-ui-gtk/src/gtk_app/terminal_signals.rs`: replace VTE signal handlers with Ghostty event/model handlers and visible-tail prompt checks.
- Modify `crates/forktty-ui-gtk/src/gtk_app/terminal_appearance.rs`: keep colors/fonts but apply them to Ghostty renderer/widget.
- Modify `crates/forktty-ui-gtk/src/gtk_app/app.rs`, `actions.rs`, `settings_dialog.rs`, `ui_common.rs`, `placeholders.rs`, and tests as touched by controller/doctor renames.
- Modify `crates/forktty-core/src/config.rs`: accept legacy `terminal_renderer = "vte"` and normalize to `auto`; update validation text/tests.
- Modify docs and packaging: `README.md`, `CONTRIBUTING.md`, `SPEC.md`, `SECURITY.md`, `PRIVACY.md`, `SUPPORT.md`, `ROADMAP.md`, `RELEASING.md`, `docs/native-gtk-vte.md` replacement, `docs/release-qa.md`, `docs/QA.md`, `scripts/build-deb.sh`, `scripts/build-appimage.sh`, and Linux metadata if needed.

## Milestone M1: Backend Boundary Cleanup

### Task 1: Rename GTK Backend Without Behavior Change

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app/backend.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/app.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`

- [ ] **Step 1: Write failing rename tests**

Add or update a GTK test that imports `GtkTerminalBackend` and verifies spawn/send/resize readiness through the existing channel:

```rust
#[test]
fn gtk_terminal_backend_blocks_send_until_ready() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let backend = GtkTerminalBackend::new(tx);
    let request = test_spawn_request("surface-1");

    backend.spawn(request).unwrap();
    assert!(matches!(
        backend.send_text("surface-1", "echo ok\n"),
        Err(TerminalError::NotReady(id)) if id == "surface-1"
    ));
    backend.mark_surface_ready("surface-1").unwrap();
    backend.send_text("surface-1", "echo ok\n").unwrap();
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk gtk_terminal_backend_blocks_send_until_ready --no-default-features --features gtk-vte`

Expected: FAIL because `GtkTerminalBackend` does not exist yet.

- [ ] **Step 3: Rename the type**

Rename `GtkVteBackend` to `GtkTerminalBackend` and update call sites. Do not change command enum behavior.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p forktty-ui-gtk gtk_terminal_backend_blocks_send_until_ready --no-default-features --features gtk-vte`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/backend.rs crates/forktty-ui-gtk/src/gtk_app/app.rs crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/gtk_app/tests.rs
git commit -m "refactor: neutralize gtk terminal backend name"
```

### Task 2: Add Terminal Widget Interface Around Existing VTE Widget

**Files:**
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`

- [ ] **Step 1: Write failing interface test**

Add a small unit test for a fake `TerminalWidgetOps` implementor so controller code can depend on methods instead of VTE extension traits:

```rust
#[test]
fn terminal_widget_ops_reset_sends_form_feed() {
    let widget = TestTerminalWidget::default();
    widget.reset_and_clear();
    assert_eq!(widget.sent_text(), vec!["\x0c"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk terminal_widget_ops_reset_sends_form_feed --no-default-features --features gtk-vte`

Expected: FAIL because `TerminalWidgetOps` and `TestTerminalWidget` do not exist.

- [ ] **Step 3: Implement VTE-backed adapter only**

Create `TerminalWidgetOps` with:

```rust
pub(super) trait TerminalWidgetOps {
    fn widget(&self) -> gtk::Widget;
    fn has_focus(&self) -> bool;
    fn grab_focus(&self);
    fn copy_text(&self);
    fn paste_text(&self);
    fn select_all_text(&self);
    fn reset_and_clear(&self);
    fn send_text(&self, text: &str);
    fn resize_cells(&self, cols: u16, rows: u16);
}
```

Wrap the current VTE calls in a temporary `VteTerminalWidgetAdapter` so M1 has no runtime behavior change.

- [ ] **Step 4: Run focused and workspace tests**

Run: `cargo test -p forktty-ui-gtk terminal_widget --no-default-features --features gtk-vte`

Run: `cargo test --workspace --all-targets --no-default-features --features gtk-vte`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/gtk_app/controller.rs crates/forktty-ui-gtk/src/gtk_app/tests.rs
git commit -m "refactor: introduce terminal widget interface"
```

## Milestone M2: Ghostty Core and PTY Prototype

### Task 3: Add Ghostty Feature, MSRV, and Shared Spawn Helpers

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/forktty-terminal/Cargo.toml`
- Modify: `crates/forktty-terminal/src/lib.rs`
- Create: `crates/forktty-terminal/src/spawn.rs`
- Create: `crates/forktty-terminal/src/ghostty.rs`

- [ ] **Step 1: Write failing spawn helper tests**

Move existing VTE environment tests into `spawn.rs` and add:

```rust
#[test]
fn child_environment_is_available_without_vte_feature() {
    let request = test_spawn_request();
    let env = child_environment(&request);
    assert!(env.iter().any(|entry| entry == "TERM=xterm-256color"));
    assert!(env.iter().any(|entry| entry == "COLORTERM=truecolor"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-terminal child_environment_is_available_without_vte_feature --no-default-features`

Expected: FAIL because helpers are still hidden in `vte.rs`.

- [ ] **Step 3: Implement shared spawn helpers and feature metadata**

Add `ghostty-vt = ["dep:libghostty-vt", "dep:nix", "dep:libc"]`, raise `rust-version = "1.93"` in workspace, and move `child_environment`, `child_argv`, `child_cwd`, and AppImage sanitization to `spawn.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-terminal --no-default-features`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/forktty-terminal/Cargo.toml crates/forktty-terminal/src/lib.rs crates/forktty-terminal/src/spawn.rs crates/forktty-terminal/src/ghostty.rs
git commit -m "feat: add ghostty terminal feature boundary"
```

### Task 4: Implement OSC 9/99 Metadata Parser

**Files:**
- Create: `crates/forktty-terminal/src/ghostty/events.rs`
- Create: `crates/forktty-terminal/src/ghostty/metadata.rs`
- Modify: `crates/forktty-terminal/src/ghostty.rs`

- [ ] **Step 1: Write failing parser tests**

```rust
#[test]
fn parses_fragmented_osc_99_metadata() {
    let mut parser = MetadataParser::new();
    assert!(parser.feed(b"\x1b]99;status=ready").is_empty());
    let events = parser.feed(b";label=Build\x07");
    assert_eq!(events, vec![TerminalMetadataEvent::Osc99 {
        payload: "status=ready;label=Build".to_string()
    }]);
}

#[test]
fn parses_osc_9_metadata_with_st_terminator() {
    let mut parser = MetadataParser::new();
    let events = parser.feed(b"\x1b]9;notify=done\x1b\\");
    assert_eq!(events, vec![TerminalMetadataEvent::Osc9 {
        payload: "notify=done".to_string()
    }]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-terminal metadata --no-default-features --features ghostty-vt`

Expected: FAIL because parser module is missing.

- [ ] **Step 3: Implement streaming parser**

Implement a byte-state parser for `ESC ] <code> ; <payload> BEL` and `ESC ] <code> ; <payload> ESC \`, emitting only code `9` and `99`, capping payloads at 16 KiB, and resetting on malformed/incomplete overflow.

- [ ] **Step 4: Run parser tests**

Run: `cargo test -p forktty-terminal metadata --no-default-features --features ghostty-vt`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-terminal/src/ghostty.rs crates/forktty-terminal/src/ghostty/events.rs crates/forktty-terminal/src/ghostty/metadata.rs
git commit -m "feat: parse terminal metadata osc sequences"
```

### Task 5: Implement Ghostty Core

**Files:**
- Create: `crates/forktty-terminal/src/ghostty/core.rs`
- Modify: `crates/forktty-terminal/src/ghostty/events.rs`
- Modify: `crates/forktty-terminal/src/ghostty.rs`

- [ ] **Step 1: Write failing core tests**

```rust
#[test]
fn core_formats_visible_text_and_emits_title_and_bell() {
    let mut core = GhosttyCore::new(GhosttyCoreOptions {
        cols: 20,
        rows: 4,
        scrollback_lines: 100,
    }).unwrap();
    let events = core.feed(b"hello\r\n\x1b]2;ForkTTY\x1b\\\x07").unwrap();
    assert!(events.iter().any(|event| matches!(event, GhosttyEvent::TitleChanged(title) if title == "ForkTTY")));
    assert!(events.iter().any(|event| matches!(event, GhosttyEvent::Bell)));
    assert!(core.visible_text().unwrap().contains("hello"));
}

#[test]
fn core_collects_pty_responses() {
    let mut core = GhosttyCore::new(GhosttyCoreOptions { cols: 80, rows: 24, scrollback_lines: 0 }).unwrap();
    let events = core.feed(b"\x1B[?7$p").unwrap();
    assert!(events.iter().any(|event| matches!(event, GhosttyEvent::PtyWrite(bytes) if !bytes.is_empty())));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-terminal ghostty::core --no-default-features --features ghostty-vt`

Expected: FAIL because `GhosttyCore` is missing.

- [ ] **Step 3: Implement core**

Create `GhosttyCore` with `libghostty_vt::Terminal`, `RenderState`, `key::Encoder`, `mouse::Encoder`, and metadata parser. Register `on_pty_write`, `on_bell`, and `on_title_changed` callbacks using `Rc<RefCell<Vec<GhosttyEvent>>>`. Implement `feed`, `resize`, `reset`, `visible_text`, `select_all_text`, `bracketed_paste_bytes`, and `render_frame`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-terminal ghostty::core --no-default-features --features ghostty-vt`

Expected: PASS, unless Zig is missing; if Zig is missing, install/provide Zig and rerun before coding beyond this point.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-terminal/src/ghostty.rs crates/forktty-terminal/src/ghostty/core.rs crates/forktty-terminal/src/ghostty/events.rs
git commit -m "feat: add ghostty terminal core"
```

### Task 6: Implement PTY Spawn/Read/Write/Resize/Exit

**Files:**
- Create: `crates/forktty-terminal/src/ghostty/pty.rs`
- Modify: `crates/forktty-terminal/src/ghostty/events.rs`
- Modify: `crates/forktty-terminal/src/ghostty.rs`

- [ ] **Step 1: Write failing PTY tests**

```rust
#[test]
fn pty_spawns_controlled_command_and_reads_output() {
    let request = test_spawn_request_for_shell("/bin/sh")
        .with_args(["-lc", "printf forktty-pty"]);
    let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
    let output = session.read_until(b"forktty-pty", Duration::from_secs(2)).unwrap();
    assert!(output.windows("forktty-pty".len()).any(|w| w == b"forktty-pty"));
    assert_eq!(session.wait_timeout(Duration::from_secs(2)).unwrap().code(), Some(0));
}

#[test]
fn pty_resize_tracks_requested_size() {
    let request = test_spawn_request_for_shell("/bin/sh").with_args(["-lc", "sleep 1"]);
    let mut session = PtySession::spawn(&request, PtySize { cols: 80, rows: 24 }).unwrap();
    session.resize(PtySize { cols: 120, rows: 40 }).unwrap();
    assert_eq!(session.size(), PtySize { cols: 120, rows: 40 });
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-terminal ghostty::pty --no-default-features --features ghostty-vt`

Expected: FAIL because `PtySession` is missing.

- [ ] **Step 3: Implement PTY**

Use `nix::pty::openpty`, `nix::unistd::setsid`, `dup2`, `CommandExt::pre_exec`, nonblocking master reads, direct writer, `ioctl(TIOCSWINSZ)`, and `try_wait`/`wait_timeout` child monitoring. Keep `PtySession` `Send`-safe only for file descriptors/process handles; no Ghostty objects cross threads.

- [ ] **Step 4: Run PTY tests**

Run: `cargo test -p forktty-terminal ghostty::pty --no-default-features --features ghostty-vt`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-terminal/src/ghostty.rs crates/forktty-terminal/src/ghostty/pty.rs crates/forktty-terminal/src/ghostty/events.rs
git commit -m "feat: own terminal pty lifecycle"
```

## Milestone M3: Minimal GTK Renderer

### Task 7: Add Ghostty GTK Feature and Runtime Skeleton

**Files:**
- Modify: `crates/forktty-ui-gtk/Cargo.toml`
- Modify: `crates/forktty-ui-gtk/src/main.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app.rs`
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`

- [x] **Step 1: Write failing GTK runtime tests**

```rust
#[test]
fn ghostty_runtime_marks_surface_ready_after_spawn() {
    let runtime = TestTerminalRuntimeHarness::new();
    runtime.spawn(test_spawn_request("surface-1"));
    assert!(runtime.backend_ready("surface-1"));
    assert!(runtime.child_pid("surface-1").is_some());
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk ghostty_runtime_marks_surface_ready_after_spawn --no-default-features --features gtk-ghostty`

Expected: FAIL because feature/runtime are missing.

- [x] **Step 3: Implement feature and skeleton**

Add `gtk-ghostty = ["dep:adw", "dep:gtk4", "dep:global-hotkey", "dep:libloading", "forktty-terminal/ghostty-vt"]`; make `default = ["gtk-ghostty"]`; create a `GhosttyTerminalWidget` wrapping `gtk::DrawingArea` and `TerminalRuntime` owning `GhosttyCore` and `PtySession` on GTK main thread.

- [x] **Step 4: Run focused build**

Run: `cargo test -p forktty-ui-gtk terminal_runtime --no-default-features --features gtk-ghostty`

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/Cargo.toml crates/forktty-ui-gtk/src/main.rs crates/forktty-ui-gtk/src/gtk_app.rs crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs
git commit -m "feat: add ghostty gtk terminal runtime"
```

### Task 8: Render Plain Text, ANSI Colors, Cursor, and Scrollback

**Files:**
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_renderer.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_appearance.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/style.css`

- [x] **Step 1: Write failing renderer tests**

```rust
#[test]
fn renderer_maps_theme_colors_to_ansi_palette() {
    let config = config::AppConfig::default();
    let palette = RendererPalette::from_terminal_colors(terminal_colors_for_config(&config));
    assert_eq!(palette.ansi.len(), 16);
    assert_eq!(palette.background.to_string(), "#181818");
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk terminal_renderer --no-default-features --features gtk-ghostty`

Expected: FAIL because renderer is missing.

- [x] **Step 3: Implement renderer**

Use Pango layout measurement for cell width/height, draw background, per-cell background, graphemes, bold/italic/underline/inverse colors, hyperlinks as underline, and cursor from `RenderFrame`. Add CSS class `ghostty-terminal`.

- [x] **Step 4: Run renderer tests and build**

Run: `cargo test -p forktty-ui-gtk terminal_renderer --no-default-features --features gtk-ghostty`

Run: `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty`

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_renderer.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs crates/forktty-ui-gtk/src/gtk_app/terminal_appearance.rs crates/forktty-ui-gtk/src/gtk_app/style.css
git commit -m "feat: render ghostty terminal surface"
```

## Milestone M4: Input, Resize, and Socket Parity

### Task 9: Implement GTK Input Translation

**Files:**
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_input.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`

- [x] **Step 1: Write failing input tests**

```rust
#[test]
fn key_translation_encodes_enter_and_ctrl_c() {
    assert_eq!(encode_test_key(GhosttyKeySpec::enter()).unwrap(), b"\r");
    assert_eq!(encode_test_key(GhosttyKeySpec::ctrl('c')).unwrap(), b"\x03");
}
```

- [x] **Step 2: Run test to verify it fails**

Run: `cargo test -p forktty-ui-gtk terminal_input --no-default-features --features gtk-ghostty`

Expected: FAIL because input module is missing.

- [x] **Step 3: Implement input**

Translate GTK `EventControllerKey`, mouse press/release/motion/scroll, focus gained/lost, and IM text into `GhosttyCore` encoder methods, then write encoded bytes to PTY. Preserve app accelerators by returning `glib::Propagation::Proceed` for ForkTTY shortcuts.

- [x] **Step 4: Run tests**

Run: `cargo test -p forktty-ui-gtk terminal_input --no-default-features --features gtk-ghostty`

Expected: PASS.

- [x] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_input.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs
git commit -m "feat: translate gtk input for ghostty terminals"
```

### Task 10: Implement Resize and Socket Send Text

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`

- [ ] **Step 1: Write failing parity tests**

```rust
#[test]
fn socket_send_text_writes_to_runtime_pty() {
    let harness = TestTerminalRuntimeHarness::new_ready("surface-1");
    harness.controller_send_text("surface-1", "echo ok\n");
    assert_eq!(harness.pty_writes("surface-1"), vec![b"echo ok\n".to_vec()]);
}

#[test]
fn allocation_resize_updates_pty_and_core() {
    let harness = TestTerminalRuntimeHarness::new_ready("surface-1");
    harness.resize_pixels("surface-1", 800, 480, 10, 20);
    assert_eq!(harness.runtime_size("surface-1"), (80, 24));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p forktty-ui-gtk terminal_runtime --no-default-features --features gtk-ghostty`

Expected: FAIL because controller/runtime parity is incomplete.

- [ ] **Step 3: Implement parity**

Route `GtkTerminalCommand::SendText` to `TerminalRuntime::write_text`, `Resize` to `resize_cells`, and drawing-area allocation to cell measurement based resize. Save session resize metadata as before.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-ui-gtk terminal_runtime --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/controller.rs crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs
git commit -m "feat: wire ghostty resize and socket input"
```

## Milestone M5: Effects, Notifications, and Session Lifecycle

### Task 11: Wire Title, Bell, PTY Replies, Prompt Fallback, and OSC Metadata

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_signals.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`

- [ ] **Step 1: Write failing lifecycle tests**

```rust
#[test]
fn ghostty_events_update_model_title_and_bell_notification() {
    let harness = TestTerminalRuntimeHarness::new_ready("surface-1");
    harness.feed_events(vec![
        GhosttyEvent::TitleChanged("build".to_string()),
        GhosttyEvent::Bell,
    ]);
    assert_eq!(harness.surface_title("surface-1"), "build");
    assert!(harness.notifications().iter().any(|n| n.title == "Terminal bell"));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p forktty-ui-gtk terminal_signals --no-default-features --features gtk-ghostty`

Expected: FAIL because Ghostty events are not wired.

- [ ] **Step 3: Implement event handling**

Drain `GhosttyEvent`s after each feed: write `PtyWrite` bytes to PTY, set title, dispatch bell according to loaded config, run visible-tail prompt checks with copy `A terminal appears to be waiting for input`, and map OSC 9/99 payloads to existing ForkTTY hook/status handlers where current socket metadata semantics require it.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-ui-gtk terminal_signals --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_signals.rs crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs crates/forktty-ui-gtk/src/gtk_app/controller.rs
git commit -m "feat: handle ghostty terminal effects"
```

### Task 12: Implement Child Exit, Restart, Close, and Session Restore

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/workspace_ops.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/tests.rs`

- [ ] **Step 1: Write failing lifecycle tests**

```rust
#[test]
fn child_exit_marks_surface_not_ready_and_restart_respawns() {
    let harness = TestTerminalRuntimeHarness::new_ready("surface-1");
    harness.simulate_child_exit("surface-1", 7);
    assert!(!harness.backend_ready("surface-1"));
    assert!(harness.status_text("surface-1").contains("Exited (7)"));
    harness.restart_pane("surface-1");
    assert!(harness.backend_ready("surface-1"));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p forktty-ui-gtk child_exit_marks_surface_not_ready_and_restart_respawns --no-default-features --features gtk-ghostty`

Expected: FAIL until child watcher and restart path are connected.

- [ ] **Step 3: Implement lifecycle**

Poll or receive child-exit messages from `PtySession`, remove PID by spawn token, mark backend not ready, set status/notifications, keep closed surfaces from re-spawning, and preserve session restore auto-spawn behavior for terminal and SSH surfaces only.

- [ ] **Step 4: Run lifecycle tests**

Run: `cargo test -p forktty-ui-gtk lifecycle --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs crates/forktty-ui-gtk/src/gtk_app/controller.rs crates/forktty-ui-gtk/src/gtk_app/workspace_ops.rs crates/forktty-ui-gtk/src/gtk_app/tests.rs
git commit -m "feat: complete ghostty terminal lifecycle"
```

## Milestone M6: Clipboard, Selection, and Context Menu

### Task 13: Implement Selection, Copy, Paste, Select-All, and Reset

**Files:**
- Create: `crates/forktty-ui-gtk/src/gtk_app/terminal_clipboard.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`

- [ ] **Step 1: Write failing clipboard tests**

```rust
#[test]
fn bracketed_paste_wraps_unsafe_multiline_text_when_enabled() {
    let mut core = GhosttyCore::new(GhosttyCoreOptions { cols: 80, rows: 24, scrollback_lines: 100 }).unwrap();
    core.set_bracketed_paste_for_test(true).unwrap();
    assert_eq!(
        core.paste_bytes("echo one\necho two").unwrap(),
        b"\x1b[200~echo one\necho two\x1b[201~"
    );
}

#[test]
fn select_all_uses_formatter_for_scrollback() {
    let mut core = GhosttyCore::new(GhosttyCoreOptions { cols: 10, rows: 2, scrollback_lines: 10 }).unwrap();
    core.feed(b"one\r\ntwo\r\nthree").unwrap();
    let text = core.select_all_text().unwrap();
    assert!(text.contains("one"));
    assert!(text.contains("three"));
}
```

- [ ] **Step 2: Run tests to verify failure**

Run: `cargo test -p forktty-terminal paste_bytes select_all_text --no-default-features --features ghostty-vt`

Expected: FAIL until paste/select-all helpers exist.

- [ ] **Step 3: Implement clipboard and selection**

Track drag selection in grid coordinates, format selected visible cells as plain text, use `GhosttyCore::select_all_text` for scrollback, write clipboard via GTK clipboard, paste through `paste::is_safe` plus bracketed paste mode, and implement reset as `core.reset()` plus PTY write `\x0c`.

- [ ] **Step 4: Run clipboard tests**

Run: `cargo test -p forktty-terminal ghostty::core --no-default-features --features ghostty-vt`

Run: `cargo test -p forktty-ui-gtk terminal_clipboard --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/terminal_clipboard.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs crates/forktty-ui-gtk/src/gtk_app/terminal_runtime.rs crates/forktty-ui-gtk/src/gtk_app/controller.rs crates/forktty-terminal/src/ghostty/core.rs
git commit -m "feat: add ghostty clipboard and selection"
```

### Task 14: Context Menu Parity

**Files:**
- Modify: `crates/forktty-ui-gtk/src/gtk_app/controller.rs`
- Modify: `crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs`

- [ ] **Step 1: Write failing context-menu test**

```rust
#[test]
fn context_menu_copy_targets_focused_ghostty_widget() {
    let harness = TestTerminalRuntimeHarness::new_ready("surface-1");
    harness.focus("surface-1");
    assert!(harness.controller().copy_focused_terminal());
    assert_eq!(harness.widget_calls("surface-1"), vec!["copy_text"]);
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p forktty-ui-gtk context_menu_copy_targets_focused_ghostty_widget --no-default-features --features gtk-ghostty`

Expected: FAIL until context menu uses Ghostty widget methods.

- [ ] **Step 3: Implement context menu parity**

Route copy/paste/select-all/reset actions through `TerminalWidgetOps` methods for focused and active widgets; keep split/new-tab/close items unchanged.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-ui-gtk context_menu --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/gtk_app/controller.rs crates/forktty-ui-gtk/src/gtk_app/terminal_widget.rs crates/forktty-ui-gtk/src/gtk_app/tests.rs
git commit -m "feat: route terminal context actions to ghostty widget"
```

## Milestone M7: Remove VTE

### Task 15: Remove VTE Features, Dependencies, Imports, and Lockfile Entries

**Files:**
- Modify: `crates/forktty-terminal/Cargo.toml`
- Modify: `crates/forktty-ui-gtk/Cargo.toml`
- Modify: `crates/forktty-terminal/src/lib.rs`
- Delete: `crates/forktty-terminal/src/vte.rs`
- Modify: all source files returned by `rg -n "vte4|gtk-vte|gtk_vte|Vte|VTE|vte" crates`

- [ ] **Step 1: Write failing absence checks**

Run: `rg -n "use vte4|vte4::|gtk-vte|forktty-terminal/vte|feature = \"vte\"|feature = \"gtk-vte\"" crates Cargo.toml`

Expected before cleanup: output contains VTE references.

- [ ] **Step 2: Remove VTE**

Delete `vte.rs`, remove `vte4` from manifests, remove `gtk-vte` and `vte` features, ensure `browser = ["gtk-ghostty", ...]`, and update cfgs in `main.rs`, `socket_cli.rs`, `cli.rs`, and GTK modules.

- [ ] **Step 3: Regenerate lockfile**

Run: `cargo update -p vte4 -p vte4-sys`

If Cargo refuses because packages are no longer present, run: `cargo generate-lockfile`.

- [ ] **Step 4: Run absence checks**

Run: `rg -n "use vte4|vte4::|gtk-vte|forktty-terminal/vte|feature = \"vte\"|feature = \"gtk-vte\"" crates Cargo.toml Cargo.lock`

Expected: no output.

Run: `rg -n "vte4|vte4-sys" Cargo.lock`

Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/forktty-terminal crates/forktty-ui-gtk
git commit -m "refactor: remove vte terminal backend"
```

### Task 16: Update Config Compatibility

**Files:**
- Modify: `crates/forktty-core/src/config.rs`
- Modify: `README.md`
- Modify: `SPEC.md`

- [ ] **Step 1: Write failing config tests**

```rust
#[test]
fn legacy_vte_renderer_normalizes_to_auto() {
    let raw = r#"
        [appearance]
        terminal_renderer = "vte"
    "#;
    let config = load_config_from_str(raw).unwrap();
    assert_eq!(config.appearance.terminal_renderer, "auto");
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p forktty-core legacy_vte_renderer_normalizes_to_auto`

Expected: FAIL because current normalization preserves `vte`.

- [ ] **Step 3: Implement compatibility**

Update validation to accept `"vte"` only as legacy input, normalize it to `"auto"` during load, and update error text to `auto, dom, canvas, webgl, ghostty`.

- [ ] **Step 4: Run tests**

Run: `cargo test -p forktty-core config`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-core/src/config.rs README.md SPEC.md
git commit -m "feat: normalize legacy vte renderer config"
```

### Task 17: Update Doctor, Docs, Packaging, and Runtime Bundling

**Files:**
- Modify: `crates/forktty-ui-gtk/src/cli.rs`
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs`
- Modify: `README.md`
- Modify: `CONTRIBUTING.md`
- Modify: `SPEC.md`
- Modify: `SECURITY.md`
- Modify: `PRIVACY.md`
- Modify: `SUPPORT.md`
- Modify: `ROADMAP.md`
- Modify: `RELEASING.md`
- Modify: `docs/release-qa.md`
- Modify: `docs/QA.md`
- Rename or replace: `docs/native-gtk-vte.md`
- Modify: `scripts/build-deb.sh`
- Modify: `scripts/build-appimage.sh`
- Modify: Linux metadata files if VTE is mentioned

- [ ] **Step 1: Write failing doctor tests**

```rust
#[test]
fn doctor_reports_gtk_ghostty_and_libghostty_runtime() {
    let report = DoctorReport {
        feature_gtk_ghostty: true,
        ..minimal_report()
    };
    let text = format_report(&report);
    assert!(text.contains("built with gtk-ghostty feature: true"));
    assert!(!text.contains("GTK/VTE"));
}
```

- [ ] **Step 2: Run test to verify failure**

Run: `cargo test -p forktty-ui-gtk doctor_reports_gtk_ghostty_and_libghostty_runtime --no-default-features --features gtk-ghostty`

Expected: FAIL because doctor still reports `gtk-vte`.

- [ ] **Step 3: Update docs and scripts**

Replace build commands with `--no-default-features --features gtk-ghostty`, remove VTE package names (`libvte-2.91-gtk4-dev`, `vte291-gtk4-devel`, `vte4`, `libvte-2.91-gtk4-0`), add `zig` and `git` build requirements, and make AppImage/deb copy or depend on `libghostty-vt.so.0.1.0` produced by `libghostty-vt-sys`.

- [ ] **Step 4: Run absence checks**

Run: `rg -n "VTE|vte|gtk-vte|libvte|vte291" README.md CONTRIBUTING.md SPEC.md SECURITY.md PRIVACY.md SUPPORT.md ROADMAP.md RELEASING.md docs scripts packaging crates`

Expected: only intentional historical note for `terminal_renderer = "vte"` legacy compatibility, if present.

- [ ] **Step 5: Commit**

```bash
git add crates/forktty-ui-gtk/src/cli.rs crates/forktty-ui-gtk/src/socket_cli.rs README.md CONTRIBUTING.md SPEC.md SECURITY.md PRIVACY.md SUPPORT.md ROADMAP.md RELEASING.md docs scripts packaging
git commit -m "docs: document ghostty terminal runtime"
```

## Milestone M8: Hardening and QA

### Task 18: Full Verification and Packaging Checks

**Files:**
- Modify only files required by failing checks.

- [ ] **Step 1: Ensure native prerequisites**

Run: `zig version`

Expected: version output. If missing, install Zig before final acceptance builds because `libghostty-vt-sys` vendored build invokes `zig build`.

- [ ] **Step 2: Run acceptance tests**

Run: `cargo test --workspace --all-targets --no-default-features --features gtk-ghostty`

Expected: PASS.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings`

Expected: PASS.

- [ ] **Step 4: Run release build**

Run: `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty --release`

Expected: PASS.

- [ ] **Step 5: Run dependency absence checks**

Run: `rg -n "use vte4|vte4::|gtk-vte|feature = \"vte\"|feature = \"gtk-vte\"" crates Cargo.toml Cargo.lock`

Run: `rg -n "vte4|vte4-sys|libvte|vte291|vte-2.91" Cargo.lock scripts packaging README.md CONTRIBUTING.md SPEC.md docs`

Expected: no output except documented legacy config value if the second check is narrowed to docs.

- [ ] **Step 6: Run manual smoke test**

Launch: `cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty`

Verify: shell launch, typing, split panes, resize, copy/paste, socket `send-text`, title/bell/exit notifications, restart pane, close pane, session restore, and browser feature build with `cargo test -p forktty-ui-gtk --all-targets --features browser`.

- [ ] **Step 7: Commit final hardening**

```bash
git add .
git commit -m "test: verify ghostty terminal migration"
```

## Self-Review

- Spec coverage: M1 covers backend boundary cleanup; M2 covers Rust 1.93, Ghostty core, PTY, spawn helpers, and OSC 9/99 parser; M3 covers GTK widget/rendering; M4 covers input, resize, and socket send-text; M5 covers effects, notifications, child exit, restart, close, and session restore; M6 covers clipboard/selection/context menu; M7 removes VTE and updates config/docs/packaging/doctor; M8 covers final QA and acceptance commands.
- Placeholder scan: all tasks have explicit files, tests, commands, expected outcomes, and commit commands. No implementation step relies on “TBD” behavior.
- Type consistency: core names used throughout are `GhosttyCore`, `GhosttyEvent`, `TerminalMetadataEvent`, `PtySession`, `GhosttyTerminalWidget`, `TerminalRuntime`, `GtkTerminalBackend`, and `TerminalController`.
- Known local prerequisite: `rustc 1.96.0` satisfies MSRV, but `zig` is not currently installed locally, so Ghostty vendored builds will fail until Zig is available.
