use std::path::PathBuf;

fn main() {
    let Ok(include_dir) = std::env::var("DEP_GHOSTTY_VT_INCLUDE") else {
        return;
    };
    let include_dir = PathBuf::from(include_dir);
    let Some(prefix) = include_dir.parent() else {
        return;
    };
    let lib_dir = prefix.join("lib");
    if !lib_dir.exists() {
        return;
    }
    let rpath = format!("-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rustc-link-arg={rpath}");
    println!("cargo:rustc-link-arg-tests={rpath}");
    println!("cargo:rustc-link-arg-bins={rpath}");
}
