//! Hook installer planning, config merging, and provider-specific config paths.

use super::super::integration_files::{read_json_file, MAX_HOOK_CONFIG_SIZE_BYTES};
use super::super::{trimmed_env, CliError, CliResult};
use super::{
    AgentSpec, HookEntrySpec, HookInstallKind, HookSetupProfile, AGENTS,
    CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES, DEFAULT_HOOK_SETUP_AGENT_KEYS, FORKTTY_HOOK_TAG,
    HOOK_CONTINUE_JSON, LEGACY_GEMINI_HOOK_AGENT, OPENCODE_HOOK_TIMEOUT_MS,
    OPENCODE_MAX_INPUT_BYTES, OPENCODE_PLUGIN_TAG,
};
use serde_json::{json, Map, Value};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub(in crate::socket_cli) struct HookSetupPlan {
    pub(in crate::socket_cli) spec: &'static AgentSpec,
    pub(in crate::socket_cli) config_path: PathBuf,
    pub(in crate::socket_cli) changed: bool,
    pub(in crate::socket_cli) content: String,
    /// Generated wrapper scripts written alongside the config (Antigravity
    /// only: its hook `command` is a bare executable path with no arguments).
    pub(in crate::socket_cli) scripts: Vec<(PathBuf, String)>,
}

pub(in crate::socket_cli) enum HookRemoveAction {
    Write(String),
    DeleteFile,
    None,
}

pub(in crate::socket_cli) struct HookRemovePlan {
    pub(in crate::socket_cli) spec: &'static AgentSpec,
    pub(in crate::socket_cli) config_path: PathBuf,
    pub(in crate::socket_cli) changed: bool,
    pub(in crate::socket_cli) action: HookRemoveAction,
    /// ForkTTY-owned generated scripts directory deleted on removal.
    pub(in crate::socket_cli) scripts_dir: Option<PathBuf>,
}

pub(in crate::socket_cli) fn build_hook_setup_plan(
    spec: &'static AgentSpec,
    launcher: &Path,
) -> CliResult<HookSetupPlan> {
    build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Lifecycle)
}

pub(in crate::socket_cli) fn build_hook_setup_plan_with_profile(
    spec: &'static AgentSpec,
    launcher: &Path,
    profile: HookSetupProfile,
) -> CliResult<HookSetupPlan> {
    let config_path = (spec.config_path)();
    match spec.install_kind {
        HookInstallKind::JsonConfig => {
            let existing = read_agent_config(spec, &config_path)?;
            let (changed, config) = if profile == HookSetupProfile::Full {
                merge_hook_config(&existing, spec, launcher)?
            } else {
                merge_hook_config_with_profile(&existing, spec, launcher, profile)?
            };
            Ok(HookSetupPlan {
                spec,
                config_path,
                changed,
                content: format!("{}\n", serde_json::to_string_pretty(&config)?),
                scripts: Vec::new(),
            })
        }
        HookInstallKind::OpenCodePlugin => {
            let existing = read_opencode_plugin_file(spec, &config_path)?;
            let content = build_opencode_plugin(launcher)?;
            let changed = existing.as_deref() != Some(content.as_str());
            Ok(HookSetupPlan {
                spec,
                config_path,
                changed,
                content,
                scripts: Vec::new(),
            })
        }
        HookInstallKind::AntigravityConfig => {
            let existing = read_agent_config(spec, &config_path)?;
            let (config_changed, config) = merge_antigravity_hook_config(&existing, spec)?;
            let scripts = spec
                .hook_entries
                .iter()
                .map(|entry| {
                    (
                        antigravity_script_path(entry.hook_event_name),
                        build_antigravity_hook_script(launcher, spec, entry.hook_event_name),
                    )
                })
                .collect::<Vec<_>>();
            let scripts_changed = scripts.iter().any(|(path, content)| {
                fs::read_to_string(path).ok().as_deref() != Some(content.as_str())
            });
            Ok(HookSetupPlan {
                spec,
                config_path,
                changed: config_changed || scripts_changed,
                content: format!("{}\n", serde_json::to_string_pretty(&config)?),
                scripts,
            })
        }
    }
}

