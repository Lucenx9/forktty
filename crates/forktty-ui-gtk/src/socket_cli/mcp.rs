//! MCP stdio bridge registration CLI setup, removal, and config merge helpers.

use super::hooks::{
    antigravity_config_dir, codex_home_dir, home_dir, legacy_gemini_config_path,
    normalize_agent_name,
};
use super::integration_files::{
    atomic_write_file, backup_file, ensure_parent_dir, hook_config_write_path,
    read_json_file_with_limit, read_text_config_with_limit, stable_hook_launcher_path,
};
use super::{
    bool_option, parse_flags, print_json, reject_unknown_options, write_stdout_line, CliContext,
    CliError, CliResult,
};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

const MAX_MCP_CONFIG_SIZE_BYTES: u64 = 16 * 1024 * 1024;
pub(super) const MCP_SERVER_NAME: &str = "forktty";
pub(super) const MCP_MANAGED_ENV: &str = "FORKTTY_MCP_MANAGED";

#[derive(Clone, Copy, PartialEq, Eq)]
enum McpConfigKind {
    CodexToml,
    JsonMcpServers,
}

#[derive(Clone, Copy)]
pub(super) struct McpAgentSpec {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) config_path: fn() -> PathBuf,
    config_kind: McpConfigKind,
}

pub(super) const MCP_AGENTS: &[McpAgentSpec] = &[
    McpAgentSpec {
        key: "codex",
        label: "Codex",
        config_path: codex_mcp_config_path,
        config_kind: McpConfigKind::CodexToml,
    },
    McpAgentSpec {
        key: "claude",
        label: "Claude",
        config_path: claude_mcp_config_path,
        config_kind: McpConfigKind::JsonMcpServers,
    },
    McpAgentSpec {
        key: "antigravity",
        label: "Antigravity",
        config_path: antigravity_mcp_config_path,
        config_kind: McpConfigKind::JsonMcpServers,
    },
];

const DEFAULT_MCP_SETUP_AGENT_KEYS: &[&str] = &["codex", "claude", "antigravity"];

static LEGACY_GEMINI_MCP_AGENT: McpAgentSpec = McpAgentSpec {
    key: "gemini",
    label: "Gemini",
    config_path: legacy_gemini_mcp_config_path,
    config_kind: McpConfigKind::JsonMcpServers,
};

pub(super) struct McpSetupPlan {
    pub(super) spec: &'static McpAgentSpec,
    pub(super) config_path: PathBuf,
    pub(super) changed: bool,
    pub(super) content: String,
}

pub(super) enum McpRemoveAction {
    Write(String),
    DeleteFile,
    None,
}

pub(super) struct McpRemovePlan {
    pub(super) spec: &'static McpAgentSpec,
    pub(super) config_path: PathBuf,
    pub(super) changed: bool,
    pub(super) action: McpRemoveAction,
}

pub(super) fn handle_mcp(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("setup") => handle_mcp_setup(context, args[1..].to_vec()),
        Some("remove") | Some("uninstall") => handle_mcp_remove(context, args[1..].to_vec()),
        Some("help") | Some("--help") | Some("-h") => {
            write_stdout_line(
                "Usage: forktty mcp | forktty mcp setup [codex] [claude] [antigravity] | forktty mcp remove [codex] [claude] [antigravity] [gemini]\nDefault setup agents: codex, claude, antigravity. gemini is legacy cleanup only for remove.",
            )
        }
        Some(other) => Err(CliError::new(format!("mcp: unknown subcommand {other}"))),
        None => crate::mcp_server::run_stdio(context.socket_path.clone()),
    }
}

