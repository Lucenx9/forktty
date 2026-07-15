//! Agent hook installer, health checks, and hook event handling.

use super::integration_files::{
    atomic_write_file, backup_file, ensure_parent_dir, hook_config_write_path, read_json_file,
    stable_hook_launcher_path, APPIMAGE_EXTRACT_AND_RUN_ENV,
};
use super::{
    bool_option, format_doctor_path, inspect_path, parse_flags, print_json, reject_unknown_options,
    require_no_args, sanitize_for_terminal, send_socket_request, socket_path_from_env, trimmed_env,
    write_stdout_line, write_stdout_text, CliContext, CliError, CliResult, HOOKS_HELP_TEXT,
};
use serde_json::{json, Map, Value};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) mod event;
pub(super) mod install;
mod specs;
use install::{
    agent_spec, antigravity_script_path, build_hook_remove_plan, build_hook_setup_plan,
    build_hook_setup_plan_with_profile, default_hook_setup_agents,
    ensure_private_antigravity_hook_dirs, hook_setup_profile_name, is_claude_high_frequency_event,
    is_forktty_managed_entry, normalize_agent_name, read_opencode_plugin_file, supported_agents,
    supported_hook_remove_agents, HookRemoveAction,
};

pub(super) use event::{
    handle_hook_event, hook_target_params, increment_hook_event_order, next_hook_event_order,
};
#[cfg(test)]
pub(super) use specs::HOOK_ENTRY_TIMEOUT_SECS;
pub(super) use specs::{
    AgentSpec, HookEntrySpec, HookInstallKind, HookSetupProfile, AGENTS,
    CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES, CLAUDE_HOOK_ENTRIES, CODEX_HOOK_ENTRIES,
    DEFAULT_HOOK_SETUP_AGENT_KEYS, FORKTTY_HOOK_TAG, HOOK_CONTINUE_JSON, HOOK_EVENT_CLOCK,
    HOOK_EVENT_ORDER_PARAM, HOOK_STATUS_TIMEOUT, HOOK_TOKEN_CEILING_DEFAULT, HOOK_TOOL_LABEL_MAX,
    LEGACY_GEMINI_HOOK_AGENT, OPENCODE_HOOK_TIMEOUT_MS, OPENCODE_MAX_INPUT_BYTES,
    OPENCODE_PLUGIN_TAG,
};

pub(super) fn handle_hooks(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("help" | "--help" | "-h") => write_stdout_text(HOOKS_HELP_TEXT),
        Some("setup") => handle_hooks_setup(context, args[1..].to_vec()),
        Some("remove") | Some("uninstall") => handle_hooks_remove(context, args[1..].to_vec()),
        Some("doctor") => handle_hooks_doctor(context, args[1..].to_vec()),
        Some("test") => handle_hooks_test(context, args[1..].to_vec()),
        Some(_) => handle_hook_event(context, args),
        None => Err(CliError::new(format!(
            "hooks requires a subcommand: setup, remove, doctor, test, or `<agent> <event>` (agents: {})",
            supported_agent_keys()
        ))),
    }
}

