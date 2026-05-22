# Contributing to ForkTTY

Thanks for your interest in ForkTTY. This document covers how to set up a
development environment, what to run before opening a PR, and how patches
are reviewed.

ForkTTY is Linux-only and currently in alpha (`0.2.0-alpha.x`). The GTK4 +
VTE shell is the primary implementation.

## Quick links

- Architecture and behavior contract: [`SPEC.md`](SPEC.md)
- Roadmap and scope: [`ROADMAP.md`](ROADMAP.md)
- Release process: [`RELEASING.md`](RELEASING.md)
- Pre-release QA matrix: [`docs/QA.md`](docs/QA.md) and [`docs/release-qa.md`](docs/release-qa.md)
- Security policy and threat model: [`SECURITY.md`](SECURITY.md)
- Privacy posture: [`PRIVACY.md`](PRIVACY.md)
- Community expectations: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)

## Development environment

You will need:

- Linux
- Rust 1.88+ (install via [rustup](https://rustup.rs/))
- Node.js 20+ (only for the repo-local CLI helper and its tests)
- GTK4, libadwaita, and VTE GTK4 development libraries

Distro-specific install commands are in the [README](README.md#quick-start).

Clone and build:

```bash
git clone https://github.com/Lucenx9/forktty.git
cd forktty
cargo run -p forktty-ui-gtk --features gtk-vte
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

## Pre-PR checks

Run these locally before pushing. CI runs the same set in
[`.github/workflows/ci.yml`](.github/workflows/ci.yml):

```bash
cargo fmt --all --check
cargo clippy --workspace --features gtk-vte -- -D warnings
cargo test --workspace
cargo build -p forktty-ui-gtk --features gtk-vte
node --test scripts/forktty.test.mjs
desktop-file-validate packaging/linux/forktty.desktop
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

If your change touches dependencies, also run:

```bash
cargo audit
cargo deny check    # optional, requires cargo-deny
```

## Tests

- Rust unit and integration tests live next to the code under
  `crates/*/src/` and `crates/*/tests/`.
- Socket-API and CLI behavior is tested via the Node.js test runner in
  `scripts/forktty.test.mjs`.
- GTK runtime smoke is manual; see [`docs/release-qa.md`](docs/release-qa.md).

Prefer tests that pin observable behavior (socket responses, config
quarantine, validation rejections) over tests that mock internals.

## Code style

- Run `cargo fmt`; do not hand-format.
- Keep `clippy --workspace --features gtk-vte -- -D warnings` clean.
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
- Fill out the PR template. The "what changed" and "how was this
  tested" sections matter.
- Describe user-visible behavior changes in the PR body even if you've
  also updated `CHANGELOG.md`.
- Sign-off (`Signed-off-by:` line) and signed commits are appreciated
  but not required.

## License

By contributing to ForkTTY, you agree your contributions are licensed
under the project's [AGPL-3.0-only](LICENSE) license.
