#!/bin/bash
sed -i 's/\.name = \.ghostty,/.name = "ghostty",/' target/debug/build/libghostty-vt-sys-*/out/ghostty-src/build.zig.zon
sed -i 's/minimum_zig_version = "0.15.2"/minimum_zig_version = "0.13.0"/' target/debug/build/libghostty-vt-sys-*/out/ghostty-src/build.zig.zon
