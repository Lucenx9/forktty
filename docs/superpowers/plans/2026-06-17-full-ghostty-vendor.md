# Full Ghostty Vendor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put full upstream Ghostty in ForkTTY as a pinned source dependency so the next work can spike a real Ghostty renderer/widget bridge.

**Architecture:** Keep the current GTK/libghostty-vt runtime untouched. Add Ghostty as `vendor/ghostty` submodule, guard the exact URL/SHA in `xtask`, and document that this is a renderer-bridge prerequisite rather than a release runtime switch.

**Tech Stack:** Git submodule, Rust `xtask`, existing ForkTTY docs.

---

### Task 1: Pin Full Ghostty Source

**Files:**
- Create: `.gitmodules`
- Add gitlink: `vendor/ghostty`
- Modify: `xtask/src/main.rs`
- Create: `docs/ghostty-full-vendor.md`
- Modify: `CHANGELOG.md`
- Modify: `ROADMAP.md`
- Modify: `SPEC.md`
- Modify: `docs/native-gtk-ghostty.md`

- [x] **Step 1: Write the failing manifest guard**

Add a focused `xtask` unit test that calls `validate_ghostty_gitmodules(raw)` with:

```toml
[submodule "vendor/ghostty"]
	path = vendor/ghostty
	url = https://github.com/ghostty-org/ghostty.git
```

- [x] **Step 2: Run test to verify it fails**

Run:

```bash
cargo test -p xtask validates_full_ghostty_submodule_manifest
```

Expected: fails because `validate_ghostty_gitmodules` does not exist.

- [x] **Step 3: Add the minimal guard**

Implement constants for `vendor/ghostty`, `https://github.com/ghostty-org/ghostty.git`,
and the chosen SHA. Have `cargo run -p xtask -- check` verify `.gitmodules`,
`vendor/ghostty/build.zig`, `vendor/ghostty/include/ghostty.h`,
`vendor/ghostty/LICENSE`, and `git -C vendor/ghostty rev-parse HEAD`.

- [x] **Step 4: Verify the guard is red before the submodule**

Run:

```bash
cargo run -p xtask -- check
```

Expected: fails because `.gitmodules` or `vendor/ghostty` is missing.

- [x] **Step 5: Add the submodule**

Run:

```bash
git submodule add https://github.com/ghostty-org/ghostty.git vendor/ghostty
git -C vendor/ghostty checkout e8e7fea103ab8bff5384673a60e04b59939738dd
```

- [x] **Step 6: Document the boundary**

Add docs that say `vendor/ghostty` is for the renderer bridge spike, while the
current runtime still uses `vendor/libghostty-rs` and ForkTTY's GTK renderer.

- [x] **Step 7: Verify**

Run:

```bash
cargo fmt --all --check
cargo test -p xtask validates_full_ghostty_submodule_manifest
cargo run -p xtask -- check
git diff --check
```
