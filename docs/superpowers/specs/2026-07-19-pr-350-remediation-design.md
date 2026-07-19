# PR #350 Remediation Design

**Date:** 2026-07-19
**PR:** <https://github.com/Lucenx9/forktty/pull/350>
**Branch:** `fix/audit-remediation-batch`

## Goal

Resolve every verified merge blocker and the non-blocking review findings in PR
#350 without broadening the product scope. The result must preserve user-owned
hook configuration, package the exact artifacts produced by the current build,
ship complete and accurate legal notices, keep release validation testable, and
restore documentation consistency. After the implementation passes the full
local gate and GitHub CI, merge PR #350.

## Scope

### Required runtime and packaging fixes

1. Migrate retired ForkTTY-managed Claude `WorktreeCreate` hooks during setup,
   removal, and doctor reconciliation while preserving unrelated user hooks.
2. Replace mtime-based `libghostty-vt-sys` output discovery with the exact
   Cargo `OUT_DIR` reported for the current build.
3. Copy `libgtk4-layer-shell.so` from the same Ghostty Zig output as
   `ghostty-gtk-embed.so`, eliminating host `ldd`/`ldconfig` discovery and the
   `pipefail` failure in the Debian build.
4. Correct and complete package license material.
5. Move release tag/version validation into tested repository tooling and make
   workflow comments match the actual post-publication artifact gate.

### Required quality and documentation fixes

1. Add regression coverage for the Claude migration and project-action rollback.
2. Update Claude event counts from 26/29 to 25/28 throughout ForkTTY and the
   public site.
3. Document the log-output sanitization boundary in `SPEC.md` and `SECURITY.md`.
4. Remove duplicated packaging helper logic by using one shared script.
5. Update `CHANGELOG.md`, hook documentation, native packaging documentation,
   third-party notices, and generated/public agent context where applicable.

The pre-existing untracked `/home/simone/forktty-site/public/images/` directory
is outside this change and must remain untouched.

## Design

### Retired Claude hook reconciliation

Extend the hook provider specification with a separate list of retired managed
entries. Claude registers `WorktreeCreate/worktree-create` in this retired list;
it does not return to the supported event list or templates.

The setup and removal planners process current and retired entries. For retired
entries they remove only entries that are either tagged with
`forkttySource: "forktty"` or match the existing legacy ForkTTY command
recognizer for `hooks claude worktree-create`. User-authored `WorktreeCreate`
hooks are preserved. Doctor already regenerates expected configuration through
the setup planner, so the same reconciliation makes stale installations report
as incomplete until setup removes the retired entry.

Fresh Claude event counts remain 25 for lifecycle and 28 for full.

### Exact Ghostty build artifact identity

Add one small shell helper sourced by both packaging scripts. It runs the
release Cargo build with JSON messages and extracts the single
`build-script-executed.out_dir` associated with `libghostty-vt-sys`. Diagnostics
remain visible on stderr. Missing or ambiguous output is a hard error.

Both package builders derive `libghostty-vt`, shell integration, and terminfo
from that one `OUT_DIR`; they never search all cached hashes independently.
This preserves Cargo cache reuse while binding every copied resource to the
build invocation that produced the ForkTTY binary.

The embedded GTK probe returns `ghostty-gtk-embed.so`. The required
`libgtk4-layer-shell.so` is resolved as a sibling artifact from the same Ghostty
Zig output, validated as a regular file, checked against the embed library's
`DT_NEEDED`, and copied with any required SONAME-compatible links. The build
never copies an arbitrary host library.

### Release validation

Add an `xtask` command that receives a tag, structurally reads the workspace
package version through the repository's Cargo metadata/TOML source of truth,
and rejects anything except `v<workspace-version>`. Unit tests cover matching,
mismatching, missing, and malformed tags.

The workflow invokes this command after Rust setup and before packaging. Its
comments and release documentation state the actual guarantee: checks and tag
validation gate artifact construction/upload for an already-published GitHub
Release. This change does not redesign release creation around tag pushes.

### Legal material

Correct bash-preexec 0.6.0 attribution to 2017. Store complete, pinned license
texts needed only for packaging under a clearly named packaging license
location. Package legal output includes:

- ForkTTY AGPL-3.0-only;
- Ghostty MIT;
- GPL-3.0 for the Kitty-derived shell integration files;
- bash-preexec MIT with its own copyright notice;
- gtk4-layer-shell MIT;
- libghostty-rs MIT.

`THIRD_PARTY_NOTICES.md` names the bundled gtk4-layer-shell runtime and points
to the corresponding source/license material.

### Observable rollback and security documentation

Add a socket-level regression test that runs a project action whose executable
cannot be resolved and verifies that the model's surface set is unchanged. Keep
the implementation rollback at the existing transaction boundary.

Document that log level/message fields are untrusted socket/agent input and are
escaped before terminal output, matching the existing status/progress boundary.

## Error handling

- Retired-hook cleanup is selective; malformed or user-owned entries survive.
- Cargo JSON extraction fails on zero or multiple matching `OUT_DIR` values.
- Packaging fails before assembly when any expected artifact, SONAME, or
  dependency relationship is absent.
- Tag validation reports the observed tag and expected `v<version>` value.
- No fallback silently selects cached files or host libraries.

## Verification

### Targeted regression tests

- tagged retired `WorktreeCreate` removed by Claude setup;
- legacy untagged ForkTTY `WorktreeCreate` removed by setup;
- user `WorktreeCreate` preserved;
- hook removal removes retired managed entries;
- doctor reports stale retired entries;
- Claude profile counts and event lists are 25/28;
- failed project-action program resolution leaves no new surface;
- release tag validation accepts and rejects the expected cases;
- generated package legal output contains each required license and corrected
  attribution.

### Repository gates

Run the relevant full local gate from `AGENTS.md`, including both
`gtk-ghostty` and browser configurations, `xtask`, formatting, tests, clippy,
Cargo audit, Debian package build, AppImage build, bundled-container smoke, and
runtime loader checks. Run `npm test` and `npm run build` in `forktty-site`.

## Delivery and merge

1. Commit the implementation on `fix/audit-remediation-batch` and push it to PR
   #350.
2. Commit and push the required tracked documentation changes in
   `forktty-site` without touching its pre-existing untracked images.
3. Confirm the PR head SHA, mergeability, review state, and every required CI
   check after the push.
4. Merge PR #350 only when the local gate and GitHub checks are green and no
   blocker remains.
