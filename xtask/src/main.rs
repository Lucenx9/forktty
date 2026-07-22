use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy)]
struct HookTemplate {
    file: &'static str,
    agent: &'static str,
    label: &'static str,
    disabled_env: &'static str,
    matcher: Option<&'static str>,
    entries: &'static [(&'static str, &'static str, u64)],
}

// Keep repository templates aligned with the native installer's hook timeout
// constants.
const CODEX_ENTRIES: &[(&str, &str, u64)] = &[
    ("SessionStart", "session-start", 30),
    ("UserPromptSubmit", "prompt-submit", 30),
    ("PreToolUse", "pre-tool", 30),
    ("PostToolUse", "post-tool", 30),
    ("PermissionRequest", "permission-request", 30),
    ("PreCompact", "pre-compact", 30),
    ("PostCompact", "post-compact", 30),
    ("SubagentStart", "subagent-start", 30),
    ("SubagentStop", "subagent-stop", 30),
    ("Stop", "stop", 30),
];

const CLAUDE_ENTRIES: &[(&str, &str, u64)] = &[
    ("SessionStart", "session-start", 30),
    ("UserPromptSubmit", "prompt-submit", 30),
    ("UserPromptExpansion", "prompt-expansion", 30),
    ("Setup", "setup", 30),
    ("PermissionRequest", "permission-request", 30),
    ("PermissionDenied", "permission-denied", 30),
    ("PostToolBatch", "post-tool-batch", 30),
    ("SubagentStart", "subagent-start", 30),
    ("SubagentStop", "subagent-stop", 30),
    ("TaskCreated", "task-created", 30),
    ("TaskCompleted", "task-completed", 30),
    ("Elicitation", "elicitation", 30),
    ("ElicitationResult", "elicitation-result", 30),
    ("PreCompact", "pre-compact", 30),
    ("PostCompact", "post-compact", 30),
    ("Stop", "stop", 30),
    ("StopFailure", "stop-failure", 30),
    ("TeammateIdle", "teammate-idle", 30),
    ("Notification", "notification", 30),
    ("ConfigChange", "config-change", 30),
    ("InstructionsLoaded", "instructions-loaded", 30),
    ("CwdChanged", "cwd-changed", 30),
    ("FileChanged", "file-changed", 30),
    // WorktreeCreate is intentionally not registered: Claude Code treats it as a
    // provider hook that REPLACES default git worktree creation and must print a
    // worktree path on stdout, so registering an observational hook here breaks
    // `claude --worktree` and `isolation: "worktree"` subagents. WorktreeRemove
    // is advisory (its output is ignored) and stays.
    ("WorktreeRemove", "worktree-remove", 30),
    ("SessionEnd", "session-end", 30),
];

const TEMPLATES: &[HookTemplate] = &[
    HookTemplate {
        file: "codex-hooks.json",
        agent: "codex",
        label: "Codex",
        disabled_env: "FORKTTY_CODEX_HOOKS_DISABLED",
        matcher: None,
        entries: CODEX_ENTRIES,
    },
    HookTemplate {
        file: "claude-settings.json",
        agent: "claude",
        label: "Claude",
        disabled_env: "FORKTTY_CLAUDE_HOOKS_DISABLED",
        matcher: Some("*"),
        // The checked-in Claude template represents the default lifecycle
        // profile; `forktty hooks setup --full claude` adds per-tool events.
        entries: CLAUDE_ENTRIES,
    },
];

const REMOVED_NODE_CLI_FILES: &[&str] = &["scripts/forktty.mjs", "scripts/forktty.test.mjs"];

const OBSOLETE_NODE_CLI_REFS: &[&str] = &[
    "node --test scripts/forktty.test.mjs",
    "node scripts/forktty.mjs",
    "scripts/forktty.test.mjs",
    "scripts/forktty.mjs",
];

const REMOVED_VTE_REFS: &[&str] = &[
    "gtk-vte",
    "vte4",
    "vte4-sys",
    "libvte",
    "libvte-2.91",
    "native-gtk-vte",
];

const VTE_CHECK_PATHS: &[&str] = &[
    ".github",
    "Cargo.lock",
    "Cargo.toml",
    "README.md",
    "crates",
    "docs/QA.md",
    "docs/native-gtk-ghostty.md",
    "docs/release-qa.md",
    "packaging",
    "scripts",
];

const GHOSTTY_VENDOR_PATH: &str = "vendor/ghostty";
const GHOSTTY_VENDOR_URL: &str = "https://github.com/Lucenx9/ghostty.git";
const GHOSTTY_VENDOR_REV: &str = "24bd566a5d52bc338032e5582f423472afe094ad";
const GHOSTTY_GTK_LIB_PROBE_SCRIPT: &str = "scripts/ghostty-gtk-lib-probe.sh";
const PACKAGING_GHOSTTY_HELPER: &str = "scripts/packaging-ghostty.sh";
const PACKAGING_SCRIPTS: &[&str] = &["scripts/build-deb.sh", "scripts/build-appimage.sh"];
const GTK_GHOSTTY_SMOKE_SCRIPT: &str = "scripts/gtk-ghostty-smoke.sh";
const APPIMAGE_BUNDLED_CHECK_SCRIPT: &str = "scripts/check-appimage-bundled-container.sh";
const PACKAGE_LEGAL_SCRIPT: &str = "scripts/write-package-legal-docs.sh";
const CI_WORKFLOW: &str = ".github/workflows/ci.yml";
// Mirrors the probe script's packaging ABI. GhosttyGtkEmbedder::load is the
// source of truth for the first five symbols, which it loads unconditionally.
const GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS: &[&str] = &[
    "ghostty_gtk_context_new",
    "ghostty_gtk_context_free",
    "ghostty_gtk_context_register",
    "ghostty_gtk_context_tick",
    "ghostty_gtk_surface_new",
    "ghostty_gtk_context_set_wakeup_callback",
    "ghostty_gtk_surface_send_text",
    "ghostty_gtk_surface_new_with_working_directory_and_command",
    "ghostty_gtk_surface_new_with_working_directory_command_and_scrollback_limit",
    "ghostty_gtk_surface_read_text_limited",
    "ghostty_gtk_surface_exit_code",
    "ghostty_gtk_surface_child_pid",
    "ghostty_gtk_surface_perform_action",
    "ghostty_gtk_surface_restore_scrollback",
    "ghostty_gtk_text_free",
];
const GHOSTTY_GTK_RESIZE_PATCH_MARKERS: &[(&str, &str)] = &[
    (
        "src/terminal/PageList.zig",
        "FORKTTY PATCH: cursor-pin walking can cycle past the page list",
    ),
    (
        "src/terminal/PageList.zig",
        "FORKTTY PATCH: after column reflow, cursor-pin walking can",
    ),
    (
        "src/terminal/Screen.zig",
        "FORKTTY PATCH: use a temporary tracked pin for resize",
    ),
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "check".to_string());
    match command.as_str() {
        "check" => check_all(),
        "check-hooks" | "hooks" => check_hook_templates(),
        "check-release-tag" => check_release_tag(args.next().as_deref()),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(format!(
            "unknown command `{other}`\nRun `cargo run -p xtask -- help` for usage."
        )),
    }
}

