use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentSetupStatusKind {
    UpToDate,
    NotInstalled,
    UpdateAvailable,
    CheckFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentSetupStatus {
    pub(super) kind: AgentSetupStatusKind,
    pub(super) label: String,
    pub(super) detail: String,
}

impl AgentSetupStatus {
    pub(super) fn action_label(&self) -> &'static str {
        match self.kind {
            AgentSetupStatusKind::UpToDate => "Repair",
            AgentSetupStatusKind::NotInstalled => "Set Up",
            AgentSetupStatusKind::UpdateAvailable => "Update",
            AgentSetupStatusKind::CheckFailed => "Retry",
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetupSummary {
    agent: String,
    changed: bool,
    #[serde(rename = "configPath")]
    config_path: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct AgentSetupAutoRefresh {
    pub(super) hooks_updated: Vec<String>,
    pub(super) mcp_updated: Vec<String>,
    pub(super) skills_updated: Vec<String>,
    pub(super) errors: Vec<String>,
}

pub(super) fn agent_integration_refresh_message(outcome: &AgentSetupAutoRefresh) -> Option<String> {
    let mut parts = Vec::new();
    if !outcome.hooks_updated.is_empty() {
        parts.push(format!("hooks: {}", outcome.hooks_updated.join(", ")));
    }
    if !outcome.mcp_updated.is_empty() {
        parts.push(format!("MCP: {}", outcome.mcp_updated.join(", ")));
    }
    if !outcome.skills_updated.is_empty() {
        parts.push(format!("skills: {}", outcome.skills_updated.join(", ")));
    }
    if parts.is_empty() {
        return None;
    }

    let mut message = format!(
        "Refreshed managed ForkTTY integrations ({})",
        parts.join("; ")
    );
    if outcome.hooks_updated.iter().any(|agent| agent == "codex") {
        message.push_str(". Open /hooks inside Codex to review the changed hook definitions.");
    }
    Some(message)
}

/// Run setup subcommands against this same binary. These commands are
/// dispatched by the CLI layer before any GUI launch, so they write provider
/// config files and skill directories without touching the socket server.
pub(super) fn run_agent_integrations_setup() -> Result<(), String> {
    run_agent_hooks_setup()?;
    run_mcp_bridge_setup()?;
    run_agent_skills_setup()?;
    Ok(())
}

pub(super) fn run_agent_hooks_setup() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    run_setup_subcommand(&exe, &["hooks", "setup"])
}

pub(super) fn run_mcp_bridge_setup() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    run_setup_subcommand(&exe, &["mcp", "setup"])
}

pub(super) fn run_agent_skills_setup() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    run_setup_subcommand(&exe, &["skills", "setup"])
}

pub(super) fn inspect_agent_integrations_setup() -> AgentSetupStatus {
    combine_setup_statuses(
        inspect_agent_hooks_setup(),
        inspect_mcp_bridge_setup(),
        inspect_agent_skills_setup(),
    )
}

pub(super) fn inspect_agent_hooks_setup() -> AgentSetupStatus {
    inspect_setup_subcommand(
        "Hooks",
        "Agent hooks",
        &["--json", "hooks", "setup", "--dry-run"],
    )
}

pub(super) fn inspect_mcp_bridge_setup() -> AgentSetupStatus {
    inspect_setup_subcommand(
        "MCP",
        "MCP bridge",
        &["--json", "mcp", "setup", "--dry-run"],
    )
}

pub(super) fn inspect_agent_skills_setup() -> AgentSetupStatus {
    inspect_setup_subcommand(
        "Skills",
        "Agent skills",
        &["--json", "skills", "setup", "--dry-run"],
    )
}

pub(super) fn auto_refresh_managed_agent_integrations() -> AgentSetupAutoRefresh {
    let mut outcome = AgentSetupAutoRefresh::default();
    match managed_changed_agents(
        &["--json", "hooks", "setup", "--dry-run"],
        &[
            "--json",
            "hooks",
            "remove",
            "--dry-run",
            "codex",
            "claude",
            "antigravity",
            "opencode",
        ],
    ) {
        Ok(agents) if !agents.is_empty() => {
            if let Err(err) = run_setup_subcommand_dynamic("hooks setup", &agents) {
                outcome.errors.push(err);
            } else {
                outcome.hooks_updated = agents;
            }
        }
        Ok(_) => {}
        Err(err) => outcome.errors.push(err),
    }
    match managed_changed_agents(
        &["--json", "mcp", "setup", "--dry-run"],
        &[
            "--json",
            "mcp",
            "remove",
            "--dry-run",
            "codex",
            "claude",
            "antigravity",
        ],
    ) {
        Ok(agents) if !agents.is_empty() => {
            if let Err(err) = run_setup_subcommand_dynamic("mcp setup", &agents) {
                outcome.errors.push(err);
            } else {
                outcome.mcp_updated = agents;
            }
        }
        Ok(_) => {}
        Err(err) => outcome.errors.push(err),
    }
    match managed_changed_agents(
        &["--json", "skills", "setup", "--dry-run"],
        &[
            "--json",
            "skills",
            "remove",
            "--dry-run",
            "agents",
            "claude",
        ],
    ) {
        Ok(targets) if !targets.is_empty() => {
            if let Err(err) = run_setup_subcommand_dynamic("skills setup", &targets) {
                outcome.errors.push(err);
            } else {
                outcome.skills_updated = targets;
            }
        }
        Ok(_) => {}
        Err(err) => outcome.errors.push(err),
    }
    outcome
}

fn run_setup_subcommand(exe: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("`forktty {}` failed", args.join(" ")))
    } else {
        Err(format!("`forktty {}`: {detail}", args.join(" ")))
    }
}