pub(in crate::socket_cli) fn build_hook_remove_plan(
    spec: &'static AgentSpec,
    current_launcher: Option<&Path>,
) -> CliResult<HookRemovePlan> {
    let config_path = (spec.config_path)();
    match spec.install_kind {
        HookInstallKind::JsonConfig => {
            let existing = read_agent_config(spec, &config_path)?;
            let (changed, config) = remove_hook_config(&existing, spec, current_launcher)?;
            let action = if changed {
                HookRemoveAction::Write(format!("{}\n", serde_json::to_string_pretty(&config)?))
            } else {
                HookRemoveAction::None
            };
            Ok(HookRemovePlan {
                spec,
                config_path,
                changed,
                action,
                scripts_dir: None,
            })
        }
        HookInstallKind::OpenCodePlugin => {
            let existing = read_opencode_plugin_file(spec, &config_path)?;
            let changed = existing
                .as_deref()
                .is_some_and(|text| text.contains(OPENCODE_PLUGIN_TAG));
            Ok(HookRemovePlan {
                spec,
                config_path,
                changed,
                action: if changed {
                    HookRemoveAction::DeleteFile
                } else {
                    HookRemoveAction::None
                },
                scripts_dir: None,
            })
        }
        HookInstallKind::AntigravityConfig => {
            let existing = read_agent_config(spec, &config_path)?;
            let mut config = existing.as_object().cloned().unwrap_or_default();
            let group_removed = config.remove(ANTIGRAVITY_HOOK_GROUP).is_some();
            let scripts_dir = antigravity_scripts_dir();
            let scripts_present = scripts_dir.is_dir();
            let action = if !group_removed {
                HookRemoveAction::None
            } else if config.is_empty() {
                HookRemoveAction::DeleteFile
            } else {
                HookRemoveAction::Write(format!(
                    "{}\n",
                    serde_json::to_string_pretty(&Value::Object(config))?
                ))
            };
            Ok(HookRemovePlan {
                spec,
                config_path,
                changed: group_removed || scripts_present,
                action,
                scripts_dir: scripts_present.then_some(scripts_dir),
            })
        }
    }
}

