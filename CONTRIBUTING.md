# Contributing to ForkTTY

Thanks for your interest in ForkTTY. This document covers how to set up a
development environment, what to run before opening a PR, and how patches
are reviewed.

ForkTTY is Linux-only and currently in alpha (`0.2.0-alpha.x`). The
GTK4/libadwaita shell with embedded Ghostty terminal panes is the primary
implementation.

## Quick links

- Architecture and behavior contract: [`SPEC.md`](SPEC.md)
- Roadmap and scope: [`ROADMAP.md`](ROADMAP.md)
- Feature-quality brief for non-trivial changes: [`docs/feature-quality-template.md`](docs/feature-quality-template.md)
- Release process: [`RELEASING.md`](RELEASING.md)
- Pre-release QA matrix: [`docs/QA.md`](docs/QA.md) and [`docs/release-qa.md`](docs/release-qa.md)
- Security policy and threat model: [`SECURITY.md`](SECURITY.md)
- Privacy posture: [`PRIVACY.md`](PRIVACY.md)

## Development environment

You will need:

- Linux
- Rust 1.96+ (install via [rustup](https://rustup.rs/))
- GTK4 and libadwaita development files
- `git`, Zig, and the full Ghostty source submodule for the vendored Ghostty
  terminal libraries
- WebKitGTK 6 development files when working on the optional browser-pane feature

Distro-specific install commands are in the [README](README.md#build-from-source).

Clone and build:

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
git submodule update --init vendor/ghostty
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
cargo run -p forktty-ui-gtk
```

## Workflow

1. Open an issue first for non-trivial changes so we can align on scope
   before code review.
2. Branch from `main`. Keep branches focused on one change.
3. Make the smallest change that solves the problem. ForkTTY favours
   surgical edits over refactors; please don't reformat or restyle code
   that is unrelated to your change.
4. Update [`CHANGELOG.md`](CHANGELOG.md) under `## [Unreleased]` using the
   existing `Added` / `Changed` / `Fixed` / `Security` headings.
5. Update [`SPEC.md`](SPEC.md) when you change behavior that the spec
   describes (config fields, socket methods, security boundaries).
6. Add or update tests where it is reasonable to do so (see "Tests"
   below).

For a non-trivial change that crosses modules or affects socket, session,
config, security, worktree, packaging, or release contracts, use
[`docs/feature-quality-template.md`](docs/feature-quality-template.md) unless
the originating issue or design already contains equivalent requirements,
failure/recovery behavior, and traceability. Complete its consistency review
before implementation and its requirement-to-evidence review before opening
the PR. Focused fixes do not need a new brief when their issue and regression
test already provide that evidence.

## Pre-PR checks

Run these locally before pushing. CI runs the same set in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml):

```bash
cargo fmt --all --check
cargo run -p xtask -- check
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser
cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings
desktop-file-validate packaging/linux/dev.forktty.forktty.desktop
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

If your change touches browser-pane code, also run:

```bash
cargo build -p forktty-ui-gtk --no-default-features --features browser
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser
```

If your change touches dependencies, also run:

```bash
cargo audit
cargo deny check    # optional, requires cargo-deny
```

## Tests

- Rust unit and integration tests live next to the code under
  `crates/*/src/` and `crates/*/tests/`.
- Native socket CLI and hook behavior is covered by Rust tests in
  `crates/forktty-ui-gtk/src/socket_cli.rs`.
- Repository consistency checks live in the Rust `xtask` crate. Run
  `git submodule update --init vendor/ghostty` once, then
  `cargo run -p xtask -- check` after editing hook templates, release
  automation, or the Ghostty source pin.
- GTK runtime smoke has an automated embedded-pane path in
  `scripts/gtk-ghostty-smoke.sh`; manual desktop and packaging coverage is in
  [`docs/release-qa.md`](docs/release-qa.md).

Prefer tests that pin observable behavior (socket responses, config
quarantine, validation rejections) over tests that mock internals.

## Code style

- Run `cargo fmt`; do not hand-format.
- Keep `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings` clean.
- Match the surrounding style in the file you are editing. Do not
  introduce new abstractions to "make room" for hypothetical future
  changes.
- Avoid comments that restate what the code does. Comment *why* when the
  intent is non-obvious (security invariant, workaround, ordering
  requirement).
- Don't add features, configuration toggles, or "flexibility" that the
  current change doesn't need.

## Security-sensitive code

The paths listed in [`.github/CODEOWNERS`](.github/CODEOWNERS) and the
"Security Boundaries" table in [`SECURITY.md`](SECURITY.md) are
security-sensitive. Changes there get extra scrutiny.

If you believe you've found a vulnerability, do **not** open a public
issue. Follow the disclosure process in [`SECURITY.md`](SECURITY.md).

## Pull requests

- One topic per PR. Smaller PRs are reviewed faster.
- Fill out [the PR template](.github/PULL_REQUEST_TEMPLATE.md). Its scope,
  verification, and release/docs-impact sections matter.
- Describe user-visible behavior changes in the PR body even if you've
  also updated `CHANGELOG.md`.
- Sign-off (`Signed-off-by:` line) and signed commits are appreciated
  but not required.

## License

By contributing to ForkTTY, you agree your contributions are licensed
under the project's [AGPL-3.0-only](LICENSE) license.