fn run_setup_subcommand_dynamic(command: &str, agents: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let mut args = command
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    args.extend(agents.iter().cloned());
    let output = Command::new(&exe)
        .args(&args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("`forktty {}` failed", args.join(" ")))
    } else {
        Err(format!("`forktty {}`: {detail}", args.join(" ")))
    }
}

fn inspect_setup_subcommand(label: &str, detail_label: &str, args: &[&str]) -> AgentSetupStatus {
    let result = (|| -> Result<Vec<SetupSummary>, String> {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        let output = run_setup_subcommand_output(&exe, args)?;
        serde_json::from_slice(&output).map_err(|err| err.to_string())
    })();
    match result {
        Ok(summaries) => classify_setup_summaries(label, detail_label, &summaries),
        Err(err) => AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{detail_label} status could not be checked: {err}"),
        },
    }
}

fn run_setup_subcommand_output(exe: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("`forktty {}` failed", args.join(" ")))
    } else {
        Err(format!("`forktty {}`: {detail}", args.join(" ")))
    }
}

fn managed_changed_agents(
    setup_args: &[&str],
    remove_args: &[&str],
) -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let setup = setup_summaries(&exe, setup_args)?;
    let remove = setup_summaries(&exe, remove_args)?;
    let managed = remove
        .into_iter()
        .filter(|summary| summary.changed)
        .map(|summary| summary.agent)
        .collect::<BTreeSet<_>>();
    Ok(setup
        .into_iter()
        .filter(|summary| summary.changed && managed.contains(&summary.agent))
        .map(|summary| summary.agent)
        .collect())
}

fn setup_summaries(exe: &Path, args: &[&str]) -> Result<Vec<SetupSummary>, String> {
    let output = run_setup_subcommand_output(exe, args)?;
    serde_json::from_slice(&output).map_err(|err| err.to_string())
}

