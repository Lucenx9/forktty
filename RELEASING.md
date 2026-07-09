# Releasing ForkTTY

This is the end-to-end release runbook for maintainers. The QA gate it
relies on is in [`docs/release-qa.md`](docs/release-qa.md); the platform
coverage grid is in [`docs/QA.md`](docs/QA.md).

ForkTTY is Linux-only and currently in alpha (`0.2.0-alpha.x`). The
`forktty` binary, the Debian package, and the Cargo workspace version
are all driven from `Cargo.toml`'s `[workspace.package].version`.

## 1. Pre-flight

1. Confirm `main` is green on CI for the commit you want to ship.
2. Run the full QA checklist locally on at least one supported distro
   (see [`docs/QA.md`](docs/QA.md)):
   - `cargo fmt --all --check`
   - `git submodule update --init vendor/ghostty`
   - `cargo run -p xtask -- check`
   - `scripts/ghostty-gtk-lib-probe.sh --ensure --print-path`
   - `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
   - `cargo test --workspace --all-targets --no-default-features --features gtk-ghostty`
   - `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty`
   - `scripts/gtk-ghostty-smoke.sh`
   - `cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser`
   - `desktop-file-validate packaging/linux/dev.forktty.forktty.desktop`
   - `bash scripts/build-deb.sh`
   - `dpkg-deb -c target/packaging/deb/forktty_*.deb | grep -F /usr/share/doc/forktty/copyright`
   - `scripts/check-deb-piuparts.sh` (optional but recommended for `.deb`
     install/purge validation; defaults to Debian 13/Trixie)
   - `bash scripts/build-appimage.sh`
3. Run `cargo audit` and (optionally) `cargo deny check`. Resolve any
   `high`/`critical` advisories before tagging.
4. Walk the GTK runtime smoke tests in [`docs/release-qa.md`](docs/release-qa.md).
5. Run `forktty doctor` from the freshly built binary and confirm the
   summary matches a known-good environment.

## 2. Version bump

1. Update `version` in `Cargo.toml` (workspace package).
2. Run `cargo check --workspace` so `Cargo.lock` re-pins.
3. In `CHANGELOG.md`:
   - Move every entry under `## [Unreleased]` into a new
     `## [<version>] - <YYYY-MM-DD>` section.
   - Leave an empty `## [Unreleased]` heading above it.
4. Update the README download badge / link if it tracks the prerelease.
   The packaged artifacts are built with `--no-default-features --features gtk-ghostty`
   (see `scripts/build-deb.sh` and `scripts/build-appimage.sh`). The browser
   feature remains source-only and must stay covered by CI, but it is not shipped
   in the `.deb` or AppImage for alpha releases.
5. Commit:
   ```
   git commit -am "release: forktty 0.2.0-alpha.N"
   ```

## 3. Tag and push

```
git tag -s v0.2.0-alpha.N -m "ForkTTY 0.2.0-alpha.N"
git push origin main
git push origin v0.2.0-alpha.N
```

Signed tags (`-s`) are preferred; if you don't have a signing key,
`-a` is acceptable for alphas.

## 4. Publish the GitHub release

The streamlined path creates the tag and the release in one step from
the pushed `main` commit (skip section 3's manual tagging):

```
gh release create v0.2.0-alpha.N --prerelease \
  --title "ForkTTY 0.2.0-alpha.N" --notes-file <notes.md> --target main
```

Or via the GitHub UI:

1. Open the tag in the GitHub UI and click "Draft a new release".
2. Title: `ForkTTY 0.2.0-alpha.N`.
3. Body: copy the section you just moved in `CHANGELOG.md`, plus:
   - Supported distros (link to `docs/QA.md`).
   - A note that the AppImage is the primary download for this alpha,
     while the `.deb` remains available for Debian 13/Trixie+ and Ubuntu
     24.04 LTS+.
   - Source availability: link to the release tag and note that source builds
     should clone with `git clone --recurse-submodules
     https://github.com/Lucenx9/forktty.git` to include the pinned Ghostty
     submodules used by the release artifacts.
   - The SHA256SUMS lines for the `.deb` and AppImage.
4. Tick "Set as a pre-release" while we are in alpha.
5. Publish.

Publishing the release triggers the `release-package` job in
`.github/workflows/ci.yml`, which:

- Builds the `.deb`, AppImage, and AppImage `.zsync` metadata from the tagged commit.
- Generates `SHA256SUMS` for all three artifacts.
- Uploads all three artifacts and `SHA256SUMS` into the release.

## 5. Post-publish verification

1. Download the `.deb`, AppImage, AppImage `.zsync`, and `SHA256SUMS` from the
   published release.
2. Run `sha256sum -c SHA256SUMS` in the download directory — it must
   print `OK` for all three artifacts.
3. Install on a clean Debian 13/Trixie+ or Ubuntu 24.04 LTS+ VM
   (`sudo apt install ./forktty_*.deb`). Debian 12/Bookworm is below the
   packaged `.deb` baseline because it does not provide libadwaita 1.4+.
4. Launch `forktty`, run `forktty doctor`, and walk the runtime smoke
   checks from [`docs/release-qa.md`](docs/release-qa.md).
5. Confirm the `.deb` contains `/usr/share/doc/forktty/copyright` and
   `THIRD_PARTY_NOTICES.md`.
6. Mark the AppImage executable, launch it on the same VM, and note any
   AppImage-specific runtime dependency issue in the release notes.
7. Mount or extract the AppImage and confirm it contains
   `usr/share/doc/forktty/copyright` and `THIRD_PARTY_NOTICES.md`.
8. Remove the package and confirm `/usr/bin/forktty` and the desktop
   entry are gone (`dpkg -L forktty` should fail after removal).

## 6. If anything is wrong

- **Packaging failed or the tagged commit is bad**: fix forward on
  `main`, wait for CI green, then recut the release on the new commit:
  `gh release delete v0.2.0-alpha.N --yes --cleanup-tag` followed by
  the `gh release create` command above. Do not edit a tag in place.
- **Bad artifact uploaded to release**: re-run the workflow from the
  Actions tab; `--clobber` will replace it. If the release itself is
  broken, mark it as a draft and recut.
- Always verify the recut artifacts end to end (checksums, contents,
  binaries launch) — a green packaging job alone has missed broken
  artifacts before.
- **Security regression**: follow the response steps in
  [`SECURITY.md`](SECURITY.md), including private disclosure if the
  report came in through GitHub's advisory channel.

## 7. Branch protection (one-time setup)

These rules live in the GitHub repo settings, not in the tree, so they
are documented here for new maintainers:

- `main` requires a pull request before merging.
- Require status checks to pass: at minimum the `Build & Test` job in
  CI (and `Dependency review` for PRs from forks).
- Require branches to be up to date before merging.
- Restrict who can push to `main` (maintainers only).
- Disallow force-pushes and deletions on `main`.
- Require signed commits if your signing setup permits it.

Tags matching `v*` should also be protected so dropped or rewritten
releases are not possible without an explicit override.
