use super::*;
use crate::socket_cli::HookSetupProfile;
use serde::Deserialize;
use std::collections::HashSet;
use std::ffi::OsStr;

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
    pub(super) installed_agent_labels: Vec<String>,
}

impl AgentSetupStatus {
    pub(super) fn action_label(&self) -> &'static str {
        match self.kind {
            AgentSetupStatusKind::UpToDate => "Repair",
            AgentSetupStatusKind::NotInstalled => "Set up",
            AgentSetupStatusKind::UpdateAvailable => "Update",
            AgentSetupStatusKind::CheckFailed => "Retry",
        }
    }

    pub(super) fn has_managed_hooks(&self) -> bool {
        !self.installed_agent_labels.is_empty()
    }
}

impl AgentSetupStatusKind {
    pub(super) fn should_run_setup(self) -> bool {
        !matches!(self, Self::CheckFailed)
    }
}

#[derive(Debug, Deserialize)]
struct SetupSummary {
    agent: String,
    changed: bool,
    #[serde(default, rename = "configPath")]
    config_path: Option<PathBuf>,
    #[serde(default, rename = "requiresTrustReview")]
    requires_trust_review: bool,
    #[serde(default, rename = "trustReviewCommand")]
    trust_review_command: Option<String>,
}

/// Run setup subcommands against this same binary. These commands are
/// dispatched by the CLI layer before any GUI launch, so they write agent
/// config files without touching the socket server.
pub(super) fn run_agent_integrations_setup() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let removal_summaries =
        run_setup_subcommand_summaries(&exe, &["--json", "hooks", "remove", "--dry-run"])?;
    if removal_summaries.is_empty() {
        return Err("Agent hooks status returned no integrations.".to_string());
    }
    let installed_agents = removal_summaries
        .iter()
        .filter(|summary| summary.changed)
        .map(|summary| summary.agent.clone())
        .collect::<Vec<_>>();
    let profile = installed_claude_setup_profile(&removal_summaries)?;
    let args = json_hook_setup_args(&installed_agents, profile, false);
    let summaries = run_setup_subcommand_summaries(&exe, &args)?;
    setup_success_message(&summaries)
}

pub(super) fn run_agent_integrations_remove() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let summaries = run_setup_subcommand_summaries(&exe, &["--json", "hooks", "remove"])?;
    removal_success_message(&summaries)
}

pub(super) fn inspect_agent_integrations_setup() -> AgentSetupStatus {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            return check_failed_status(
                format!("Agent hooks status could not be checked: {err}"),
                Vec::new(),
            );
        }
    };
    let remove =
        match run_setup_subcommand_summaries(&exe, &["--json", "hooks", "remove", "--dry-run"]) {
            Ok(remove) => remove,
            Err(err) => {
                return check_failed_status(
                    format!("Agent hooks status could not be checked: {err}"),
                    Vec::new(),
                );
            }
        };
    let installed_agent_labels = installed_agent_labels(&remove);
    let profile = match installed_claude_setup_profile(&remove) {
        Ok(profile) => profile,
        Err(err) => {
            return check_failed_status(
                format!("Agent hooks status could not be checked: {err}"),
                installed_agent_labels,
            );
        }
    };
    let setup_args = json_hook_setup_args(&[], profile, true);
    match run_setup_subcommand_summaries(&exe, &setup_args) {
        Ok(setup) => classify_setup_summaries(&setup, &remove),
        Err(err) => check_failed_status(
            format!("Agent hooks status could not be checked: {err}"),
            installed_agent_labels,
        ),
    }
}

fn hook_setup_args(installed_agents: &[String], claude_profile: HookSetupProfile) -> Vec<String> {
    let mut args = vec!["hooks".to_string(), "setup".to_string()];
    if claude_profile == HookSetupProfile::Full {
        args.push("--full".to_string());
    }
    args.extend(installed_agents.iter().cloned());
    args
}

fn json_hook_setup_args(
    installed_agents: &[String],
    claude_profile: HookSetupProfile,
    dry_run: bool,
) -> Vec<String> {
    let mut args = vec!["--json".to_string()];
    args.extend(hook_setup_args(installed_agents, claude_profile));
    if dry_run {
        args.push("--dry-run".to_string());
    }
    args
}