pub(super) fn handle_mcp_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "mcp setup")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new("mcp setup: --dry-run must be true or false"));
    };
    let agents = if parsed.positionals.is_empty() {
        default_mcp_setup_agents()
    } else {
        supported_mcp_agents(&parsed.positionals)?
    };
    let launcher = stable_hook_launcher_path()
        .ok_or_else(|| CliError::new("mcp setup: could not resolve current forktty executable"))?;
    if !launcher.is_absolute() {
        return Err(CliError::new(
            "mcp setup: forktty executable path must be absolute",
        ));
    }

    let mut plans = Vec::new();
    for spec in agents {
        plans.push(build_mcp_setup_plan(spec, &launcher)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            let write_path = hook_config_write_path(&plan.config_path)?;
            ensure_parent_dir(&write_path)?;
            backup_path = backup_file(&write_path)?;
            atomic_write_file(&write_path, plan.content.as_bytes())?;
        }
        summaries.push(json!({
            "agent": plan.spec.key,
            "label": plan.spec.label,
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
        let agent = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["agent"].as_str().unwrap_or("agent"));
        let config_path = summary["configPath"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would register"
        } else if changed {
            "registered"
        } else {
            "already registered"
        };
        write_stdout_line(&format!("{agent}: {verb} MCP server at {config_path}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
    }
    Ok(())
}

pub(super) fn handle_mcp_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "mcp remove")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new("mcp remove: --dry-run must be true or false"));
    };
    let agents = supported_mcp_remove_agents(&parsed.positionals)?;

    let mut plans = Vec::new();
    for spec in agents {
        plans.push(build_mcp_remove_plan(spec)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            match &plan.action {
                McpRemoveAction::Write(content) => {
                    let write_path = hook_config_write_path(&plan.config_path)?;
                    ensure_parent_dir(&write_path)?;
                    backup_path = backup_file(&write_path)?;
                    atomic_write_file(&write_path, content.as_bytes())?;
                }
                McpRemoveAction::DeleteFile => {
                    backup_path = backup_file(&plan.config_path)?;
                    fs::remove_file(&plan.config_path)?;
                }
                McpRemoveAction::None => {}
            }
        }
        summaries.push(json!({
            "agent": plan.spec.key,
            "label": plan.spec.label,
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
        let agent = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["agent"].as_str().unwrap_or("agent"));
        let config_path = summary["configPath"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would remove"
        } else if changed {
            "removed"
        } else {
            "not registered"
        };
        write_stdout_line(&format!("{agent}: {verb} MCP server at {config_path}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
    }
    Ok(())
}

pub(super) fn build_mcp_setup_plan(
    spec: &'static McpAgentSpec,
    launcher: &Path,
) -> CliResult<McpSetupPlan> {
    let config_path = (spec.config_path)();
    let (changed, content) = match spec.config_kind {
        McpConfigKind::CodexToml => merge_codex_mcp_config(&config_path, launcher)?,
        McpConfigKind::JsonMcpServers => merge_json_mcp_config(&config_path, launcher)?,
    };
    Ok(McpSetupPlan {
        spec,
        config_path,
        changed,
        content,
    })
}

pub(super) fn build_mcp_remove_plan(spec: &'static McpAgentSpec) -> CliResult<McpRemovePlan> {
    let config_path = (spec.config_path)();
    let action = match spec.config_kind {
        McpConfigKind::CodexToml => remove_codex_mcp_config(&config_path)?,
        McpConfigKind::JsonMcpServers => remove_json_mcp_config(&config_path)?,
    };
    let changed = !matches!(action, McpRemoveAction::None);
    Ok(McpRemovePlan {
        spec,
        config_path,
        changed,
        action,
    })
}

// Codex's config.toml is the user's main hand-edited config, so it is edited
// with toml_edit to preserve comments, ordering, and formatting; a serde
// round-trip would silently destroy them.
fn merge_codex_mcp_config(path: &Path, launcher: &Path) -> CliResult<(bool, String)> {
    let existing =
        read_text_config_with_limit(path, "codex mcp config", MAX_MCP_CONFIG_SIZE_BYTES)?;
    let mut doc = parse_toml_document(existing.as_deref().unwrap_or(""), path)?;
    if doc
        .get("mcp_servers")
        .is_some_and(|item| !item.is_table_like())
    {
        eprintln!(
            "Warning: existing [mcp_servers] value is not a table and will be replaced; the previous file is kept in the backup."
        );
        doc.remove("mcp_servers");
    }
    let servers = doc.entry("mcp_servers").or_insert_with(|| {
        let mut table = toml_edit::Table::new();
        table.set_implicit(true);
        toml_edit::Item::Table(table)
    });
    let servers = servers.as_table_like_mut().ok_or_else(|| {
        CliError::new(format!(
            "failed to update MCP config at {}: [mcp_servers] is not a table",
            path.display()
        ))
    })?;
    servers.insert(MCP_SERVER_NAME, codex_mcp_server_item(launcher));
    let content = doc.to_string();
    let changed = existing.as_deref() != Some(content.as_str());
    Ok((changed, content))
}

fn remove_codex_mcp_config(path: &Path) -> CliResult<McpRemoveAction> {
    let Some(text) =
        read_text_config_with_limit(path, "codex mcp config", MAX_MCP_CONFIG_SIZE_BYTES)?
    else {
        return Ok(McpRemoveAction::None);
    };
    let mut doc = parse_toml_document(&text, path)?;
    let Some(servers) = doc
        .get_mut("mcp_servers")
        .and_then(toml_edit::Item::as_table_like_mut)
    else {
        return Ok(McpRemoveAction::None);
    };
    let should_remove = servers
        .get(MCP_SERVER_NAME)
        .is_some_and(is_managed_codex_mcp_server);
    if !should_remove {
        return Ok(McpRemoveAction::None);
    }
    servers.remove(MCP_SERVER_NAME);
    if servers.is_empty() {
        doc.remove("mcp_servers");
    }
    let content = doc.to_string();
    if content.trim().is_empty() {
        Ok(McpRemoveAction::DeleteFile)
    } else if content == text {
        Ok(McpRemoveAction::None)
    } else {
        Ok(McpRemoveAction::Write(content))
    }
}

fn merge_json_mcp_config(path: &Path, launcher: &Path) -> CliResult<(bool, String)> {
    let existing = read_json_file_with_limit(path, MAX_MCP_CONFIG_SIZE_BYTES, "MCP config")?;
    let mut config = existing.as_object().cloned().ok_or_else(|| {
        CliError::new(format!(
            "failed to read MCP config at {}: expected a JSON object at the top level",
            path.display()
        ))
    })?;
    let servers_was_object = config.get("mcpServers").is_some_and(Value::is_object);
    let mut changed = false;
    let mut servers = match config.remove("mcpServers") {
        Some(Value::Object(servers)) => servers,
        Some(_) => {
            eprintln!(
                "Warning: existing \"mcpServers\" value is not a map and will be replaced; the previous file is kept in the backup."
            );
            changed = true;
            Map::new()
        }
        None => Map::new(),
    };
    if !servers_was_object {
        changed = true;
    }
    let next = json_mcp_server_config(launcher);
    if servers.get(MCP_SERVER_NAME) != Some(&next) {
        changed = true;
    }
    servers.insert(MCP_SERVER_NAME.to_string(), next);
    config.insert("mcpServers".to_string(), Value::Object(servers));
    Ok((
        changed,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(config))?
        ),
    ))
}

fn remove_json_mcp_config(path: &Path) -> CliResult<McpRemoveAction> {
    let existing = read_json_file_with_limit(path, MAX_MCP_CONFIG_SIZE_BYTES, "MCP config")?;
    let mut config = existing.as_object().cloned().ok_or_else(|| {
        CliError::new(format!(
            "failed to read MCP config at {}: expected a JSON object at the top level",
            path.display()
        ))
    })?;
    let Some(Value::Object(mut servers)) = config.remove("mcpServers") else {
        return Ok(McpRemoveAction::None);
    };
    let should_remove = servers
        .get(MCP_SERVER_NAME)
        .is_some_and(is_managed_json_mcp_server);
    if !should_remove {
        config.insert("mcpServers".to_string(), Value::Object(servers));
        return Ok(McpRemoveAction::None);
    }
    servers.remove(MCP_SERVER_NAME);
    if !servers.is_empty() {
        config.insert("mcpServers".to_string(), Value::Object(servers));
    }
    if config.is_empty() {
        Ok(McpRemoveAction::DeleteFile)
    } else {
        Ok(McpRemoveAction::Write(format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(config))?
        )))
    }
}

