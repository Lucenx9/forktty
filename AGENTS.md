# AGENTS.md

Guidance for coding agents (Claude Code, Codex, Gemini CLI, …) working in this repository. `CLAUDE.md` and `GEMINI.md` are symlinks to this file.

ForkTTY is a Linux-only GTK4/libadwaita terminal multiplexer for coding agents: embedded Ghostty-backed terminals, a JSON-RPC Unix socket API, git worktree workflows, and agent hook integration. Rust workspace, AGPL-3.0-only, currently in alpha.

## Commands

Build and run the app:

```bash
cargo run -p forktty-ui-gtk                                          # default = gtk-ghostty
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty   # exact release config
```

The local gate — run the relevant subset before finishing a change. Before every push, run all of these if the local environment has the needed GTK/WebKit deps. `cargo fmt` fixes formatting; CI uses `--check`, and `-D warnings` means any clippy lint fails the build:

```bash
cargo fmt --all                # CI runs --check
cargo run -p xtask -- check    # repo consistency (hook templates, release automation)
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser
cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings
```

Additional PR CI parity / release-sensitive checks:

```bash
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
cargo build -p forktty-ui-gtk --no-default-features --features browser
desktop-file-validate packaging/linux/dev.forktty.forktty.desktop
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
scripts/gtk-ghostty-smoke.sh
bash scripts/build-deb.sh
readelf -d target/release/forktty | grep -E 'RUNPATH|RPATH' | grep -F '$ORIGIN/../lib'
cargo audit
```

Both `gtk-ghostty` and `browser` feature combinations must stay compiling even when you only touch one.

Single test / single crate:

```bash
cargo test -p forktty-ui-gtk --no-default-features --features gtk-ghostty <test_name>
cargo test -p forktty-core <module>::tests
```

Packaging (validate locally before tagging a release):

```bash
bash scripts/build-deb.sh        # → target/packaging/deb/; builds/verifies ghostty-gtk-embed.so
scripts/check-deb-piuparts.sh    # optional .deb install/purge check; defaults to Debian 13/trixie
bash scripts/build-appimage.sh   # → target/packaging/appimage/; builds/verifies ghostty-gtk-embed.so, needs appimagetool on PATH
```

## Critical constraints (violating these has broken releases before)

- **GTK 4.14 CSS compatibility**: `crates/forktty-ui-gtk/src/style.css` must NOT use CSS custom properties (`var(--x)` / `--x:` definitions) — the AppImage may run against a bundled GTK 4.14, which silently drops them (this shipped a release with no accent colors). Use literal colors and `@named_color` references only. Accent is `#e88745`, dark surfaces `#181818`/`#232323`.
- **libghostty pin**:
  - `[patch.crates-io]` in the workspace `Cargo.toml` points `libghostty-vt` AND `libghostty-vt-sys` at the vendored tree in `vendor/libghostty-rs` (upstream rev 20edad15 plus `FORKTTY PATCH`-marked changes) — never let the two crates resolve from different sources.
  - Keep the vendored build.rs `-Dcpu=baseline` zig flag for native builds; without it the built library's ISA follows the build host's CPU, and an AVX-512 CI runner shipped binaries that SIGILL on every non-AVX-512 machine (first alpha.10 cut).
  - Keep `.cargo/config.toml` setting `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe`; upstream maps Cargo debug profile to zig Debug, whose VT parser is ~870x slower and drags every test run.
  - Drop the vendor only when upstream pins the CPU baseline for native builds; see `vendor/libghostty-rs/FORKTTY-VENDORED.md`.
