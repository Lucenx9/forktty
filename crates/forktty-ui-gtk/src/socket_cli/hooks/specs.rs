//! Provider hook metadata shared by hook setup, events, doctor, and tests.

use super::super::MAX_STDIN_TEXT_BYTES;
use super::install::{
    antigravity_config_path, claude_config_path, codex_config_path, legacy_gemini_config_path,
    opencode_plugin_path,
};
use std::path::PathBuf;
use std::time::Duration;

pub(in crate::socket_cli) const HOOK_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
pub(in crate::socket_cli) const HOOK_CONTINUE_JSON: &str =
    "{\"continue\":true,\"suppressOutput\":false}\n";
pub(in crate::socket_cli) const HOOK_EVENT_CLOCK: &str = "boottime-ns";
pub(in crate::socket_cli) const HOOK_EVENT_ORDER_PARAM: &str = "hook_event_order";
pub(in crate::socket_cli) const HOOK_TOOL_LABEL_MAX: usize = 48;
pub(in crate::socket_cli) const HOOK_TOKEN_CEILING_DEFAULT: u64 = 200_000;
pub(in crate::socket_cli) const FORKTTY_HOOK_TAG: &str = "forktty";
pub(in crate::socket_cli) const OPENCODE_PLUGIN_TAG: &str = "forktty-managed-opencode-plugin";

