//! Agent hook installer, health checks, and hook event handling.

use crate::agent_guide;

use super::integration_files::{
    atomic_write_file, backup_file, ensure_parent_dir, hook_config_write_path, read_json_file,
    stable_hook_launcher_path, MAX_HOOK_CONFIG_SIZE_BYTES,
};
use super::transport::send_socket_request_with_timeout;
use super::{
    bool_option, format_doctor_path, inspect_path, now_nanos, parse_flags, print_json,
    read_optional_stdin_json, reject_unknown_options, require_no_args, safe_string_field,
    sanitize_for_terminal, send_socket_request, socket_path_from_env, string_field, trimmed_env,
    write_stdout_line, CliContext, CliError, CliResult, MAX_STDIN_TEXT_BYTES,
};
use serde_json::{json, Map, Value};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const HOOK_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const HOOK_CONTINUE_JSON: &str = "{\"continue\":true,\"suppressOutput\":false}\n";
pub(super) const HOOK_EVENT_CLOCK: &str = "boottime-ns";
pub(super) const HOOK_EVENT_ORDER_PARAM: &str = "hook_event_order";
pub(super) const HOOK_TOOL_LABEL_MAX: usize = 48;
pub(super) const HOOK_TOKEN_CEILING_DEFAULT: u64 = 200_000;
pub(super) const FORKTTY_HOOK_TAG: &str = "forktty";
pub(super) const OPENCODE_PLUGIN_TAG: &str = "forktty-managed-opencode-plugin";

#[derive(Clone, Copy)]
pub(super) struct HookEntrySpec {
    pub(super) event_name: &'static str,
    pub(super) hook_event_name: &'static str,
    pub(super) timeout: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HookInstallKind {
    JsonConfig,
    OpenCodePlugin,
    // Antigravity CLI executes the hook `command` as a bare executable path —
    // no argument splitting and no shell — so the JSON config points at
    // generated wrapper scripts that invoke the forktty launcher.
    AntigravityConfig,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HookSetupProfile {
    Lifecycle,
    Full,
}

#[derive(Clone, Copy)]
pub(super) struct AgentSpec {
    pub(super) key: &'static str,
    pub(super) label: &'static str,
    pub(super) disabled_env: &'static str,
    pub(super) config_path: fn() -> PathBuf,
    pub(super) hook_entries: &'static [HookEntrySpec],
    pub(super) matcher: Option<&'static str>,
    pub(super) install_kind: HookInstallKind,
}

// Codex and Claude Code both treat the `timeout` field as seconds (Codex default 600s;
// Claude default 600s, 30s for UserPromptSubmit). The previous Codex value of 5000
// was a millisecond assumption that meant ~83 minutes; cap at 30s for both providers
// so a forktty hook never holds the agent loop for longer than a generous local
// round-trip while still leaving headroom over the socket request budget.
pub(super) const HOOK_ENTRY_TIMEOUT_SECS: u64 = 30;

pub(super) const OPENCODE_HOOK_TIMEOUT_MS: u64 = HOOK_ENTRY_TIMEOUT_SECS * 1000;
pub(super) const OPENCODE_MAX_INPUT_BYTES: usize = MAX_STDIN_TEXT_BYTES;

// Gemini CLI is no longer an active ForkTTY integration target. Keep only
// enough legacy metadata to remove ForkTTY-managed config written by older
// releases from ~/.gemini/settings.json.
pub(super) const LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS: u64 = HOOK_ENTRY_TIMEOUT_SECS * 1000;

pub(super) const CODEX_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PermissionRequest",
        hook_event_name: "permission-request",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreCompact",
        hook_event_name: "pre-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostCompact",
        hook_event_name: "post-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SubagentStart",
        hook_event_name: "subagent-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SubagentStop",
        hook_event_name: "subagent-stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

pub(super) const CLAUDE_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "UserPromptSubmit",
        hook_event_name: "prompt-submit",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "UserPromptExpansion",
        hook_event_name: "prompt-expansion",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Setup",
        hook_event_name: "setup",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PermissionRequest",
        hook_event_name: "permission-request",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PermissionDenied",
        hook_event_name: "permission-denied",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUseFailure",
        hook_event_name: "post-tool-failure",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolBatch",
        hook_event_name: "post-tool-batch",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SubagentStart",
        hook_event_name: "subagent-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SubagentStop",
        hook_event_name: "subagent-stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "TaskCreated",
        hook_event_name: "task-created",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "TaskCompleted",
        hook_event_name: "task-completed",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Elicitation",
        hook_event_name: "elicitation",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "ElicitationResult",
        hook_event_name: "elicitation-result",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreCompact",
        hook_event_name: "pre-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostCompact",
        hook_event_name: "post-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "StopFailure",
        hook_event_name: "stop-failure",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "TeammateIdle",
        hook_event_name: "teammate-idle",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "Notification",
        hook_event_name: "notification",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "ConfigChange",
        hook_event_name: "config-change",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "InstructionsLoaded",
        hook_event_name: "instructions-loaded",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "CwdChanged",
        hook_event_name: "cwd-changed",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "FileChanged",
        hook_event_name: "file-changed",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "WorktreeCreate",
        hook_event_name: "worktree-create",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "WorktreeRemove",
        hook_event_name: "worktree-remove",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

pub(super) const CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUseFailure",
        hook_event_name: "post-tool-failure",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolBatch",
        hook_event_name: "post-tool-batch",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

// Antigravity CLI v1.0.3 parses exactly these hook events from hooks.json;
// unknown event names are dropped silently (verified against the binary's
// "N total handlers" load log). PreInvocation fires before each model call.
// Lifecycle hooks use flat handler objects; tool hooks use matcher wrappers.
// The timeout field is unverified for Antigravity and never emitted.
pub(super) const ANTIGRAVITY_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "PreInvocation",
        hook_event_name: "before-model",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PreToolUse",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostToolUse",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

pub(super) const OPENCODE_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "session.created",
        hook_event_name: "session-start",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "session.status",
        hook_event_name: "prompt-submit",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "permission.asked",
        hook_event_name: "permission-request",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "permission.replied",
        hook_event_name: "permission-replied",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "tool.execute.before",
        hook_event_name: "pre-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "tool.execute.after",
        hook_event_name: "post-tool",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "experimental.session.compacting",
        hook_event_name: "pre-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "session.compacted",
        hook_event_name: "post-compact",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "session.idle",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "session.error",
        hook_event_name: "stop-failure",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "session.deleted",
        hook_event_name: "session-end",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

pub(super) const LEGACY_GEMINI_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "SessionStart",
        hook_event_name: "session-start",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "BeforeAgent",
        hook_event_name: "prompt-submit",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "BeforeTool",
        hook_event_name: "pre-tool",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "BeforeToolSelection",
        hook_event_name: "before-tool-selection",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "AfterTool",
        hook_event_name: "post-tool",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "BeforeModel",
        hook_event_name: "before-model",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "AfterModel",
        hook_event_name: "after-model",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "AfterAgent",
        hook_event_name: "stop",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "Notification",
        hook_event_name: "notification",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "PreCompress",
        hook_event_name: "pre-compact",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS,
    },
];