pub(in crate::socket_cli) fn supported_agents(
    names: &[String],
) -> CliResult<Vec<&'static AgentSpec>> {
    if names.is_empty() {
        return Ok(AGENTS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_agent_name(name);
        let spec = agent_spec(&normalized)
            .ok_or_else(|| CliError::new(format!("Unsupported agent: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&AgentSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

pub(in crate::socket_cli) fn supported_hook_remove_agents(
    names: &[String],
) -> CliResult<Vec<&'static AgentSpec>> {
    if names.is_empty() {
        return Ok(AGENTS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_agent_name(name);
        let spec = agent_spec(&normalized)
            .or_else(|| (normalized == "gemini").then_some(&LEGACY_GEMINI_HOOK_AGENT))
            .ok_or_else(|| CliError::new(format!("Unsupported agent: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&AgentSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

pub(in crate::socket_cli) fn default_hook_setup_agents() -> Vec<&'static AgentSpec> {
    DEFAULT_HOOK_SETUP_AGENT_KEYS
        .iter()
        .map(|key| agent_spec(key).expect("default hook setup agent exists"))
        .collect()
}

pub(in crate::socket_cli) fn agent_spec(agent: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|spec| spec.key == agent)
}

pub(in crate::socket_cli) fn normalize_agent_name(agent: &str) -> String {
    match agent.to_lowercase().as_str() {
        "claude-code" | "claude_code" => "claude".to_string(),
        "open-code" | "open_code" => "opencode".to_string(),
        "agy" => "antigravity".to_string(),
        other => other.to_string(),
    }
}

pub(in crate::socket_cli) fn home_dir() -> PathBuf {
    trimmed_env("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(in crate::socket_cli) fn codex_home_dir() -> PathBuf {
    trimmed_env("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

pub(in crate::socket_cli) fn codex_config_path() -> PathBuf {
    codex_home_dir().join("hooks.json")
}

pub(in crate::socket_cli) fn claude_config_path() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("settings.json")
}

pub(in crate::socket_cli) fn legacy_gemini_config_path() -> PathBuf {
    home_dir().join(".gemini/settings.json")
}

// Antigravity CLI loads user-level hooks from ~/.gemini/config/hooks.json
// (verified against agy 1.0.3; the workspace-level .agents/hooks.json is
// intentionally not managed so hooks work from any project).
pub(in crate::socket_cli) fn antigravity_root_dir() -> PathBuf {
    home_dir().join(".gemini")
}

pub(in crate::socket_cli) fn antigravity_config_dir() -> PathBuf {
    antigravity_root_dir().join("config")
}

pub(in crate::socket_cli) fn antigravity_config_path() -> PathBuf {
    antigravity_config_dir().join("hooks.json")
}

pub(in crate::socket_cli) fn antigravity_scripts_dir() -> PathBuf {
    antigravity_config_dir().join("forktty-hooks.generated")
}

pub(in crate::socket_cli) fn antigravity_script_path(hook_event_name: &str) -> PathBuf {
    antigravity_scripts_dir().join(format!("{hook_event_name}.sh"))
}

pub(in crate::socket_cli) fn opencode_plugin_path() -> PathBuf {
    trimmed_env("OPENCODE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config/opencode"))
        .join("plugins/forktty.generated.js")
}

pub(in crate::socket_cli) fn build_hook_shell_command(
    launcher: &Path,
    spec: &AgentSpec,
    event: &str,
) -> String {
    format!(
        "[ \"${{{}:-}}\" != \"1\" ] && {} hooks {} {} || echo '{}'",
        spec.disabled_env,
        shell_quote(&launcher.display().to_string()),
        spec.key,
        event,
        HOOK_CONTINUE_JSON.trim_end()
    )
}

pub(in crate::socket_cli) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(in crate::socket_cli) fn build_hook_entry(
    spec: &AgentSpec,
    command: String,
    timeout: u64,
) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "hooks".to_string(),
        json!([{
            "type": "command",
            "command": command,
            "statusMessage": format!("ForkTTY {} hooks", spec.label),
            "timeout": timeout,
        }]),
    );
    entry.insert(
        "forkttySource".to_string(),
        Value::String(FORKTTY_HOOK_TAG.to_string()),
    );
    if let Some(matcher) = spec.matcher {
        entry.insert("matcher".to_string(), Value::String(matcher.to_string()));
    }
    Value::Object(entry)
}

pub(in crate::socket_cli) fn merge_hook_config(
    existing: &Value,
    spec: &AgentSpec,
    launcher: &Path,
) -> CliResult<(bool, Value)> {
    merge_hook_config_with_profile(existing, spec, launcher, HookSetupProfile::Full)
}

pub(in crate::socket_cli) fn merge_hook_config_with_profile(
    existing: &Value,
    spec: &AgentSpec,
    launcher: &Path,
    profile: HookSetupProfile,
) -> CliResult<(bool, Value)> {
    let mut config = existing.as_object().cloned().unwrap_or_default();
    let hooks_was_object = config.get("hooks").is_some_and(Value::is_object);
    if !hooks_was_object && config.contains_key("hooks") {
        eprintln!(
            "Warning: existing \"hooks\" value is not a map and will be replaced \
             with a forktty-managed hooks map; the previous file is kept in the backup."
        );
    }
    let mut hooks = config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut changed = !hooks_was_object;

    for entry_spec in spec
        .hook_entries
        .iter()
        .filter(|entry| hook_entry_enabled_for_setup(spec, profile, entry))
    {
        let command = build_hook_shell_command(launcher, spec, entry_spec.hook_event_name);
        let next_entry = build_hook_entry(spec, command.clone(), entry_spec.timeout);
        let existing_entries = hooks
            .get(entry_spec.event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut filtered = existing_entries
            .iter()
            .filter(|entry| {
                !is_forktty_managed_entry(entry)
                    && !is_legacy_forktty_hook_command(
                        entry,
                        spec,
                        entry_spec.hook_event_name,
                        &command,
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        filtered.push(next_entry);
        if filtered != existing_entries {
            changed = true;
        }
        hooks.insert(entry_spec.event_name.to_string(), Value::Array(filtered));
    }
    for entry_spec in spec
        .hook_entries
        .iter()
        .filter(|entry| hook_entry_removed_by_setup(spec, profile, entry))
    {
        let Some(existing_entries) = hooks
            .get(entry_spec.event_name)
            .and_then(Value::as_array)
            .cloned()
        else {
            continue;
        };
        let command = build_hook_shell_command(launcher, spec, entry_spec.hook_event_name);
        let filtered = existing_entries
            .iter()
            .filter(|entry| {
                !is_forktty_managed_entry(entry)
                    && !is_legacy_forktty_hook_command(
                        entry,
                        spec,
                        entry_spec.hook_event_name,
                        &command,
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        if filtered == existing_entries {
            continue;
        }
        changed = true;
        if filtered.is_empty() {
            hooks.remove(entry_spec.event_name);
        } else {
            hooks.insert(entry_spec.event_name.to_string(), Value::Array(filtered));
        }
    }

    config.insert("hooks".to_string(), Value::Object(hooks));
    Ok((changed, Value::Object(config)))
}

pub(in crate::socket_cli) fn hook_entry_enabled_for_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    !(spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name))
}

pub(in crate::socket_cli) fn hook_entry_removed_by_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name)
}

pub(in crate::socket_cli) fn is_claude_high_frequency_event(event_name: &str) -> bool {
    CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES
        .iter()
        .any(|entry| entry.event_name == event_name)
}

pub(in crate::socket_cli) fn hook_setup_profile_name(profile: HookSetupProfile) -> &'static str {
    match profile {
        HookSetupProfile::Lifecycle => "lifecycle",
        HookSetupProfile::Full => "full",
    }
}

pub(in crate::socket_cli) fn remove_hook_config(
    existing: &Value,
    spec: &AgentSpec,
    current_launcher: Option<&Path>,
) -> CliResult<(bool, Value)> {
    let mut config = existing.as_object().cloned().unwrap_or_default();
    let Some(hooks) = config.get("hooks").and_then(Value::as_object).cloned() else {
        return Ok((false, Value::Object(config)));
    };
    let mut next_hooks = hooks.clone();
    let mut changed = false;

    for entry_spec in spec.hook_entries {
        let Some(existing_entries) = hooks
            .get(entry_spec.event_name)
            .and_then(Value::as_array)
            .cloned()
        else {
            continue;
        };
        let next_command = current_launcher
            .map(|launcher| build_hook_shell_command(launcher, spec, entry_spec.hook_event_name))
            .unwrap_or_default();
        let filtered = existing_entries
            .iter()
            .filter(|entry| {
                !is_forktty_managed_entry(entry)
                    && !is_legacy_forktty_hook_command(
                        entry,
                        spec,
                        entry_spec.hook_event_name,
                        &next_command,
                    )
            })
            .cloned()
            .collect::<Vec<_>>();
        if filtered == existing_entries {
            continue;
        }
        changed = true;
        if filtered.is_empty() {
            next_hooks.remove(entry_spec.event_name);
        } else {
            next_hooks.insert(entry_spec.event_name.to_string(), Value::Array(filtered));
        }
    }

    if changed {
        if next_hooks.is_empty() {
            config.remove("hooks");
        } else {
            config.insert("hooks".to_string(), Value::Object(next_hooks));
        }
    }
    Ok((changed, Value::Object(config)))
}

pub(in crate::socket_cli) fn is_forktty_managed_entry(entry: &Value) -> bool {
    entry.get("forkttySource").and_then(Value::as_str) == Some(FORKTTY_HOOK_TAG)
}

/// ForkTTY owns this named hook group in Antigravity's hooks.json; other
/// top-level groups belong to the user and are never touched.
pub(in crate::socket_cli) const ANTIGRAVITY_HOOK_GROUP: &str = "forktty";
pub(in crate::socket_cli) const ANTIGRAVITY_SCRIPT_TAG: &str = "forktty-managed-antigravity-hook";

/// Antigravity executes `command` as one executable path (no argv splitting,
/// no shell), so each event points at a generated wrapper script that runs
/// the launcher with the usual guard line. PreToolUse is a gating hook, so its
/// disabled/failed fallback explicitly approves tool use; non-gating events
/// fall back to `{}` because Antigravity rejects unknown response fields like
/// `continue` under strict protojson unmarshaling.
pub(in crate::socket_cli) fn build_antigravity_hook_script(
    launcher: &Path,
    spec: &AgentSpec,
    event: &str,
) -> String {
    let fallback = if event == "pre-tool" {
        r#"{"decision":"approve"}"#
    } else {
        "{}"
    };
    format!(
        "#!/bin/sh\n# {tag}\n# Generated by `forktty hooks setup`; local edits will be replaced.\n[ \"${{{disabled}:-}}\" != \"1\" ] && {launcher} hooks {agent} {event} || printf '%s\\n' {fallback}\n",
        tag = ANTIGRAVITY_SCRIPT_TAG,
        disabled = spec.disabled_env,
        launcher = shell_quote(&launcher.display().to_string()),
        agent = spec.key,
        fallback = shell_quote(fallback),
    )
}

pub(in crate::socket_cli) fn merge_antigravity_hook_config(
    existing: &Value,
    spec: &AgentSpec,
) -> CliResult<(bool, Value)> {
    let mut config = existing.as_object().cloned().unwrap_or_default();
    let mut group = Map::new();
    group.insert("enabled".to_string(), Value::Bool(true));
    for entry_spec in spec.hook_entries {
        let mut entry = Map::new();
        let command = antigravity_script_path(entry_spec.hook_event_name);
        if is_antigravity_flat_hook_event(entry_spec.event_name) {
            entry.insert("type".to_string(), Value::String("command".to_string()));
            entry.insert("command".to_string(), json!(command));
        } else {
            if let Some(matcher) = spec.matcher {
                entry.insert("matcher".to_string(), Value::String(matcher.to_string()));
            }
            entry.insert(
                "hooks".to_string(),
                json!([{
                    "type": "command",
                    "command": command,
                }]),
            );
        }
        group.insert(
            entry_spec.event_name.to_string(),
            json!([Value::Object(entry)]),
        );
    }
    let next_group = Value::Object(group);
    let changed = config.get(ANTIGRAVITY_HOOK_GROUP) != Some(&next_group);
    config.insert(ANTIGRAVITY_HOOK_GROUP.to_string(), next_group);
    Ok((changed, Value::Object(config)))
}

pub(in crate::socket_cli) fn is_antigravity_flat_hook_event(event_name: &str) -> bool {
    matches!(event_name, "PreInvocation" | "PostInvocation" | "Stop")
}

pub(in crate::socket_cli) fn is_legacy_forktty_hook_command(
    entry: &Value,
    spec: &AgentSpec,
    event: &str,
    next_command: &str,
) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    hooks.iter().any(|hook| {
        if hook.get("type").and_then(Value::as_str) != Some("command") {
            return false;
        }
        let Some(command) = hook.get("command").and_then(Value::as_str) else {
            return false;
        };
        let suffix = format!(" hooks {} {}", spec.key, event);
        (!next_command.is_empty() && command == next_command)
            || (command.contains(&suffix)
                && (command.contains("forktty.mjs") || command.contains(spec.disabled_env)))
    })
}

pub(in crate::socket_cli) fn read_agent_config(spec: &AgentSpec, path: &Path) -> CliResult<Value> {
    let value = read_json_file(path).map_err(|err| {
        CliError::new(format!(
            "failed to read {} hook config at {}: {}",
            spec.key,
            path.display(),
            err.message
        ))
    })?;
    if !value.is_object() {
        return Err(CliError::new(format!(
            "failed to read {} hook config at {}: expected a JSON object at the top level",
            spec.key,
            path.display()
        )));
    }
    Ok(value)
}

pub(in crate::socket_cli) fn read_opencode_plugin_file(
    spec: &AgentSpec,
    path: &Path,
) -> CliResult<Option<String>> {
    let link_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let followed = if link_meta.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "warning: {} is a broken symlink; replacing with a fresh file",
                    path.display()
                );
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        link_meta
    };
    // See read_json_file: never open(2) a non-regular file (FIFOs block).
    if !followed.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    let file = File::open(path)?;
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    if stat.len() > MAX_HOOK_CONFIG_SIZE_BYTES {
        return Err(CliError::new(format!(
            "hook plugin is too large ({} bytes; max {} bytes)",
            stat.len(),
            MAX_HOOK_CONFIG_SIZE_BYTES
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_HOOK_CONFIG_SIZE_BYTES + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > MAX_HOOK_CONFIG_SIZE_BYTES {
        return Err(CliError::new(format!(
            "hook plugin is too large ({} bytes; max {} bytes)",
            text.len(),
            MAX_HOOK_CONFIG_SIZE_BYTES
        )));
    }
    if !text.trim().is_empty() && !text.contains(OPENCODE_PLUGIN_TAG) {
        return Err(CliError::new(format!(
            "failed to read {} hook plugin at {}: refusing to overwrite unmanaged plugin file",
            spec.key,
            path.display()
        )));
    }
    Ok(Some(text))
}

pub(in crate::socket_cli) fn build_opencode_plugin(launcher: &Path) -> CliResult<String> {
    let launcher = serde_json::to_string(&launcher.display().to_string())?;
    Ok(format!(
        r#"// {tag}
// Generated by `forktty hooks setup`; local edits will be replaced.
import {{ spawnSync }} from "node:child_process";

const FORKTTY_LAUNCHER = {launcher};
const DISABLED_ENV = "FORKTTY_OPENCODE_HOOKS_DISABLED";
const HOOK_TIMEOUT_MS = {timeout};
const MAX_INPUT_BYTES = {max_input_bytes};
const MAX_SANITIZE_DEPTH = 32;
const MAX_SANITIZE_ITEMS = 128;
const MAX_SANITIZE_NODES = 4096;
const textEncoder = new TextEncoder();

function utf8Len(value) {{
  return textEncoder.encode(value).length;
}}

function makeBudget() {{
  return {{ remaining: Math.floor(MAX_INPUT_BYTES / 2), nodes: MAX_SANITIZE_NODES, truncated: false }};
}}

function takeNode(budget, overhead = 1) {{
  if (budget.remaining <= 0 || budget.nodes <= 0) {{
    budget.truncated = true;
    return false;
  }}
  budget.nodes -= 1;
  budget.remaining -= Math.max(1, overhead);
  if (budget.remaining < 0) {{
    budget.truncated = true;
    return false;
  }}
  return true;
}}

function takeBytes(budget, bytes) {{
  budget.remaining -= Math.max(1, bytes);
  if (budget.remaining < 0) {{
    budget.truncated = true;
    return false;
  }}
  return true;
}}

function truncateString(value, budget) {{
  const bytes = utf8Len(value);
  if (bytes <= budget.remaining) {{
    budget.remaining -= bytes;
    return value;
  }}
  let out = "";
  for (const ch of value) {{
    const chBytes = utf8Len(ch);
    if (budget.remaining < chBytes) break;
    out += ch;
    budget.remaining -= chBytes;
  }}
  budget.truncated = true;
  return `${{out}}[forktty:truncated]`;
}}

function sanitizeJson(value, budget, depth = 0) {{
  if (!takeNode(budget)) return "[forktty:truncated]";
  if (value === null || value === undefined) return value ?? null;
  const kind = typeof value;
  if (kind === "string") {{
    return truncateString(value, budget);
  }}
  if (kind === "number" || kind === "boolean") {{
    takeBytes(budget, utf8Len(String(value)));
    return value;
  }}
  if (kind === "bigint") {{
    return truncateString(String(value), budget);
  }}
  if (kind !== "object") return `[forktty:${{kind}}]`;
  if (depth >= MAX_SANITIZE_DEPTH) {{
    budget.truncated = true;
    return "[forktty:max-depth]";
  }}
  if (Array.isArray(value)) {{
    const out = [];
    for (let i = 0; i < value.length && i < MAX_SANITIZE_ITEMS; i++) {{
      out.push(sanitizeJson(value[i], budget, depth + 1));
      if (budget.remaining <= 0) break;
    }}
    if (value.length > out.length) {{
      budget.truncated = true;
      out.push(`[forktty:truncated ${{value.length - out.length}} array items]`);
    }}
    return out;
  }}
  const out = {{}};
  let count = 0;
  for (const key in value) {{
    if (!Object.prototype.propertyIsEnumerable.call(value, key)) continue;
    if (count >= MAX_SANITIZE_ITEMS || budget.remaining <= 0 || budget.nodes <= 0) {{
      budget.truncated = true;
      out.forktty_truncated = "object fields";
      break;
    }}
    if (!takeBytes(budget, utf8Len(key) + 3)) {{
      out.forktty_truncated = "object fields";
      break;
    }}
    out[key] = sanitizeJson(value[key], budget, depth + 1);
    count += 1;
  }}
  return out;
}}

function cloneJson(value) {{
  const budget = makeBudget();
  const cloned = sanitizeJson(value ?? {{}}, budget);
  if (budget.truncated && cloned && typeof cloned === "object" && !Array.isArray(cloned)) {{
    cloned.forktty_note = "opencode payload truncated before forwarding";
  }}
  return cloned;
}}

function hookInput(body) {{
  const budget = makeBudget();
  const sanitized = sanitizeJson(body ?? {{}}, budget);
  if (budget.truncated && sanitized && typeof sanitized === "object" && !Array.isArray(sanitized)) {{
    sanitized.forktty_note = "opencode payload truncated before forwarding";
  }}
  try {{
    const input = JSON.stringify(sanitized);
    if (utf8Len(input) <= MAX_INPUT_BYTES) return input;
  }} catch (error) {{
    return JSON.stringify({{ provider: "opencode", forktty_note: `unserializable opencode payload: ${{error.message}}` }});
  }}
  return JSON.stringify({{ provider: "opencode", forktty_note: "opencode payload exceeded ForkTTY input limit after sanitization" }});
}}

function findString(value, names, depth = 0) {{
  // Tool payloads can nest arbitrarily deep; bail out before the recursion
  // can overflow the call stack and crash the host opencode session.
  if (depth > 32 || !value || typeof value !== "object") return undefined;
  for (const name of names) {{
    const candidate = value[name];
    if (typeof candidate === "string" && candidate.trim() !== "") return candidate.trim();
    if (typeof candidate === "number" || typeof candidate === "boolean") return String(candidate);
  }}
  for (const child of Object.values(value)) {{
    const candidate = findString(child, names, depth + 1);
    if (candidate) return candidate;
  }}
  return undefined;
}}

function payload(source, extra = {{}}) {{
  const raw = cloneJson(source);
  const session_id =
    findString(raw, ["session_id", "sessionId", "sessionID"]) ??
    (typeof raw?.session?.id === "string" ? raw.session.id : undefined);
  return {{
    provider: "opencode",
    ...(session_id ? {{ session_id }} : {{}}),
    ...extra,
    raw,
  }};
}}

function runForkTTY(hookEvent, body) {{
  if (process.env[DISABLED_ENV] === "1") return;
  const result = spawnSync(FORKTTY_LAUNCHER, ["hooks", "opencode", hookEvent], {{
    input: hookInput(body),
    encoding: "utf8",
    stdio: ["pipe", "pipe", "pipe"],
    timeout: HOOK_TIMEOUT_MS,
  }});
  if (result.error) process.stderr.write(`ForkTTY OpenCode hook warning: ${{result.error.message}}\n`);
  if (result.stderr) process.stderr.write(result.stderr);
}}

const eventMap = {{
  "session.created": "session-start",
  "session.status": "prompt-submit",
  "permission.asked": "permission-request",
  "permission.replied": "permission-replied",
  "session.compacted": "post-compact",
  "session.idle": "stop",
  "session.error": "stop-failure",
  "session.deleted": "session-end",
}};

export const ForkTTYPlugin = async () => ({{
  event: async (input) => {{
    const event = input?.event ?? input;
    const hookEvent = eventMap[event?.type];
    if (hookEvent) runForkTTY(hookEvent, payload(event, {{ message: event?.type }}));
  }},
  "tool.execute.before": async (input, output) => {{
    runForkTTY("pre-tool", payload(input, {{
      tool_name: input?.tool ?? output?.tool,
      tool_input: output?.args ?? input?.args,
    }}));
  }},
  "tool.execute.after": async (input, output) => {{
    runForkTTY("post-tool", payload(input, {{
      tool_name: input?.tool ?? output?.tool,
      tool_response: output,
    }}));
  }},
  "experimental.session.compacting": async (input, output) => {{
    runForkTTY("pre-compact", payload(input, {{ compact_output: output }}));
  }},
}});
"#,
        tag = OPENCODE_PLUGIN_TAG,
        launcher = launcher,
        timeout = OPENCODE_HOOK_TIMEOUT_MS,
        max_input_bytes = OPENCODE_MAX_INPUT_BYTES,
    ))
}

pub(in crate::socket_cli) fn ensure_private_antigravity_hook_dirs() -> CliResult<()> {
    for dir in [
        antigravity_root_dir(),
        antigravity_config_dir(),
        antigravity_scripts_dir(),
    ] {
        fs::create_dir_all(&dir)?;
        let link_meta = fs::symlink_metadata(&dir)?;
        if link_meta.file_type().is_symlink() {
            return Err(CliError::new(format!(
                "antigravity hooks setup: refusing symlinked hook directory {}",
                dir.display()
            )));
        }
        if !link_meta.is_dir() {
            return Err(CliError::new(format!(
                "antigravity hooks setup: {} is not a directory",
                dir.display()
            )));
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let meta = fs::symlink_metadata(&dir)?;
        let mode = meta.permissions().mode();
        if mode & 0o022 != 0 {
            return Err(CliError::new(format!(
                "antigravity hooks setup: refusing group/world-writable hook directory {}",
                dir.display()
            )));
        }
    }
    Ok(())
}
