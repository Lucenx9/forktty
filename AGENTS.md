# AGENTS.md

Guidance for coding agents (Claude Code, Codex, Gemini CLI, …) working in this repository. `CLAUDE.md` and `GEMINI.md` are symlinks to this file.

ForkTTY is a Linux-only GTK4/libadwaita terminal multiplexer for coding agents: embedded Ghostty-backed terminals, a JSON-RPC Unix socket API, git worktree workflows, and agent hook integration. Rust workspace, AGPL-3.0-only, currently in alpha.

## Commands

Build and run the app:

```bash
cargo run -p forktty-ui-gtk                                          # default = gtk-ghostty
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty   # exact release config
```

The CI gate — run all of these before every push (CI runs the same set and `-D warnings` means any clippy lint fails the build):

```bash
cargo fmt --all                # CI runs --check; never push unformatted code
cargo run -p xtask -- check    # repo consistency (hook templates, release automation)
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings
```

CI also tests the browser feature (`cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser`), validates the desktop file, and builds the deb. Both feature combinations must stay compiling even when you only touch one.

Single test / single crate:

```bash
cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty <test_name>
cargo test -p forktty-core <module>::tests
```

Packaging (validate locally before tagging a release):

```bash
bash scripts/build-deb.sh        # → target/packaging/deb/
bash scripts/build-appimage.sh   # → target/packaging/appimage/ (needs appimagetool on PATH)
```

## Critical constraints (violating these has broken releases before)

- **GTK 4.14 CSS compatibility**: `crates/forktty-ui-gtk/src/style.css` must NOT use CSS custom properties (`var(--x)` / `--x:` definitions) — the AppImage may run against a bundled GTK 4.14, which silently drops them (this shipped a release with no accent colors). Use literal colors and `@named_color` references only. Accent is `#e88745`, dark surfaces `#181818`/`#232323`.
- **libghostty pin**: `[patch.crates-io]` in the workspace `Cargo.toml` pins `libghostty-vt` AND `libghostty-vt-sys` to the same git rev — never let them drift apart. `.cargo/config.toml` sets `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe`; never remove it (upstream maps Cargo debug profile to zig Debug, whose VT parser is ~870x slower and drags every test run). Drop the whole patch only when upstream publishes a crates.io release newer than 0.1.1.
- **Ghostty scrollback is bytes, not lines**: the C API `max_scrollback` is a byte budget (upstream won't-fix). `forktty-terminal/src/ghostty/core.rs` converts via `SCROLLBACK_BYTES_PER_LINE = 2048`; this conversion is permanent.
- **Browser feature is source-only**: release artifacts (AppImage, deb) are built with `--no-default-features --features gtk-ghostty` and must never include the browser feature. Browser code stays behind `#[cfg(feature = "browser")]` and must keep compiling.
- **Single-instance app**: the GtkApplication uses DBus single-instance; a second launch delegates to the running one and exits immediately. Kill existing instances before launching for manual testing.
- **Test sockets**: bind under `$XDG_RUNTIME_DIR`, never `/tmp` — the socket security check rejects parents owned by another uid (e.g. root-owned `/tmp`).
- **AppImage library policy**: `usr/lib` carries only the vendored libghostty; the host GUI stack is preferred. A GTK fallback lives in `usr/lib/bundled`, added to `LD_LIBRARY_PATH` by AppRun only when `ldconfig -p` finds no host libgtk-4. Canonical excludelist libraries (fontconfig, freetype, harfbuzz, wayland-*, X11/xcb) are never bundled.

## Architecture

Five workspace crates plus `xtask`, with a strict dependency flow: `forktty-core` (no GUI deps) ← `forktty-terminal` / `forktty-import` ← `forktty-socket` ← `forktty-ui-gtk` (the only binary, named `forktty`).

- **forktty-core** — domain logic and pure types: `config.rs` (TOML config with validation/quarantine), `model.rs` (workspaces/surfaces/panes), `protocol.rs` (socket JSON-RPC request/response types), `worktree.rs` (git2 worktree create/attach/remove/merge), `session.rs` (session-v2.json persistence), `agents.rs` + `notification.rs` (hook events), `command_safety.rs` (argv validation — no `sh -c` anywhere).
- **forktty-terminal** — PTY + VT layer over libghostty-vt: `ghostty/pty.rs` (fork/exec, CLOEXEC handling), `ghostty/core.rs` (VT state, scrollback, OSC parsing, theme-reset re-seeding). Behind the `ghostty-vt` feature so core/socket tests don't need zig.
- **forktty-socket** — tokio Unix-socket JSON-RPC server logic, shared by the GTK app (server) and CLI (client). Owner-only permissions, size-bounded request lines.
- **forktty-ui-gtk** — the `forktty` binary is *both* the GTK app and the socket CLI: `main.rs` dispatches CLI subcommands (`socket_cli.rs`, covered by Rust tests) vs. GUI launch. The GTK shell lives in `src/gtk_app/`: `controller.rs` is the central orchestrator (workspaces, pane tree, focus); `terminal_runtime.rs` bridges PTY I/O to the UI (pump loop, resize); `terminal_widget.rs` + `terminal_renderer.rs` draw cells and handle input/selection; `socket_server.rs` connects socket requests to controller actions. Pane chrome (header, dividers) is hidden when a workspace has a single pane — that's by design, not a bug.
- **forktty-import** — headless browser-profile import (history/bookmarks/cookies from Firefox/Chromium); `keyring` feature gates the Secret Service path.
- **hooks/** — agent hook templates (Claude Code, Codex, Gemini) installed by `forktty hooks setup`; after editing them run `cargo run -p xtask -- check`.

Useful CLI for inspecting a running instance: `forktty doctor`, `forktty list`, `forktty surfaces`, `forktty events`, `forktty capabilities`.

## Conventions

- Surgical edits only: don't reformat, restyle, or refactor code unrelated to the change (see CONTRIBUTING.md).
- Every user-visible change gets a `CHANGELOG.md` entry under `## [Unreleased]` (`Added`/`Changed`/`Fixed`/`Security` headings).
- Update `SPEC.md` when changing behavior it describes (config fields, socket methods, security boundaries).
- Prefer tests that pin observable behavior (socket responses, validation rejections) over mocking internals. Tests that read env vars must guard with the existing `with_env` helper — tests run in parallel.
- Release process is in `RELEASING.md`; after a release publishes, download the actual assets and verify them end-to-end (run the AppImage, check theming/icons) — green CI alone has not been sufficient in the past.