fn print_help() {
    println!(
        "\
ForkTTY repository tasks

Usage:
  cargo run -p xtask -- check                Run all repository consistency checks
  cargo run -p xtask -- check-hooks          Validate agent hook templates
  cargo run -p xtask -- check-release-tag TAG  Verify TAG is v<workspace-version>
"
    );
}

fn check_release_tag(tag: Option<&str>) -> Result<()> {
    let manifest_path = repo_root().join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|err| format!("failed to read {}: {err}", manifest_path.display()))?;
    let version = workspace_version(&manifest)?;
    validate_release_tag(&version, tag)?;
    println!("release tag matches Cargo version: v{version}");
    Ok(())
}

fn workspace_version(manifest: &str) -> Result<String> {
    let manifest = manifest
        .parse::<toml::Table>()
        .map_err(|err| format!("failed to parse Cargo.toml: {err}"))?;
    manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .and_then(|workspace| workspace.get("package"))
        .and_then(toml::Value::as_table)
        .and_then(|package| package.get("version"))
        .and_then(toml::Value::as_str)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Cargo.toml is missing [workspace.package].version".to_string())
}

fn validate_release_tag(version: &str, tag: Option<&str>) -> Result<()> {
    let tag = tag.ok_or_else(|| "check-release-tag requires a tag argument".to_string())?;
    if !tag.starts_with('v') || tag.len() == 1 || tag.chars().any(char::is_whitespace) {
        return Err(format!(
            "malformed release tag `{tag}`; expected `v{version}`"
        ));
    }
    let expected = format!("v{version}");
    if tag != expected {
        return Err(format!(
            "release tag `{tag}` does not match Cargo version (expected `{expected}`)"
        ));
    }
    Ok(())
}

fn check_all() -> Result<()> {
    check_no_legacy_node_cli()?;
    check_no_vte_references()?;
    check_libghostty_release_invariants()?;
    check_full_ghostty_vendor()?;
    check_packaging_ghostty_gtk_lib_guard()?;
    check_hook_templates()
}

fn check_libghostty_release_invariants() -> Result<()> {
    check_libghostty_release_invariants_at(&repo_root())?;
    println!("libghostty release invariants: ok");
    Ok(())
}

fn check_libghostty_release_invariants_at(root: &Path) -> Result<()> {
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

    let build_script_path = root.join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs");
    let build_script = fs::read_to_string(&build_script_path)
        .map_err(|err| format!("failed to read {}: {err}", build_script_path.display()))?;
    for marker in ["FORKTTY PATCH", "build.arg(\"-Dcpu=baseline\")"] {
        if !build_script.contains(marker) {
            return Err(format!(
                "{} must retain the native -Dcpu=baseline FORKTTY PATCH marker and build argument",
                build_script_path.display()
            ));
        }
    }
    Ok(())
}

fn check_no_legacy_node_cli() -> Result<()> {
    let root = repo_root();
    let mut found = Vec::new();
    for path in REMOVED_NODE_CLI_FILES {
        if root.join(path).exists() {
            found.push(*path);
        }
    }
    if !found.is_empty() {
        return Err(format!(
            "legacy Node CLI files should stay removed: {}",
            found.join(", ")
        ));
    }
    for path in [
        ".github/workflows/ci.yml",
        "README.md",
        "CONTRIBUTING.md",
        "RELEASING.md",
        "docs/release-qa.md",
    ] {
        let full_path = root.join(path);
        let raw = fs::read_to_string(&full_path)
            .map_err(|err| format!("failed to read {}: {err}", full_path.display()))?;
        for needle in OBSOLETE_NODE_CLI_REFS {
            if raw.contains(needle) {
                return Err(format!(
                    "{} contains obsolete `{needle}`",
                    full_path.display()
                ));
            }
        }
    }
    println!("legacy Node CLI: absent");
    Ok(())
}

fn check_no_vte_references() -> Result<()> {
    let root = repo_root();
    let mut violations = Vec::new();
    for path in VTE_CHECK_PATHS {
        collect_vte_violations(&root, &root.join(path), &mut violations)?;
    }
    if !violations.is_empty() {
        return Err(format!(
            "VTE references should stay removed:\n{}",
            violations.join("\n")
        ));
    }
    println!("VTE references: absent");
    Ok(())
}