pub(super) const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        key: "codex",
        label: "Codex",
        disabled_env: "FORKTTY_CODEX_HOOKS_DISABLED",
        config_path: codex_config_path,
        hook_entries: CODEX_HOOK_ENTRIES,
        matcher: None,
        install_kind: HookInstallKind::JsonConfig,
    },
    AgentSpec {
        key: "claude",
        label: "Claude",
        disabled_env: "FORKTTY_CLAUDE_HOOKS_DISABLED",
        config_path: claude_config_path,
        hook_entries: CLAUDE_HOOK_ENTRIES,
        matcher: Some("*"),
        install_kind: HookInstallKind::JsonConfig,
    },
    AgentSpec {
        key: "antigravity",
        label: "Antigravity",
        disabled_env: "FORKTTY_ANTIGRAVITY_HOOKS_DISABLED",
        config_path: antigravity_config_path,
        // Matcher is applied to tool events only; PreInvocation takes none.
        matcher: Some("*"),
        hook_entries: ANTIGRAVITY_HOOK_ENTRIES,
        install_kind: HookInstallKind::AntigravityConfig,
    },
    AgentSpec {
        key: "opencode",
        label: "OpenCode",
        disabled_env: "FORKTTY_OPENCODE_HOOKS_DISABLED",
        config_path: opencode_plugin_path,
        hook_entries: OPENCODE_HOOK_ENTRIES,
        matcher: None,
        install_kind: HookInstallKind::OpenCodePlugin,
    },
];

pub(super) const DEFAULT_HOOK_SETUP_AGENT_KEYS: &[&str] =
    &["codex", "claude", "antigravity", "opencode"];

pub(super) static LEGACY_GEMINI_HOOK_AGENT: AgentSpec = AgentSpec {
    key: "gemini",
    label: "Gemini",
    disabled_env: "FORKTTY_GEMINI_HOOKS_DISABLED",
    config_path: legacy_gemini_config_path,
    hook_entries: LEGACY_GEMINI_HOOK_ENTRIES,
    matcher: None,
    install_kind: HookInstallKind::JsonConfig,
};

pub(super) fn handle_hooks(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
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

pub(super) struct HookSetupPlan {
    pub(super) spec: &'static AgentSpec,
    pub(super) config_path: PathBuf,
    pub(super) changed: bool,
    pub(super) content: String,
    /// Generated wrapper scripts written alongside the config (Antigravity
    /// only: its hook `command` is a bare executable path with no arguments).
    pub(super) scripts: Vec<(PathBuf, String)>,
}

pub(super) enum HookRemoveAction {
    Write(String),
    DeleteFile,
    None,
}

pub(super) struct HookRemovePlan {
    pub(super) spec: &'static AgentSpec,
    pub(super) config_path: PathBuf,
    pub(super) changed: bool,
    pub(super) action: HookRemoveAction,
    /// ForkTTY-owned generated scripts directory deleted on removal.
    pub(super) scripts_dir: Option<PathBuf>,
}

pub(super) fn build_hook_setup_plan(
    spec: &'static AgentSpec,
    launcher: &Path,
) -> CliResult<HookSetupPlan> {
    build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Lifecycle)
}

pub(super) fn build_hook_setup_plan_with_profile(
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

pub(super) fn build_hook_remove_plan(
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

pub(super) fn supported_agents(names: &[String]) -> CliResult<Vec<&'static AgentSpec>> {
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

pub(super) fn supported_hook_remove_agents(names: &[String]) -> CliResult<Vec<&'static AgentSpec>> {
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

pub(super) fn default_hook_setup_agents() -> Vec<&'static AgentSpec> {
    DEFAULT_HOOK_SETUP_AGENT_KEYS
        .iter()
        .map(|key| agent_spec(key).expect("default hook setup agent exists"))
        .collect()
}

pub(super) fn agent_spec(agent: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|spec| spec.key == agent)
}

pub(super) fn normalize_agent_name(agent: &str) -> String {
    match agent.to_lowercase().as_str() {
        "claude-code" | "claude_code" => "claude".to_string(),
        "open-code" | "open_code" => "opencode".to_string(),
        "agy" => "antigravity".to_string(),
        other => other.to_string(),
    }
}

