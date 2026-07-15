use super::*;
use serde::Deserialize;
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
    changed: bool,
    #[serde(rename = "configPath")]
    config_path: PathBuf,
}

/// Run setup subcommands against this same binary. These commands are
/// dispatched by the CLI layer before any GUI launch, so they write agent
/// config files without touching the socket server.
pub(super) fn run_agent_integrations_setup() -> Result<(), String> {
    run_agent_hooks_setup()
}

pub(super) fn run_agent_hooks_setup() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    run_setup_subcommand(&exe, &["hooks", "setup"])
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

fn classify_setup_summaries(
    label: &str,
    detail_label: &str,
    summaries: &[SetupSummary],
) -> AgentSetupStatus {
    if summaries.is_empty() {
        return AgentSetupStatus {
            kind: AgentSetupStatusKind::CheckFailed,
            label: "Check failed".to_string(),
            detail: format!("{detail_label} status returned no integrations."),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(path: PathBuf, changed: bool) -> SetupSummary {
        SetupSummary {
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
}
