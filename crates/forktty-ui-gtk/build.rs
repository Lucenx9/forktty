fn main() {
    // Let the installed binary find the bundled libghostty next to itself:
    // both the AppImage (usr/bin/forktty + usr/lib/libghostty-vt.so.0) and the
    // deb (/usr/bin + /usr/lib) use this layout. Without it the binary loads
    // only when LD_LIBRARY_PATH happens to be set — AppRun sets it for the GUI,
    // but `forktty` CLI calls from shells (agent hooks, `forktty ping`) do not
    // inherit it because the PTY child environment strips AppImage lib paths.
    // Note: a link-arg emitted by forktty-terminal's build script would NOT
    // reach this package's binary; rustc-link-arg only applies to artifacts of
    // the emitting package.
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN/../lib");
}
