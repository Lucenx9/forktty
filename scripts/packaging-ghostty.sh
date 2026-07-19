#!/usr/bin/env bash

# Shared Ghostty packaging helpers. This file is sourced by the Debian and
# AppImage builders so both package formats select and validate the same build
# artifacts.

forktty_build_release_and_print_ghostty_out_dir() {
  local root_dir="$1"
  local messages out_dir build_status=0

  command -v python3 >/dev/null || {
    echo "python3 is required to resolve Cargo build artifacts" >&2
    return 1
  }

  messages="$(mktemp "${TMPDIR:-/tmp}/forktty-cargo-messages.XXXXXX")"
  (
    cd "$root_dir"
    cargo build -p forktty-ui-gtk \
      --no-default-features \
      --features gtk-ghostty \
      --release \
      --message-format=json-render-diagnostics > "$messages"
  ) || build_status=$?

  if ! out_dir="$(python3 - "$messages" <<'PY'
import json
import sys

matches = set()
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        if not line.lstrip().startswith("{"):
            sys.stderr.write(line)
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            sys.stderr.write(line)
            continue
        if message.get("reason") == "compiler-message":
            rendered = message.get("message", {}).get("rendered")
            if rendered:
                sys.stderr.write(rendered)
        package_id = message.get("package_id", "")
        if (
            message.get("reason") == "build-script-executed"
            and "/vendor/libghostty-rs/crates/libghostty-vt-sys#" in package_id
        ):
            matches.add(message["out_dir"])

if len(matches) != 1:
    print(
        "expected exactly one vendored libghostty-vt-sys OUT_DIR "
        f"from the current Cargo build, found {len(matches)}",
        file=sys.stderr,
    )
    raise SystemExit(1)

print(matches.pop())
PY
  )"; then
    rm -f "$messages"
    return 1
  fi
  rm -f "$messages"

  if [[ "$build_status" -ne 0 ]]; then
    return "$build_status"
  fi

  if [[ ! -d "$out_dir" ]]; then
    echo "Cargo reported a missing libghostty-vt-sys OUT_DIR: $out_dir" >&2
    return 1
  fi
  printf '%s\n' "$out_dir"
}

forktty_copy_ghostty_layer_shell_lib() {
  local ghostty_gtk_lib="$1"
  local lib_dir="$2"
  local source soname install_name

  command -v readelf >/dev/null || {
    echo "readelf is required to validate Ghostty runtime libraries" >&2
    return 1
  }
  if [[ ! -f "$ghostty_gtk_lib" ]]; then
    echo "Missing embedded Ghostty GTK library: $ghostty_gtk_lib" >&2
    return 1
  fi
  if [[ "$(
    readelf -d "$ghostty_gtk_lib" 2>/dev/null |
      awk '/Shared library: \[libgtk4-layer-shell\.so\]/ { found = 1 } END { print found + 0 }'
  )" != "1" ]]; then
    echo "Embedded Ghostty GTK library does not require libgtk4-layer-shell.so" >&2
    return 1
  fi

  # The Ghostty Zig build installs both libraries into one output directory.
  # Using the sibling binds the package to the exact embed build instead of an
  # unrelated host copy found by ldd or ldconfig.
  source="$(dirname "$ghostty_gtk_lib")/libgtk4-layer-shell.so"
  if [[ ! -f "$source" ]]; then
    echo "Missing Ghostty-built layer-shell sibling: $source" >&2
    return 1
  fi

  soname="$(
    readelf -d "$source" 2>/dev/null |
      awk '/\(SONAME\)/ { value = $NF; gsub(/[][]/, "", value) } END { print value }'
  )"
  install_name="${soname:-$(basename "$source")}"
  if [[ "$install_name" != libgtk4-layer-shell.so* ]]; then
    echo "Unexpected gtk4-layer-shell SONAME: $install_name" >&2
    return 1
  fi

  mkdir -p "$lib_dir"
  rm -f "$lib_dir"/libgtk4-layer-shell.so*
  install -Dm755 "$source" "$lib_dir/$install_name"
  if [[ "$install_name" != "libgtk4-layer-shell.so" ]]; then
    ln -s "$install_name" "$lib_dir/libgtk4-layer-shell.so"
  fi
  test -e "$lib_dir/libgtk4-layer-shell.so"
}