fn codex_mcp_server_item(launcher: &Path) -> toml_edit::Item {
    let mut table = toml_edit::Table::new();
    table.insert("command", toml_edit::value(launcher.display().to_string()));
    let mut args = toml_edit::Array::new();
    args.push("mcp");
    table.insert("args", toml_edit::value(args));
    let mut env_vars = toml_edit::Array::new();
    for name in [
        "FORKTTY_SOCKET_PATH",
        "FORKTTY_WORKSPACE_ID",
        "FORKTTY_SURFACE_ID",
    ] {
        env_vars.push(name);
    }
    table.insert("env_vars", toml_edit::value(env_vars));
    let mut env = toml_edit::Table::new();
    env.insert(MCP_MANAGED_ENV, toml_edit::value(MCP_SERVER_NAME));
    table.insert("env", toml_edit::Item::Table(env));
    toml_edit::Item::Table(table)
}

pub(super) fn json_mcp_server_config(launcher: &Path) -> Value {
    json!({
        "command": launcher.display().to_string(),
        "args": ["mcp"],
        "env": {
            MCP_MANAGED_ENV: MCP_SERVER_NAME,
        },
    })
}

fn is_managed_codex_mcp_server(server: &toml_edit::Item) -> bool {
    server
        .as_table_like()
        .and_then(|table| table.get("env"))
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|env| env.get(MCP_MANAGED_ENV))
        .and_then(toml_edit::Item::as_str)
        == Some(MCP_SERVER_NAME)
}

