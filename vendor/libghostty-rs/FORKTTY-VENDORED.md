# Vendored libghostty-rs (rev 20edad15)

Copy of https://github.com/Uzaaft/libghostty-rs at rev
`20edad15d7984c727acc4f4facdadf045609f543` (MIT OR Apache-2.0), applied via
`[patch.crates-io]` in the workspace `Cargo.toml`. Both crates are vendored
(not just `libghostty-vt-sys`) because `libghostty-vt` from git reaches
`libghostty-vt-sys` through an in-repo path dependency, so a single-crate
patch cannot redirect it.

Local changes, marked `FORKTTY PATCH`:

- `crates/libghostty-vt-sys/build.rs`: pass `-Dcpu=baseline` to `zig build`
  for native (non-cross) builds. Upstream lets zig target the build host's
  CPU, so the ISA of the built library depends on whichever machine compiled
  it. zig's compiler_rt `memset` is statically linked into the final
  executable and overrides the libc symbol; built on an AVX-512 CI runner it
  emitted EVEX instructions and every non-AVX-512 machine crashed with
  SIGILL at startup (v0.2.0-alpha.10 first cut).
- Both `Cargo.toml` manifests: workspace-inherited fields
  (`version`/`edition`/`license`/`repository`/`rust-version` and the
  `libghostty-vt-sys` dependency) replaced with the literal values from the
  upstream workspace root, plus an empty `[workspace]` table, so the crates
  resolve standalone under `[patch.crates-io]` — cargo resolves patch paths
  against the patching workspace, where the inherited keys don't exist.
- Upstream workspace scaffolding not needed by the two crates was removed:
  `Cargo.toml`, `Cargo.lock`, `example/`, `flake.*`, `AGENTS.md`.

Drop this directory and point both `[patch.crates-io]` entries back at the
upstream git rev once upstream pins the CPU baseline (or exposes a knob for
it) for native builds.
