# AGENTS.md

Guidance for coding agents (Claude Code, Codex, Pi, OpenCode, Antigravity, …) working in this repository. `CLAUDE.md` is a symlink to this file.

ForkTTY is a Linux-only GTK4/libadwaita workspace terminal for shells and coding agents: embedded Ghostty-backed terminals, a JSON-RPC Unix socket API, git worktree workflows, notifications/metadata, and optional agent lifecycle hooks. Rust workspace, AGPL-3.0-only, currently in alpha.

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
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser -- --test-threads=1
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
GitHub CI sets `FORKTTY_SKIP_GTK_WIDGET_TESTS=1` because headless runners can
segfault in GTK widget construction even after `gtk::init()` succeeds; local
widget tests remain opt-in by running without that variable.

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
scripts/check-appimage-bundled-container.sh /path/to/forktty.AppImage  # bundled/helper smoke; explicit path avoids ambiguity; needs bubblewrap
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
- **Worktree identity and transactions**: modeled worktree identity is the exact `(worktree_name, canonical_path)` pair; retries must reuse an existing exact-identity workspace ID and allocate no new modeled surface, without collapsing same-named worktrees at different canonical paths. GTK and socket paths must share `SocketAppState`'s process-local worktree/surface guards (worktree guard first) through commit or complete rollback. Removal must suppress controller auto-spawn before closing the exact target's surfaces and retain suppression through model commit or the complete rollback restoration attempt; if a rollback terminal respawn fails, record a blocking terminal error status before releasing suppression. These guards are not a cross-process Git lock.
- **AppImage library policy**: `usr/lib` carries ForkTTY's private runtime libraries (`libghostty-vt`, `ghostty-gtk-embed.so`, and `libgtk4-layer-shell.so`). `usr/lib/bundled` carries the GTK/libadwaita fallback. AppRun `auto` mode uses an eager loader compatibility probe with the effective host loader environment and selects host GTK only when both the ForkTTY binary and embedded Ghostty library load; otherwise it adds `usr/lib/bundled`. Canonical excludelist libraries (glibc, fontconfig, freetype, harfbuzz, wayland-*, X11/xcb, OpenGL/Vulkan/Mesa/driver stack) are never bundled.

## Architecture

Five workspace crates plus `xtask`, with a strict dependency flow: `forktty-core` (no GUI deps) ← `forktty-terminal` / `forktty-import` ← `forktty-socket` ← `forktty-ui-gtk` (the only binary, named `forktty`).

