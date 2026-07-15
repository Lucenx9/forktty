use super::*;
use serde::Deserialize;
use std::collections::HashSet;

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
}

/// Run setup subcommands against this same binary. These commands are
/// dispatched by the CLI layer before any GUI launch, so they write agent
/// config files without touching the socket server.
pub(super) fn run_agent_integrations_setup() -> Result<(), String> {
    run_agent_hooks_setup()
}

pub(super) fn run_agent_hooks_setup() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let remove_output =
        run_setup_subcommand_output(&exe, &["--json", "hooks", "remove", "--dry-run"])?;
    let removal_summaries: Vec<SetupSummary> =
        serde_json::from_slice(&remove_output).map_err(|err| err.to_string())?;
    if removal_summaries.is_empty() {
        return Err("Agent hooks status returned no integrations.".to_string());
    }
    let installed_agents = removal_summaries
        .into_iter()
        .filter_map(|summary| summary.changed.then_some(summary.agent))
        .collect::<Vec<_>>();
    run_setup_subcommand(&exe, &hook_setup_args(&installed_agents))
}

pub(super) fn inspect_agent_integrations_setup() -> AgentSetupStatus {
    inspect_agent_hooks_setup()
}

pub(super) fn inspect_agent_hooks_setup() -> AgentSetupStatus {
    inspect_setup_subcommand(
        "Hooks",
        "Agent hooks",
        &["--json", "hooks", "setup", "--dry-run"],
    )
}

fn hook_setup_args(installed_agents: &[String]) -> Vec<String> {
    let mut args = vec!["hooks".to_string(), "setup".to_string()];
    args.extend(installed_agents.iter().cloned());
    args
}

fn run_setup_subcommand(exe: &Path, args: &[String]) -> Result<(), String> {
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

fn inspect_setup_subcommand(label: &str, detail_label: &str, args: &[&str]) -> AgentSetupStatus {
    let result = (|| -> Result<(Vec<SetupSummary>, Vec<SetupSummary>), String> {
        let exe = std::env::current_exe().map_err(|err| err.to_string())?;
        let setup_output = run_setup_subcommand_output(&exe, args)?;
        let remove_output =
            run_setup_subcommand_output(&exe, &["--json", "hooks", "remove", "--dry-run"])?;
        let setup = serde_json::from_slice(&setup_output).map_err(|err| err.to_string())?;
        let remove = serde_json::from_slice(&remove_output).map_err(|err| err.to_string())?;
        Ok((setup, remove))
    })();
    match result {
        Ok((setup, remove)) => classify_setup_summaries(label, detail_label, &setup, &remove),
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

fn classify_setup_summaries(
    label: &str,
    detail_label: &str,
    setup_summaries: &[SetupSummary],
    removal_summaries: &[SetupSummary],
) -> AgentSetupStatus {
    if setup_summaries.is_empty() || removal_summaries.is_empty() {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{detail_label} status returned no integrations."),
        };
    }
    if !removal_summaries.iter().any(|summary| summary.changed) {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::NotInstalled,
            label: "Not installed".to_string(),
            detail: format!("{detail_label} are not installed yet."),
        };
    }
    let installed_agents = removal_summaries
        .iter()
        .filter(|summary| summary.changed)
        .map(|summary| summary.agent.as_str())
        .collect::<HashSet<_>>();
    if !installed_agents.iter().all(|agent| {
        setup_summaries
            .iter()
            .any(|summary| summary.agent == *agent)
    }) {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{detail_label} status returned inconsistent integrations."),
        };
    }
    if setup_summaries
        .iter()
        .filter(|summary| installed_agents.contains(summary.agent.as_str()))
        .all(|summary| !summary.changed)
    {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::UpToDate,
            label: "Up to date".to_string(),
            detail: format!("Installed {detail_label} are current for this ForkTTY build."),
        };
    }
    AgentSetupStatus {
        kind: AgentSetupStatusKind::UpdateAvailable,
        label: "Update available".to_string(),
        detail: format!("Re-run setup to refresh managed {label} entries."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(agent: &str, changed: bool) -> SetupSummary {
        SetupSummary {
            agent: agent.to_string(),
            changed,
        }
    }

    #[test]
    fn setup_status_is_up_to_date_when_dry_run_has_no_changes() {
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary("codex", false)],
            &[summary("codex", true)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpToDate);
        assert_eq!(status.action_label(), "Repair");
    }

    #[test]
    fn setup_status_is_not_installed_when_provider_config_exists_without_managed_hooks() {
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary("codex", true)],
            &[summary("codex", false)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::NotInstalled);
        assert_eq!(status.action_label(), "Set Up");
    }

    #[test]
    fn setup_status_is_update_available_when_existing_configs_would_change() {
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary("codex", true)],
            &[summary("codex", true)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpdateAvailable);
        assert_eq!(status.action_label(), "Update");
    }

    #[test]
    fn setup_status_ignores_intentionally_absent_providers() {
        let status = classify_setup_summaries(
            "Hooks",
            "Agent hooks",
            &[summary("codex", false), summary("claude", true)],
            &[summary("codex", true), summary("claude", false)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpToDate);
        assert_eq!(
            hook_setup_args(&["codex".to_string()]),
            ["hooks", "setup", "codex"]
        );
    }
}
