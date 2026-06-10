use std::env;
use std::path::PathBuf;

fn main() {
    // The include directory is produced by build.rs. After a successful
    // `cargo build -p libghostty-vt-sys`, the headers live in:
    //   target/<profile>/build/libghostty-vt-sys-<hash>/out/ghostty-install/include
    //
    // For convenience, also allow GHOSTTY_SOURCE_DIR/zig-out/include or
    // an explicit GHOSTTY_INCLUDE_DIR override.
    let include_dir = if let Ok(dir) = env::var("GHOSTTY_INCLUDE_DIR") {
        PathBuf::from(dir)
    } else if let Ok(src) = env::var("GHOSTTY_SOURCE_DIR") {
        PathBuf::from(src).join("zig-out").join("include")
    } else {
        // Walk target/debug/build/ to find the libghostty-vt-sys output.
        let manifest_dir =
            PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
        let workspace_root = manifest_dir
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root must exist")
            .to_path_buf();

        let build_dir = workspace_root.join("target").join("debug").join("build");
        let mut found = None;
        if let Ok(entries) = std::fs::read_dir(&build_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("libghostty-vt-sys-") {
                    let candidate = entry
                        .path()
                        .join("out")
                        .join("ghostty-install")
                        .join("include");
                    if candidate.join("ghostty").join("vt.h").exists() {
                        found = Some(candidate);
                        break;
                    }
                }
            }
        }
        found.unwrap_or_else(|| {
            panic!(
                "could not find ghostty headers; run `cargo build -p libghostty-vt-sys` first, \
                 or set GHOSTTY_INCLUDE_DIR or GHOSTTY_SOURCE_DIR"
            )
        })
    };

    let header = include_dir.join("ghostty").join("vt.h");
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let out = manifest_dir.join("src").join("bindings.rs");

    let mut builder = bindgen::Builder::default()
        .header(header.to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.to_string_lossy()))
        .allowlist_function("[Gg]hostty.*")
        .allowlist_type("[Gg]hostty.*")
        .allowlist_var("GHOSTTY_.*")
        .generate_cstr(true)
        .derive_default(true)
        .size_t_is_usize(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()));

    if cfg!(target_os = "linux") {
        builder = builder.clang_arg("-I/usr/include");
    }

    let bindings = builder
        .generate()
        .expect("failed to generate bindings from include/ghostty/vt.h");

    bindings
        .write_to_file(&out)
        .unwrap_or_else(|error| panic!("failed to write bindings to {}: {error}", out.display()));
}