fn collect_vte_violations(root: &Path, path: &Path, violations: &mut Vec<String>) -> Result<()> {
    if path.is_dir() {
        let entries = fs::read_dir(path)
            .map_err(|err| format!("failed to read directory {}: {err}", path.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|err| format!("failed to read {} entry: {err}", path.display()))?;
            collect_vte_violations(root, &entry.path(), violations)?;
        }
        return Ok(());
    }
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    for needle in REMOVED_VTE_REFS {
        if contains_bytes(&raw, needle.as_bytes()) {
            let relative = path.strip_prefix(root).unwrap_or(path);
            violations.push(format!("{} contains `{needle}`", relative.display()));
        }
    }
    Ok(())
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn check_full_ghostty_vendor() -> Result<()> {
    let root = repo_root();
    let gitmodules_path = root.join(".gitmodules");
    let gitmodules = fs::read_to_string(&gitmodules_path)
        .map_err(|err| format!("failed to read {}: {err}", gitmodules_path.display()))?;
    validate_ghostty_gitmodules(&gitmodules)?;

    let path = root.join(GHOSTTY_VENDOR_PATH);
    for file in ["build.zig", "include/ghostty.h", "LICENSE"] {
        let required = path.join(file);
        if !required.is_file() {
            return Err(format!(
                "{} is missing; run `git submodule update --init {GHOSTTY_VENDOR_PATH}`",
                required.display()
            ));
        }
    }

    let output = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|err| format!("failed to run git for {GHOSTTY_VENDOR_PATH}: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to read {GHOSTTY_VENDOR_PATH} revision: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if actual != GHOSTTY_VENDOR_REV {
        return Err(format!(
            "{GHOSTTY_VENDOR_PATH} must be pinned to {GHOSTTY_VENDOR_REV}, got {actual}"
        ));
    }
    check_ghostty_vendor_resize_patch_markers(&root)?;

    println!("full Ghostty vendor: {actual}");
    Ok(())
}

fn check_ghostty_vendor_resize_patch_markers(root: &Path) -> Result<()> {
    let vendor = root.join(GHOSTTY_VENDOR_PATH);
    for (relative_path, marker) in GHOSTTY_GTK_RESIZE_PATCH_MARKERS {
        let path = vendor.join(relative_path);
        let raw = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        if !raw.contains(marker) {
            return Err(format!(
                "{} is missing Ghostty GTK resize safety patch marker `{marker}`",
                path.display()
            ));
        }
    }
    Ok(())
}

fn validate_ghostty_gitmodules(raw: &str) -> Result<()> {
    if !raw.contains("[submodule \"vendor/ghostty\"]") {
        return Err(format!(
            ".gitmodules must contain submodule section for {GHOSTTY_VENDOR_PATH}"
        ));
    }
    for line in [
        format!("path = {GHOSTTY_VENDOR_PATH}"),
        format!("url = {GHOSTTY_VENDOR_URL}"),
    ] {
        if !raw.lines().any(|raw_line| raw_line.trim() == line) {
            return Err(format!(".gitmodules is missing `{line}`"));
        }
    }
    Ok(())
}

fn check_packaging_ghostty_gtk_lib_guard() -> Result<()> {
    let root = repo_root();
    let probe_path = root.join(GHOSTTY_GTK_LIB_PROBE_SCRIPT);
    let probe = fs::read_to_string(&probe_path)
        .map_err(|err| format!("failed to read {}: {err}", probe_path.display()))?;
    for needle in ["--ensure", "--print-path"] {
        if !probe.contains(needle) {
            return Err(format!(
                "{} must support `{needle}` so packaging can reuse the Ghostty GTK probe",
                GHOSTTY_GTK_LIB_PROBE_SCRIPT
            ));
        }
    }
    if !probe.contains("\nbuild_ghostty_gtk_lib\nLIB_PATH=\"$(verified_lib_path)\"") {
        return Err(format!(
            "{GHOSTTY_GTK_LIB_PROBE_SCRIPT} must run the incremental Zig build before verification"
        ));
    }
    if probe.contains("if [[ \"$ENSURE\" != \"1\" ]]") {
        return Err(format!(
            "{GHOSTTY_GTK_LIB_PROBE_SCRIPT} must not let --ensure bypass the incremental Zig build"
        ));
    }
    for symbol in GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS {
        if !probe.contains(symbol) {
            return Err(format!(
                "{} must verify required Ghostty GTK ABI symbol `{symbol}`",
                GHOSTTY_GTK_LIB_PROBE_SCRIPT
            ));
        }
    }
    let helper_path = root.join(PACKAGING_GHOSTTY_HELPER);
    let helper = fs::read_to_string(&helper_path)
        .map_err(|err| format!("failed to read {}: {err}", helper_path.display()))?;
    for needle in [
        "--message-format=json-render-diagnostics",
        "build-script-executed",
        "/vendor/libghostty-rs/crates/libghostty-vt-sys#",
        "expected exactly one vendored libghostty-vt-sys OUT_DIR",
        "$(dirname \"$ghostty_gtk_lib\")/libgtk4-layer-shell.so",
        "Shared library: \\[libgtk4-layer-shell\\.so\\]",
    ] {
        if !helper.contains(needle) {
            return Err(format!("{PACKAGING_GHOSTTY_HELPER} is missing `{needle}`"));
        }
    }
    for script in PACKAGING_SCRIPTS {
        let path = root.join(script);
        let raw = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        for needle in [
            GHOSTTY_GTK_LIB_PROBE_SCRIPT,
            "--ensure",
            "--print-path",
            "ghostty-gtk-embed.so",
            "copy_vendored_ghostty_themes",
            "Catppuccin Mocha",
            "ghostty_iterm2_themes_field",
            "zig fetch",
            PACKAGING_GHOSTTY_HELPER,
            "forktty_build_release_and_print_ghostty_out_dir",
            "GHOSTTY_BUILD_OUT_DIR/ghostty-install/lib/libghostty-vt.so.0.1.0",
        ] {
            if !raw.contains(needle) {
                return Err(format!("{script} is missing `{needle}`"));
            }
        }
        for obsolete in ["find_newest_build_output", "ldconfig -p"] {
            if raw.contains(obsolete) {
                return Err(format!(
                    "{script} must not select Ghostty packaging artifacts through `{obsolete}`"
                ));
            }
        }
    }
    let legal_script_path = root.join(PACKAGE_LEGAL_SCRIPT);
    let legal_script = fs::read_to_string(&legal_script_path)
        .map_err(|err| format!("failed to read {}: {err}", legal_script_path.display()))?;
    for license in [
        "packaging/licenses/GPL-3.0-or-later.txt",
        "packaging/licenses/bash-preexec-0.6.0-MIT.txt",
        "packaging/licenses/gtk4-layer-shell-1.1.0-MIT.txt",
    ] {
        if !root.join(license).is_file() {
            return Err(format!("missing package license text `{license}`"));
        }
        if !legal_script.contains(license) {
            return Err(format!("{PACKAGE_LEGAL_SCRIPT} must include `{license}`"));
        }
    }
    let gtk_smoke_path = root.join(GTK_GHOSTTY_SMOKE_SCRIPT);
    let gtk_smoke = fs::read_to_string(&gtk_smoke_path)
        .map_err(|err| format!("failed to read {}: {err}", gtk_smoke_path.display()))?;
    for needle in [
        "shell = \"/bin/bash\"",
        "forktty-smoke-term=xterm-ghostty",
        "__ghostty_precmd",
    ] {
        if !gtk_smoke.contains(needle) {
            return Err(format!("{GTK_GHOSTTY_SMOKE_SCRIPT} is missing `{needle}`"));
        }
    }
    let bundled_check_path = root.join(APPIMAGE_BUNDLED_CHECK_SCRIPT);
    let bundled_check = fs::read_to_string(&bundled_check_path)
        .map_err(|err| format!("failed to read {}: {err}", bundled_check_path.display()))?;
    for needle in [
        "FORKTTY_APPIMAGE_GTK_RUNTIME=bundled",
        "appimage bundled container: AppRun auto mode did not fall back to bundled GTK",
        "appimage-child-exec",
        "--unset",
        "LD_LIBRARY_PATH",
        "/usr/bin/env",
        "xterm-ghostty",
        "__ghostty_precmd",
        "unusable-host-gtk",
    ] {
        if !bundled_check.contains(needle) {
            return Err(format!(
                "{APPIMAGE_BUNDLED_CHECK_SCRIPT} is missing `{needle}`"
            ));
        }
    }
    let appimage = fs::read_to_string(root.join("scripts/build-appimage.sh"))
        .map_err(|err| format!("failed to read scripts/build-appimage.sh: {err}"))?;
    for needle in [
        "libgtk4-layer-shell.so",
        "usr/lib/bundled",
        "copy_appimage_runtime_libs \"$APPDIR/usr/lib/ghostty-gtk-embed.so\"",
        "verify_appimage_private_lib_deps \"$APPDIR/usr/lib/ghostty-gtk-embed.so\"",
        r#"host_ld_library_path="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
host_gtk_stack_compatible() {
  LD_LIBRARY_PATH="$host_ld_library_path" \
  LD_PRELOAD="$HERE/usr/lib/ghostty-gtk-embed.so${LD_PRELOAD:+:$LD_PRELOAD}" \
  LD_BIND_NOW=1 \
  "$HERE/usr/bin/forktty" --version >/dev/null 2>&1
}"#,
        "if host_gtk_stack_compatible; then",
        "export LD_LIBRARY_PATH=\"$host_ld_library_path\"",
        r#"echo "Invalid FORKTTY_APPIMAGE_GTK_RUNTIME=$gtk_runtime; expected auto, host, or bundled" >&2"#,
    ] {
        if !appimage.contains(needle) {
            return Err(format!("scripts/build-appimage.sh is missing `{needle}`"));
        }
    }
    for obsolete in [
        "host_gtk_stack_available()",
        "grep -q 'libgtk-4\\.so\\.1'",
        "grep -q 'libadwaita-1\\.so\\.0'",
        "export LD_PRELOAD",
        "export LD_BIND_NOW",
        "AppRun always adds",
    ] {
        if appimage.contains(obsolete) {
            return Err(format!(
                "scripts/build-appimage.sh must not contain obsolete AppRun contract `{obsolete}`"
            ));
        }
    }
    let deb = fs::read_to_string(root.join("scripts/build-deb.sh"))
        .map_err(|err| format!("failed to read scripts/build-deb.sh: {err}"))?;
    if !deb.contains("forktty_copy_ghostty_layer_shell_lib") {
        return Err(
            "scripts/build-deb.sh must bundle the private libgtk4-layer-shell.so via \
             `forktty_copy_ghostty_layer_shell_lib` for the embedded Ghostty GTK library"
                .to_string(),
        );
    }
    if deb.contains("libgtk4-layer-shell0") {
        // ghostty-gtk-embed.so records DT_NEEDED libgtk4-layer-shell.so (unversioned),
        // which the Debian/Ubuntu runtime package does not provide (the unversioned
        // symlink is -dev only, and Ubuntu 24.04 lacks the package entirely). Depending
        // on the runtime package ships a .deb whose terminal panes cannot start; the
        // library must be bundled privately instead.
        return Err(
            "scripts/build-deb.sh must not depend on `libgtk4-layer-shell0`: bundle the \
             unversioned libgtk4-layer-shell.so privately instead"
                .to_string(),
        );
    }
    let zon_path = root.join(GHOSTTY_VENDOR_PATH).join("build.zig.zon");
    let zon = fs::read_to_string(&zon_path)
        .map_err(|err| format!("failed to read {}: {err}", zon_path.display()))?;
    for needle in [".iterm2_themes", "ghostty-themes-release", ".hash"] {
        if !zon.contains(needle) {
            return Err(format!(
                "{} must keep Ghostty's pinned `{needle}` theme dependency",
                zon_path.display()
            ));
        }
    }
    check_ci_builds_ghostty_gtk_lib_before_deb(&root)?;
    println!("Ghostty GTK packaging guard: ok");
    Ok(())
}

fn check_ci_builds_ghostty_gtk_lib_before_deb(root: &Path) -> Result<()> {
    let path = root.join(CI_WORKFLOW);
    let raw = fs::read_to_string(&path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let check_job = section_between(&raw, "\n  check:\n", "\n  browser-feature:\n")
        .ok_or_else(|| format!("{CI_WORKFLOW} is missing the check job before browser-feature"))?;
    let deb_index = check_job
        .find("- name: Debian package")
        .ok_or_else(|| format!("{CI_WORKFLOW} check job is missing Debian package step"))?;
    let before_deb = &check_job[..deb_index];

    for needle in [
        "blueprint_compiler_commit",
        GHOSTTY_GTK_LIB_PROBE_SCRIPT,
        "meson",
        "ninja-build",
        "python3-gi",
        "libxml2-utils",
        "pkg-config",
    ] {
        if !before_deb.contains(needle) {
            return Err(format!(
                "{CI_WORKFLOW} check job must prepare `{needle}` before Debian packaging"
            ));
        }
    }
    let release_index = raw
        .find("\n  release-package:\n")
        .ok_or_else(|| format!("{CI_WORKFLOW} is missing the release-package job"))?;
    let release_job = &raw[release_index..];
    for needle in [
        "xvfb",
        APPIMAGE_BUNDLED_CHECK_SCRIPT,
        "FORKTTY_SMOKE_BIN=\"$PWD/$appimage_artifact\"",
        GTK_GHOSTTY_SMOKE_SCRIPT,
    ] {
        if !release_job.contains(needle) {
            return Err(format!(
                "{CI_WORKFLOW} release-package job must run packaged AppImage smoke with `{needle}`"
            ));
        }
    }
    Ok(())
}

fn section_between<'a>(raw: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let start_index = raw.find(start)? + start.len();
    let rest = &raw[start_index..];
    let end_index = rest.find(end)?;
    Some(&rest[..end_index])
}

fn check_hook_templates() -> Result<()> {
    let hooks_dir = repo_root().join("hooks");
    for template in TEMPLATES {
        let path = hooks_dir.join(template.file);
        let raw = fs::read_to_string(&path)
            .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
        reject_obsolete_hook_refs(&path, &raw)?;
        let value: Value = serde_json::from_str(&raw)
            .map_err(|err| format!("failed to parse {}: {err}", path.display()))?;
        validate_hook_template(template, &path, &value)?;
    }
    println!("hook templates: ok");
    Ok(())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives under repo root")
        .to_path_buf()
}

fn reject_obsolete_hook_refs(path: &Path, raw: &str) -> Result<()> {
    for needle in [
        "{{FORKTTY_SCRIPT}}",
        "FORKTTY_SCRIPT",
        "FORKTTY_NODE",
        "forktty.mjs",
    ] {
        if raw.contains(needle) {
            return Err(format!("{} contains obsolete `{needle}`", path.display()));
        }
    }
    Ok(())
}

fn validate_hook_template(template: &HookTemplate, path: &Path, value: &Value) -> Result<()> {
    let hooks = value
        .get("hooks")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{} must contain a top-level hooks object", path.display()))?;
    let expected_events = template
        .entries
        .iter()
        .map(|(event, _, _)| *event)
        .collect::<BTreeSet<_>>();
    let actual_events = hooks.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual_events != expected_events {
        return Err(format!(
            "{} event set mismatch: expected {:?}, got {:?}",
            path.display(),
            expected_events,
            actual_events
        ));
    }

    for (event, hook_event, timeout) in template.entries {
        let entries = hooks
            .get(*event)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{}: {event} must be an array", path.display()))?;
        if entries.len() != 1 {
            return Err(format!(
                "{}: {event} must have exactly one ForkTTY entry",
                path.display()
            ));
        }
        validate_hook_group(template, path, event, hook_event, *timeout, &entries[0])?;
    }
    Ok(())
}

fn validate_hook_group(
    template: &HookTemplate,
    path: &Path,
    event: &str,
    hook_event: &str,
    timeout: u64,
    group: &Value,
) -> Result<()> {
    let object = group
        .as_object()
        .ok_or_else(|| format!("{}: {event} entry must be an object", path.display()))?;
    match hook_event_matcher(template, event) {
        Some(matcher) if object.get("matcher").and_then(Value::as_str) == Some(matcher) => {}
        Some(matcher) => {
            return Err(format!(
                "{}: {event} must use matcher `{matcher}`",
                path.display()
            ))
        }
        None if object.get("matcher").is_none() => {}
        None => {
            return Err(format!(
                "{}: {event} must not set a matcher",
                path.display()
            ))
        }
    }

    let hooks = object
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{}: {event}.hooks must be an array", path.display()))?;
    if hooks.len() != 1 {
        return Err(format!(
            "{}: {event}.hooks must contain exactly one command",
            path.display()
        ));
    }
    validate_hook_command(template, path, event, hook_event, timeout, &hooks[0])
}

fn hook_event_matcher(template: &HookTemplate, event: &str) -> Option<&'static str> {
    if template.agent == "claude" && event == "PostToolBatch" {
        None
    } else {
        template.matcher
    }
}