pub(super) fn supported_agent_keys() -> String {
    AGENTS
        .iter()
        .map(|spec| spec.key)
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn handle_hooks_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run", "full"]);
    reject_unknown_options(&parsed.options, &["dry-run", "full"], "hooks setup")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "hooks setup: --dry-run must be true or false",
        ));
    };
    let Some(full) = bool_option(&parsed.options, "full") else {
        return Err(CliError::new("hooks setup: --full must be true or false"));
    };
    let profile = if full {
        HookSetupProfile::Full
    } else {
        HookSetupProfile::Lifecycle
    };
    let agents = if parsed.positionals.is_empty() {
        default_hook_setup_agents()
    } else {
        supported_agents(&parsed.positionals)?
    };
    let launcher = stable_hook_launcher_path().ok_or_else(|| {
        CliError::new("hooks setup: could not resolve current forktty executable")
    })?;
    if !launcher.is_absolute() {
        return Err(CliError::new(
            "hooks setup: forktty executable path must be absolute",
        ));
    }
    if !dry_run {
        for spec in &agents {
            if spec.key == "antigravity" {
                ensure_private_antigravity_hook_dirs()?;
            }
        }
    }

    let mut plans = Vec::new();
    for spec in agents {
        let plan = if profile == HookSetupProfile::Lifecycle {
            build_hook_setup_plan(spec, &launcher)?
        } else {
            build_hook_setup_plan_with_profile(spec, &launcher, profile)?
        };
        plans.push(plan);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            let write_path = hook_config_write_path(&plan.config_path)?;
            ensure_parent_dir(&write_path)?;
            backup_path = backup_file(&write_path)?;
            atomic_write_file(&write_path, plan.content.as_bytes())?;
            for (script_path, content) in &plan.scripts {
                ensure_parent_dir(script_path)?;
                atomic_write_file(script_path, content.as_bytes())?;
                fs::set_permissions(script_path, fs::Permissions::from_mode(0o700))?;
            }
        }
        let mut summary = json!({
            "agent": plan.spec.key,
            "configPath": plan.config_path,
            "changed": plan.changed,
            "backupPath": backup_path,
            "dryRun": dry_run,
        });
        if plan.spec.key == "claude" {
            summary["profile"] = json!(hook_setup_profile_name(profile));
        }
        if plan.spec.key == "codex" {
            summary["requiresTrustReview"] = json!(plan.changed);
            summary["trustReviewCommand"] = json!("/hooks");
        }
        summaries.push(summary);
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let agent = summary["agent"].as_str().unwrap_or("agent");
        let config_path = summary["configPath"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would update"
        } else if changed {
            "updated"
        } else {
            "already configured"
        };
        write_stdout_line(&format!("{agent}: {verb} at {config_path}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
        if summary["requiresTrustReview"].as_bool() == Some(true) {
            let prefix = if dry_run {
                "  after updating"
            } else {
                "  action required"
            };
            write_stdout_line(&format!(
                "{prefix}: run /hooks inside Codex to review the changed hook definitions"
            ))?;
        }
    }
    Ok(())
}