fn classify_setup_summaries(
    label: &str,
    detail_label: &str,
    summaries: &[SetupSummary],
) -> AgentSetupStatus {
    if summaries.is_empty() {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{detail_label} status returned no providers."),
        };
    }
    if summaries.iter().all(|summary| !summary.changed) {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::UpToDate,
            label: "Up to date".to_string(),
            detail: format!("{detail_label} are installed for this ForkTTY build."),
        };
    }
    if summaries
        .iter()
        .any(|summary| fs::symlink_metadata(&summary.config_path).is_ok())
    {
        AgentSetupStatus {
            kind: AgentSetupStatusKind::UpdateAvailable,
            label: "Update available".to_string(),
            detail: format!("Re-run setup to refresh managed {label} entries."),
        }
    } else {
        AgentSetupStatus {
            kind: AgentSetupStatusKind::NotInstalled,
            label: "Not installed".to_string(),
            detail: format!("{detail_label} are not installed yet."),
        }
    }
}

fn combine_setup_statuses(
    hooks: AgentSetupStatus,
    mcp: AgentSetupStatus,
    skills: AgentSetupStatus,
) -> AgentSetupStatus {
    if hooks.kind == AgentSetupStatusKind::CheckFailed
        || mcp.kind == AgentSetupStatusKind::CheckFailed
        || skills.kind == AgentSetupStatusKind::CheckFailed
    {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{} {} {}", hooks.detail, mcp.detail, skills.detail),
        };
    }
    if hooks.kind == AgentSetupStatusKind::UpToDate
        && mcp.kind == AgentSetupStatusKind::UpToDate
        && skills.kind == AgentSetupStatusKind::UpToDate
    {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::UpToDate,
            label: "Up to date".to_string(),
            detail: "Hooks, MCP, and skills are installed for this ForkTTY build.".to_string(),
        };
    }
    if hooks.kind == AgentSetupStatusKind::UpdateAvailable
        || mcp.kind == AgentSetupStatusKind::UpdateAvailable
        || skills.kind == AgentSetupStatusKind::UpdateAvailable
    {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::UpdateAvailable,
            label: "Update available".to_string(),
            detail: "Refresh managed hooks, MCP registrations, and skills for this ForkTTY build."
                .to_string(),
        };
    }
    AgentSetupStatus {
        kind: AgentSetupStatusKind::NotInstalled,
        label: "Not installed".to_string(),
        detail: "One or more ForkTTY agent integrations are not installed yet.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(path: PathBuf, changed: bool) -> SetupSummary {
        SetupSummary {
            agent: "codex".to_string(),
            changed,
            config_path: path,
        }
    }

    #[test]
    fn setup_status_is_up_to_date_when_dry_run_has_no_changes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary(dir.path().join("hooks.json"), false)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpToDate);
        assert_eq!(status.action_label(), "Repair");
    }

    #[test]
    fn setup_status_is_not_installed_when_dry_run_would_create_missing_configs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary(dir.path().join("missing-hooks.json"), true)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::NotInstalled);
        assert_eq!(status.action_label(), "Set Up");
    }

    #[test]
    fn setup_status_is_update_available_when_existing_configs_would_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hooks.json");
        fs::write(&path, "{}").expect("write config");
        let status = classify_setup_summaries("Hooks", "Agent hooks", &[summary(path, true)]);

        assert_eq!(status.kind, AgentSetupStatusKind::UpdateAvailable);
        assert_eq!(status.action_label(), "Update");
    }

    #[test]
    fn refresh_message_calls_out_codex_hook_trust_review() {
        let outcome = AgentSetupAutoRefresh {
            hooks_updated: vec!["codex".to_string(), "claude".to_string()],
            mcp_updated: vec!["codex".to_string()],
            ..AgentSetupAutoRefresh::default()
        };

        let message = agent_integration_refresh_message(&outcome).unwrap();
        assert!(message.contains("hooks: codex, claude"));
        assert!(message.contains("Open /hooks inside Codex"));

        let outcome = AgentSetupAutoRefresh {
            hooks_updated: vec!["claude".to_string()],
            ..AgentSetupAutoRefresh::default()
        };
        assert!(!agent_integration_refresh_message(&outcome)
            .unwrap()
            .contains("/hooks"));
        assert!(agent_integration_refresh_message(&AgentSetupAutoRefresh::default()).is_none());
    }
}
