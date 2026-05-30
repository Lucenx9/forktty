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
   - `cargo clippy --workspace --features browser -- -D warnings`
   - `cargo test --workspace --features browser`
   - `cargo build -p forktty-ui-gtk --features browser`
   - `desktop-file-validate packaging/linux/dev.forktty.forktty.desktop`
   - `bash scripts/build-deb.sh`
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
   The packaged artifacts are built with `--features browser` (see
   `scripts/build-deb.sh` and `scripts/build-appimage.sh`), so browser-pane
   changes ship in the `.deb` and AppImage, not only in source builds.
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

1. Open the tag in the GitHub UI and click "Draft a new release".
2. Title: `ForkTTY 0.2.0-alpha.N`.
3. Body: copy the section you just moved in `CHANGELOG.md`, plus:
   - Supported distros (link to `docs/QA.md`).
   - A note that the AppImage is the default download for this alpha,
     while the `.deb` remains available for Debian/Ubuntu.
   - The SHA256SUMS lines for the `.deb` and experimental AppImage.
4. Tick "Set as a pre-release" while we are in alpha.
5. Publish.

Publishing the release triggers the `release-package` job in
`.github/workflows/ci.yml`, which:

- Builds the `.deb` and experimental AppImage from the tagged commit.
- Generates `SHA256SUMS` for both artifacts.
- Uploads both artifacts and `SHA256SUMS` into the release.

## 5. Post-publish verification

1. Download the `.deb`, AppImage, and `SHA256SUMS` from the published release.
2. Run `sha256sum -c SHA256SUMS` in the download directory — it must
   print `OK` for both artifacts.
3. Install on a clean VM (`sudo dpkg -i forktty_*.deb`).
4. Launch `forktty`, run `forktty doctor`, and walk the runtime smoke
   checks from [`docs/release-qa.md`](docs/release-qa.md).
5. Mark the AppImage executable, launch it on the same VM, and note any
   AppImage-specific runtime dependency issue in the release notes.
6. Remove the package and confirm `/usr/bin/forktty` and the desktop
   entry are gone (`dpkg -L forktty` should fail after removal).

## 6. If anything is wrong

- **CI failed after tag push**: delete the tag both locally and on the
  remote (`git tag -d`, `git push --delete origin <tag>`), fix forward
  on `main`, then re-tag with the same version. Do not edit the tag
  in place.
- **Bad artifact uploaded to release**: re-run the workflow from the
  Actions tab; `--clobber` will replace it. If the release itself is
  broken, mark it as a draft and recut.
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