pub(super) fn home_dir() -> PathBuf {
    trimmed_env("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn codex_home_dir() -> PathBuf {
    trimmed_env("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

pub(super) fn codex_config_path() -> PathBuf {
    codex_home_dir().join("hooks.json")
}

pub(super) fn claude_config_path() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("settings.json")
}

pub(super) fn legacy_gemini_config_path() -> PathBuf {
    home_dir().join(".gemini/settings.json")
}

// Antigravity CLI loads user-level hooks from ~/.gemini/config/hooks.json
// (verified against agy 1.0.3; the workspace-level .agents/hooks.json is
// intentionally not managed so hooks work from any project).
pub(super) fn antigravity_root_dir() -> PathBuf {
    home_dir().join(".gemini")
}

pub(super) fn antigravity_config_dir() -> PathBuf {
    antigravity_root_dir().join("config")
}

pub(super) fn antigravity_config_path() -> PathBuf {
    antigravity_config_dir().join("hooks.json")
}

pub(super) fn antigravity_scripts_dir() -> PathBuf {
    antigravity_config_dir().join("forktty-hooks.generated")
}

pub(super) fn antigravity_script_path(hook_event_name: &str) -> PathBuf {
    antigravity_scripts_dir().join(format!("{hook_event_name}.sh"))
}

pub(super) fn opencode_plugin_path() -> PathBuf {
    trimmed_env("OPENCODE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config/opencode"))
        .join("plugins/forktty.generated.js")
}

pub(super) fn build_hook_shell_command(launcher: &Path, spec: &AgentSpec, event: &str) -> String {
    format!(
        "[ \"${{{}:-}}\" != \"1\" ] && {} hooks {} {} || echo '{}'",
        spec.disabled_env,
        shell_quote(&launcher.display().to_string()),
        spec.key,
        event,
        HOOK_CONTINUE_JSON.trim_end()
    )
}

pub(super) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub(super) fn build_hook_entry(spec: &AgentSpec, command: String, timeout: u64) -> Value {
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

pub(super) fn merge_hook_config(
    existing: &Value,
    spec: &AgentSpec,
    launcher: &Path,
) -> CliResult<(bool, Value)> {
    merge_hook_config_with_profile(existing, spec, launcher, HookSetupProfile::Full)
}

pub(super) fn merge_hook_config_with_profile(
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

pub(super) fn hook_entry_enabled_for_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    !(spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name))
}

pub(super) fn hook_entry_removed_by_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name)
}

pub(super) fn is_claude_high_frequency_event(event_name: &str) -> bool {
    CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES
        .iter()
        .any(|entry| entry.event_name == event_name)
}

pub(super) fn hook_setup_profile_name(profile: HookSetupProfile) -> &'static str {
    match profile {
        HookSetupProfile::Lifecycle => "lifecycle",
        HookSetupProfile::Full => "full",
    }
}

