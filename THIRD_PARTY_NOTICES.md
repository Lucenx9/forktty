# Third-Party Notices

ForkTTY is licensed under AGPL-3.0-only. See `LICENSE` for the full license
text and `Cargo.toml` for the SPDX identifier.

Complete corresponding source for release binaries is available from the
ForkTTY repository. Clone with submodules to get the pinned vendored sources:

```bash
git clone --recurse-submodules https://github.com/Lucenx9/forktty.git
```

## Vendored Ghostty Runtime

ForkTTY packages a pinned Ghostty GTK embedding library and Ghostty runtime
resources from `vendor/ghostty`.

- License: MIT
- Copyright: 2024 Mitchell Hashimoto, Ghostty contributors
- Source in this repository: `vendor/ghostty`
- License text: `vendor/ghostty/LICENSE`

## Vendored libghostty-rs Runtime

ForkTTY packages a pinned `libghostty-vt` shared library built from
`vendor/libghostty-rs`.

- License: MIT
- Copyright: 2026 Uzair Aftab, Leah Amelia Chen
- Source in this repository: `vendor/libghostty-rs`
- License text: `vendor/libghostty-rs/LICENSE`

## Rust Crates

ForkTTY statically links Rust crate dependencies resolved by Cargo. See
`Cargo.lock` for exact versions and each crate's published license metadata.

## System Libraries

Linux packages depend on system GTK, libadwaita, OpenSSL, libssh2, zlib, zstd,
and related runtime libraries provided by the target distribution. The AppImage
prefers host GUI/display/font libraries and bundles only the fallback libraries
needed to start on supported systems.
