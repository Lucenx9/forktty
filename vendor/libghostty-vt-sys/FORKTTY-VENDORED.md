# Vendored libghostty-vt-sys 0.1.1

Verbatim copy of `libghostty-vt-sys` 0.1.1 from crates.io
(https://github.com/uzaaft/libghostty-rs, MIT OR Apache-2.0), applied via
`[patch.crates-io]` in the workspace `Cargo.toml`.

Local changes, marked `FORKTTY PATCH` in `build.rs`:

- Pass `-Doptimize=ReleaseSafe` to `zig build`. Upstream passes no optimize
  flag, so zig defaulted to Debug and every ForkTTY build shipped an
  unoptimized libghostty (VT parsing measured at ~65 KB/s). Overridable via
  `LIBGHOSTTY_VT_SYS_OPTIMIZE` (e.g. `Debug`, `ReleaseFast`).

Drop this directory and the `[patch.crates-io]` entry once upstream ships an
optimized (or configurable) build.