pub(super) fn handle_hooks_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "hooks remove")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "hooks remove: --dry-run must be true or false",
        ));
    };
    let agents = supported_hook_remove_agents(&parsed.positionals)?;
    let current_launcher = stable_hook_launcher_path();

    let mut plans = Vec::new();
    for spec in agents {
        plans.push(build_hook_remove_plan(spec, current_launcher.as_deref())?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            match &plan.action {
                HookRemoveAction::Write(content) => {
                    let write_path = hook_config_write_path(&plan.config_path)?;
                    ensure_parent_dir(&write_path)?;
                    backup_path = backup_file(&write_path)?;
                    atomic_write_file(&write_path, content.as_bytes())?;
                }
                HookRemoveAction::DeleteFile => {
                    backup_path = backup_file(&plan.config_path)?;
                    fs::remove_file(&plan.config_path)?;
                }
                HookRemoveAction::None => {}
            }
            if let Some(scripts_dir) = &plan.scripts_dir {
                if let Err(err) = fs::remove_dir_all(scripts_dir) {
                    if err.kind() != io::ErrorKind::NotFound {
                        return Err(err.into());
                    }
                }
            }
        }
        summaries.push(json!({
            "agent": plan.spec.key,
            "configPath": plan.config_path,
            "changed": plan.changed,
            "backupPath": backup_path,
            "dryRun": dry_run,
        }));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let agent = summary["agent"].as_str().unwrap_or("agent");
        let config_path = summary["configPath"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would remove"
        } else if changed {
            "removed"
        } else {
            "not installed"
        };
        write_stdout_line(&format!("{agent}: {verb} at {config_path}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
    }
    Ok(())
}

pub(super) fn handle_hooks_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let (spec, rest) = single_agent_command(args, "hooks doctor")?;
    require_no_args(&rest, &format!("hooks doctor {}", spec.key))?;
    let socket_info = inspect_path(&context.socket_path);
    let current_launcher = stable_hook_launcher_path();
    let launcher_info = current_launcher.as_ref().map(|path| inspect_path(path));
    let config_path = (spec.config_path)();
    let config_info = inspect_path(&config_path);
    let launcher_check = describe_launcher_check(spec, &config_path, current_launcher.as_deref());
    let supported_events: Vec<&str> = spec
        .hook_entries
        .iter()
        .map(|entry| entry.event_name)
        .collect();
    let mut report = json!({
        "agent": spec.key,
        "socket": {
            "path": context.socket_path,
            "source": if context.socket_explicit { "argument" } else if socket_path_from_env().is_some() { "FORKTTY_SOCKET_PATH" } else { "default" },
            "inspect": socket_info,
        },
        "env": {
            "FORKTTY_SOCKET_PATH": trimmed_env("FORKTTY_SOCKET_PATH"),
            "FORKTTY_WORKSPACE_ID": trimmed_env("FORKTTY_WORKSPACE_ID"),
            "FORKTTY_SURFACE_ID": trimmed_env("FORKTTY_SURFACE_ID"),
            "CODEX_HOME": trimmed_env("CODEX_HOME"),
            "CLAUDE_CONFIG_DIR": trimmed_env("CLAUDE_CONFIG_DIR"),
            "OPENCODE_CONFIG_DIR": trimmed_env("OPENCODE_CONFIG_DIR"),
            "HOME": trimmed_env("HOME"),
        },
        "executable": {
            "forktty": launcher_info,
        },
        "hookConfig": config_info,
        "launcherCheck": launcher_check,
        "supportedEvents": supported_events,
    });
    if spec.key == "claude" {
        report["installedProfile"] = json!(describe_claude_installed_profile(&config_path));
    }
    let hooks_installed = report["launcherCheck"]["status"].as_str() != Some("not_installed");
    if spec.key == "codex" && hooks_installed {
        report["trustCheck"] = describe_codex_hook_trust(&config_path);
    }
    let healthy = report["socket"]["inspect"]["exists"] == json!(true)
        && report["socket"]["inspect"]["readable"] == json!(true)
        && report["socket"]["inspect"]["writable"] == json!(true)
        && report["hookConfig"]["exists"] == json!(true)
        && report["launcherCheck"]["status"] == json!("ok");
    report["version"] = json!(1);
    report["ok"] = json!(healthy);
    if context.json {
        print_json(&report)?;
        return hooks_health_exit("hooks doctor", spec.key, healthy);
    }
    eprintln!("ForkTTY {} hook doctor", spec.label);
    eprintln!(
        "socket source: {}",
        report["socket"]["source"].as_str().unwrap_or("default")
    );
    eprintln!(
        "{}",
        format_doctor_path("socket", &report["socket"]["inspect"])
    );
    if !report["executable"]["forktty"].is_null() {
        eprintln!(
            "{}",
            format_doctor_path("forktty", &report["executable"]["forktty"])
        );
    }
    eprintln!("environment:");
    if let Some(env) = report["env"].as_object() {
        for (key, value) in env {
            eprintln!("  {key}={}", value.as_str().unwrap_or("(unset)"));
        }
    }
    eprintln!(
        "{}",
        format_doctor_path(&format!("{} hook config", spec.key), &report["hookConfig"])
    );
    eprintln!("supported events: {}", supported_events.join(", "));
    if let Some(profile) = report["installedProfile"].as_str() {
        eprintln!("installed profile: {profile}");
    }
    if let Some(line) = format_launcher_check(&report["launcherCheck"]) {
        eprintln!("{line}");
    }
    if let Some(line) = format_codex_trust_check(&report["trustCheck"]) {
        eprintln!("{line}");
    }
    hooks_health_exit("hooks doctor", spec.key, healthy)
}