- **forktty-core** — domain logic and pure types: `config.rs` (TOML config with validation/quarantine), `model.rs` (workspaces/surfaces/panes), `protocol.rs` (socket JSON-RPC request/response types), `worktree.rs` (git2 worktree create/attach/remove/merge), `session.rs` (session-v2.json persistence), `agents.rs` + `notification.rs` (optional hook lifecycle and notifications), `project_actions.rs` (repo-local safe actions), `command_safety.rs` (argv validation — no `sh -c` anywhere).
- **forktty-terminal** — terminal boundary types plus the headless test backend and legacy ForkTTY-owned libghostty-vt/PTY stack (`ghostty/pty.rs`, `ghostty/core.rs`). Current GTK terminal panes do not use this as a renderer fallback; they are embedded Ghostty GTK widgets. The `ghostty-vt` feature still gates libghostty-vt so core/socket tests don't need zig.
- **forktty-socket** — tokio Unix-socket JSON-RPC server logic, shared by the GTK app (server) and CLI (client). Owner-only permissions, size-bounded request lines, bounded terminal reads, and methods for workspace/pane/surface control, notifications/metadata, context/status, thin agent lifecycle, remote inventory, worktrees, and project actions.
- **forktty-ui-gtk** — the `forktty` binary is the GTK app, socket CLI, and optional hook installer: `main.rs` dispatches CLI subcommands (`cli.rs`/`socket_cli.rs`, covered by Rust tests) vs. GUI launch. The GTK shell lives in `src/gtk_app/`: `controller.rs` coordinates workspaces, pane trees, and focus; `ghostty_gtk_embed.rs` loads `ghostty-gtk-embed.so`; `pane_chrome.rs` wraps surfaces in ForkTTY headers/dividers; `socket_server.rs` connects requests to controller actions. `agent_guide.rs` carries the compact context injected by hooks. `terminal_runtime.rs`, `terminal_widget.rs`, and `terminal_renderer.rs` are legacy classic-pane cleanup debt unless the code proves a path still calls them. Pane chrome is hidden when a workspace has a single pane by design.
- **forktty-import** — headless browser-profile import (history/bookmarks/cookies from Firefox/Chromium); `keyring` feature gates the Secret Service path.
- **hooks/** — agent hook templates and generated integration references for Claude Code, Codex, Antigravity, and OpenCode installed by `forktty hooks setup`; after editing them run `cargo run -p xtask -- check`.

Useful CLI for inspecting a running instance: `forktty doctor`, `forktty --json doctor`, `forktty capabilities`, `forktty identify`, `forktty context-snapshot`, `forktty top`, `forktty surfaces`, `forktty agents`, `forktty agent-health`, `forktty events`, and `forktty wait agent-status`.

## Optional local skills

Some agents working on ForkTTY may have local skills installed, including the
Matt Pocock engineering skills. Treat these as workflow accelerators, not hard
dependencies: when a named skill is unavailable, follow the equivalent manual
workflow using this file, local code, tests, and issue/spec context.

- Use `ask-matt` when unsure which local engineering skill fits the task.
- Use `diagnosing-bugs` for reported bugs, regressions, failing behavior, slow paths, or confusing UI/socket state.
- Use `implement` when executing a PRD, issue batch, or explicit implementation brief; use `tdd` as well when the request calls for test-first work or integration tests.
- Use `code-review` before finishing any non-trivial code change. Review the final diff against the relevant fixed point: use the supplied base if the user gave one, the merge-base with the target branch for branch/PR work, or the current uncommitted diff when working directly in the local tree.
- Use `codebase-design`, `domain-modeling`, or `improve-codebase-architecture` for module-boundary, deep-module, ubiquitous-language, or architecture-improvement work. Do not turn a small bug fix into an architecture project just because these skills exist.
- Use `research` when external API, packaging, GTK/Ghostty, OS, or standards behavior must be checked against high-trust sources.
- Use `triage`, `to-prd`, and `to-issues` only when converting reports or conversations into issue-tracker work; do not create tracker artifacts unless the user asked for that workflow.
- Use `grilling`, `grill-me`, or `grill-with-docs` only when the user wants a plan or design stress-tested before implementation.
- Use `prototype` for throwaway UI/state-model experiments, and `handoff` when stopping midstream and another agent needs to continue.

## GitHub repository structure

ForkTTY uses standard GitHub community/automation locations plus a small set of root source-of-truth docs. Keep the repository shape predictable for GitHub, contributors, release automation, and coding agents.

- Keep root-level project docs stable and purpose-specific: `README.md` for user-facing overview/install/usage, `CONTRIBUTING.md` for contributor workflow, `SECURITY.md` for private vulnerability reporting, `RELEASING.md` for maintainer release steps, `CHANGELOG.md` for user-visible changes, `LICENSE` for licensing, and `AGENTS.md` for coding-agent operating rules. Do not bury these in feature directories.
- Keep `.github/` for GitHub-native metadata only: workflows in `.github/workflows/`, ownership in `.github/CODEOWNERS`, dependency automation in `.github/dependabot.yml`, and future issue/PR templates under `.github/ISSUE_TEMPLATE/` or `.github/PULL_REQUEST_TEMPLATE/`. Do not put product docs, release notes, or generated app assets there.
- Treat GitHub community health files as discoverability contracts. GitHub recognizes `README`, `LICENSE`, `CONTRIBUTING`, `CODE_OF_CONDUCT`, security policy files, and issue/PR templates from supported locations; in this repo, prefer root files for ForkTTY-specific policies and `.github/` for templates/automation. Adding a new governance file such as `CODE_OF_CONDUCT.md` is an owner/product decision, not a drive-by cleanup.
- Keep `.github/workflows/*.yml` small and task-focused. Shared command knowledge belongs in scripts, `xtask`, or this file; workflows should call those commands instead of duplicating long shell logic. When changing workflow behavior, update the matching local command/check section above so CI and local guidance do not drift.
- Keep GitHub templates actionable and short. Issue templates should ask for environment, version/build, reproduction steps, logs/screenshots when relevant, and expected/actual behavior. PR templates should ask for scope, user-visible changes, linked issues, tests run, and release/docs impact. Avoid broad checklists that contributors cannot verify.
- Keep security entry points explicit. `SECURITY.md` is the public source for vulnerability reporting and supported versions; `.github/workflows/security.yml`, `.github/workflows/codeql.yml`, `cargo audit`, and Dependabot are automation, not substitutes for the policy.
- Keep release entry points explicit. `RELEASING.md`, `CHANGELOG.md`, packaging scripts, and `.github/workflows/*` must tell one consistent story about artifacts, features, signing/checksums, and post-release verification.
- Prefer adding a new top-level directory only for a durable category of repo content (`crates/`, `scripts/`, `packaging/`, `hooks/`, `vendor/`, `docs/` if introduced). If a file exists only to support one crate or feature, keep it near that owner instead of creating a new root bucket.
- Keep the separate `forktty-site` checkout as the public website source, not a shadow docs tree in this repo. When README-facing behavior, install flows, screenshots, public security/privacy wording, hook setup, or release assets change, update the site in the same task or report the exact site update still needed.
- When adding or moving GitHub-facing files, verify both the GitHub convention and the local contract: use GitHub Docs for supported filenames/locations, then run the relevant local check (`cargo run -p xtask -- check`, workflow syntax checks if available, or docs/site tests when touched).

## Rust workspace structure

ForkTTY is a Cargo workspace, so prefer standard Cargo/Rust layout before inventing local structure. The root `Cargo.toml` owns workspace membership, shared dependency/version/lint policy, resolver behavior, and release profile choices; each crate `Cargo.toml` owns only crate-specific dependencies, features, and metadata.

- Keep Rust packages under `crates/<package>/` unless the package is an explicit repo tool such as `xtask`. Do not add ad hoc top-level Rust packages for one feature.
- Inside each package, follow Cargo conventions: `src/lib.rs` for library entry points, `src/main.rs` or `src/bin/*.rs` for binaries, `tests/` for integration tests against public behavior, `examples/` for runnable examples, and `benches/` only when benchmarked by the local toolchain.
- Keep module paths predictable. Prefer the existing modern Rust layout (`feature.rs` plus `feature/*.rs` children, as in `socket_cli.rs` and `socket_cli/...`) over adding new `mod.rs` trees, unless the surrounding subtree already uses `mod.rs`.
- Name modules by domain or feature boundary (`worktree`, `notification`, `socket_cli/hooks`) instead of generic buckets such as `common`, `misc`, `helpers`, or `utils`. Shared helpers should remain crate-private and sit near their owning feature until two or more real owners justify extraction.
- Put unit tests next to private implementation when they need private access; put integration tests in the crate's `tests/` directory when they pin public CLI/socket/model behavior. Shared test fixtures belong in a clearly named crate-local `test_support` module or `tests/support`, not in production `utils`.
- Keep feature flags additive and compile-tested. Optional UI/browser/provider code must stay behind the matching `#[cfg(feature = "...")]`, and lower crates must not gain GUI or binary-only dependencies to make an upper-crate feature easier.
- Add dependencies at the narrowest crate that needs them. Prefer workspace dependency declarations for version consistency, but do not expose a dependency through `forktty-core` just because multiple upper crates use it.
- Keep generated, vendored, and embedded assets out of normal source paths when possible: generated outputs should point back to the generator/template, vendored code belongs under `vendor/`, and hook assets must preserve their generator/check workflow.

## Rust modularity and anti-monolith rules

Use Rust's module system intentionally: group related functionality, keep implementation details private, and expose the smallest useful API. Follow Cargo's normal layout conventions before inventing local structure.

- Treat files above roughly 1,500 lines, or files mixing unrelated command families, as extraction candidates. When touching `crates/forktty-ui-gtk/src/socket_cli.rs`, `crates/forktty-socket/src/lib.rs`, `crates/forktty-ui-gtk/src/gtk_app/controller.rs`, or another orchestrator, first decide whether the change belongs in an existing feature module or a new sibling module.
- Orchestrator files should parse, route, and coordinate. Feature behavior, request/response shaping, text formatting, and feature-local tests should live in the feature module (`socket_cli/worktree.rs`, `socket_cli/hooks/`, `gtk_app/...`, etc.).
- Prefer `pub(crate)` and narrow helper APIs. Do not make helpers public for convenience, and do not create broad `utils` modules for single-use code.
- Extract incrementally: move one cohesive family at a time, preserve public strings, JSON shapes, ordering, aliases, and error behavior first, then run targeted tests before any behavior change. Split large refactors into small commits that can be reviewed and rolled back independently.
- Keep crate boundaries meaningful. Add a crate only for a stable ownership/dependency boundary, feature-gating need, or reuse across existing crates; otherwise prefer modules within the current crate.
- Keep dependency flow intact: domain logic stays in `forktty-core`, socket protocol/server behavior in `forktty-socket`, terminal boundaries in `forktty-terminal`, and GTK/CLI presentation in `forktty-ui-gtk`. Do not pull GUI or process/runtime dependencies into lower crates.
- Model recoverable failures with `Result` and meaningful error context. Reserve `panic!`, `unwrap`, and `expect` for tests or impossible internal invariants, and explain non-obvious `expect` messages.
- Follow Rust naming and API conventions (`as_`/`to_`/`into_`, `iter`/`iter_mut`/`into_iter`, standard conversion traits, common derived traits where appropriate). Prefer types and enums over stringly typed state when the value crosses module boundaries.
- Add or keep behavior-boundary tests during extraction: CLI output tests, socket JSON responses, and model/store invariants should remain the proof that the move was behavior-preserving.
- Use web references for external behavior, but ground internal Rust structure in local code plus the Rust Book, Cargo Book, Rust API Guidelines, small-change review practice, and incremental replacement patterns.

## LLM-readable code rules

Optimize for future readers who arrive with `rg`, type signatures, tests, and this file as their map. Code that is easy for an agent to locate and verify is usually easier for humans to review too.

- Keep repo guidance concise, current, and scoped. Put broad rules in this root file; if a subtree needs special setup or review rules, add a nearer `AGENTS.md`/`AGENTS.override.md` instead of expanding the root with path-specific detail.
- State the source of truth when several files describe the same behavior. If generated docs, embedded strings, CLI help, and Rust code can drift, name which file/template should be edited first.
- Prefer descriptive domain names over abbreviations. Public socket method names, command names, event names, and state-machine states should appear as searchable constants or enum variants, not only as dynamically assembled strings.
- Put a short `//!` module summary on new non-trivial modules explaining responsibility, primary entry points, and the important invariants. For public Rust APIs, use rustdoc with purpose, examples when useful, and `# Errors`, `# Panics`, or `# Safety` sections when they apply.
- Comments should explain why, invariants, ordering constraints, external contracts, and surprising edge cases. Do not add comments that merely restate the next line of code.
- Make data contracts explicit with structs, enums, newtypes, and typed errors instead of loose maps, booleans with unclear meaning, or stringly typed state. Derive or implement `Debug` for public and boundary-crossing types when practical.
- Keep feature entry points easy to trace: dispatcher arms should name the helper they delegate to, helpers should live in the feature module, and tests should use behavior names that can be found with `rg <method_or_command>`.
- Keep examples small, realistic, and adjacent to the API or command they explain. Favor executable doctests or CLI/socket tests when possible; otherwise make clear that an example is illustrative.
- Avoid hiding important behavior behind broad macros, global mutable state, callbacks, or trait objects unless the indirection is necessary. When it is necessary, leave a narrow comment or module summary that points to the runtime path.
- Mark generated, vendored, embedded, or source-of-truth files clearly. If an agent should edit a generator/template instead of generated output, say that near the generated include or in the nearest module docs.
- When adding a new module or moving ownership, update the relevant architecture note, module summary, tests, and command/help references in the same change so future agents do not follow stale maps.

## Conventions

- Surgical edits only: don't reformat, restyle, or refactor code unrelated to the change (see CONTRIBUTING.md).
- Before applying a bug fix, check current, reliable web sources for the external behavior involved: prefer official docs, upstream source/issues, standards, or maintainer notes over blog posts and forum guesses. State which source informed the fix. If the issue is purely internal to ForkTTY and no external behavior is relevant, say that explicitly and ground the fix in local code/tests instead.
- Every user-visible change gets a `CHANGELOG.md` entry under `## [Unreleased]` (`Added`/`Changed`/`Fixed`/`Security` headings).
- Update `SPEC.md` when changing behavior it describes (config fields, socket methods, security boundaries).
- Keep the public website in sync. The separate site repo is usually checked out in the user's home directory as `forktty-site`; when a change affects install instructions, release assets, screenshots, public docs, README-facing behavior, privacy/security wording, hook setup, Ghostty integration, settings/config, or visible UI flows, update the relevant site files in the same task (`app/docs/page.tsx`, `public/llms.txt`, `public/llms-full.txt`, home components, tests) and run `npm test` plus `npm run build` there. If the site workspace is unavailable, explicitly report the exact site update still needed instead of silently skipping it.
- Prefer tests that pin observable behavior (socket responses, validation rejections) over mocking internals. Tests that read env vars must guard with the existing `with_env` helper — tests run in parallel.
- Do not weaken enforced security boundaries to make a task easier: keep argv validation, owner-only socket checks, request size bounds, and local-first/privacy guarantees in code.
- Do not add hidden background schedulers, task routers, provider-neutral team stores, or autonomous execution engines. Agent work remains visible through terminal panes and explicit socket/CLI calls.
- For socket or agent-lifecycle changes, keep method behavior, CLI wrappers, hook templates, `SPEC.md`, `README.md`, and `forktty-site` agent context aligned.
- Release process is in `RELEASING.md`; after a release publishes, download the actual assets and verify them end-to-end (run the AppImage, check theming/icons) — green CI alone has not been sufficient in the past.

## Change checklist

- User-visible behavior, UI text, CLI output, or packaging changed → update `CHANGELOG.md`.
- Config fields, socket methods, session format, or security boundaries changed → update `SPEC.md`.
- Public docs, install/release behavior, screenshots, hook setup, Ghostty integration, settings/config, privacy/security wording, or visible UI changed → update the `forktty-site` checkout docs/agent context/home content and run its `npm test` + `npm run build`.
- Hook templates or release automation changed → run `cargo run -p xtask -- check`.
- Socket or agent-lifecycle behavior changed → update `SPEC.md`, CLI help/tests, hook docs/templates when relevant, and `forktty-site` agent context together.
- Browser-gated code changed → run browser feature test, clippy, and build.
- Packaging/AppImage/runtime loader changed → build artifacts and smoke-test them.
- Dependencies changed → run `cargo audit` and justify the dependency.
