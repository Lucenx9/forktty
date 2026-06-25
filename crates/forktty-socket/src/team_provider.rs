use crate::{provider_runtime, DispatchError};
use forktty_core::config;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::path::PathBuf;

pub(crate) const CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS: &str = "Read,Grep,Glob";

#[derive(Debug, Clone)]
pub(crate) struct TeamWorkerProviderSelection {
    pub(crate) requested_agent: String,
    pub(crate) selected_agent: String,
    pub(crate) program: String,
    pub(crate) default_program: String,
    pub(crate) configured_command: Option<String>,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) reason: String,
    pub(crate) considered: Vec<Value>,
}

pub(crate) fn select_team_worker_provider(
    requested_agent: Option<&str>,
) -> Result<TeamWorkerProviderSelection, DispatchError> {
    let app_config = config::load_config().unwrap_or_default();
    let path = std::env::var_os("PATH");
    select_team_worker_provider_with_path(requested_agent, &app_config.team, path.as_deref())
}

fn select_team_worker_provider_with_path(
    requested_agent: Option<&str>,
    team: &forktty_core::TeamConfig,
    path: Option<&OsStr>,
) -> Result<TeamWorkerProviderSelection, DispatchError> {
    let requested = requested_agent
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(config::TEAM_AGENT_AUTO);
    let normalized = config::normalize_team_agent_choice(requested).ok_or_else(|| {
        DispatchError::InvalidParam(format!("unsupported team worker agent: {requested}"))
    })?;
    if normalized != config::TEAM_AGENT_AUTO {
        return select_explicit_team_worker_provider(&normalized, requested, team, path);
    }
    select_auto_team_worker_provider(team, path)
}

fn select_explicit_team_worker_provider(
    agent: &str,
    requested: &str,
    team: &forktty_core::TeamConfig,
    path: Option<&OsStr>,
) -> Result<TeamWorkerProviderSelection, DispatchError> {
    if team
        .disabled_agents
        .iter()
        .any(|disabled| disabled == agent)
    {
        return Err(DispatchError::PreconditionFailed(format!(
            "Team worker provider {agent} is disabled in settings"
        )));
    }
    let Some(capability) = provider_runtime::capability(agent) else {
        return Err(DispatchError::InvalidParam(format!(
            "unsupported team worker agent: {requested}"
        )));
    };
    let resolution = provider_runtime::program_resolution(capability, team, path);
    if resolution.executable.is_none() {
        let reason = if resolution.configured {
            "configured command is not executable or not found"
        } else {
            "is not available on PATH"
        };
        return Err(DispatchError::PreconditionFailed(format!(
            "Team worker provider {agent} {reason}"
        )));
    }
    Ok(TeamWorkerProviderSelection {
        requested_agent: requested.to_string(),
        selected_agent: agent.to_string(),
        program: resolution.program.clone(),
        default_program: capability.program.to_string(),
        configured_command: resolution.configured.then(|| resolution.program.clone()),
        executable: resolution.executable,
        reason: "explicit_agent".to_string(),
        considered: vec![provider_runtime::considered_row(
            capability, team, true, "selected", path,
        )],
    })
}

fn select_auto_team_worker_provider(
    team: &forktty_core::TeamConfig,
    path: Option<&OsStr>,
) -> Result<TeamWorkerProviderSelection, DispatchError> {
    let candidates = team_worker_auto_provider_candidates(team);
    let mut considered = Vec::new();
    for (index, agent) in candidates.iter().enumerate() {
        let Some(capability) = provider_runtime::capability(agent) else {
            continue;
        };
        let disabled = team
            .disabled_agents
            .iter()
            .any(|disabled| disabled == agent);
        let resolution = provider_runtime::program_resolution(capability, team, path);
        let available = resolution.executable.is_some() && !disabled;
        let reason = if disabled {
            "disabled_by_config"
        } else if let Some(reason) = resolution.unavailable_reason.as_ref() {
            *reason
        } else {
            "selected"
        };
        considered.push(provider_runtime::considered_row(
            capability, team, available, reason, path,
        ));
        if available {
            let selection_reason = if index == 0 && team.default_agent != config::TEAM_AGENT_AUTO {
                "configured_default"
            } else {
                "provider_order"
            };
            return Ok(TeamWorkerProviderSelection {
                requested_agent: config::TEAM_AGENT_AUTO.to_string(),
                selected_agent: agent.clone(),
                program: resolution.program.clone(),
                default_program: capability.program.to_string(),
                configured_command: resolution.configured.then(|| resolution.program.clone()),
                executable: resolution.executable,
                reason: selection_reason.to_string(),
                considered,
            });
        }
    }
    Err(DispatchError::PreconditionFailed(
        "No team worker provider is available from the configured provider order".to_string(),
    ))
}