/// Exit-code contract for hooks doctor/test: 0 when every check passes,
/// 1 otherwise, so CI can gate on the exit code alone.
pub(super) fn hooks_health_exit(command: &str, agent: &str, healthy: bool) -> CliResult<()> {
    if healthy {
        Ok(())
    } else {
        Err(CliError {
            message: format!("{command} {agent}: problems found (see report above)"),
            code: None,
            exit: 1,
        })
    }
}

pub(super) fn record_hook_check(
    checks: &mut Vec<Value>,
    method: &str,
    result: Result<Value, CliError>,
) -> Option<Value> {
    match result {
        Ok(value) => {
            checks.push(json!({ "method": method, "ok": true }));
            Some(value)
        }
        Err(err) => {
            checks.push(json!({ "method": method, "ok": false, "error": err.message }));
            None
        }
    }
}

pub(super) fn format_codex_trust_check(check: &Value) -> Option<String> {
    let status = check.get("status").and_then(Value::as_str)?;
    let unrecorded = check
        .get("unrecordedEvents")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    match status {
        "partial" | "none_recorded" => Some(format!(
            "hook trust: no Codex trust record yet for {unrecorded}; if those hooks seem inactive, run /hooks inside Codex to review approval."
        )),
        "all_recorded" => Some(
            "hook trust: records exist, but ForkTTY cannot verify that they match the current hook hashes; after a hook update, run /hooks inside Codex to review approval."
                .to_string(),
        ),
        _ => None,
    }
}

pub(super) fn describe_launcher_check(
    spec: &AgentSpec,
    config_path: &Path,
    current_launcher: Option<&Path>,
) -> Value {
    let installed = match spec.install_kind {
        HookInstallKind::JsonConfig => match read_json_file(config_path) {
            Ok(value) => extract_managed_launcher_from_config(spec, &value),
            Err(_) => None,
        },
        HookInstallKind::OpenCodePlugin => match read_opencode_plugin_file(spec, config_path) {
            Ok(Some(text)) => extract_launcher_from_opencode_plugin(&text),
            Ok(None) | Err(_) => None,
        },
        // The launcher path lives in the generated wrapper scripts, not in
        // hooks.json (whose commands are bare script paths).
        HookInstallKind::AntigravityConfig => spec.hook_entries.iter().find_map(|entry| {
            let text = fs::read_to_string(antigravity_script_path(entry.hook_event_name)).ok()?;
            text.lines()
                .find_map(|line| parse_launcher_from_managed_command(line, spec))
        }),
    };
    let current = current_launcher.map(|path| path.display().to_string());
    // A recorded launcher that still resolves to a working executable keeps the
    // hooks functional even if the process now runs from a different path. This
    // is routine with AppImage desktop-integration, which copies the AppImage to
    // a version-suffixed filename: hooks installed from `forktty.appimage` keep
    // working when the desktop entry launches `forktty.appimage_<ver>.appimage`.
    // Only flag staleness when the recorded launcher is actually broken, so the
    // reminder is a real signal instead of firing on every launch.
    let installed_usable = installed
        .as_deref()
        .is_some_and(|path| forktty_core::command_safety::is_executable_file(Path::new(path)));
    let status = match (&installed, &current) {
        (Some(installed_path), Some(current_path)) if installed_path == current_path => "ok",
        (Some(_), _) if installed_usable => "ok",
        (Some(_), Some(_)) => "stale",
        (Some(_), None) => "current_launcher_unknown",
        (None, _) => "not_installed",
    };
    json!({
        "status": status,
        "installedLauncher": installed,
        "currentLauncher": current,
    })
}