pub(super) fn remove_hook_config(
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

pub(super) fn is_forktty_managed_entry(entry: &Value) -> bool {
    entry.get("forkttySource").and_then(Value::as_str) == Some(FORKTTY_HOOK_TAG)
}

/// ForkTTY owns this named hook group in Antigravity's hooks.json; other
/// top-level groups belong to the user and are never touched.
pub(super) const ANTIGRAVITY_HOOK_GROUP: &str = "forktty";
pub(super) const ANTIGRAVITY_SCRIPT_TAG: &str = "forktty-managed-antigravity-hook";

/// Antigravity executes `command` as one executable path (no argv splitting,
/// no shell), so each event points at a generated wrapper script that runs
/// the launcher with the usual guard line. PreToolUse is a gating hook, so its
/// disabled/failed fallback explicitly approves tool use; non-gating events
/// fall back to `{}` because Antigravity rejects unknown response fields like
/// `continue` under strict protojson unmarshaling.
pub(super) fn build_antigravity_hook_script(
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

pub(super) fn merge_antigravity_hook_config(
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

pub(super) fn is_antigravity_flat_hook_event(event_name: &str) -> bool {
    matches!(event_name, "PreInvocation" | "PostInvocation" | "Stop")
}

pub(super) fn is_legacy_forktty_hook_command(
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

pub(super) fn read_agent_config(spec: &AgentSpec, path: &Path) -> CliResult<Value> {
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

pub(super) fn read_opencode_plugin_file(
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

pub(super) fn build_opencode_plugin(launcher: &Path) -> CliResult<String> {
    let launcher = serde_json::to_string(&launcher.display().to_string())?;
    Ok(format!(
        r#"// {tag}
// Generated by `forktty hooks setup`; local edits will be replaced.
import {{ spawnSync }} from "node:child_process";

pub(super) const FORKTTY_LAUNCHER = {launcher};
pub(super) const DISABLED_ENV = "FORKTTY_OPENCODE_HOOKS_DISABLED";
pub(super) const HOOK_TIMEOUT_MS = {timeout};
pub(super) const MAX_INPUT_BYTES = {max_input_bytes};
pub(super) const MAX_SANITIZE_DEPTH = 32;
pub(super) const MAX_SANITIZE_ITEMS = 128;
pub(super) const MAX_SANITIZE_NODES = 4096;
pub(super) const textEncoder = new TextEncoder();

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

pub(super) const eventMap = {{
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

pub(super) fn ensure_private_antigravity_hook_dirs() -> CliResult<()> {
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
        "hint": "Codex asks for approval before running hooks it has no trust record for; run /hooks inside Codex to review.",
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
            if !is_forktty_managed_entry(entry) {
                continue;
            }
            let Some(commands) = entry.get("hooks").and_then(Value::as_array) else {
                continue;
            };
            for hook in commands {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(launcher) = parse_launcher_from_managed_command(command, spec) {
                    return Some(launcher);
                }
            }
        }
    }
    None
}

pub(super) fn parse_launcher_from_managed_command(
    command: &str,
    spec: &AgentSpec,
) -> Option<String> {
    let marker = "&& '";
    let start = command.find(marker)? + marker.len();
    let rest = &command[start..];
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

pub(super) fn handle_hook_event(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let agent_name = args
        .first()
        .map(|value| normalize_agent_name(value))
        .unwrap_or_default();
    let event = args
        .get(1)
        .map(|value| value.to_lowercase())
        .unwrap_or_default();
    // Real hooks always pass a known agent key from the generated templates,
    // so an unknown name here is a typed-in typo: fail loudly instead of
    // printing the lenient continue JSON and exiting 0.
    let Some(spec) = agent_spec(&agent_name) else {
        return Err(CliError::new(format!(
            "Unsupported hooks subcommand or agent: {}. Expected setup, remove, doctor, test, or `<agent> <event>` (agents: {})",
            args.first().map(String::as_str).unwrap_or("(missing)"),
            supported_agent_keys()
        )));
    };
    if !is_supported_hook_event(&event) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unsupported hook event for {}: {}",
                spec.key,
                args.get(1).map(String::as_str).unwrap_or("(missing)")
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }
    if let Some(extra) = args.get(2) {
        eprintln!(
            "{}",
            sanitize_for_terminal(&format!(
                "Unexpected hook argument for {} {}: {}",
                spec.key, event, extra
            ))
        );
        print!("{HOOK_CONTINUE_JSON}");
        return Ok(());
    }

    let payload = match read_optional_stdin_json() {
        Ok(payload) => payload,
        Err(err) => {
            eprintln!(
                "{}",
                sanitize_for_terminal(&format!(
                    "ForkTTY hook warning: failed to read hook stdin: {}",
                    err.message
                ))
            );
            Value::Null
        }
    };
    let order = next_hook_event_order();
    let actions = build_hook_actions(spec, &event, &payload, &order);
    hook_debug(
        context,
        &format!(
            "{} {} order={} actions={} socket={}",
            spec.key,
            event,
            order,
            actions.len(),
            if should_send_hook_actions(context) {
                context.socket_path.display().to_string()
            } else {
                "(disabled)".to_string()
            }
        ),
    );
    if should_send_hook_actions(context) {
        for (method, params) in actions {
            if let Err(err) = send_socket_request_with_timeout(
                &context.socket_path,
                &method,
                params,
                HOOK_STATUS_TIMEOUT,
            ) {
                // Keep going on failure: a transient error on one action (e.g.
                // an informational log) must not skip later cleanup actions
                // like clearing a stale status or permission marker.
                eprintln!(
                    "{}",
                    sanitize_for_terminal(&format!("ForkTTY hook warning: {}", err.message))
                );
            }
        }
    }
    let enrichments = gather_hook_enrichments(context, spec, &event, &payload);
    if let Some(token_action) = build_token_progress_action(spec, &enrichments, &event, &order) {
        if should_send_hook_actions(context) {
            let _ = send_socket_request_with_timeout(
                &context.socket_path,
                "metadata.set_progress",
                token_action,
                HOOK_STATUS_TIMEOUT,
            );
        }
    }
    let response = build_hook_response(spec, &event, &enrichments)?;
    write_stdout_line(&serde_json::to_string(&response)?)?;
    Ok(())
}

pub(super) fn is_supported_hook_event(event: &str) -> bool {
    matches!(
        event,
        "after-model"
            | "before-model"
            | "before-tool-selection"
            | "config-change"
            | "cwd-changed"
            | "elicitation"
            | "elicitation-result"
            | "file-changed"
            | "instructions-loaded"
            | "notification"
            | "permission-denied"
            | "permission-replied"
            | "permission-request"
            | "post-tool"
            | "post-tool-batch"
            | "post-tool-failure"
            | "pre-compact"
            | "pre-tool"
            | "post-compact"
            | "prompt-expansion"
            | "prompt-submit"
            | "session-end"
            | "session-start"
            | "setup"
            | "stop"
            | "stop-failure"
            | "subagent-start"
            | "subagent-stop"
            | "task-completed"
            | "task-created"
            | "teammate-idle"
            | "worktree-create"
            | "worktree-remove"
    )
}

pub(super) fn should_send_hook_actions(context: &CliContext) -> bool {
    context.socket_explicit || socket_path_from_env().is_some()
}

pub(super) fn hook_debug(context: &CliContext, message: &str) {
    if context.verbose || is_truthy_env("FORKTTY_HOOK_DEBUG") {
        eprintln!("ForkTTY hook debug: {}", sanitize_for_terminal(message));
    }
}

pub(super) fn is_truthy_env(key: &str) -> bool {
    trimmed_env(key)
        .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Hook event ordering must survive wall-clock steps (NTP, manual `date`):
/// orders are compared across short-lived CLI processes, so use
/// CLOCK_BOOTTIME — system-wide, monotonic, and advancing across suspend —
/// instead of `SystemTime`, which previously dropped every hook update issued
/// after the clock stepped backwards.
pub(super) fn next_hook_event_order() -> String {
    boottime_nanos().to_string()
}

pub(super) fn boottime_nanos() -> u128 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is a valid, writable timespec for the duration of the call.
    if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) } == 0 {
        (ts.tv_sec as u128) * 1_000_000_000 + (ts.tv_nsec as u128)
    } else {
        // CLOCK_BOOTTIME cannot fail on the Linux kernels ForkTTY supports;
        // fall back to the wall clock rather than aborting a hook.
        now_nanos()
    }
}

pub(super) fn increment_hook_event_order(order: &str) -> String {
    order
        .parse::<u128>()
        .map(|value| value.saturating_add(1).to_string())
        .unwrap_or_else(|_| next_hook_event_order())
}

pub(super) fn hook_target_params() -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    params
}

pub(super) fn add_hook_metadata(
    mut params: Map<String, Value>,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Value {
    params.insert(
        HOOK_EVENT_ORDER_PARAM.to_string(),
        Value::String(order.to_string()),
    );
    params.insert(
        "hook_event_clock".to_string(),
        Value::String(HOOK_EVENT_CLOCK.to_string()),
    );
    params.insert(
        "hook_event_name".to_string(),
        Value::String(event.to_string()),
    );
    if let Some(turn_id) = extract_hook_turn_id(event, payload) {
        params.insert("hook_turn_id".to_string(), Value::String(turn_id));
    }
    if let Some(session_id) = extract_hook_session_id(payload) {
        params.insert("hook_session_id".to_string(), Value::String(session_id));
    }
    if let Some(cwd) = hook_session_cwd_for_metadata(spec, payload) {
        params.insert("hook_session_cwd".to_string(), Value::String(cwd));
    }
    Value::Object(params)
}

pub(super) fn hook_session_cwd_for_metadata(spec: &AgentSpec, payload: &Value) -> Option<String> {
    if spec.key == "antigravity" {
        return extract_antigravity_workspace_cwd(payload);
    }
    std::env::current_dir()
        .ok()
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .map(|cwd| cwd.to_string_lossy().into_owned())
}

pub(super) fn extract_antigravity_workspace_cwd(payload: &Value) -> Option<String> {
    extract_first_string_array_item(payload, &["workspacePaths", "workspace_paths"]).or_else(|| {
        extract_first_string_like(
            payload,
            &[
                "workspacePath",
                "workspace_path",
                "workspaceRoot",
                "workspace_root",
            ],
        )
        .and_then(|value| valid_hook_session_cwd(&value))
    })
}

pub(super) fn extract_first_string_array_item(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            if let Some(Value::Array(values)) = object.get(*key) {
                for value in values {
                    if let Some(path) = value.as_str().and_then(valid_hook_session_cwd) {
                        return Some(path);
                    }
                }
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

pub(super) fn valid_hook_session_cwd(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        return None;
    }
    if !Path::new(trimmed).is_absolute() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Map documented permission modes to a status color so risky modes are
/// visible at a glance. Unknown provider-specific values stay neutral
/// (`muted`) to avoid inventing a risk model the provider does not publish.
pub(super) fn permission_mode_color(spec: &AgentSpec, mode: &str) -> &'static str {
    if !matches!(spec.key, "claude" | "codex") {
        return "muted";
    }
    match mode {
        "bypassPermissions" => "red",
        "acceptEdits" | "auto" | "dontAsk" => "yellow",
        _ => "muted",
    }
}

pub(super) struct HookActionBuilder<'a> {
    spec: &'a AgentSpec,
    event: &'a str,
    payload: &'a Value,
    order: &'a str,
    target: Map<String, Value>,
    key: String,
    message: String,
    permission_key: String,
    permission_mode: Option<String>,
}

impl<'a> HookActionBuilder<'a> {
    fn new(spec: &'a AgentSpec, event: &'a str, payload: &'a Value, order: &'a str) -> Self {
        let target = hook_target_params();
        let key = format!("agent:{}", spec.key);
        let message = sanitize_for_terminal(&extract_hook_message(payload));
        let permission_key = format!("agent:{}:permission", spec.key);
        let permission_mode = extract_hook_permission_mode(payload);

        Self {
            spec,
            event,
            payload,
            order,
            target,
            key,
            message,
            permission_key,
            permission_mode,
        }
    }

    fn log(&self, level: &str, message: String) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert("level".to_string(), Value::String(level.to_string()));
        params.insert("message".to_string(), Value::String(message));
        ("metadata.log".to_string(), Value::Object(params))
    }

    fn status(&self, value: &str, color: &str, event_name: &str) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert("key".to_string(), Value::String(self.key.clone()));
        params.insert(
            "label".to_string(),
            Value::String(self.spec.label.to_string()),
        );
        params.insert("value".to_string(), Value::String(value.to_string()));
        params.insert("color".to_string(), Value::String(color.to_string()));
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, self.spec, event_name, self.payload, self.order),
        )
    }

    fn permission_status(&self, mode: &str, event_name: &str) -> (String, Value) {
        let mut params = self.target.clone();
        params.insert(
            "key".to_string(),
            Value::String(self.permission_key.clone()),
        );
        params.insert(
            "label".to_string(),
            Value::String(format!("{} mode", self.spec.label)),
        );
        params.insert("value".to_string(), Value::String(mode.to_string()));
        params.insert(
            "color".to_string(),
            Value::String(permission_mode_color(self.spec, mode).to_string()),
        );
        (
            "metadata.set_status".to_string(),
            add_hook_metadata(params, self.spec, event_name, self.payload, self.order),
        )
    }

    fn with_permission(
        &self,
        mut actions: Vec<(String, Value)>,
        event_name: &str,
    ) -> Vec<(String, Value)> {
        if let Some(mode) = self.permission_mode.as_deref() {
            actions.push(self.permission_status(mode, event_name));
        }
        actions
    }

    fn handle_session_start(&self) -> Vec<(String, Value)> {
        let source = extract_hook_source(self.payload)
            .map(|source| format!(" ({source})"))
            .unwrap_or_default();
        self.with_permission(
            vec![
                self.log(
                    "info",
                    format!("{} session started{source}", self.spec.label),
                ),
                self.status("Ready", "green", self.event),
            ],
            self.event,
        )
    }

    fn handle_prompt_submit(&self) -> Vec<(String, Value)> {
        self.with_permission(
            vec![
                self.log("info", format!("{} prompt submitted", self.spec.label)),
                self.status("Running", "blue", self.event),
            ],
            self.event,
        )
    }

    fn handle_notification(&self) -> Vec<(String, Value)> {
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} needs input", self.spec.label)),
        );
        note.insert(
            "body".to_string(),
            Value::String(if self.message.is_empty() {
                format!("{} reported a prompt or attention event.", self.spec.label)
            } else {
                self.message.clone()
            }),
        );
        note.insert("kind".to_string(), Value::String("prompt".to_string()));
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} requested attention", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Needs input", "yellow", self.event),
            ("notification.create".to_string(), Value::Object(note)),
        ]
    }

    fn handle_permission_request(&self) -> Vec<(String, Value)> {
        let body = if self.message.is_empty() {
            format!("{} requested permission.", self.spec.label)
        } else {
            self.message.clone()
        };
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} permission required", self.spec.label)),
        );
        note.insert("body".to_string(), Value::String(body));
        note.insert("kind".to_string(), Value::String("prompt".to_string()));
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} requested permission", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Permission required", "yellow", self.event),
            ("notification.create".to_string(), Value::Object(note)),
        ]
    }

    fn handle_permission_denied(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} permission denied", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Permission denied", "yellow", self.event),
        ]
    }

    fn handle_failure(&self) -> Vec<(String, Value)> {
        let body = if self.message.is_empty() {
            format!("{} reported a failure.", self.spec.label)
        } else {
            self.message.clone()
        };
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} error", self.spec.label)),
        );
        note.insert("body".to_string(), Value::String(body));
        note.insert("kind".to_string(), Value::String("error".to_string()));
        vec![
            self.log(
                "error",
                if self.message.is_empty() {
                    format!("{} reported a failure", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Error", "red", self.event),
            ("notification.create".to_string(), Value::Object(note)),
        ]
    }

    fn handle_basic_info(&self) -> Vec<(String, Value)> {
        vec![self.log(
            "info",
            if self.message.is_empty() {
                format!("{} reported {}", self.spec.label, self.event)
            } else {
                self.message.clone()
            },
        )]
    }

    fn handle_prompt_expansion(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} prompt expansion", self.spec.label)),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_before_model(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} model request started", self.spec.label)),
            self.status("Thinking", "blue", self.event),
        ]
    }

    fn handle_after_model(&self) -> Vec<(String, Value)> {
        vec![self.log(
            "info",
            if self.message.is_empty() {
                format!("{} model response received", self.spec.label)
            } else {
                self.message.clone()
            },
        )]
    }

    fn handle_before_tool_selection(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} selecting tools", self.spec.label)),
            self.status("Selecting tool", "blue", self.event),
        ]
    }

    fn handle_pre_tool(&self) -> Vec<(String, Value)> {
        let tool = extract_hook_tool_name(self.payload);
        vec![
            self.log(
                "info",
                tool.map(|tool| format!("{} running {tool}", self.spec.label))
                    .unwrap_or_else(|| format!("{} running tool", self.spec.label)),
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_post_tool(&self) -> Vec<(String, Value)> {
        let tool = extract_hook_tool_name(self.payload).unwrap_or_else(|| "tool".to_string());
        let is_error = extract_hook_tool_error(self.payload);
        let mut actions = vec![self.log(
            if is_error { "error" } else { "info" },
            if is_error {
                format!("{} {tool} reported an error", self.spec.label)
            } else {
                format!("{} finished {tool}", self.spec.label)
            },
        )];
        if is_error {
            let mut note = self.target.clone();
            note.insert(
                "title".to_string(),
                Value::String(format!("{} tool error", self.spec.label)),
            );
            note.insert(
                "body".to_string(),
                Value::String(format!("{tool} returned an error response.")),
            );
            note.insert("kind".to_string(), Value::String("error".to_string()));
            actions.push(("notification.create".to_string(), Value::Object(note)));
        }
        actions
    }

    fn handle_post_tool_batch(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} finished tool batch", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_subagent_start(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} subagent started", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Subagent running", "blue", self.event),
        ]
    }

    fn handle_subagent_stop(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} subagent finished", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_task_created(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} task created", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_task_completed(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} task completed", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_elicitation(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "warn",
                if self.message.is_empty() {
                    format!("{} elicitation requested", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Needs input", "yellow", self.event),
        ]
    }

    fn handle_elicitation_result(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} received {}", self.spec.label, self.event)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_pre_compact(&self) -> Vec<(String, Value)> {
        let trigger = extract_hook_compact_trigger(self.payload);
        let trigger_msg = trigger
            .as_ref()
            .map(|trigger| format!(" ({trigger})"))
            .unwrap_or_default();
        let mut note = self.target.clone();
        note.insert(
            "title".to_string(),
            Value::String(format!("{} compacting context", self.spec.label)),
        );
        note.insert(
            "body".to_string(),
            Value::String(
                trigger
                    .map(|trigger| format!("Context compaction triggered: {trigger}."))
                    .unwrap_or_else(|| "Context compaction in progress.".to_string()),
            ),
        );
        note.insert("kind".to_string(), Value::String("info".to_string()));
        vec![
            self.log(
                "warn",
                format!("{} context compacting{trigger_msg}", self.spec.label),
            ),
            self.status("Compacting", "yellow", self.event),
            ("notification.create".to_string(), Value::Object(note)),
        ]
    }

    fn handle_post_compact(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} context compacted", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn clear_status(&self, key: &str) -> (String, Value) {
        let mut clear = self.target.clone();
        clear.insert("key".to_string(), Value::String(key.to_string()));
        (
            "metadata.clear_status".to_string(),
            add_hook_metadata(clear, self.spec, self.event, self.payload, self.order),
        )
    }

    fn handle_stop(&self) -> Vec<(String, Value)> {
        let mut actions = vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} stopped", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Ready", "green", self.event),
        ];
        if !self
            .spec
            .hook_entries
            .iter()
            .any(|entry| entry.hook_event_name == "session-end")
        {
            actions.push(self.clear_status(&self.permission_key));
        }
        actions
    }

    fn handle_teammate_idle(&self) -> Vec<(String, Value)> {
        vec![
            self.log(
                "info",
                if self.message.is_empty() {
                    format!("{} teammate idle", self.spec.label)
                } else {
                    self.message.clone()
                },
            ),
            self.status("Running", "blue", self.event),
        ]
    }

    fn handle_session_end(&self) -> Vec<(String, Value)> {
        vec![
            self.log("info", format!("{} session ended", self.spec.label)),
            self.clear_status(&self.key),
            self.clear_status(&self.permission_key),
        ]
    }

    fn build(self) -> Vec<(String, Value)> {
        match self.event {
            "session-start" => self.handle_session_start(),
            "prompt-submit" => self.handle_prompt_submit(),
            "notification" => self.handle_notification(),
            "permission-request" => self.handle_permission_request(),
            "permission-denied" => self.handle_permission_denied(),
            "stop-failure" | "post-tool-failure" => self.handle_failure(),
            "setup"
            | "config-change"
            | "instructions-loaded"
            | "cwd-changed"
            | "file-changed"
            | "worktree-create"
            | "worktree-remove" => self.handle_basic_info(),
            "prompt-expansion" => self.handle_prompt_expansion(),
            "before-model" => self.handle_before_model(),
            "after-model" => self.handle_after_model(),
            "before-tool-selection" => self.handle_before_tool_selection(),
            "pre-tool" => self.handle_pre_tool(),
            "post-tool" => self.handle_post_tool(),
            "post-tool-batch" => self.handle_post_tool_batch(),
            "subagent-start" => self.handle_subagent_start(),
            "subagent-stop" => self.handle_subagent_stop(),
            "task-created" => self.handle_task_created(),
            "task-completed" => self.handle_task_completed(),
            "elicitation" => self.handle_elicitation(),
            "elicitation-result" | "permission-replied" => self.handle_elicitation_result(),
            "pre-compact" => self.handle_pre_compact(),
            "post-compact" => self.handle_post_compact(),
            "stop" => self.handle_stop(),
            "teammate-idle" => self.handle_teammate_idle(),
            "session-end" => self.handle_session_end(),
            _ => Vec::new(),
        }
    }
}

