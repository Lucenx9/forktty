use forktty_terminal::spawn::resolve_child_program;
use serde_json::{json, Value};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderCapability {
    pub(crate) agent: &'static str,
    pub(crate) program: &'static str,
    aliases: &'static [&'static str],
    safe_resume: bool,
    cwd_resume_flag: bool,
    permission_bypass_resume: bool,
    supports_plan_mode: bool,
}

const PROVIDER_CAPABILITIES: &[ProviderCapability] = &[
    ProviderCapability {
        agent: "codex",
        program: "codex",
        aliases: &["codex"],
        safe_resume: true,
        cwd_resume_flag: true,
        permission_bypass_resume: true,
        supports_plan_mode: false,
    },
    ProviderCapability {
        agent: "claude",
        program: "claude",
        aliases: &["claude", "claude_code", "claude-code"],
        safe_resume: true,
        cwd_resume_flag: false,
        permission_bypass_resume: true,
        supports_plan_mode: true,
    },
    ProviderCapability {
        agent: "opencode",
        program: "opencode",
        aliases: &["opencode", "open_code", "open-code"],
        safe_resume: true,
        cwd_resume_flag: false,
        permission_bypass_resume: false,
        supports_plan_mode: false,
    },
    ProviderCapability {
        agent: "pi",
        program: "pi",
        aliases: &["pi"],
        safe_resume: true,
        cwd_resume_flag: false,
        permission_bypass_resume: false,
        supports_plan_mode: false,
    },
    ProviderCapability {
        agent: "antigravity",
        program: "agy",
        aliases: &["antigravity", "agy"],
        safe_resume: true,
        cwd_resume_flag: false,
        permission_bypass_resume: false,
        supports_plan_mode: false,
    },
    ProviderCapability {
        agent: "grok",
        program: "grok",
        aliases: &["grok", "grok_build", "grok-build"],
        safe_resume: true,
        cwd_resume_flag: true,
        permission_bypass_resume: false,
        supports_plan_mode: false,
    },
];

#[derive(Debug, Clone)]
pub(crate) struct TeamProviderProgramResolution {
    pub(crate) program: String,
    pub(crate) configured: bool,
    pub(crate) available_on_path: bool,
    pub(crate) executable: Option<PathBuf>,
    pub(crate) unavailable_reason: Option<&'static str>,
}

pub(crate) fn capabilities(path: Option<&OsStr>, team: &forktty_core::TeamConfig) -> Value {
    let disabled = team
        .disabled_agents
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let mut providers = serde_json::Map::new();
    for capability in PROVIDER_CAPABILITIES {
        let resolution = program_resolution(capability, team, path);
        let disabled_by_config = disabled.contains(&capability.agent);
        let unavailable_reason = if disabled_by_config {
            Value::String("disabled_by_config".to_string())
        } else if let Some(reason) = resolution.unavailable_reason.as_ref() {
            Value::String((*reason).to_string())
        } else {
            Value::Null
        };
        providers.insert(
            capability.agent.to_string(),
            json!({
                "program": resolution.program.clone(),
                "default_program": capability.program,
                "configured_command": resolution.configured.then(|| resolution.program.clone()),
                "team_worker_launch": true,
                "launchable": resolution.executable.is_some() && !disabled_by_config,
                "safe_resume": capability.safe_resume,
                "cwd_resume_flag": capability.cwd_resume_flag,
                "permission_bypass_resume": capability.permission_bypass_resume,
                "plan_mode": capability.supports_plan_mode,
                "aliases": capability.aliases,
                "available_on_path": resolution.available_on_path,
                "executable": resolution
                    .executable
                    .as_ref()
                    .map(|path| Value::String(path.to_string_lossy().into_owned()))
                    .unwrap_or(Value::Null),
                "disabled_by_config": disabled_by_config,
                "unavailable_reason": unavailable_reason,
            }),
        );
    }
    Value::Object(providers)
}

pub(crate) fn team_policy(team: &forktty_core::TeamConfig) -> Value {
    json!({
        "default_agent": team.default_agent,
        "provider_order": team.provider_order,
        "auto_fallback": team.auto_fallback,
        "disabled_agents": team.disabled_agents,
        "provider_commands": team.provider_commands,
    })
}

pub(crate) fn pty_persistence(path: Option<&OsStr>, config_enabled: bool) -> Value {
    let detected = forktty_core::pty_persistence::detect_with_path(path);
    let broker = detected.as_ref().map(|persistence| persistence.broker);
    let broker_executable = detected
        .as_ref()
        .map(|persistence| persistence.broker_path.to_string_lossy().into_owned());
    json!({
        "config_enabled": config_enabled,
        "active": config_enabled && detected.is_some(),
        "available": detected.is_some(),
        "broker": broker.map(|broker| broker.program_name()),
        "broker_executable": broker_executable,
        "scope": "plain_terminal_surfaces",
        "unavailable_reason": if detected.is_none() { Some("broker_not_found") } else { None },
    })
}

pub(crate) fn capability(agent: &str) -> Option<&'static ProviderCapability> {
    let canonical = forktty_core::config::canonical_team_provider(agent)?;
    PROVIDER_CAPABILITIES
        .iter()
        .find(|capability| capability.agent == canonical)
}

pub(crate) fn program_resolution(
    capability: &ProviderCapability,
    team: &forktty_core::TeamConfig,
    path: Option<&OsStr>,
) -> TeamProviderProgramResolution {
    let configured_command = forktty_core::config::team_provider_command(team, capability.agent);
    let program = configured_command.unwrap_or(capability.program).to_string();
    let executable = resolve_child_program(&program, path);
    let available_on_path = executable.is_some() && !Path::new(&program).is_absolute();
    let unavailable_reason = if executable.is_some() {
        None
    } else if configured_command.is_some() {
        Some("configured_program_not_found")
    } else {
        Some("program_not_found")
    };
    TeamProviderProgramResolution {
        program,
        configured: configured_command.is_some(),
        available_on_path,
        executable,
        unavailable_reason,
    }
}

pub(crate) fn considered_row(
    capability: &ProviderCapability,
    team: &forktty_core::TeamConfig,
    available: bool,
    reason: &str,
    path: Option<&OsStr>,
) -> Value {
    let resolution = program_resolution(capability, team, path);
    json!({
        "agent": capability.agent,
        "program": resolution.program.clone(),
        "default_program": capability.program,
        "configured_command": resolution.configured.then(|| resolution.program.clone()),
        "available": available,
        "reason": reason,
        "plan_mode": capability.supports_plan_mode,
        "available_on_path": resolution.available_on_path,
        "executable": resolution
            .executable
            .map(|path| Value::String(path.to_string_lossy().into_owned()))
            .unwrap_or(Value::Null),
    })
}