pub(super) fn describe_claude_installed_profile(config_path: &Path) -> &'static str {
    let Some(spec) = agent_spec("claude") else {
        return "not_installed";
    };
    let Ok(config) = read_json_file(config_path) else {
        return "not_installed";
    };
    let has_high_frequency = CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES
        .iter()
        .any(|entry| config_has_forktty_hook(&config, spec, entry));
    if has_high_frequency {
        return "full";
    }
    let has_lifecycle = CLAUDE_HOOK_ENTRIES
        .iter()
        .filter(|entry| !is_claude_high_frequency_event(entry.event_name))
        .any(|entry| config_has_forktty_hook(&config, spec, entry));
    if has_lifecycle {
        "lifecycle"
    } else {
        "not_installed"
    }
}

pub(super) fn config_has_forktty_hook(
    config: &Value,
    spec: &AgentSpec,
    entry_spec: &HookEntrySpec,
) -> bool {
    let Some(entries) = config
        .get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(entry_spec.event_name))
        .and_then(Value::as_array)
    else {
        return false;
    };
    let suffix = format!(" hooks {} {}", spec.key, entry_spec.hook_event_name);
    entries.iter().any(|entry| {
        is_forktty_managed_entry(entry)
            || entry
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| {
                    hooks.iter().any(|hook| {
                        hook.get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|command| command.contains(&suffix))
                    })
                })
    })
}

/// Codex records per-hook trust approvals under `[hooks.state]` in its
/// config.toml, keyed `"<hooks.json path>:<snake_case_event>:<group>:<hook>"`.
/// Newly installed hooks have no record until Codex prompts for approval, so
/// an installed-but-unapproved hook silently does nothing. This is reported
/// as information, not as an error: the approval semantics belong to Codex.
pub(super) fn describe_codex_hook_trust(config_path: &Path) -> Value {
    let Some(config_toml) = config_path.parent().map(|dir| dir.join("config.toml")) else {
        return json!({ "status": "unavailable" });
    };
    let Ok(text) = fs::read_to_string(&config_toml) else {
        return json!({ "status": "config_missing", "configPath": config_toml });
    };
    let Ok(parsed) = text.parse::<toml::Table>() else {
        return json!({ "status": "unreadable", "configPath": config_toml });
    };
    let state = parsed
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml::Value::as_table);
    codex_hook_trust_report(&config_toml, config_path, CODEX_HOOK_ENTRIES, state)
}

pub(super) fn codex_hook_trust_report(
    config_toml: &Path,
    hooks_json: &Path,
    entries: &[HookEntrySpec],
    state: Option<&toml::value::Table>,
) -> Value {
    let hooks_json = hooks_json.display().to_string();
    let mut recorded = Vec::new();
    let mut unrecorded = Vec::new();
    for entry in entries {
        let prefix = format!(
            "{hooks_json}:{}:",
            camel_to_snake_event_name(entry.event_name)
        );
        let has_record =
            state.is_some_and(|table| table.keys().any(|key| key.starts_with(&prefix)));
        if has_record {
            recorded.push(entry.event_name);
        } else {
            unrecorded.push(entry.event_name);
        }
    }
    let status = if unrecorded.is_empty() {
        "all_recorded"
    } else if recorded.is_empty() {
        "none_recorded"
    } else {
        "partial"
    };
    json!({
        "status": status,
        "configPath": config_toml,
        "recordedEvents": recorded,
        "unrecordedEvents": unrecorded,
        "currentHashesVerified": false,
        "hint": "Codex ties approval to each hook's current hash. ForkTTY can detect trust records but cannot verify those hashes; after hook definitions change, run /hooks inside Codex to review.",
    })
}