fn is_managed_json_mcp_server(server: &Value) -> bool {
    server
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get(MCP_MANAGED_ENV))
        .and_then(Value::as_str)
        == Some(MCP_SERVER_NAME)
}

fn parse_toml_document(text: &str, path: &Path) -> CliResult<toml_edit::DocumentMut> {
    text.parse::<toml_edit::DocumentMut>().map_err(|err| {
        CliError::new(format!(
            "failed to read MCP config at {}: {}",
            path.display(),
            err
        ))
    })
}

fn supported_mcp_agents(names: &[String]) -> CliResult<Vec<&'static McpAgentSpec>> {
    if names.is_empty() {
        return Ok(MCP_AGENTS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_agent_name(name);
        let spec = mcp_agent_spec(&normalized)
            .ok_or_else(|| CliError::new(format!("Unsupported mcp agent: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&McpAgentSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

fn supported_mcp_remove_agents(names: &[String]) -> CliResult<Vec<&'static McpAgentSpec>> {
    if names.is_empty() {
        return Ok(MCP_AGENTS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_agent_name(name);
        let spec = mcp_agent_spec(&normalized)
            .or_else(|| (normalized == "gemini").then_some(&LEGACY_GEMINI_MCP_AGENT))
            .ok_or_else(|| CliError::new(format!("Unsupported mcp agent: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&McpAgentSpec| existing.key == spec.key)
        {
            out.push(spec);
        }
    }
    Ok(out)
}

fn default_mcp_setup_agents() -> Vec<&'static McpAgentSpec> {
    DEFAULT_MCP_SETUP_AGENT_KEYS
        .iter()
        .map(|key| mcp_agent_spec(key).expect("default MCP setup agent exists"))
        .collect()
}

pub(super) fn mcp_agent_spec(agent: &str) -> Option<&'static McpAgentSpec> {
    MCP_AGENTS.iter().find(|spec| spec.key == agent)
}

pub(super) fn codex_mcp_config_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

pub(super) fn claude_mcp_config_path() -> PathBuf {
    home_dir().join(".claude.json")
}

fn legacy_gemini_mcp_config_path() -> PathBuf {
    legacy_gemini_config_path()
}

pub(super) fn antigravity_mcp_config_path() -> PathBuf {
    antigravity_config_dir().join("mcp_config.json")
}