pub(super) fn build_hook_actions(
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Vec<(String, Value)> {
    HookActionBuilder::new(spec, event, payload, order).build()
}

pub(super) struct HookEnrichments {
    pub(super) token_usage: Option<TokenUsage>,
    pub(super) workspace: Option<HookWorkspaceContext>,
}

#[derive(Clone)]
pub(super) struct HookWorkspaceContext {
    pub(super) name: String,
    pub(super) git_branch: Option<String>,
}

#[derive(Clone, Copy)]
pub(super) struct TokenUsage {
    pub(super) input: u64,
    pub(super) output: u64,
    pub(super) cache_read: u64,
    pub(super) cache_creation: u64,
}

impl TokenUsage {
    fn input_total(self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }
}

pub(super) fn gather_hook_enrichments(
    context: &CliContext,
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
) -> HookEnrichments {
    let mut enrichments = HookEnrichments {
        token_usage: None,
        workspace: None,
    };
    if spec.key != "claude" {
        return enrichments;
    }
    if event == "session-start" {
        enrichments.workspace = hook_workspace_context(context);
    }
    if event == "prompt-submit" {
        if let Some(path) =
            extract_first_string_like(payload, &["transcript_path", "transcriptPath"])
        {
            enrichments.token_usage = read_token_usage_from_transcript(Path::new(&path));
        }
    }
    enrichments
}

pub(super) fn hook_workspace_context(context: &CliContext) -> Option<HookWorkspaceContext> {
    let workspace_id = trimmed_env("FORKTTY_WORKSPACE_ID")?;
    if !should_send_hook_actions(context) {
        return None;
    }
    let workspaces = send_socket_request_with_timeout(
        &context.socket_path,
        "workspace.list",
        json!({}),
        HOOK_STATUS_TIMEOUT,
    )
    .ok()?;
    workspaces.as_array()?.iter().find_map(|workspace| {
        if string_field(workspace, "id") != Some(workspace_id.as_str()) {
            return None;
        }
        let name = safe_string_field(workspace, "name")?;
        Some(HookWorkspaceContext {
            name,
            git_branch: safe_string_field(workspace, "gitBranch")
                .or_else(|| safe_string_field(workspace, "git_branch"))
                .filter(|branch| !branch.trim().is_empty()),
        })
    })
}

pub(super) fn build_token_progress_action(
    spec: &AgentSpec,
    enrichments: &HookEnrichments,
    event: &str,
    order: &str,
) -> Option<Value> {
    let usage = enrichments.token_usage?;
    let total = usage.input_total();
    if total == 0 {
        return None;
    }
    let mut params = hook_target_params();
    params.insert(
        "key".to_string(),
        Value::String(format!("agent:{}:tokens", spec.key)),
    );
    params.insert(
        "label".to_string(),
        Value::String(format!("{} input tokens", spec.label)),
    );
    params.insert("value".to_string(), json!(total));
    params.insert("total".to_string(), json!(resolve_token_ceiling()));
    Some(add_hook_metadata(params, spec, event, &Value::Null, order))
}

pub(super) fn build_hook_response(
    spec: &AgentSpec,
    event: &str,
    enrichments: &HookEnrichments,
) -> CliResult<Value> {
    if spec.key == "claude" && event == "session-start" {
        let workspace_id =
            trimmed_env("FORKTTY_WORKSPACE_ID").unwrap_or_else(|| "(none)".to_string());
        let workspace_line = enrichments
            .workspace
            .as_ref()
            .map(|workspace| {
                format!(
                    "Workspace: {}{}.",
                    workspace.name,
                    workspace
                        .git_branch
                        .as_deref()
                        .map(|branch| format!(" on branch {branch}"))
                        .unwrap_or_default()
                )
            })
            .unwrap_or_else(|| format!("Workspace: {workspace_id}; branch unknown."));
        let mut context_lines = vec![
            format!(
                "Running inside ForkTTY. workspace_id={} surface_id={} socket={}.",
                workspace_id,
                trimmed_env("FORKTTY_SURFACE_ID").unwrap_or_else(|| "(none)".to_string()),
                trimmed_env("FORKTTY_SOCKET_PATH").unwrap_or_else(|| "(default)".to_string()),
            ),
            workspace_line,
            "MCP tools: context_snapshot gives a compact read-only view; workspace_list, surface_list, topology_tree, surface_read_text, and surface_capture_tail inspect panes; surface_focus and surface_send_text drive them.".to_string(),
            "Worktrees: worktree_create creates an isolated git worktree + workspace; worktree_attach, worktree_remove, and worktree_merge manage branches.".to_string(),
            "Status: status_set and notification_create publish progress; CLI fallback is forktty list/surfaces/send-text/worktree-*.".to_string(),
        ];
        context_lines.extend(
            agent_guide::session_context_lines()
                .into_iter()
                .map(str::to_string),
        );
        let additional_context = context_lines.join("\n");
        let mut hook_output = Map::new();
        hook_output.insert(
            "hookEventName".to_string(),
            Value::String("SessionStart".to_string()),
        );
        hook_output.insert(
            "additionalContext".to_string(),
            Value::String(additional_context),
        );
        if trimmed_env("FORKTTY_WORKSPACE_ID").is_some() {
            if let Some(workspace) = &enrichments.workspace {
                hook_output.insert(
                    "sessionTitle".to_string(),
                    Value::String(workspace.name.clone()),
                );
            }
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": Value::Object(hook_output),
        }));
    }
    if spec.key == "claude" && event == "prompt-submit" {
        let mut sections = Vec::new();
        if let Some(usage) = enrichments.token_usage {
            sections.push(format_token_usage_block(usage));
        }
        if sections.is_empty() {
            return serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into);
        }
        return Ok(json!({
            "continue": true,
            "suppressOutput": false,
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": sections.join("\n\n"),
            }
        }));
    }
    // Antigravity unmarshals hook stdout with strict protojson and rejects
    // unknown fields ("continue" included), logging the hook as failed. Tool
    // hooks are gating hooks and need an explicit allow decision; non-gating
    // events can use an empty object as a no-op response.
    if spec.key == "antigravity" {
        if event == "pre-tool" {
            return Ok(json!({ "decision": "approve" }));
        }
        return Ok(json!({}));
    }
    serde_json::from_str(HOOK_CONTINUE_JSON.trim()).map_err(Into::into)
}