pub(super) fn camel_to_snake_event_name(event: &str) -> String {
    let mut out = String::with_capacity(event.len() + 4);
    for (idx, ch) in event.char_indices() {
        if ch.is_ascii_uppercase() {
            if idx > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(feature = "gtk-ghostty")]
pub(crate) fn hook_setup_reminder_message() -> Option<String> {
    let current_launcher = stable_hook_launcher_path();
    let statuses = default_hook_setup_agents()
        .iter()
        .map(|spec| {
            let config_path = (spec.config_path)();
            describe_launcher_check(spec, &config_path, current_launcher.as_deref())
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("not_installed")
                .to_string()
        })
        .collect::<Vec<_>>();
    hook_setup_reminder_message_for_statuses(statuses.iter().map(String::as_str))
}

#[cfg(any(test, feature = "gtk-ghostty"))]
pub(super) fn hook_setup_reminder_message_for_statuses<'a>(
    statuses: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let statuses = statuses.into_iter().collect::<Vec<_>>();
    let installed = statuses
        .iter()
        .any(|status| matches!(*status, "ok" | "stale" | "current_launcher_unknown"));
    let stale = statuses
        .iter()
        .any(|status| matches!(*status, "stale" | "current_launcher_unknown"));
    if stale {
        Some(
            "Refresh ForkTTY agent hooks by running `forktty hooks setup` so Codex, Claude Code, Antigravity, and OpenCode can publish status, progress, and notifications."
                .to_string(),
        )
    } else if !installed {
        Some(
            "Install ForkTTY agent hooks by running `forktty hooks setup` to connect Codex, Claude Code, Antigravity, and OpenCode to status, progress, and notifications."
                .to_string(),
        )
    } else {
        None
    }
}

pub(super) fn extract_launcher_from_opencode_plugin(text: &str) -> Option<String> {
    let marker = "const FORKTTY_LAUNCHER = ";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find(';')?;
    serde_json::from_str(rest[..end].trim()).ok()
}

pub(super) fn format_launcher_check(check: &Value) -> Option<String> {
    let status = check.get("status").and_then(Value::as_str)?;
    match status {
        "stale" => {
            let installed = check
                .get("installedLauncher")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)");
            let current = check
                .get("currentLauncher")
                .and_then(Value::as_str)
                .unwrap_or("(unknown)");
            Some(format!(
                "launcher mismatch: hook config points at {installed} but the current forktty launcher is {current}. Re-run `forktty hooks setup` to refresh the hook commands."
            ))
        }
        "current_launcher_unknown" => Some(
            "launcher mismatch: could not resolve the current forktty executable; hook commands may be stale."
                .to_string(),
        ),
        _ => None,
    }
}

pub(super) fn extract_managed_launcher_from_config(
    spec: &AgentSpec,
    config: &Value,
) -> Option<String> {
    let hooks = config.get("hooks")?.as_object()?;
    for events in hooks.values() {
        let Some(entries) = events.as_array() else {
            continue;
        };
        for entry in entries {
            let Some(commands) = entry.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in commands {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if !is_forktty_managed_entry(entry)
                    && !is_legacy_forktty_hook_command(command, spec)
                {
                    continue;
                }
                if let Some(launcher) = parse_launcher_from_managed_command(command, spec) {
                    return Some(launcher);
                }
            }
        }
    }
    None
}

fn is_legacy_forktty_hook_command(command: &str, spec: &AgentSpec) -> bool {
    let suffix = format!(" hooks {} ", spec.key);
    command.contains(&suffix)
}

pub(super) fn parse_launcher_from_managed_command(
    command: &str,
    spec: &AgentSpec,
) -> Option<String> {
    let marker = "&& ";
    let start = command.find(marker)? + marker.len();
    let rest = &command[start..];
    let appimage_prefix = format!("{APPIMAGE_EXTRACT_AND_RUN_ENV}=1 ");
    let rest = rest.strip_prefix(&appimage_prefix).unwrap_or(rest);
    let rest = rest.strip_prefix('\'')?;
    let mut launcher = String::new();
    let mut chars = rest.char_indices();
    while let Some((idx, ch)) = chars.next() {
        if ch != '\'' {
            launcher.push(ch);
            continue;
        }
        if rest[idx..].starts_with("'\"'\"'") {
            launcher.push('\'');
            for _ in 0..4 {
                chars.next();
            }
            continue;
        }
        let suffix = format!("' hooks {} ", spec.key);
        if rest[idx..].starts_with(suffix.as_str()) {
            return Some(launcher);
        }
        return None;
    }
    None
}