- **Ghostty scrollback is bytes, not lines**: the C API `max_scrollback` is a byte budget (upstream won't-fix). `forktty-terminal/src/ghostty/core.rs` converts via `SCROLLBACK_BYTES_PER_LINE = 2048`; this conversion is permanent.
- **Browser feature is source-only**: release artifacts (AppImage, deb) are built with `--no-default-features --features gtk-ghostty` and must never include the browser feature. Browser code stays behind `#[cfg(feature = "browser")]` and must keep compiling.
- **Single-instance app**: the GtkApplication uses DBus single-instance; a second launch delegates to the running one and exits immediately. Kill existing instances before launching for manual testing.
- **Test sockets**: bind under `$XDG_RUNTIME_DIR`, never `/tmp` — the socket security check rejects parents owned by another uid (e.g. root-owned `/tmp`).
- **AppImage library policy**: `usr/lib` carries ForkTTY's private runtime libraries (`libghostty-vt`, `ghostty-gtk-embed.so`, and `libgtk4-layer-shell.so`). `usr/lib/bundled` carries the GTK/libadwaita userspace stack and is always added by AppRun so terminal panes do not depend on host GTK packages. Canonical excludelist libraries (glibc, fontconfig, freetype, harfbuzz, wayland-*, X11/xcb, OpenGL/Vulkan/Mesa/driver stack) are never bundled.

## Architecture

Five workspace crates plus `xtask`, with a strict dependency flow: `forktty-core` (no GUI deps) ← `forktty-terminal` / `forktty-import` ← `forktty-socket` ← `forktty-ui-gtk` (the only binary, named `forktty`).

- **forktty-core** — domain logic and pure types: `config.rs` (TOML config with validation/quarantine), `model.rs` (workspaces/surfaces/panes), `protocol.rs` (socket JSON-RPC request/response types), `worktree.rs` (git2 worktree create/attach/remove/merge), `session.rs` (session-v2.json persistence), `agents.rs` + `notification.rs` (hook events), `command_safety.rs` (argv validation — no `sh -c` anywhere).
- **forktty-terminal** — terminal boundary types plus the headless test backend and legacy ForkTTY-owned libghostty-vt/PTY stack (`ghostty/pty.rs`, `ghostty/core.rs`). Current GTK terminal panes do not use this as a renderer fallback; they are embedded Ghostty GTK widgets. The `ghostty-vt` feature still gates libghostty-vt so core/socket tests don't need zig.
- **forktty-socket** — tokio Unix-socket JSON-RPC server logic, shared by the GTK app (server) and CLI (client). Owner-only permissions, size-bounded request lines.
- **forktty-ui-gtk** — the `forktty` binary is *both* the GTK app and the socket CLI: `main.rs` dispatches CLI subcommands (`cli.rs`, covered by Rust tests) vs. GUI launch. The GTK shell lives in `src/gtk_app/`: `controller.rs` is the central orchestrator (workspaces, pane tree, focus); `ghostty_gtk_embed.rs` loads `ghostty-gtk-embed.so` and drives embedded Ghostty surfaces; `pane_chrome.rs` wraps those surfaces in ForkTTY headers/dividers; `socket_server.rs` connects socket requests to controller actions. `terminal_runtime.rs`, `terminal_widget.rs`, and `terminal_renderer.rs` are legacy classic-pane cleanup debt unless the code you are reading proves a path still calls them. Pane chrome (header, dividers) is hidden when a workspace has a single pane — that's by design, not a bug.
- **forktty-import** — headless browser-profile import (history/bookmarks/cookies from Firefox/Chromium); `keyring` feature gates the Secret Service path.
- **hooks/** — agent hook templates (Claude Code, Codex, Gemini) installed by `forktty hooks setup`; after editing them run `cargo run -p xtask -- check`.

Useful CLI for inspecting a running instance: `forktty doctor`, `forktty list`, `forktty surfaces`, `forktty events`, `forktty capabilities`.

## Conventions

- Surgical edits only: don't reformat, restyle, or refactor code unrelated to the change (see CONTRIBUTING.md).
- Before applying a bug fix, check current, reliable web sources for the external behavior involved: prefer official docs, upstream source/issues, standards, or maintainer notes over blog posts and forum guesses. State which source informed the fix. If the issue is purely internal to ForkTTY and no external behavior is relevant, say that explicitly and ground the fix in local code/tests instead.
- Every user-visible change gets a `CHANGELOG.md` entry under `## [Unreleased]` (`Added`/`Changed`/`Fixed`/`Security` headings).
- Update `SPEC.md` when changing behavior it describes (config fields, socket methods, security boundaries).
- Keep the public website in sync. The separate site repo is usually checked out in the user's home directory as `forktty-site`; when a change affects install instructions, release assets, screenshots, public docs, README-facing behavior, privacy/security wording, hooks/MCP setup, Ghostty integration, settings/config, or visible UI flows, update the relevant site files in the same task (`app/docs/page.tsx`, `public/llms.txt`, `public/llms-full.txt`, home components, tests) and run `npm test` plus `npm run build` there. If the site workspace is unavailable, explicitly report the exact site update still needed instead of silently skipping it.
- Prefer tests that pin observable behavior (socket responses, validation rejections) over mocking internals. Tests that read env vars must guard with the existing `with_env` helper — tests run in parallel.
- Do not weaken enforced security boundaries to make a task easier: keep argv validation, owner-only socket checks, request size bounds, and local-first/privacy guarantees in code.
- Release process is in `RELEASING.md`; after a release publishes, download the actual assets and verify them end-to-end (run the AppImage, check theming/icons) — green CI alone has not been sufficient in the past.

## Change checklist

- User-visible behavior, UI text, CLI output, or packaging changed → update `CHANGELOG.md`.
- Config fields, socket methods, session format, or security boundaries changed → update `SPEC.md`.
- Public docs, install/release behavior, screenshots, hooks/MCP setup, Ghostty integration, settings/config, privacy/security wording, or visible UI changed → update the `forktty-site` checkout docs/agent context/home content and run its `npm test` + `npm run build`.
- Hook templates or release automation changed → run `cargo run -p xtask -- check`.
- Browser-gated code changed → run browser feature test, clippy, and build.
- Packaging/AppImage/runtime loader changed → build artifacts and smoke-test them.
- Dependencies changed → run `cargo audit` and justify the dependency.