#[derive(Clone, Copy)]
pub(in crate::socket_cli) struct HookEntrySpec {
    pub(in crate::socket_cli) event_name: &'static str,
    pub(in crate::socket_cli) hook_event_name: &'static str,
    pub(in crate::socket_cli) timeout: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::socket_cli) enum HookInstallKind {
    JsonConfig,
    OpenCodePlugin,
    // Antigravity CLI executes the hook `command` as a bare executable path -
    // no argument splitting and no shell - so the JSON config points at
    // generated wrapper scripts that invoke the forktty launcher.
    AntigravityConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookSetupProfile {
    Lifecycle,
    Full,
}

#[derive(Clone, Copy)]
pub(in crate::socket_cli) struct AgentSpec {
    pub(in crate::socket_cli) key: &'static str,
    pub(in crate::socket_cli) label: &'static str,
    pub(in crate::socket_cli) disabled_env: &'static str,
    pub(in crate::socket_cli) config_path: fn() -> PathBuf,
    pub(in crate::socket_cli) hook_entries: &'static [HookEntrySpec],
    pub(in crate::socket_cli) retired_hook_entries: &'static [HookEntrySpec],
    pub(in crate::socket_cli) matcher: Option<&'static str>,
    pub(in crate::socket_cli) install_kind: HookInstallKind,
}

// Codex and Claude Code both treat the `timeout` field as seconds (Codex default 600s;
// Claude default 600s, 30s for UserPromptSubmit). The previous Codex value of 5000
// was a millisecond assumption that meant ~83 minutes; cap routine entries at 30s
// so a forktty hook never holds the agent loop for longer than a generous local
// round-trip while still leaving headroom over the socket request budget. Codex
// SessionEnd uses that provider event's three-second maximum.
pub(in crate::socket_cli) const HOOK_ENTRY_TIMEOUT_SECS: u64 = 30;
pub(in crate::socket_cli) const CODEX_SESSION_END_TIMEOUT_SECS: u64 = 3;

pub(in crate::socket_cli) const OPENCODE_HOOK_TIMEOUT_MS: u64 = HOOK_ENTRY_TIMEOUT_SECS * 1000;
pub(in crate::socket_cli) const OPENCODE_MAX_INPUT_BYTES: usize = MAX_STDIN_TEXT_BYTES;

// Gemini CLI is no longer an active ForkTTY integration target. Keep only
// enough legacy metadata to remove ForkTTY-managed config written by older
// releases from ~/.gemini/settings.json.
pub(in crate::socket_cli) const LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS: u64 =
    HOOK_ENTRY_TIMEOUT_SECS * 1000;

pub(in crate::socket_cli) const CODEX_HOOK_ENTRIES: &[HookEntrySpec] = &[
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
    HookEntrySpec {
        event_name: "SessionEnd",
        hook_event_name: "session-end",
        timeout: CODEX_SESSION_END_TIMEOUT_SECS,
    },
];

pub(in crate::socket_cli) const CLAUDE_HOOK_ENTRIES: &[HookEntrySpec] = &[
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
    // WorktreeCreate is deliberately omitted: Claude Code treats it as a
    // provider hook that replaces default git worktree creation and requires the
    // hook to print a worktree path on stdout (missing path fails creation).
    // Registering an observational hook there breaks `claude --worktree` and
    // `isolation: "worktree"` subagents. WorktreeRemove is advisory only.
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

// Retired entries are cleanup-only metadata: setup and remove delete prior
// ForkTTY-owned registrations without installing or reporting them as supported.
pub(in crate::socket_cli) const CLAUDE_RETIRED_HOOK_ENTRIES: &[HookEntrySpec] = &[HookEntrySpec {
    event_name: "WorktreeCreate",
    hook_event_name: "worktree-create",
    timeout: HOOK_ENTRY_TIMEOUT_SECS,
}];

pub(in crate::socket_cli) const CLAUDE_PER_TOOL_HOOK_ENTRIES: &[&str] =
    &["PreToolUse", "PostToolUse", "PostToolUseFailure"];

// Antigravity lifecycle hooks use flat handler objects, while tool hooks use
// matcher wrappers. The timeout field is unverified and never emitted.
pub(in crate::socket_cli) const ANTIGRAVITY_HOOK_ENTRIES: &[HookEntrySpec] = &[
    HookEntrySpec {
        event_name: "PreInvocation",
        hook_event_name: "before-model",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
    HookEntrySpec {
        event_name: "PostInvocation",
        hook_event_name: "after-model",
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
        event_name: "Stop",
        hook_event_name: "stop",
        timeout: HOOK_ENTRY_TIMEOUT_SECS,
    },
];

pub(in crate::socket_cli) const OPENCODE_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

pub(in crate::socket_cli) const LEGACY_GEMINI_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

pub(in crate::socket_cli) const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        key: "codex",
        label: "Codex",
        disabled_env: "FORKTTY_CODEX_HOOKS_DISABLED",
        config_path: codex_config_path,
        hook_entries: CODEX_HOOK_ENTRIES,
        retired_hook_entries: &[],
        matcher: None,
        install_kind: HookInstallKind::JsonConfig,
    },
    AgentSpec {
        key: "claude",
        label: "Claude",
        disabled_env: "FORKTTY_CLAUDE_HOOKS_DISABLED",
        config_path: claude_config_path,
        hook_entries: CLAUDE_HOOK_ENTRIES,
        retired_hook_entries: CLAUDE_RETIRED_HOOK_ENTRIES,
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
        retired_hook_entries: &[],
        install_kind: HookInstallKind::AntigravityConfig,
    },
    AgentSpec {
        key: "opencode",
        label: "OpenCode",
        disabled_env: "FORKTTY_OPENCODE_HOOKS_DISABLED",
        config_path: opencode_plugin_path,
        hook_entries: OPENCODE_HOOK_ENTRIES,
        retired_hook_entries: &[],
        matcher: None,
        install_kind: HookInstallKind::OpenCodePlugin,
    },
];

pub(in crate::socket_cli) const DEFAULT_HOOK_SETUP_AGENT_KEYS: &[&str] =
    &["codex", "claude", "antigravity", "opencode"];

pub(in crate::socket_cli) static LEGACY_GEMINI_HOOK_AGENT: AgentSpec = AgentSpec {
    key: "gemini",
    label: "Gemini",
    disabled_env: "FORKTTY_GEMINI_HOOKS_DISABLED",
    config_path: legacy_gemini_config_path,
    hook_entries: LEGACY_GEMINI_HOOK_ENTRIES,
    retired_hook_entries: &[],
    matcher: None,
    install_kind: HookInstallKind::JsonConfig,
};