pub(super) fn test_system_ping(context: &CliContext, checks: &mut Vec<Value>) {
    let ping = match send_socket_request(&context.socket_path, "system.ping", json!({})) {
        Ok(value) if value.as_str() == Some("pong") => Ok(value),
        Ok(value) => Err(CliError::new(format!(
            "system.ping returned {value}, expected \"pong\""
        ))),
        Err(err) => Err(err),
    };
    record_hook_check(checks, "system.ping", ping);
}

pub(super) fn test_metadata_set_status(
    context: &CliContext,
    spec: &AgentSpec,
    target: &Map<String, Value>,
    status_key: &str,
    order: &str,
    checks: &mut Vec<Value>,
) {
    let mut status_params = target.clone();
    status_params.insert("key".to_string(), Value::String(status_key.to_string()));
    status_params.insert(
        "label".to_string(),
        Value::String(format!("{} hook test", spec.label)),
    );
    status_params.insert("value".to_string(), Value::String("Running".to_string()));
    status_params.insert("color".to_string(), Value::String("blue".to_string()));
    status_params.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(order.to_string()),
    );
    record_hook_check(
        checks,
        "metadata.set_status",
        send_socket_request(
            &context.socket_path,
            "metadata.set_status",
            Value::Object(status_params),
        ),
    );
}

pub(super) fn test_metadata_log(
    context: &CliContext,
    spec: &AgentSpec,
    target: &Map<String, Value>,
    checks: &mut Vec<Value>,
) {
    let mut log_params = target.clone();
    log_params.insert("level".to_string(), Value::String("info".to_string()));
    log_params.insert(
        "message".to_string(),
        Value::String(format!("{} hook roundtrip test", spec.label)),
    );
    record_hook_check(
        checks,
        "metadata.log",
        send_socket_request(
            &context.socket_path,
            "metadata.log",
            Value::Object(log_params),
        ),
    );
}

pub(super) fn test_notification_create(
    context: &CliContext,
    spec: &AgentSpec,
    target: &Map<String, Value>,
    checks: &mut Vec<Value>,
) -> (Value, Option<Value>) {
    let before = send_socket_request(&context.socket_path, "notification.list", json!({}))
        .unwrap_or(Value::Null);
    let mut notification_params = target.clone();
    notification_params.insert(
        "title".to_string(),
        Value::String(format!("{} hook test", spec.label)),
    );
    notification_params.insert(
        "body".to_string(),
        Value::String("Roundtrip validation".to_string()),
    );
    notification_params.insert("kind".to_string(), Value::String("prompt".to_string()));
    let created = record_hook_check(
        checks,
        "notification.create",
        send_socket_request(
            &context.socket_path,
            "notification.create",
            Value::Object(notification_params),
        ),
    );
    (before, created)
}

pub(super) fn test_metadata_clear_status(
    context: &CliContext,
    target: &Map<String, Value>,
    status_key: &str,
    order: &str,
    checks: &mut Vec<Value>,
) {
    let mut clear_status = target.clone();
    clear_status.insert("key".to_string(), Value::String(status_key.to_string()));
    clear_status.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(increment_hook_event_order(order)),
    );
    record_hook_check(
        checks,
        "metadata.clear_status",
        send_socket_request(
            &context.socket_path,
            "metadata.clear_status",
            Value::Object(clear_status),
        ),
    );
}