fn team_worker_auto_provider_candidates(team: &forktty_core::TeamConfig) -> Vec<String> {
    let mut candidates = Vec::new();
    if team.default_agent != config::TEAM_AGENT_AUTO {
        candidates.push(team.default_agent.clone());
        if !team.auto_fallback {
            return candidates;
        }
    }
    for agent in &team.provider_order {
        if !candidates.iter().any(|candidate| candidate == agent) {
            candidates.push(agent.clone());
        }
    }
    candidates
}

pub(crate) fn team_worker_provider_selection_value(
    selection: &TeamWorkerProviderSelection,
) -> Value {
    json!({
        "requested_agent": selection.requested_agent,
        "selected_agent": selection.selected_agent,
        "program": selection.program,
        "default_program": selection.default_program,
        "configured_command": selection.configured_command,
        "executable": selection.executable
            .as_ref()
            .map(|path| Value::String(path.to_string_lossy().into_owned()))
            .unwrap_or(Value::Null),
        "reason": selection.reason,
        "considered": selection.considered,
    })
}

#[cfg(test)]
pub(crate) fn team_worker_launch_command(
    agent: &str,
    role: Option<&str>,
    extra_args: Vec<String>,
) -> Result<(String, Vec<String>), DispatchError> {
    let Some(capability) = provider_runtime::capability(agent) else {
        return Err(DispatchError::InvalidParam(format!(
            "unsupported team worker agent: {agent}"
        )));
    };
    team_worker_launch_command_with_program(agent, capability.program, role, extra_args)
}

pub(crate) fn team_worker_launch_command_with_program(
    agent: &str,
    program: &str,
    role: Option<&str>,
    extra_args: Vec<String>,
) -> Result<(String, Vec<String>), DispatchError> {
    let Some(capability) = provider_runtime::capability(agent) else {
        return Err(DispatchError::InvalidParam(format!(
            "unsupported team worker agent: {agent}"
        )));
    };
    if extra_args.len() > 64 {
        return Err(DispatchError::InvalidParam(
            "team worker launch args exceed 64 entries".to_string(),
        ));
    }
    for arg in &extra_args {
        if arg.len() > 4096 || arg.chars().any(char::is_control) {
            return Err(DispatchError::InvalidParam(
                "team worker launch args must be non-control UTF-8 strings under 4096 bytes"
                    .to_string(),
            ));
        }
    }
    let mut args =
        if capability.agent == "claude" && !claude_args_have_permission_override(&extra_args) {
            claude_team_permission_args(role)
        } else if capability.agent == "pi"
            && role.is_some_and(team_role_is_review)
            && !pi_args_have_tool_override(&extra_args)
        {
            vec!["--tools".to_string(), "read,grep,find,ls".to_string()]
        } else {
            Vec::new()
        };
    args.extend(extra_args);
    if forktty_core::command_safety::is_shell_trampoline(program, &args) {
        return Err(DispatchError::InvalidParam(
            "team worker launch rejects shell trampolines".to_string(),
        ));
    }
    Ok((program.to_string(), args))
}

fn claude_args_have_permission_override(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let option = arg.split_once('=').map_or(arg.as_str(), |(key, _)| key);
        matches!(
            option,
            "--permission-mode"
                | "--dangerously-skip-permissions"
                | "--allow-dangerously-skip-permissions"
                | "--allowedTools"
                | "--allowed-tools"
                | "--disallowedTools"
                | "--disallowed-tools"
                | "--settings"
        )
    })
}

fn pi_args_have_tool_override(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let option = arg.split_once('=').map_or(arg.as_str(), |(key, _)| key);
        matches!(
            option,
            "--tools"
                | "-t"
                | "--exclude-tools"
                | "-xt"
                | "--no-builtin-tools"
                | "-nbt"
                | "--no-tools"
                | "-nt"
        )
    })
}

fn claude_team_permission_args(role: Option<&str>) -> Vec<String> {
    if role.is_some_and(team_role_is_review) {
        vec![
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--allowedTools".to_string(),
            CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS.to_string(),
        ]
    } else {
        vec!["--permission-mode".to_string(), "auto".to_string()]
    }
}

fn team_role_is_review(role: &str) -> bool {
    role.split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token.to_ascii_lowercase().starts_with("review"))
}