fn validate_hook_command(
    template: &HookTemplate,
    path: &Path,
    event: &str,
    hook_event: &str,
    timeout: u64,
    hook: &Value,
) -> Result<()> {
    let object = hook
        .as_object()
        .ok_or_else(|| format!("{}: {event} command must be an object", path.display()))?;
    if object.get("type").and_then(Value::as_str) != Some("command") {
        return Err(format!(
            "{}: {event} hook type must be command",
            path.display()
        ));
    }
    let expected_status = format!("ForkTTY {} hooks", template.label);
    if object.get("statusMessage").and_then(Value::as_str) != Some(expected_status.as_str()) {
        return Err(format!(
            "{}: {event} statusMessage must be `{expected_status}`",
            path.display()
        ));
    }
    if object.get("timeout").and_then(Value::as_u64) != Some(timeout) {
        return Err(format!(
            "{}: {event} timeout must be {timeout}",
            path.display()
        ));
    }
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{}: {event} command must be a string", path.display()))?;
    for needle in [
        format!("${{{}:-}}", template.disabled_env),
        "'{{FORKTTY_LAUNCHER}}'".to_string(),
        format!("hooks {} {hook_event}", template.agent),
        "echo '{\"continue\":true,\"suppressOutput\":false}'".to_string(),
    ] {
        if !command.contains(&needle) {
            return Err(format!(
                "{}: {event} command is missing `{needle}`",
                path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(name: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = env::temp_dir().join(format!(
                "forktty-xtask-{name}-{}-{timestamp}-{}",
                std::process::id(),
                NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[derive(Debug)]
    struct AppRunInvocation {
        args: String,
        ld_library_path: String,
        ld_preload: String,
        ld_bind_now: String,
    }

    struct AppRunResult {
        appdir: PathBuf,
        inherited_preload: PathBuf,
        invocations: Vec<AppRunInvocation>,
        stderr: String,
    }

    fn generated_app_run() -> String {
        let script = fs::read_to_string(repo_root().join("scripts/build-appimage.sh")).unwrap();
        let start_marker = "cat > \"$APPDIR/AppRun\" <<'APPRUN'\n";
        let start = script.find(start_marker).unwrap() + start_marker.len();
        let end = script[start..].find("\nAPPRUN\n").unwrap() + start;
        script[start..end].to_string()
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    // Independent expectations intentionally duplicate the packaging manifest
    // so the tests detect accidental removal from either the guard or script.
    const GHOSTTY_GTK_EMBEDDER_UNCONDITIONAL_SYMBOLS: &[&str] = &[
        "ghostty_gtk_context_new",
        "ghostty_gtk_context_free",
        "ghostty_gtk_context_register",
        "ghostty_gtk_context_tick",
        "ghostty_gtk_surface_new",
    ];

    const GHOSTTY_GTK_OPTIONAL_TOTAL_LINES_SYMBOL: &str =
        "ghostty_gtk_surface_read_text_limited_with_total_lines";

    const EXPECTED_GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS: &[&str] = &[
        "ghostty_gtk_context_new",
        "ghostty_gtk_context_free",
        "ghostty_gtk_context_register",
        "ghostty_gtk_context_tick",
        "ghostty_gtk_surface_new",
        "ghostty_gtk_context_set_wakeup_callback",
        "ghostty_gtk_surface_send_text",
        "ghostty_gtk_surface_new_with_working_directory_and_command",
        "ghostty_gtk_surface_new_with_working_directory_command_and_scrollback_limit",
        "ghostty_gtk_surface_read_text_limited",
        "ghostty_gtk_surface_exit_code",
        "ghostty_gtk_surface_child_pid",
        "ghostty_gtk_surface_perform_action",
        "ghostty_gtk_surface_restore_scrollback",
        "ghostty_gtk_text_free",
    ];

    struct GhosttyProbeFixture {
        _temp: TestDir,
        script: PathBuf,
        fake_bin: PathBuf,
        lib_path: PathBuf,
        symbols: PathBuf,
        zig_trace: PathBuf,
    }

    impl GhosttyProbeFixture {
        fn new() -> Self {
            let temp = TestDir::new("ghostty-probe");
            let root = temp.path();
            let scripts = root.join("scripts");
            let ghostty = root.join("vendor/ghostty");
            let fake_bin = root.join("fake-bin");
            fs::create_dir_all(&scripts).unwrap();
            fs::create_dir_all(&ghostty).unwrap();
            fs::create_dir_all(&fake_bin).unwrap();

            let script = scripts.join("ghostty-gtk-lib-probe.sh");
            fs::copy(repo_root().join(GHOSTTY_GTK_LIB_PROBE_SCRIPT), &script).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

            write_executable(
                &fake_bin.join("zig"),
                r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FORKTTY_TEST_ZIG_TRACE"
mkdir -p zig-out/lib
: > zig-out/lib/ghostty-gtk-embed.so
"#,
            );
            write_executable(
                &fake_bin.join("nm"),
                r#"#!/bin/sh
set -eu
while IFS= read -r symbol; do
  printf '0000000000000000 T %s\n' "$symbol"
done < "$FORKTTY_TEST_NM_SYMBOLS"
"#,
            );

            Self {
                lib_path: ghostty.join("zig-out/lib/ghostty-gtk-embed.so"),
                symbols: root.join("symbols"),
                zig_trace: root.join("zig-trace"),
                _temp: temp,
                script,
                fake_bin,
            }
        }

        fn run(&self, symbols: &[&str]) -> std::process::Output {
            fs::write(&self.symbols, format!("{}\n", symbols.join("\n"))).unwrap();
            Command::new("/bin/bash")
                .arg(&self.script)
                .args(["--ensure", "--print-path"])
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        self.fake_bin.display(),
                        env::var("PATH").unwrap_or_default()
                    ),
                )
                .env("FORKTTY_TEST_NM_SYMBOLS", &self.symbols)
                .env("FORKTTY_TEST_ZIG_TRACE", &self.zig_trace)
                .env_remove("FORKTTY_GHOSTTY_GTK_BUILD_OPTIMIZE")
                .output()
                .unwrap()
        }

        fn help(&self) -> std::process::Output {
            Command::new("/bin/bash")
                .arg(&self.script)
                .arg("--help")
                .output()
                .unwrap()
        }

        fn zig_invocations(&self) -> Vec<String> {
            fs::read_to_string(&self.zig_trace)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    fn inherited_preload_path() -> PathBuf {
        fs::read_to_string("/proc/self/maps")
            .unwrap()
            .lines()
            .filter_map(|line| line.split_whitespace().last())
            .find(|path| path.starts_with('/') && path.contains("/libc.so."))
            .map(PathBuf::from)
            .expect("test process should have libc mapped")
    }

    fn run_app_run(runtime: Option<&str>, probe_succeeds: bool) -> AppRunResult {
        let temp = TestDir::new("app-run");
        let appdir = temp.path().join("ForkTTY.AppDir");
        let bin_dir = appdir.join("usr/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(appdir.join("usr/lib/bundled")).unwrap();

        let trace = temp.path().join("trace");
        let app_run = appdir.join("AppRun");
        write_executable(&app_run, &generated_app_run());
        write_executable(
            &bin_dir.join("forktty"),
            r#"#!/bin/sh
set -eu
printf '%s\t%s\t%s\t%s\n' "$*" "${LD_LIBRARY_PATH:-}" "${LD_PRELOAD:-}" "${LD_BIND_NOW:-}" >> "$FORKTTY_TEST_TRACE"
if [ "${1:-}" = "--version" ]; then
  exit "$FORKTTY_TEST_PROBE_STATUS"
fi
"#,
        );
        write_executable(
            &bin_dir.join("ldconfig"),
            r#"#!/bin/sh
printf '%s\n' 'libgtk-4.so.1 => /host/libgtk-4.so.1' 'libadwaita-1.so.0 => /host/libadwaita-1.so.0'
"#,
        );

        let inherited_preload = inherited_preload_path();
        let mut command = Command::new("/bin/sh");
        command
            .arg(&app_run)
            .arg("launch-arg")
            .env("APPDIR", &appdir)
            .env("FORKTTY_TEST_TRACE", &trace)
            .env(
                "FORKTTY_TEST_PROBE_STATUS",
                if probe_succeeds { "0" } else { "1" },
            )
            .env("LD_LIBRARY_PATH", "/inherited/host-libs")
            .env("LD_PRELOAD", &inherited_preload)
            .env_remove("LD_BIND_NOW");
        if let Some(runtime) = runtime {
            command.env("FORKTTY_APPIMAGE_GTK_RUNTIME", runtime);
        } else {
            command.env_remove("FORKTTY_APPIMAGE_GTK_RUNTIME");
        }
        let output = command.output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "AppRun failed: {stderr}");

        let invocations = fs::read_to_string(trace)
            .unwrap()
            .lines()
            .map(|line| {
                let mut fields = line.splitn(4, '\t');
                AppRunInvocation {
                    args: fields.next().unwrap().to_string(),
                    ld_library_path: fields.next().unwrap().to_string(),
                    ld_preload: fields.next().unwrap().to_string(),
                    ld_bind_now: fields.next().unwrap().to_string(),
                }
            })
            .collect();
        AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            stderr,
        }
    }

    fn assert_real_launch(
        invocation: &AppRunInvocation,
        expected_ld_library_path: &str,
        expected_ld_preload: &str,
    ) {
        assert_eq!(invocation.args, "launch-arg");
        assert_eq!(invocation.ld_library_path, expected_ld_library_path);
        assert_eq!(invocation.ld_preload, expected_ld_preload);
        assert_eq!(invocation.ld_bind_now, "");
    }

    #[test]
    fn ghostty_probe_required_symbol_manifest_matches_expected_contract() {
        assert_eq!(
            GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS,
            EXPECTED_GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS
        );
    }

    #[test]
    fn ghostty_probe_ensure_enters_incremental_build_on_every_invocation() {
        let fixture = GhosttyProbeFixture::new();
        for invocation in 1..=2 {
            let output = fixture.run(EXPECTED_GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS);
            assert!(
                output.status.success(),
                "--ensure invocation {invocation} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                fixture.lib_path.display().to_string(),
                "--print-path must return the verified fake library"
            );
        }

        let invocations = fixture.zig_invocations();
        assert_eq!(
            invocations.len(),
            2,
            "each --ensure call must enter Zig's incremental build graph: {invocations:?}"
        );
        for invocation in invocations {
            assert!(invocation.contains("-Doptimize=ReleaseSafe"));
            assert!(invocation.contains("-Dcpu=baseline"));
        }
        assert_eq!(
            fs::read_to_string(format!("{}.forktty-build", fixture.lib_path.display())).unwrap(),
            "optimize=ReleaseSafe\nblueprint-helper=llvm\n"
        );

        let help = fixture.help();
        assert!(help.status.success());
        assert!(String::from_utf8_lossy(&help.stdout).contains(
            "--ensure      Run the incremental Zig build, then verify the resulting library."
        ));
    }

    #[test]
    fn ghostty_probe_rejects_each_missing_unconditional_embedder_symbol() {
        for missing in GHOSTTY_GTK_EMBEDDER_UNCONDITIONAL_SYMBOLS {
            let fixture = GhosttyProbeFixture::new();
            let symbols = EXPECTED_GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS
                .iter()
                .copied()
                .filter(|symbol| symbol != missing)
                .collect::<Vec<_>>();
            let output = fixture.run(&symbols);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success(),
                "probe accepted a library missing {missing}: {stderr}"
            );
            assert!(
                stderr.contains(missing),
                "missing-symbol error did not name {missing}: {stderr}"
            );
        }
    }

    #[test]
    fn ghostty_probe_accepts_library_without_optional_total_lines_symbol() {
        let fixture = GhosttyProbeFixture::new();
        let symbols = EXPECTED_GHOSTTY_GTK_PROBE_REQUIRED_SYMBOLS
            .iter()
            .copied()
            .filter(|symbol| *symbol != GHOSTTY_GTK_OPTIONAL_TOTAL_LINES_SYMBOL)
            .collect::<Vec<_>>();

        let output = fixture.run(&symbols);
        assert!(
            output.status.success(),
            "probe rejected a library without optional {GHOSTTY_GTK_OPTIONAL_TOTAL_LINES_SYMBOL}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn app_run_auto_uses_host_only_when_loader_probe_succeeds() {
        let AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            ..
        } = run_app_run(None, true);
        let appdir = appdir.display();
        let inherited_preload = inherited_preload.display();
        assert_eq!(invocations.len(), 2, "expected probe followed by launch");
        assert_eq!(invocations[0].args, "--version");
        assert_eq!(
            invocations[0].ld_library_path,
            format!("{appdir}/usr/lib:/inherited/host-libs")
        );
        assert_eq!(
            invocations[0].ld_preload,
            format!("{appdir}/usr/lib/ghostty-gtk-embed.so:{inherited_preload}")
        );
        assert_eq!(invocations[0].ld_bind_now, "1");
        assert_real_launch(
            &invocations[1],
            &format!("{appdir}/usr/lib:/inherited/host-libs"),
            &inherited_preload.to_string(),
        );
    }

    #[test]
    fn app_run_auto_falls_back_to_bundled_when_loader_probe_fails() {
        let AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            ..
        } = run_app_run(None, false);
        let appdir = appdir.display();
        let inherited_preload = inherited_preload.display();
        assert_eq!(invocations.len(), 2, "expected probe followed by launch");
        assert_eq!(invocations[0].args, "--version");
        assert_eq!(
            invocations[0].ld_library_path,
            format!("{appdir}/usr/lib:/inherited/host-libs")
        );
        assert_eq!(
            invocations[0].ld_preload,
            format!("{appdir}/usr/lib/ghostty-gtk-embed.so:{inherited_preload}")
        );
        assert_eq!(invocations[0].ld_bind_now, "1");
        assert_real_launch(
            &invocations[1],
            &format!("{appdir}/usr/lib:{appdir}/usr/lib/bundled:/inherited/host-libs"),
            &inherited_preload.to_string(),
        );
    }

    #[test]
    fn app_run_explicit_host_forces_host_without_probing() {
        let AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            ..
        } = run_app_run(Some("host"), false);
        let appdir = appdir.display();
        let inherited_preload = inherited_preload.display();
        assert_eq!(invocations.len(), 1, "host override must skip the probe");
        assert_real_launch(
            &invocations[0],
            &format!("{appdir}/usr/lib:/inherited/host-libs"),
            &inherited_preload.to_string(),
        );
    }

    #[test]
    fn app_run_explicit_bundled_skips_host_probe() {
        let AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            ..
        } = run_app_run(Some("bundled"), true);
        let appdir = appdir.display();
        let inherited_preload = inherited_preload.display();
        assert_eq!(invocations.len(), 1, "bundled override must skip the probe");
        assert_real_launch(
            &invocations[0],
            &format!("{appdir}/usr/lib:{appdir}/usr/lib/bundled:/inherited/host-libs"),
            &inherited_preload.to_string(),
        );
    }

    #[test]
    fn app_run_invalid_runtime_warns_and_uses_bundled_without_probing() {
        let AppRunResult {
            appdir,
            inherited_preload,
            invocations,
            stderr,
        } = run_app_run(Some("invalid"), true);
        let appdir = appdir.display();
        let inherited_preload = inherited_preload.display();
        assert_eq!(
            invocations.len(),
            1,
            "invalid runtime must skip the host probe"
        );
        assert_real_launch(
            &invocations[0],
            &format!("{appdir}/usr/lib:{appdir}/usr/lib/bundled:/inherited/host-libs"),
            &inherited_preload.to_string(),
        );
        assert_eq!(
            stderr,
            "Invalid FORKTTY_APPIMAGE_GTK_RUNTIME=invalid; expected auto, host, or bundled\n"
        );
    }

    #[test]
    fn appimage_bundled_check_rejects_ambiguous_default_artifacts() {
        let temp = TestDir::new("appimage-bundled-ambiguous");
        let root = temp.path();
        let scripts = root.join("scripts");
        let artifacts = root.join("target/packaging/appimage");
        fs::create_dir_all(&scripts).unwrap();
        fs::create_dir_all(&artifacts).unwrap();

        let script = scripts.join("check-appimage-bundled-container.sh");
        fs::copy(repo_root().join(APPIMAGE_BUNDLED_CHECK_SCRIPT), &script).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        for name in ["forktty-alpha.AppImage", "forktty-beta.AppImage"] {
            write_executable(&artifacts.join(name), "#!/bin/sh\nexit 0\n");
        }

        let output = Command::new("/bin/bash").arg(&script).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.code(),
            Some(2),
            "unexpected failure: {stderr}"
        );
        assert!(
            stderr.contains("multiple AppImage artifacts found"),
            "{stderr}"
        );
        assert!(stderr.contains("pass one explicitly"), "{stderr}");
    }

    #[test]
    fn appimage_bundled_check_does_not_require_bubblewrap() {
        let script = fs::read_to_string(repo_root().join(APPIMAGE_BUNDLED_CHECK_SCRIPT)).unwrap();

        assert!(
            !script.contains("bwrap"),
            "the loader smoke must run where unprivileged user namespaces are unavailable"
        );
        assert!(script.contains("libgtk-4.so.1"));
        assert!(script.contains("libadwaita-1.so.0"));
    }

    #[test]
    fn package_legal_output_contains_complete_pinned_license_material() {
        let temp = TestDir::new("package-licenses");
        let package_root = temp.path().join("package-root");
        let output = Command::new("/bin/bash")
            .arg(repo_root().join(PACKAGE_LEGAL_SCRIPT))
            .arg(&package_root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "legal generator failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let copyright =
            fs::read_to_string(package_root.join("usr/share/doc/forktty/copyright")).unwrap();
        for needle in [
            "GNU GENERAL PUBLIC LICENSE",
            "Copyright (c) 2017 Ryan Caloras and contributors",
            "Copyright (c) 2023 Sophie Winter",
            "License: MIT (libghostty-rs)",
        ] {
            assert!(copyright.contains(needle), "missing `{needle}`");
        }
    }

    #[test]
    fn validates_full_ghostty_submodule_manifest() {
        let raw = r#"
[submodule "vendor/ghostty"]
	path = vendor/ghostty
	url = https://github.com/Lucenx9/ghostty.git
"#;

        assert!(validate_ghostty_gitmodules(raw).is_ok());
    }

    #[test]
    fn release_tag_validation_accepts_workspace_version() {
        assert!(validate_release_tag("0.2.0-alpha.19", Some("v0.2.0-alpha.19")).is_ok());
    }

    #[test]
    fn release_tag_validation_rejects_mismatch_missing_and_malformed_tags() {
        assert!(
            validate_release_tag("0.2.0-alpha.19", Some("v0.2.0-alpha.18"))
                .unwrap_err()
                .contains("expected `v0.2.0-alpha.19`")
        );
        assert!(validate_release_tag("0.2.0-alpha.19", None)
            .unwrap_err()
            .contains("requires a tag argument"));
        assert!(
            validate_release_tag("0.2.0-alpha.19", Some("0.2.0-alpha.19"))
                .unwrap_err()
                .contains("malformed release tag")
        );
    }

    #[test]
    fn workspace_version_is_read_structurally() {
        let manifest = r#"
[package]
version = "9.9.9"

[workspace.package]
version = "0.2.0-alpha.19"
"#;
        assert_eq!(workspace_version(manifest).unwrap(), "0.2.0-alpha.19");
        assert!(workspace_version("[workspace]\nmembers = []\n").is_err());
        assert!(workspace_version("not = [valid").is_err());
    }

    fn write_libghostty_release_fixture(root: &Path) {
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
// FORKTTY PATCH: keep distributed native builds on the baseline CPU.
if target == host {
    build.arg("-Dcpu=baseline");
}
"#,
        )
        .unwrap();
    }

    #[test]
    fn libghostty_release_invariants_accept_expected_pins() {
        let temp = TestDir::new("libghostty-release-invariants-valid");
        write_libghostty_release_fixture(temp.path());

        assert!(check_libghostty_release_invariants_at(temp.path()).is_ok());
    }

    #[test]
    fn libghostty_release_invariants_reject_mixed_patch_sources() {
        let temp = TestDir::new("libghostty-release-invariants-mixed-source");
        write_libghostty_release_fixture(temp.path());
        fs::write(
            temp.path().join("Cargo.toml"),
            r#"
[patch.crates-io]
libghostty-vt = { path = "vendor/libghostty-rs/crates/libghostty-vt" }
libghostty-vt-sys = { git = "https://example.invalid/libghostty-rs" }
"#,
        )
        .unwrap();

        let error = check_libghostty_release_invariants_at(temp.path()).unwrap_err();
        assert!(error.contains("libghostty-vt-sys"), "{error}");
        assert!(error.contains("vendor/libghostty-rs"), "{error}");
    }

    #[test]
    fn libghostty_release_invariants_require_release_safe_debug_builds() {
        let temp = TestDir::new("libghostty-release-invariants-optimize");
        write_libghostty_release_fixture(temp.path());
        fs::write(
            temp.path().join(".cargo/config.toml"),
            r#"
[env]
LIBGHOSTTY_VT_SYS_OPTIMIZE = "Debug"
"#,
        )
        .unwrap();

        let error = check_libghostty_release_invariants_at(temp.path()).unwrap_err();
        assert!(error.contains("LIBGHOSTTY_VT_SYS_OPTIMIZE"), "{error}");
        assert!(error.contains("ReleaseSafe"), "{error}");
    }

    #[test]
    fn libghostty_release_invariants_require_native_cpu_baseline_patch() {
        let temp = TestDir::new("libghostty-release-invariants-cpu");
        write_libghostty_release_fixture(temp.path());
        fs::write(
            temp.path()
                .join("vendor/libghostty-rs/crates/libghostty-vt-sys/build.rs"),
            r#"
if target == host {
    build.arg("-Dcpu=native");
}
"#,
        )
        .unwrap();

        let error = check_libghostty_release_invariants_at(temp.path()).unwrap_err();
        assert!(error.contains("-Dcpu=baseline"), "{error}");
        assert!(error.contains("FORKTTY PATCH"), "{error}");
    }
}