pub(super) fn test_notification_clear(
    context: &CliContext,
    before: Value,
    created: Option<Value>,
    checks: &mut Vec<Value>,
) {
    if before.as_array().is_some_and(Vec::is_empty)
        && created
            .as_ref()
            .is_some_and(|value| value.get("id").is_some())
    {
        let after = send_socket_request(&context.socket_path, "notification.list", json!({}))
            .unwrap_or(Value::Null);
        if after.as_array().is_some_and(|items| {
            items.len() == 1
                && items[0].get("id") == created.as_ref().and_then(|value| value.get("id"))
        }) {
            record_hook_check(
                checks,
                "notification.clear",
                send_socket_request(&context.socket_path, "notification.clear", json!({})),
            );
        }
    }
}

pub(super) fn print_hook_test_report(
    context: &CliContext,
    spec: &AgentSpec,
    workspace: Option<String>,
    surface: Option<String>,
    checks: Vec<Value>,
) -> CliResult<()> {
    let healthy = checks.iter().all(|check| check["ok"] == json!(true));
    let report = json!({
        "version": 1,
        "agent": spec.key,
        "socket": context.socket_path.display().to_string(),
        "workspace": workspace,
        "surface": surface,
        "checks": checks,
        "ok": healthy,
    });
    if context.json {
        print_json(&report)?;
        return hooks_health_exit("hooks test", spec.key, healthy);
    }
    eprintln!("ForkTTY {} hook test", spec.label);
    eprintln!("socket: {}", context.socket_path.display());
    eprintln!(
        "workspace: {}",
        workspace
            .as_deref()
            .unwrap_or("(active workspace fallback)")
    );
    eprintln!("surface: {}", surface.as_deref().unwrap_or("(none)"));
    if let Some(entries) = report["checks"].as_array() {
        for check in entries {
            let method = check["method"].as_str().unwrap_or("?");
            if check["ok"] == json!(true) {
                eprintln!("{method}: ok");
            } else {
                eprintln!("{method}: failed: {}", hook_check_error_for_terminal(check));
            }
        }
    }
    eprintln!(
        "ForkTTY {} hook test: {}",
        spec.label,
        if healthy { "ok" } else { "failed" }
    );
    hooks_health_exit("hooks test", spec.key, healthy)
}

pub(super) fn hook_check_error_for_terminal(check: &Value) -> String {
    sanitize_for_terminal(check["error"].as_str().unwrap_or("unknown error"))
}

pub(super) fn handle_hooks_test(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let (spec, rest) = single_agent_command(args, "hooks test")?;
    require_no_args(&rest, &format!("hooks test {}", spec.key))?;
    let target = hook_target_params();
    let status_key = format!("agent:{}:hook-test", spec.key);
    let order = next_hook_event_order();
    let workspace = target
        .get("workspace_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let surface = target
        .get("surface_id")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Every method is attempted even after a failure: the point of the
    // report is per-method pass/fail, and cleanup calls must run regardless.
    let mut checks: Vec<Value> = Vec::new();

    test_system_ping(context, &mut checks);
    test_metadata_set_status(context, spec, &target, &status_key, &order, &mut checks);
    test_metadata_log(context, spec, &target, &mut checks);
    let (before, created) = test_notification_create(context, spec, &target, &mut checks);
    test_metadata_clear_status(context, &target, &status_key, &order, &mut checks);
    test_notification_clear(context, before, created, &mut checks);

    print_hook_test_report(context, spec, workspace, surface, checks)
}

pub(super) fn single_agent_command(
    args: Vec<String>,
    command: &str,
) -> CliResult<(&'static AgentSpec, Vec<String>)> {
    let Some(agent) = args.first() else {
        return Err(CliError::new(format!("{command} requires an agent")));
    };
    let normalized = normalize_agent_name(agent);
    let spec = agent_spec(&normalized)
        .ok_or_else(|| CliError::new(format!("Unsupported {command} agent: {agent}")))?;
    Ok((spec, args[1..].to_vec()))
}
