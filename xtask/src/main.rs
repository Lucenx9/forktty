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

// Codex and Claude Code treat `timeout` as seconds, while Gemini CLI treats it
// as milliseconds. Keep these repository templates aligned with the native
// installer's hook timeout constants.
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
    ("WorktreeCreate", "worktree-create", 30),
    ("WorktreeRemove", "worktree-remove", 30),
    ("SessionEnd", "session-end", 30),
];

const GEMINI_ENTRIES: &[(&str, &str, u64)] = &[
    ("SessionStart", "session-start", 30000),
    ("BeforeAgent", "prompt-submit", 30000),
    ("BeforeTool", "pre-tool", 30000),
    ("BeforeToolSelection", "before-tool-selection", 30000),
    ("AfterTool", "post-tool", 30000),
    ("BeforeModel", "before-model", 30000),
    ("AfterModel", "after-model", 30000),
    ("AfterAgent", "stop", 30000),
    ("Notification", "notification", 30000),
    ("PreCompress", "pre-compact", 30000),
    ("SessionEnd", "session-end", 30000),
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
    HookTemplate {
        file: "gemini-settings.json",
        agent: "gemini",
        label: "Gemini",
        disabled_env: "FORKTTY_GEMINI_HOOKS_DISABLED",
        matcher: None,
        entries: GEMINI_ENTRIES,
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
const GHOSTTY_VENDOR_REV: &str = "470d3174eb10d25e21d17eff69ffcefdd4f4f91c";
const GHOSTTY_GTK_LIB_PROBE_SCRIPT: &str = "scripts/ghostty-gtk-lib-probe.sh";
const PACKAGING_SCRIPTS: &[&str] = &["scripts/build-deb.sh", "scripts/build-appimage.sh"];
const CI_WORKFLOW: &str = ".github/workflows/ci.yml";

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
    let command = env::args().nth(1).unwrap_or_else(|| "check".to_string());
    match command.as_str() {
        "check" => check_all(),
        "check-hooks" | "hooks" => check_hook_templates(),
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
  cargo run -p xtask -- check        Run all repository consistency checks
  cargo run -p xtask -- check-hooks  Validate agent hook templates
"
    );
}

fn check_all() -> Result<()> {
    check_no_legacy_node_cli()?;
    check_no_vte_references()?;
    check_full_ghostty_vendor()?;
    check_packaging_ghostty_gtk_lib_guard()?;
    check_hook_templates()
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

    println!("full Ghostty vendor: {actual}");
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
        ] {
            if !raw.contains(needle) {
                return Err(format!("{script} is missing `{needle}`"));
            }
        }
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
    match template.matcher {
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

    #[test]
    fn validates_full_ghostty_submodule_manifest() {
        let raw = r#"
[submodule "vendor/ghostty"]
	path = vendor/ghostty
	url = https://github.com/Lucenx9/ghostty.git
"#;

        assert!(validate_ghostty_gitmodules(raw).is_ok());
    }
}