fn installed_claude_setup_profile(
    removal_summaries: &[SetupSummary],
) -> Result<HookSetupProfile, String> {
    let Some(claude) = removal_summaries
        .iter()
        .find(|summary| summary.agent == "claude" && summary.changed)
    else {
        return Ok(HookSetupProfile::Lifecycle);
    };
    let config_path = claude
        .config_path
        .as_deref()
        .ok_or_else(|| "Installed Claude hooks did not report their config path.".to_string())?;
    crate::socket_cli::read_claude_installed_hook_profile(config_path)?
        .ok_or_else(|| "Installed Claude hooks did not report their setup profile.".to_string())
}

fn setup_success_message(summaries: &[SetupSummary]) -> Result<String, String> {
    if summaries.is_empty() {
        return Err("Agent hooks setup returned no integrations.".to_string());
    }
    let trust_review_command = summaries
        .iter()
        .find(|summary| summary.requires_trust_review)
        .map(|summary| summary.trust_review_command.as_deref().unwrap_or("/hooks"));
    Ok(match trust_review_command {
        Some(command) => format!("Agent hooks configured. Run {command} in Codex to approve them."),
        None => "Agent hooks configured.".to_string(),
    })
}

fn removal_success_message(summaries: &[SetupSummary]) -> Result<String, String> {
    if summaries.is_empty() {
        return Err("Agent hooks removal returned no integrations.".to_string());
    }
    if summaries.iter().any(|summary| summary.changed) {
        Ok("ForkTTY-managed hooks removed.".to_string())
    } else {
        Ok("No ForkTTY-managed hooks were installed.".to_string())
    }
}

fn run_setup_subcommand_summaries<S: AsRef<OsStr>>(
    exe: &Path,
    args: &[S],
) -> Result<Vec<SetupSummary>, String> {
    let output = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return serde_json::from_slice(&output.stdout).map_err(|err| err.to_string());
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let command = args
        .iter()
        .map(|arg| arg.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    if detail.is_empty() {
        Err(format!("`forktty {command}` failed"))
    } else {
        Err(format!("`forktty {command}`: {detail}"))
    }
}

fn classify_setup_summaries(
    setup_summaries: &[SetupSummary],
    removal_summaries: &[SetupSummary],
) -> AgentSetupStatus {
    if removal_summaries.is_empty() {
        return check_failed_status(
            "Agent hooks status returned no integrations.".to_string(),
            Vec::new(),
        );
    }
    let installed_agent_labels = installed_agent_labels(removal_summaries);
    if setup_summaries.is_empty() {
        return check_failed_status(
            "Agent hooks status returned no integrations.".to_string(),
            installed_agent_labels,
        );
    }
    if !removal_summaries.iter().any(|summary| summary.changed) {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::NotInstalled,
            label: "Not installed".to_string(),
            detail: "Optional for Codex, Claude Code, Antigravity, and OpenCode.".to_string(),
            installed_agent_labels: Vec::new(),
        };
    }
    let installed_agent_list = join_agent_labels(&installed_agent_labels);
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
        return check_failed_status(
            "Agent hooks status returned inconsistent integrations.".to_string(),
            installed_agent_labels,
        );
    }
    if setup_summaries
        .iter()
        .filter(|summary| installed_agents.contains(summary.agent.as_str()))
        .all(|summary| !summary.changed)
    {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::UpToDate,
            label: "Up to date".to_string(),
            detail: format!("Installed for {installed_agent_list}."),
            installed_agent_labels,
        };
    }
    AgentSetupStatus {
        kind: AgentSetupStatusKind::UpdateAvailable,
        label: "Update available".to_string(),
        detail: format!("Updates are available for {installed_agent_list}."),
        installed_agent_labels,
    }
}

fn check_failed_status(detail: String, installed_agent_labels: Vec<String>) -> AgentSetupStatus {
    AgentSetupStatus {
        kind: AgentSetupStatusKind::CheckFailed,
        label: "Check failed".to_string(),
        detail,
        installed_agent_labels,
    }
}

fn installed_agent_labels(summaries: &[SetupSummary]) -> Vec<String> {
    summaries
        .iter()
        .filter(|summary| summary.changed)
        .map(|summary| agent_display_label(&summary.agent).to_string())
        .collect()
}

fn agent_display_label(agent: &str) -> &str {
    match agent {
        "codex" => "Codex",
        "claude" => "Claude Code",
        "antigravity" => "Antigravity",
        "opencode" => "OpenCode",
        _ => agent,
    }
}