pub(super) fn extract_hook_message(payload: &Value) -> String {
    extract_first_string(
        payload,
        &[
            "message",
            "body",
            "reason",
            "error",
            "summary",
            "detail",
            "title",
            "text",
            "last_assistant_message",
        ],
    )
    .unwrap_or_default()
}

pub(super) fn extract_hook_source(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["source", "trigger", "reason"])
        .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

pub(super) fn extract_hook_compact_trigger(payload: &Value) -> Option<String> {
    extract_first_string_like(
        payload,
        &["trigger", "compact_trigger", "compactTrigger", "reason"],
    )
    .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

pub(super) fn extract_hook_tool_name(payload: &Value) -> Option<String> {
    let sanitized = sanitize_for_terminal(&extract_first_string_like(
        payload,
        &["tool_name", "toolName", "tool", "name"],
    )?);
    if sanitized.chars().count() <= HOOK_TOOL_LABEL_MAX {
        Some(sanitized)
    } else {
        Some(format!(
            "{}...",
            sanitized
                .chars()
                .take(HOOK_TOOL_LABEL_MAX.saturating_sub(3))
                .collect::<String>()
        ))
    }
}

pub(super) fn extract_hook_tool_error(payload: &Value) -> bool {
    // Inspect only the documented error container — the tool result object
    // (`tool_response`) and, as a fallback, the payload root — one level deep.
    //
    // A previous version walked the *entire* payload recursively and flagged an
    // error on any `error`/`is_error`/`isError` key anywhere inside it. Codex
    // PostToolUse payloads carry rich, nested tool output (e.g. MCP
    // `structuredContent`, JSON-emitting commands) that legitimately contains
    // nested `error` keys even on success, so the recursive scan produced
    // spurious "error" log lines and notifications on routine Codex use. Both
    // the Claude (`tool_response.is_error`) and MCP (`tool_response.isError`)
    // contracts expose the flag at the top of the response, so a single-level
    // check is sufficient and far less noisy.
    [payload.get("tool_response"), Some(payload)]
        .into_iter()
        .flatten()
        .any(object_signals_tool_error)
}

pub(super) fn object_signals_tool_error(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    for key in ["is_error", "isError", "error"] {
        match object.get(key) {
            Some(Value::Bool(true)) => return true,
            Some(Value::String(value)) if !value.trim().is_empty() => return true,
            Some(Value::Object(value))
                if value.contains_key("message")
                    || value.contains_key("type")
                    || value.contains_key("code") =>
            {
                return true
            }
            _ => {}
        }
    }
    false
}

pub(super) fn extract_hook_permission_mode(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["permission_mode", "permissionMode"])
        .map(|value| {
            sanitize_for_terminal(&value)
                .chars()
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
}

pub(super) fn extract_hook_session_id(payload: &Value) -> Option<String> {
    // conversationId is Antigravity's session identifier.
    extract_first_string_like(
        payload,
        &[
            "session_id",
            "sessionId",
            "sessionID",
            "conversationId",
            "conversation_id",
        ],
    )
    .map(|value| {
        sanitize_for_terminal(&value)
            .chars()
            .take(96)
            .collect::<String>()
    })
    .filter(|value| !value.is_empty())
}

pub(super) fn extract_hook_turn_id(event: &str, payload: &Value) -> Option<String> {
    let explicit = extract_first_string_like(
        payload,
        &[
            "turn_id",
            "turnId",
            "prompt_id",
            "promptId",
            "request_id",
            "requestId",
            "message_id",
            "messageId",
            "event_id",
            "eventId",
            "sequence",
            "seq",
        ],
    );
    if let Some(explicit) = explicit {
        return Some(format!("id:{}", short_hash(&explicit)));
    }
    if event != "prompt-submit" {
        return None;
    }
    extract_first_string_like(payload, &["prompt", "message", "text", "body"])
        .map(|prompt| format!("prompt:{}", short_hash(&prompt)))
}

pub(super) fn extract_first_string(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            if let Some(value) = object.get(*key).and_then(Value::as_str) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

pub(super) fn extract_first_string_like(payload: &Value, keys: &[&str]) -> Option<String> {
    let mut queue = VecDeque::from([payload]);
    while let Some(current) = queue.pop_front() {
        let Some(object) = current.as_object() else {
            continue;
        };
        for key in keys {
            match object.get(*key) {
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    return Some(value.trim().to_string())
                }
                Some(Value::Number(value)) => return Some(value.to_string()),
                Some(Value::Bool(value)) => return Some(value.to_string()),
                _ => {}
            }
        }
        for value in object.values() {
            if value.is_object() {
                queue.push_back(value);
            }
        }
    }
    None
}

pub(super) fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

pub(super) fn read_token_usage_from_transcript(path: &Path) -> Option<TokenUsage> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let size = metadata.len();
    if size == 0 {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let chunk_size = size.min(64 * 1024);
    file.seek(SeekFrom::Start(size - chunk_size)).ok()?;
    let mut buffer = vec![0; chunk_size as usize];
    file.read_exact(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer);
    for raw in text.lines().rev() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(usage) = entry
            .get("message")
            .and_then(|message| message.get("usage"))
            .or_else(|| entry.get("usage"))
        else {
            continue;
        };
        return Some(TokenUsage {
            input: usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output: usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_read: usage
                .get("cache_read_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cache_creation: usage
                .get("cache_creation_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        });
    }
    None
}

pub(super) fn resolve_token_ceiling() -> u64 {
    trimmed_env("FORKTTY_HOOK_TOKEN_CEILING")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(HOOK_TOKEN_CEILING_DEFAULT)
}

pub(super) fn format_token_usage_block(usage: TokenUsage) -> String {
    let total = usage.input_total();
    let ceiling = resolve_token_ceiling();
    let pct = if ceiling > 0 {
        ((total as f64 / ceiling as f64) * 100.0).round().min(100.0) as u64
    } else {
        0
    };
    format!(
        "ForkTTY token estimate (latest assistant turn): ~{} / {} input tokens ({}% — input={}, cache_read={}, cache_creation={}, output={}).",
        format_thousands(total),
        format_thousands(ceiling),
        pct,
        usage.input,
        usage.cache_read,
        usage.cache_creation,
        usage.output,
    )
}

pub(super) fn format_thousands(value: u64) -> String {
    let text = value.to_string();
    let mut out = String::new();
    for (index, ch) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}
