//! Release-safety checks for ForkTTY's vendored libghostty integration.
//!
//! [`check`] owns the repository gate for paired Cargo patches, the Zig
//! optimization override, and the native CPU-baseline branch. These checks
//! intentionally pin incident-derived source structure that must not drift.

use std::fs;
use std::path::Path;

const NATIVE_CPU_BASELINE_BRANCH: &str = r#"
if target != host {
let zig_target = zig_target(&target);
build.arg(format!("-Dtarget={zig_target}"));
} else {
build.arg("-Dcpu=baseline");
}
"#;

pub(crate) fn check(root: &Path) -> Result<(), String> {
    check_paired_cargo_patches(root)?;
    check_release_safe_optimization(root)?;
    check_native_cpu_baseline_branch(root)
}

fn check_paired_cargo_patches(root: &Path) -> Result<(), String> {
    let manifest_path = root.join("Cargo.toml");
    let manifest_raw = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("failed to read {}: {err}", manifest_path.display()))?;
    let manifest = manifest_raw
        .parse::<toml::Table>()
        .map_err(|err| format!("failed to parse {}: {err}", manifest_path.display()))?;
    let patches = manifest
        .get("patch")
        .and_then(toml::Value::as_table)
        .and_then(|patch| patch.get("crates-io"))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "Cargo.toml is missing [patch.crates-io]".to_string())?;
    for (package, expected_path) in [
        ("libghostty-vt", "vendor/libghostty-rs/crates/libghostty-vt"),
        (
            "libghostty-vt-sys",
            "vendor/libghostty-rs/crates/libghostty-vt-sys",
        ),
    ] {
        let actual_path = patches
            .get(package)
            .and_then(toml::Value::as_table)
            .and_then(|patch| patch.get("path"))
            .and_then(toml::Value::as_str);
        if actual_path != Some(expected_path) {
            return Err(format!(
                "[patch.crates-io].{package} must use path `{expected_path}` under vendor/libghostty-rs"
            ));
        }
    }
    Ok(())
}

fn check_release_safe_optimization(root: &Path) -> Result<(), String> {
    let cargo_config_path = root.join(".cargo/config.toml");
    let cargo_config_raw = fs::read_to_string(&cargo_config_path)
        .map_err(|err| format!("failed to read {}: {err}", cargo_config_path.display()))?;
    let cargo_config = cargo_config_raw
        .parse::<toml::Table>()
        .map_err(|err| format!("failed to parse {}: {err}", cargo_config_path.display()))?;
    let optimize = cargo_config
        .get("env")
        .and_then(toml::Value::as_table)
        .and_then(|env| env.get("LIBGHOSTTY_VT_SYS_OPTIMIZE"))
        .and_then(toml::Value::as_str);
    if optimize != Some("ReleaseSafe") {
        return Err(
            ".cargo/config.toml must set LIBGHOSTTY_VT_SYS_OPTIMIZE to `ReleaseSafe`".to_string(),
        );
    }
    Ok(())
}

fn check_native_cpu_baseline_branch(root: &Path) -> Result<(), String> {
    let build_script_path = root.join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs");
    let build_script = fs::read_to_string(&build_script_path)
        .map_err(|err| format!("failed to read {}: {err}", build_script_path.display()))?;
    let executable_lines = build_script
        .lines()
        .map(|line| line.split_once("//").map_or(line, |(code, _)| code).trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if !build_script.contains("FORKTTY PATCH")
        || !executable_lines.contains(NATIVE_CPU_BASELINE_BRANCH.trim())
    {
        return Err(format!(
            "{} must retain the FORKTTY PATCH that applies -Dcpu=baseline only in the native `target == host` branch",
            build_script_path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestDir;

    fn write_fixture(root: &Path) {
        fs::create_dir_all(root.join(".cargo")).unwrap();
        fs::create_dir_all(root.join("vendor/libghostty-rs/crates/libghostty-vt-sys")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            r#"
[patch.crates-io]
libghostty-vt = { path = "vendor/libghostty-rs/crates/libghostty-vt" }
libghostty-vt-sys = { path = "vendor/libghostty-rs/crates/libghostty-vt-sys" }
"#,
        )
        .unwrap();
        fs::write(
            root.join(".cargo/config.toml"),
            r#"
[env]
LIBGHOSTTY_VT_SYS_OPTIMIZE = "ReleaseSafe"
"#,
        )
        .unwrap();
        fs::write(
            root.join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs"),
            r#"
if target != host {
    let zig_target = zig_target(&target);
    build.arg(format!("-Dtarget={zig_target}"));
} else {
    // FORKTTY PATCH: keep distributed native builds on the baseline CPU.
    build.arg("-Dcpu=baseline");
}
"#,
        )
        .unwrap();
    }

    #[test]
    fn accepts_expected_release_invariants() {
        let temp = TestDir::new("libghostty-release-invariants-valid");
        write_fixture(temp.path());

        assert!(check(temp.path()).is_ok());
    }

    #[test]
    fn rejects_mixed_patch_sources() {
        let temp = TestDir::new("libghostty-release-invariants-mixed-source");
        write_fixture(temp.path());
        fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[patch.crates-io]
libghostty-vt = { path = "vendor/libghostty-rs/crates/libghostty-vt" }
libghostty-vt-sys = { git = "https://example.invalid/libghostty-rs" }
"#,
        )
        .unwrap();

        let error = check(temp.path()).unwrap_err();
        assert!(error.contains("libghostty-vt-sys"), "{error}");
        assert!(error.contains("vendor/libghostty-rs"), "{error}");
    }

    #[test]
    fn requires_release_safe_debug_builds() {
        let temp = TestDir::new("libghostty-release-invariants-optimize");
        write_fixture(temp.path());
        fs::write(
            temp.path().join(".cargo/config.toml"),
            r#"
[env]
LIBGHOSTTY_VT_SYS_OPTIMIZE = "Debug"
"#,
        )
        .unwrap();

        let error = check(temp.path()).unwrap_err();
        assert!(error.contains("LIBGHOSTTY_VT_SYS_OPTIMIZE"), "{error}");
        assert!(error.contains("ReleaseSafe"), "{error}");
    }

    #[test]
    fn rejects_cpu_baseline_in_cross_compile_branch() {
        let temp = TestDir::new("libghostty-release-invariants-cross-cpu");
        write_fixture(temp.path());
        fs::write(
            temp.path()
                .join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs"),
            r#"
// FORKTTY PATCH: this placement is unsafe.
if target != host {
    build.arg("-Dcpu=baseline");
} else {
    build.arg("-Dtarget=native");
}
"#,
        )
        .unwrap();

        let error = check(temp.path()).unwrap_err();
        assert!(error.contains("native `target == host` branch"), "{error}");
    }

    #[test]
    fn rejects_commented_cpu_baseline_argument() {
        let temp = TestDir::new("libghostty-release-invariants-commented-cpu");
        write_fixture(temp.path());
        fs::write(
            temp.path()
                .join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs"),
            r#"
if target != host {
    let zig_target = zig_target(&target);
    build.arg(format!("-Dtarget={zig_target}"));
} else {
    // FORKTTY PATCH: build.arg("-Dcpu=baseline");
}
"#,
        )
        .unwrap();

        let error = check(temp.path()).unwrap_err();
        assert!(error.contains("-Dcpu=baseline"), "{error}");
    }
}