fn join_agent_labels(labels: &[String]) -> String {
    match labels {
        [] => "installed agents".to_string(),
        [label] => label.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => {
            let (last, leading) = labels.split_last().expect("non-empty label list");
            format!("{}, and {last}", leading.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn summary(agent: &str, changed: bool) -> SetupSummary {
        SetupSummary {
            agent: agent.to_string(),
            changed,
            config_path: None,
            requires_trust_review: false,
            trust_review_command: None,
        }
    }

    #[test]
    fn setup_status_is_up_to_date_when_dry_run_has_no_changes() {
        let status =
            classify_setup_summaries(&[summary("codex", false)], &[summary("codex", true)]);

        assert_eq!(status.kind, AgentSetupStatusKind::UpToDate);
        assert_eq!(status.action_label(), "Repair");
    }

    #[test]
    fn setup_status_is_not_installed_when_provider_config_exists_without_managed_hooks() {
        let status =
            classify_setup_summaries(&[summary("codex", true)], &[summary("codex", false)]);

        assert_eq!(status.kind, AgentSetupStatusKind::NotInstalled);
        assert_eq!(status.action_label(), "Set up");
        assert!(!status.has_managed_hooks());
    }

    #[test]
    fn setup_status_is_update_available_when_existing_configs_would_change() {
        let status = classify_setup_summaries(
            &[summary("codex", true), summary("claude", true)],
            &[summary("codex", true), summary("claude", true)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpdateAvailable);
        assert_eq!(status.action_label(), "Update");
        assert!(status.has_managed_hooks());
        assert_eq!(
            status.installed_agent_labels,
            ["Codex".to_string(), "Claude Code".to_string()]
        );
        assert!(status.detail.contains("Codex and Claude Code"));
    }

    #[test]
    fn setup_probe_failure_keeps_managed_hooks_available_for_removal() {
        let status = classify_setup_summaries(&[], &[summary("codex", true)]);

        assert_eq!(status.kind, AgentSetupStatusKind::CheckFailed);
        assert!(status.has_managed_hooks());
        assert_eq!(status.installed_agent_labels, ["Codex".to_string()]);
    }

    #[test]
    fn setup_status_ignores_intentionally_absent_providers() {
        let status = classify_setup_summaries(
            &[summary("codex", false), summary("claude", true)],
            &[summary("codex", true), summary("claude", false)],
        );

        assert_eq!(status.kind, AgentSetupStatusKind::UpToDate);
        assert_eq!(
            hook_setup_args(&["codex".to_string()], HookSetupProfile::Lifecycle),
            ["hooks", "setup", "codex"]
        );
    }

    #[test]
    fn full_claude_profile_is_preserved_in_setup_args() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("settings.json");
        fs::write(
            &config_path,
            r#"{"hooks":{"PreToolUse":[{"forkttySource":"forktty"}]}}"#,
        )
        .expect("write Claude config");
        let mut claude = summary("claude", true);
        claude.config_path = Some(config_path);

        assert_eq!(
            hook_setup_args(
                &["claude".to_string()],
                installed_claude_setup_profile(&[claude]).expect("installed profile")
            ),
            ["hooks", "setup", "--full", "claude"]
        );
    }

    #[test]
    fn installed_claude_profile_errors_instead_of_falling_back_to_lifecycle() {
        let claude = summary("claude", true);

        assert_eq!(
            installed_claude_setup_profile(&[claude]).unwrap_err(),
            "Installed Claude hooks did not report their config path."
        );
    }

    #[test]
    fn failed_status_retries_the_check_instead_of_running_setup() {
        assert!(!AgentSetupStatusKind::CheckFailed.should_run_setup());
        assert!(AgentSetupStatusKind::NotInstalled.should_run_setup());
        assert!(AgentSetupStatusKind::UpdateAvailable.should_run_setup());
        assert!(AgentSetupStatusKind::UpToDate.should_run_setup());
    }

    #[test]
    fn codex_trust_review_is_included_in_setup_success_message() {
        let mut codex = summary("codex", true);
        codex.requires_trust_review = true;
        codex.trust_review_command = Some("/hooks".to_string());

        assert_eq!(
            setup_success_message(&[codex]).expect("success message"),
            "Agent hooks configured. Run /hooks in Codex to approve them."
        );
    }

    #[test]
    fn removal_success_message_distinguishes_removed_from_already_absent() {
        assert_eq!(
            removal_success_message(&[summary("codex", true), summary("claude", false)])
                .expect("removal summary"),
            "ForkTTY-managed hooks removed."
        );
        assert_eq!(
            removal_success_message(&[summary("codex", false), summary("claude", false)])
                .expect("removal summary"),
            "No ForkTTY-managed hooks were installed."
        );
        assert_eq!(
            removal_success_message(&[]).unwrap_err(),
            "Agent hooks removal returned no integrations."
        );
    }
}
