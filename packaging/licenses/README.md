# Package-only license texts

These pinned texts cover third-party files copied into ForkTTY release
packages but not otherwise accompanied by a complete license file in the
vendored build output.

- `GPL-3.0-or-later.txt`: GNU GPL version 3 terms used by the Kitty-derived
  Ghostty shell-integration files; sourced from the SPDX license list.
- `bash-preexec-0.6.0-MIT.txt`: bash-preexec tag `0.6.0`, commit
  `b73ed5f7f953207b958f15b1773721dded697ac3`.
- `gtk4-layer-shell-1.1.0-MIT.txt`: gtk4-layer-shell tag `v1.1.0`, commit
  `93550245220cdc514be4701b517acd374a86acc2`, matching Ghostty's pinned Zig
  dependency.

Packaging concatenates these verbatim texts into
`usr/share/doc/forktty/copyright`; they are not runtime inputs.
