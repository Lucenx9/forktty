use crate::agent_guide;
use forktty_core::protocol_limits;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
#[cfg(test)]
use std::io::{BufRead, BufReader};
#[cfg(test)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod agent;
#[cfg(any(feature = "browser", test))]
mod browser;
mod feed;
mod remote;
mod status;
mod surface;
mod team;
mod transport;
mod workflow;
mod workspace;
mod worktree;

#[cfg(test)]
use agent::{
    format_agent_health_line, format_agent_hibernate_line, format_agent_reclaim_line,
    format_agent_reclaim_plan_line, format_agent_resume_line, format_agent_session_line,
};
use agent::{
    handle_agent_health, handle_agent_reclaim_plan, handle_agents, handle_hibernate_agent,
    handle_reclaim_agents, handle_resume_agent,
};
#[cfg(any(feature = "browser", test))]
use browser::handle_browser;
use feed::handle_feed;
use remote::{handle_remote_status, handle_remotes};
#[cfg(test)]
use status::{
    context_snapshot_params, format_context_snapshot_explain_line, format_notification_line,
    format_progress_line, format_status_line, format_status_summary_line,
};
use status::{
    handle_clear_logs, handle_clear_notifications, handle_clear_progress, handle_clear_status,
    handle_context_snapshot, handle_list_progress, handle_list_status, handle_log, handle_logs,
    handle_notifications, handle_set_progress, handle_set_status, handle_status, handle_statusline,
};
#[cfg(test)]
use surface::format_surface_line;
use surface::{
    handle_capture_tail, handle_close_surface, handle_focus_surface, handle_new_tab,
    handle_read_screen, handle_select_tab, handle_send_text, handle_split_surface, handle_surfaces,
    handle_top, handle_tree,
};
#[cfg(test)]
use team::{
    format_team_ask_flow_line, format_team_summary_line, format_team_worker_health_line,
    format_team_worker_launch_line,
};
use team::{
    handle_team, handle_team_events, handle_team_get, handle_team_inbox, handle_team_list,
    handle_team_message_ack, handle_team_message_dispatch, handle_team_message_send,
    handle_team_summary, handle_team_task_upsert, handle_team_upsert, handle_team_worker_health,
    handle_team_worker_heartbeat, handle_team_worker_launch, handle_team_worker_nudge,
    handle_team_worker_shutdown, handle_team_worker_upsert,
};
pub(crate) use transport::send_socket_request_with_timeout;
use transport::stream_events;
#[cfg(test)]
use transport::{
    connect_unix_stream_with_timeout, format_socket_connect_error, lagged_dropped_count,
    unix_socket_address,
};
use workflow::{
    handle_workflow_evidence_add, handle_workflow_get, handle_workflow_loop_set,
    handle_workflow_plan_set, handle_workflow_replay, handle_workflow_upsert, handle_workflows,
};
#[cfg(test)]
use workspace::format_workspace_line;
use workspace::{
    handle_close_workspace, handle_create_workspace, handle_focus, handle_list, handle_ssh,
};
use worktree::{
    handle_project_action_list, handle_project_action_run, handle_worktree_list,
    handle_worktree_merge, handle_worktree_open, handle_worktree_remove, handle_worktree_status,
};
const SOCKET_TIMEOUT: Duration = protocol_limits::OFFICIAL_SOCKET_TIMEOUT;
const DEFAULT_AGENT_WAIT_TIMEOUT_MS: u64 = 30_000;
const MAX_AGENT_WAIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_AGENT_WAIT_INTERVAL_MS: u64 = 250;
const MAX_AGENT_WAIT_INTERVAL_MS: u64 = 5_000;
const HOOK_STATUS_TIMEOUT: Duration = Duration::from_secs(5);
const HOOK_CONTINUE_JSON: &str = "{\"continue\":true,\"suppressOutput\":false}\n";
const HOOK_EVENT_CLOCK: &str = "boottime-ns";
const HOOK_EVENT_ORDER_PARAM: &str = "hook_event_order";
const HOOK_TOOL_LABEL_MAX: usize = 48;
const HOOK_TOKEN_CEILING_DEFAULT: u64 = 200_000;
const FORKTTY_HOOK_TAG: &str = "forktty";
const OPENCODE_PLUGIN_TAG: &str = "forktty-managed-opencode-plugin";
const AGENT_SKILL_NAME: &str = "forktty-agent-orchestration";
const AGENT_SKILL_MARKER: &str = "<!-- forktty-managed-agent-skill -->";
const AGENT_SKILL_MD: &str =
    include_str!("../../../.agents/skills/forktty-agent-orchestration/SKILL.md");
const AGENT_SKILL_OPENAI_YAML: &str =
    include_str!("../../../.agents/skills/forktty-agent-orchestration/agents/openai.yaml");
const MAX_HOOK_CONFIG_SIZE_BYTES: u64 = 1024 * 1024;
const MAX_AGENT_SKILL_FILE_BYTES: u64 = MAX_HOOK_CONFIG_SIZE_BYTES;
// MCP registration edits third-party files that grow on their own —
// ~/.claude.json in particular carries per-project state and routinely
// exceeds 1 MiB — so it gets a larger budget than ForkTTY-owned hook configs.
const MAX_MCP_CONFIG_SIZE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STDIN_TEXT_BYTES: usize = protocol_limits::CLI_STDIN_TEXT_MAX_BYTES;
const MAX_SOCKET_RESPONSE_BYTES: usize = protocol_limits::OFFICIAL_SOCKET_RESPONSE_MAX_BYTES;

static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

const HELP_TEXT: &str = "\
ForkTTY CLI

Usage:
  forktty list [--json]
  forktty create-workspace [--name <name>] [--working-dir <path>] [--json]
  forktty focus <selector>
  forktty focus --workspace-id <id>
  forktty close-workspace <selector>
  forktty notify [message] [--title <title>] [--kind <kind>]
  forktty surfaces [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty agents [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty agent-health [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty agent-reclaim-plan [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--min-idle-ms <ms>] [--json]
  forktty hibernate-agent [--surface-id <id>] [--min-idle-ms <ms>] [--json]
  forktty reclaim-agents [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--min-idle-ms <ms>] [--limit <n>] [--json]
  forktty resume-agent [--surface-id <id>] [--json]
  forktty teams [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--status <status>] [--query <text>] [--limit <n>] [--json]
  forktty team-get <team-id> [--json]
  forktty team-upsert <team-id> [--workspace-id <id>] [--leader-surface-id <id>] [--name <name>] [--status <status>] [--goal <text>] [--json]
  forktty team-worker-upsert <team-id> <worker-id> [--role <role>] [--agent <agent>] [--surface-id <id>] [--worktree-name <name>] [--status <status>] [--assigned-task-id <id>] [--json]
  forktty team-worker-heartbeat <team-id> <worker-id> [--status <status>] [--assigned-task-id <id>] [--json]
  forktty team-worker-launch <team-id> <worker-id> [--agent <agent>] [--role <role>] [--assigned-task-id <id>] [--worktree-name <name>] [--args <comma-list>] [--json]
  forktty team-worker-health <team-id> [--stale-after-ms <ms>] [--json]
  forktty team-worker-nudge <team-id> <worker-id> [--text <text>] [--json]
  forktty team-worker-shutdown <team-id> <worker-id> [--text <text>] [--no-submit] [--close] [--json]
  forktty team-task-upsert <team-id> <task-id> [--title <title>] [--status <status>] [--detail <text>] [--depends-on <comma-list>] [--assigned-worker-id <id>] [--json]
  forktty team-message-send <team-id> --from <id> --body <text> [--message-id <id>] [--to-worker-id <id>] [--task-id <id>] [--json]
  forktty team-message-dispatch <team-id> <message-id> [--worker-id <id>] [--submit] [--json]
  forktty team-message-ack <team-id> <message-id> [--worker-id <id>] [--json]
  forktty team-inbox <team-id> [--worker-id <id>] [--include-delivered] [--limit <n>] [--json]
  forktty team-summary <team-id> [--json]
  forktty team-events [--team-id <id>] [--since-seq <n>] [--limit <n>] [--json]
  forktty team ask <team-id> <worker-id> [--agent <agent>] --task-id <id> --prompt <text>
  forktty team review <team-id> <worker-id> [--agent <agent>] --task-id <id> [--commit <rev>]
  forktty team watch <team-id> [--stale-after-ms <ms>] [--limit <n>] [--json]
  forktty team finish <team-id> [--dry-run] [--close-workers] [--force] [--json]
  forktty split-surface [--surface-id <id>] [--axis horizontal|vertical]
  forktty focus-surface <surface-id>
  forktty close-surface <surface-id>
  forktty new-tab [--surface-id <id>]
  forktty select-tab <surface-id>
  forktty send-text <text> [--surface-id <id>]
  forktty read-screen [--surface-id <id>] [--scope visible|all] [--max-bytes <n>] [--json]
  forktty capture-tail [--surface-id <id>] [--lines <n>] [--max-bytes <n>] [--json]
  forktty tree [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty top [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty remotes [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty remote-status [--surface-id <id>|--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty worktree-list [--cwd <repo>]
  forktty worktree-status [--path <worktree>] [--cwd <worktree>]
  forktty worktree-create <branch> [--cwd <repo>]
  forktty worktree-attach <branch> [--cwd <repo>]
  forktty worktree-remove <branch-or-worktree> [--cwd <repo>]
  forktty worktree-merge <branch-or-worktree> [--cwd <repo>]
  forktty actions [--cwd <repo>] [--json]
  forktty action-run <id> [--cwd <repo>] [--json]
  forktty set-status --key <key> --value <value> [--label <label>] [--color <color>]
  forktty list-status [--workspace-id <id>]
  forktty clear-status [--key <key>]
  forktty set-progress --key <key> --value <number> [--label <label>] [--total <number>]
  forktty list-progress [--workspace-id <id>]
  forktty clear-progress [--key <key>]
  forktty statusline [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty status explain [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>|--surface-id <id>]
  forktty status watch [--count <n>] [--interval-ms <ms>] [workspace selectors]
  forktty context-snapshot [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--include-team-details] [--include-workflow-details] [--include-feed-trace]
  forktty feed [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--limit <n>] [--json]
  forktty feed respond <approval-id> --decision approve|deny [--json]
  forktty workflows [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--surface-id <id>] [--session-id <id>] [--query <text>] [--limit <n>] [--json]
  forktty workflow-get <workflow-id> [--json]
  forktty workflow-upsert [--workflow-id <id>] [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--surface-id <id>] [--agent <agent>] [--session-id <id>] [--mode <mode>] [--status <status>] [--goal <text>] [--memory <text>] [--json]
  forktty workflow-plan-set <workflow-id> --steps-json <json-array> [--json]
  forktty workflow-loop-set <workflow-id> [--recipe <text>] [--stage <text>] [--iteration <n>] [--max-iterations <n>] [--stop-reason <text>] [--gates-json <json-array>] [--json]
  forktty workflow-evidence-add <workflow-id> --kind <kind> --title <title> [--text <text>|--text-file <path>|--text-file -] [--evidence-id <id>] [--path <path>] [--json]
  forktty workflow-replay [--workflow-id <id>] [--query <text>] [--since-seq <n>] [--limit <n>] [--json]
  forktty log [message] [--message <message>] [--level info|warn|error]
  forktty logs [--workspace-id <id>]
  forktty clear-logs [--workspace-id <id>]
  forktty notifications [--json]
  forktty clear-notifications
  forktty hooks setup [--full] [codex] [claude] [antigravity] [opencode]
      default setup agents: codex, claude, antigravity, opencode
  forktty hooks remove [codex] [claude] [antigravity] [opencode] [gemini]
      gemini is legacy cleanup only; setup remains unsupported
  forktty hooks doctor codex
  forktty hooks test codex
  forktty hooks <agent> <event>
  forktty mcp                                      Run the ForkTTY MCP stdio server
  forktty mcp setup [codex] [claude] [antigravity]
      default setup agents: codex, claude, antigravity
  forktty mcp remove [codex] [claude] [antigravity] [gemini]
      gemini is legacy cleanup only; setup remains unsupported
  forktty skills setup [agents|codex|pi|claude]
      default setup targets: agents, claude; codex and pi alias the interoperable agents target
  forktty skills remove [agents|codex|pi|claude]
  forktty --json doctor                            Socket/hook doctor; needs a global flag before
                                                   `doctor` (bare `forktty doctor` runs the local doctor)
  forktty ping
  forktty identify [workspace selectors] [--surface-id <id>] [--json]
  forktty capabilities [--json]
  forktty wait agent-status [workspace selectors] [--surface-id <id>] --status <status> [--timeout-ms <ms>] [--interval-ms <ms>]
  forktty events [--no-replay]
  forktty examples
  forktty completions bash|zsh|fish
  forktty ssh <user@host>                          Open a new workspace running ssh <user@host>
  forktty ssh <user@host> [--name <name>] [--cwd <path>]
";

const TEAM_HELP_TEXT: &str = "\
ForkTTY team commands

High-level wrappers:
  forktty team ask <team-id> <worker-id> [--agent <agent>] --task-id <id> --prompt <text>
      Create/update the team, create the task, launch a fresh worker surface, assign the task, queue the prompt, and dispatch it.
      Omit --agent, or pass --agent auto, to use Settings > Agents team provider selection.
      Submit uses provider-aware terminal input; Claude gets text, a short settle, then Enter.
      Re-running ask/review launches another worker; use team-message-send + team-message-dispatch for follow-ups.
      Options: --agent <auto|codex|claude|pi|opencode|antigravity>, --role <role>, --title <title>, --goal <text>, --worktree-name <name>,
               --args <comma-list>, --submit[=true|false] (default: true; pass --submit=false to stage only).

  forktty team review <team-id> <worker-id> [--agent <agent>] --task-id <id> [--commit <rev>]
      Same flow as ask, with a read-only commit review prompt.
      Options: --agent <auto|codex|claude|pi|opencode|antigravity>, --role <role>, --worktree-name <name>, --args <comma-list>,
               --prompt-extra <text>, --submit[=true|false] (default: true; pass --submit=false to stage only).

  forktty team watch <team-id> [--stale-after-ms <ms>] [--limit <n>] [--include-delivered]
      Read team.summary, team.worker.health, team.inbox, and team.events together.

  forktty team finish <team-id> [--dry-run] [--close-workers] [--force]
      Verify team state, optionally close current-runtime launch-owned workers, and mark the team done.

Low-level aliases still exist:
  forktty teams | team-list | team:list | team.list
  forktty team-get | team:get | team.get
  forktty team-worker-launch | team.worker.launch
  forktty team-worker-shutdown --close closes current-runtime launch-owned disposable worker panes
  forktty team-message-send | team.message.send
  forktty team-message-dispatch | team.message.dispatch
      With --submit, uses provider-aware terminal input.
";

const STATUS_HELP_TEXT: &str = "\
ForkTTY status commands

  forktty status summary [workspace selectors]
      Alias for statusline/status.summary.

  forktty status explain [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--tail-max-bytes <n>]
      Read context.snapshot and explain agent status with session/source/age/readiness/cwd evidence plus risk flags.

  forktty status watch [workspace selectors] [--surface-id <id>] [--count <n>] [--interval-ms <ms>]
      Re-run status explain output. Omit --count to watch until interrupted; interval must be greater than 0.

  forktty context-snapshot [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--tail-max-bytes <n>] [--include-team-details] [--include-workflow-details] [--include-feed-trace]
      Direct CLI alias for the context.snapshot socket/MCP method.
";

const AGENT_HELP_TEXT: &str = "\
ForkTTY agent commands

  forktty agents [workspace selectors]
  forktty agent-health [workspace selectors]
  forktty agent-reclaim-plan [workspace selectors] [--min-idle-ms <ms>]
  forktty hibernate-agent [--surface-id <id>] [--min-idle-ms <ms>]
  forktty reclaim-agents [workspace selectors] [--min-idle-ms <ms>] [--limit <n>]
  forktty resume-agent [--surface-id <id>]
  forktty wait agent-status [workspace selectors] [--surface-id <id>] [--agent <agent>] --status <running|working|idle|done|needs_input|blocked|suspended|ended|closed|unknown>
";

const WORKFLOW_HELP_TEXT: &str = "\
ForkTTY workflow commands

  forktty workflows [workspace selectors] [--surface-id <id>] [--session-id <id>] [--query <text>]
  forktty workflow-get <workflow-id>
  forktty workflow-upsert [--workflow-id <id>] [workspace selectors] [--goal <text>] [--memory <text>]
  forktty workflow-plan-set <workflow-id> --steps-json <json-array>
  forktty workflow-loop-set <workflow-id> [--recipe <text>] [--stage <text>] [--iteration <n>] [--max-iterations <n>] [--stop-reason <text>] [--gates-json <json-array>]
  forktty workflow-evidence-add <workflow-id> --kind <kind> --title <title> [--text <text>|--text-file <path>|--text-file -]
  forktty workflow-replay [--workflow-id <id>] [--query <text>] [--since-seq <n>]
";

const EXAMPLES_TEXT: &str = "\
ForkTTY examples

  forktty status explain --tail-lines 20
  forktty identify --json
  forktty wait agent-status --status needs_input --timeout-ms 30000
  forktty context-snapshot --workspace-name main --tail-lines 0 --json
  forktty team ask review-team review-worker --task-id review-head --prompt \"Review HEAD read-only\" --submit
  forktty team review review-team review-worker --task-id review-head --commit HEAD --submit
  forktty team watch review-team --stale-after-ms 120000 --limit 10
  forktty team finish review-team
  forktty workflows --query release --limit 5
  forktty workflow-loop-set loop-runtime --stage verify --iteration 2 --max-iterations 4
";

// Curated ergonomic command set, not every low-level socket alias.
const COMPLETION_COMMANDS: &[&str] = &[
    "list",
    "surfaces",
    "agents",
    "agent-health",
    "teams",
    "team",
    "team-get",
    "team-worker-launch",
    "team-worker-health",
    "team-message-send",
    "team-message-dispatch",
    "team-summary",
    "status",
    "statusline",
    "context-snapshot",
    "identify",
    "wait",
    "feed",
    "workflows",
    "workflow-get",
    "workflow-upsert",
    "workflow-loop-set",
    "tree",
    "top",
    "events",
    "capabilities",
    "examples",
    "completions",
    "help",
];

const TEAM_SUBCOMMANDS: &[&str] = &["ask", "review", "watch", "finish", "list", "get", "summary"];
const STATUS_SUBCOMMANDS: &[&str] = &["summary", "explain", "watch"];

#[cfg(feature = "browser")]
const BROWSER_HELP_TEXT: &str = "\
  forktty browser open [--workspace-id <id>] [--axis horizontal|vertical] [--profile <id|name>] <url>
  forktty browser navigate [<surface-id>] <url>
  forktty browser snapshot <surface-id>            Dump the page accessibility tree (JSON)
  forktty browser click <surface-id> <ref>         Click the element with the given snapshot ref
  forktty browser fill <surface-id> <ref> [<value>|--value-file <path>|--value-file -]
                                                   Set an input's value; prefer --value-file - for secrets
  forktty browser back <surface-id>                Navigate back in history
  forktty browser forward <surface-id>             Navigate forward in history
  forktty browser reload <surface-id>              Reload the current page
  forktty browser profile list                     List browser profiles
  forktty browser profile create <name>            Create a new browser profile with the given display name
  forktty browser profile delete <id>              Delete a browser profile by id
  forktty browser history list [--profile <id|name>] [--limit <n>]
  forktty browser history search <query> [--profile <id|name>] [--limit <n>]
  forktty browser history clear [--profile <id|name>]
  forktty browser bookmark add <url> [--title <t>] [--profile <id|name>]
  forktty browser bookmark list [--profile <id|name>]
  forktty browser bookmark remove <url> [--profile <id|name>]
";

fn print_help() {
    print!("{HELP_TEXT}");
    #[cfg(feature = "browser")]
    print!("{BROWSER_HELP_TEXT}");
}

#[derive(Debug)]
pub(crate) struct CliError {
    pub(crate) message: String,
    pub(crate) code: Option<String>,
    pub(crate) exit: i32,
}

impl CliError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            exit: 1,
        }
    }

    pub(crate) fn code(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: Some(code.into()),
            exit: 1,
        }
    }
}

impl From<io::Error> for CliError {
    fn from(err: io::Error) -> Self {
        CliError::new(err.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(err: serde_json::Error) -> Self {
        CliError::new(err.to_string())
    }
}

pub(crate) type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Default)]
struct GlobalArgs {
    args: Vec<String>,
    json: bool,
    socket_path: PathBuf,
    socket_explicit: bool,
    help: bool,
    verbose: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FlagValue {
    Bool,
    String(String),
}

#[derive(Debug, Default)]
struct ParsedFlags {
    options: BTreeMap<String, FlagValue>,
    positionals: Vec<String>,
}

struct CliContext {
    json: bool,
    socket_path: PathBuf,
    socket_explicit: bool,
    verbose: bool,
}

#[derive(Clone, Copy)]
struct HookEntrySpec {
    event_name: &'static str,
    hook_event_name: &'static str,
    timeout: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookInstallKind {
    JsonConfig,
    OpenCodePlugin,
    // Antigravity CLI executes the hook `command` as a bare executable path —
    // no argument splitting and no shell — so the JSON config points at
    // generated wrapper scripts that invoke the forktty launcher.
    AntigravityConfig,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HookSetupProfile {
    Lifecycle,
    Full,
}

#[derive(Clone, Copy)]
struct AgentSpec {
    key: &'static str,
    label: &'static str,
    disabled_env: &'static str,
    config_path: fn() -> PathBuf,
    hook_entries: &'static [HookEntrySpec],
    matcher: Option<&'static str>,
    install_kind: HookInstallKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum McpConfigKind {
    CodexToml,
    JsonMcpServers,
}

#[derive(Clone, Copy)]
struct McpAgentSpec {
    key: &'static str,
    label: &'static str,
    config_path: fn() -> PathBuf,
    config_kind: McpConfigKind,
}

#[derive(Clone, Copy)]
struct SkillTargetSpec {
    key: &'static str,
    label: &'static str,
    skill_dir: fn() -> PathBuf,
}

// Codex and Claude Code both treat the `timeout` field as seconds (Codex default 600s;
// Claude default 600s, 30s for UserPromptSubmit). The previous Codex value of 5000
// was a millisecond assumption that meant ~83 minutes; cap at 30s for both providers
// so a forktty hook never holds the agent loop for longer than a generous local
// round-trip while still leaving headroom over the socket request budget.
const HOOK_ENTRY_TIMEOUT_SECS: u64 = 30;

const OPENCODE_HOOK_TIMEOUT_MS: u64 = HOOK_ENTRY_TIMEOUT_SECS * 1000;
const OPENCODE_MAX_INPUT_BYTES: usize = MAX_STDIN_TEXT_BYTES;

// Gemini CLI is no longer an active ForkTTY integration target. Keep only
// enough legacy metadata to remove ForkTTY-managed config written by older
// releases from ~/.gemini/settings.json.
const LEGACY_GEMINI_HOOK_ENTRY_TIMEOUT_MS: u64 = HOOK_ENTRY_TIMEOUT_SECS * 1000;

const CODEX_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

const CLAUDE_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

const CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES: &[HookEntrySpec] = &[
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
const ANTIGRAVITY_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

const OPENCODE_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

const LEGACY_GEMINI_HOOK_ENTRIES: &[HookEntrySpec] = &[
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

const AGENTS: &[AgentSpec] = &[
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

const DEFAULT_HOOK_SETUP_AGENT_KEYS: &[&str] = &["codex", "claude", "antigravity", "opencode"];

static LEGACY_GEMINI_HOOK_AGENT: AgentSpec = AgentSpec {
    key: "gemini",
    label: "Gemini",
    disabled_env: "FORKTTY_GEMINI_HOOKS_DISABLED",
    config_path: legacy_gemini_config_path,
    hook_entries: LEGACY_GEMINI_HOOK_ENTRIES,
    matcher: None,
    install_kind: HookInstallKind::JsonConfig,
};

const MCP_AGENTS: &[McpAgentSpec] = &[
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

const SKILL_TARGETS: &[SkillTargetSpec] = &[
    SkillTargetSpec {
        key: "agents",
        label: "Agent Skills",
        skill_dir: agent_skills_dir,
    },
    SkillTargetSpec {
        key: "claude",
        label: "Claude",
        skill_dir: claude_skill_dir,
    },
];

const DEFAULT_SKILL_SETUP_TARGET_KEYS: &[&str] = &["agents", "claude"];

pub fn run(args: Vec<OsString>) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{}", sanitize_for_terminal(&err.message));
            err.exit
        }
    }
}

fn run_inner(args: Vec<OsString>) -> CliResult<()> {
    let argv = args
        .into_iter()
        .map(|arg| {
            arg.into_string()
                .map_err(|_| CliError::new("forktty: non-UTF-8 arguments are not supported"))
        })
        .collect::<CliResult<Vec<_>>>()?;
    let parsed = parse_global_args(argv)?;
    let mut args = parsed.args;
    if parsed.help || args.is_empty() {
        print_help();
        return Ok(());
    }
    let command = args.remove(0);
    let context = CliContext {
        json: parsed.json,
        socket_path: parsed.socket_path,
        socket_explicit: parsed.socket_explicit,
        verbose: parsed.verbose,
    };

    match command.as_str() {
        "list" => handle_list(&context, args),
        "create-workspace" => handle_create_workspace(&context, args),
        "focus" => handle_focus(&context, args),
        "close-workspace" => handle_close_workspace(&context, args),
        "notify" => handle_notify(&context, args),
        "surfaces" | "surface-list" | "surface:list" => handle_surfaces(&context, args),
        "agents" | "agent-list" | "agent:list" => handle_agents(&context, args),
        "agent-health" | "agent:health" => handle_agent_health(&context, args),
        "agent-reclaim-plan" | "agent:reclaim-plan" | "agent.reclaim.plan" => {
            handle_agent_reclaim_plan(&context, args)
        }
        "hibernate-agent" | "agent-hibernate" | "agent:hibernate" | "agent.hibernate" => {
            handle_hibernate_agent(&context, args)
        }
        "reclaim-agents" | "agent-reclaim" | "agent:reclaim" | "agent.reclaim" => {
            handle_reclaim_agents(&context, args)
        }
        "resume-agent" | "agent-resume" | "agent:resume" => handle_resume_agent(&context, args),
        "team" => handle_team(&context, args),
        "teams" | "team-list" | "team:list" | "team.list" => handle_team_list(&context, args),
        "team-get" | "team:get" | "team.get" => handle_team_get(&context, args),
        "team-upsert" | "team:upsert" | "team.upsert" => handle_team_upsert(&context, args),
        "team-worker-upsert" | "team:worker-upsert" | "team.worker.upsert" => {
            handle_team_worker_upsert(&context, args)
        }
        "team-worker-heartbeat" | "team:worker-heartbeat" | "team.worker.heartbeat" => {
            handle_team_worker_heartbeat(&context, args)
        }
        "team-worker-launch" | "team:worker-launch" | "team.worker.launch" => {
            handle_team_worker_launch(&context, args)
        }
        "team-worker-health" | "team:worker-health" | "team.worker.health" => {
            handle_team_worker_health(&context, args)
        }
        "team-worker-nudge" | "team:worker-nudge" | "team.worker.nudge" => {
            handle_team_worker_nudge(&context, args)
        }
        "team-worker-shutdown" | "team:worker-shutdown" | "team.worker.shutdown" => {
            handle_team_worker_shutdown(&context, args)
        }
        "team-task-upsert" | "team:task-upsert" | "team.task.upsert" => {
            handle_team_task_upsert(&context, args)
        }
        "team-message-send" | "team:message-send" | "team.message.send" => {
            handle_team_message_send(&context, args)
        }
        "team-message-dispatch" | "team:message-dispatch" | "team.message.dispatch" => {
            handle_team_message_dispatch(&context, args)
        }
        "team-message-ack" | "team:message-ack" | "team.message.ack" => {
            handle_team_message_ack(&context, args)
        }
        "team-inbox" | "team:inbox" | "team.inbox" => handle_team_inbox(&context, args),
        "team-summary" | "team:summary" | "team.summary" => handle_team_summary(&context, args),
        "team-events" | "team:events" | "team.events" => handle_team_events(&context, args),
        "split-surface" | "surface-split" | "surface:split" => handle_split_surface(&context, args),
        "focus-surface" | "surface-focus" | "surface:focus" => handle_focus_surface(&context, args),
        "close-surface" | "surface-close" | "surface:close" => handle_close_surface(&context, args),
        "new-tab" | "pane-new-tab" | "pane:new-tab" => handle_new_tab(&context, args),
        "select-tab" | "pane-select-tab" | "pane:select-tab" => handle_select_tab(&context, args),
        "send-text" | "send_text" => handle_send_text(&context, args),
        "read-screen" | "read_screen" | "surface-read-text" | "surface:read-text" => {
            handle_read_screen(&context, args)
        }
        "capture-tail" | "capture_tail" | "surface-capture-tail" | "surface:capture-tail" => {
            handle_capture_tail(&context, args)
        }
        "tree" | "topology-tree" | "topology:tree" | "topology.tree" => handle_tree(&context, args),
        "top" => handle_top(&context, args),
        "remotes" | "remote-list" | "remote:list" | "remote.list" => handle_remotes(&context, args),
        "remote-status" | "remote:status" | "remote.status" => handle_remote_status(&context, args),
        "worktree-list" | "worktree:list" | "worktree.list" => handle_worktree_list(&context, args),
        "worktree-status" | "worktree:status" | "worktree.status" => {
            handle_worktree_status(&context, args)
        }
        "worktree-create" | "worktree:create" | "worktree.create" => {
            handle_worktree_open(&context, args, "worktree.create", "worktree-create")
        }
        "worktree-attach" | "worktree:attach" | "worktree.attach" => {
            handle_worktree_open(&context, args, "worktree.attach", "worktree-attach")
        }
        "worktree-remove" | "worktree:remove" | "worktree.remove" => {
            handle_worktree_remove(&context, args)
        }
        "worktree-merge" | "worktree:merge" | "worktree.merge" => {
            handle_worktree_merge(&context, args)
        }
        "actions" | "project-actions" | "project:action:list" | "project.action.list" => {
            handle_project_action_list(&context, args)
        }
        "action-run" | "project-action-run" | "project:action:run" | "project.action.run" => {
            handle_project_action_run(&context, args)
        }
        "set-status" => handle_set_status(&context, args),
        "list-status" => handle_list_status(&context, args),
        "clear-status" => handle_clear_status(&context, args),
        "set-progress" => handle_set_progress(&context, args),
        "list-progress" => handle_list_progress(&context, args),
        "clear-progress" => handle_clear_progress(&context, args),
        "status" => handle_status(&context, args),
        "statusline" | "status-line" | "status:summary" => handle_statusline(&context, args),
        "context-snapshot" | "context_snapshot" | "context:snapshot" | "context.snapshot" => {
            handle_context_snapshot(&context, args)
        }
        "feed" | "feed-list" | "feed:list" => handle_feed(&context, args),
        "workflows" | "workflow-list" | "workflow:list" | "workflow.list" => {
            handle_workflows(&context, args)
        }
        "workflow-get" | "workflow:get" | "workflow.get" => handle_workflow_get(&context, args),
        "workflow-upsert" | "workflow:upsert" | "workflow.upsert" => {
            handle_workflow_upsert(&context, args)
        }
        "workflow-loop-set" | "workflow:loop:set" | "workflow.loop.set" | "loop-set" => {
            handle_workflow_loop_set(&context, args)
        }
        "workflow-plan-set" | "workflow:plan-set" | "workflow.plan.set" => {
            handle_workflow_plan_set(&context, args)
        }
        "workflow-evidence-add" | "workflow:evidence-add" | "workflow.evidence.add" => {
            handle_workflow_evidence_add(&context, args)
        }
        "workflow-replay" | "workflow:replay" | "workflow.replay" => {
            handle_workflow_replay(&context, args)
        }
        "log" => handle_log(&context, args),
        "logs" | "list-logs" => handle_logs(&context, args),
        "clear-logs" => handle_clear_logs(&context, args),
        "notifications" => handle_notifications(&context, args),
        "clear-notifications" | "notifications-clear" | "notification:clear" => {
            handle_clear_notifications(&context, args)
        }
        "hooks" => handle_hooks(&context, args),
        "mcp" => handle_mcp(&context, args),
        "skills" | "skill" => handle_skills(&context, args),
        "doctor" => handle_socket_doctor(&context, args),
        "ping" => handle_ping(&context, args),
        "identify" | "system-identify" | "system:identify" | "system.identify" => {
            handle_identify(&context, args)
        }
        "capabilities" => handle_capabilities(&context, args),
        "wait" => handle_wait(&context, args),
        "events" => handle_events(&context, args),
        "examples" => handle_examples(&context, args),
        "completion" | "completions" => handle_completions(&context, args),
        #[cfg(feature = "browser")]
        "browser" => handle_browser(&context, args),
        #[cfg(not(feature = "browser"))]
        "browser" => Err(CliError::new(
            "browser commands require building ForkTTY from source with --features browser",
        )),
        "ssh" => handle_ssh(&context, args),
        "help" => handle_help(&context, args),
        other => Err(CliError::new(format!("Unknown command: {other}"))),
    }
}

fn parse_global_args(argv: Vec<String>) -> CliResult<GlobalArgs> {
    let mut parsed = GlobalArgs {
        socket_path: default_socket_path(),
        ..GlobalArgs::default()
    };
    let mut stop_global_parsing = false;
    let mut index = 0;
    while index < argv.len() {
        let token = &argv[index];
        if !stop_global_parsing && token == "--" && !parsed.args.is_empty() {
            stop_global_parsing = true;
            parsed.args.push(token.clone());
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--json" {
            parsed.json = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && (token == "--verbose" || token == "--debug") {
            parsed.verbose = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--socket" {
            let Some(next) = argv.get(index + 1) else {
                return Err(CliError::new("--socket requires a value"));
            };
            if next.trim().is_empty() || next.starts_with("--") {
                return Err(CliError::new("--socket requires a value"));
            }
            parsed.socket_path = socket_path_from_argument(next.trim())?;
            parsed.socket_explicit = true;
            index += 2;
            continue;
        }
        if !stop_global_parsing && token.starts_with("--socket=") {
            let value = token.trim_start_matches("--socket=").trim();
            if value.is_empty() {
                return Err(CliError::new("--socket requires a value"));
            }
            parsed.socket_path = socket_path_from_argument(value)?;
            parsed.socket_explicit = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing && token == "--help" && parsed.args.is_empty() {
            parsed.help = true;
            index += 1;
            continue;
        }
        if !stop_global_parsing
            && parsed.args.is_empty()
            && token.starts_with("--")
            && token != "--"
        {
            return Err(CliError::new(format!("Unknown option: {token}")));
        }
        parsed.args.push(token.clone());
        index += 1;
    }
    Ok(parsed)
}

fn socket_path_from_argument(value: &str) -> CliResult<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(CliError::new("--socket requires an absolute path"))
    }
}

fn parse_flags(args: Vec<String>, boolean_options: &[&str]) -> ParsedFlags {
    let mut parsed = ParsedFlags::default();
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            parsed.positionals.extend(args[index + 1..].iter().cloned());
            break;
        }
        if !token.starts_with("--") {
            parsed.positionals.push(token.clone());
            index += 1;
            continue;
        }
        let raw = token.trim_start_matches("--");
        if let Some((key, value)) = raw.split_once('=') {
            parsed
                .options
                .insert(key.to_string(), FlagValue::String(value.to_string()));
            index += 1;
            continue;
        }
        if boolean_options.contains(&raw) {
            parsed.options.insert(raw.to_string(), FlagValue::Bool);
            index += 1;
            continue;
        }
        if args
            .get(index + 1)
            .is_some_and(|next| !next.starts_with("--"))
        {
            parsed
                .options
                .insert(raw.to_string(), FlagValue::String(args[index + 1].clone()));
            index += 2;
        } else {
            parsed.options.insert(raw.to_string(), FlagValue::Bool);
            index += 1;
        }
    }
    parsed
}

fn reject_unknown_options(
    options: &BTreeMap<String, FlagValue>,
    allowed: &[&str],
    command: &str,
) -> CliResult<()> {
    if options.contains_key("help") {
        // Usage is derived from the same allow-list the validation below
        // uses, so it cannot drift from the options actually accepted.
        let usage = if allowed.is_empty() {
            format!("usage: forktty {command} (no options)")
        } else {
            format!(
                "usage: forktty {command} [options]\noptions: {}",
                allowed
                    .iter()
                    .map(|flag| format!("--{flag}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(CliError {
            message: usage,
            code: None,
            exit: 0,
        });
    }
    for key in options.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(CliError::new(format!("{command}: unknown option --{key}")));
        }
    }
    Ok(())
}

fn require_no_args(args: &[String], command: &str) -> CliResult<()> {
    if let Some(arg) = args.first() {
        Err(CliError::new(format!(
            "{command}: unexpected argument{}",
            if arg.is_empty() {
                String::new()
            } else {
                format!(" {arg}")
            }
        )))
    } else {
        Ok(())
    }
}

fn string_option<'a>(
    options: &'a BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<&'a str>> {
    match options.get(key) {
        Some(FlagValue::String(value)) => Ok(Some(value)),
        Some(FlagValue::Bool) => Err(CliError::new(format!("{option_name} requires a value"))),
        None => Ok(None),
    }
}

fn non_blank_string_option<'a>(
    options: &'a BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<&'a str>> {
    match string_option(options, key, option_name)? {
        Some(value) if value.trim().is_empty() => {
            Err(CliError::new(format!("{option_name} requires a value")))
        }
        value => Ok(value),
    }
}

#[cfg(any(feature = "browser", test))]
fn required_non_blank_arg<'a>(arg: Option<&'a String>, message: &str) -> CliResult<&'a str> {
    let value = arg.ok_or_else(|| CliError::new(message))?;
    if value.trim().is_empty() {
        return Err(CliError::new(message));
    }
    Ok(value)
}

#[cfg(any(feature = "browser", test))]
fn required_trimmed_arg(arg: Option<&String>, message: &str) -> CliResult<String> {
    Ok(required_non_blank_arg(arg, message)?.trim().to_string())
}

fn parse_u64_option(
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<u64>> {
    let Some(raw) = non_blank_string_option(options, key, option_name)? else {
        return Ok(None);
    };
    raw.trim()
        .parse()
        .map(Some)
        .map_err(|_| CliError::new(format!("{option_name} must be a number")))
}

#[cfg(any(feature = "browser", test))]
fn insert_optional_trimmed_string_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
    param_name: &str,
) -> CliResult<()> {
    if let Some(value) = non_blank_string_option(options, key, option_name)? {
        params.insert(
            param_name.to_string(),
            Value::String(value.trim().to_string()),
        );
    }
    Ok(())
}

fn bool_option(options: &BTreeMap<String, FlagValue>, key: &str) -> Option<bool> {
    match options.get(key) {
        Some(FlagValue::Bool) => Some(true),
        Some(FlagValue::String(value)) if value == "true" => Some(true),
        Some(FlagValue::String(value)) if value == "false" => Some(false),
        Some(_) => None,
        None => Some(false),
    }
}

fn default_socket_path() -> PathBuf {
    socket_path_from_env().unwrap_or_else(forktty_socket::default_socket_path)
}

fn socket_path_from_env() -> Option<PathBuf> {
    let value = std::env::var("FORKTTY_SOCKET_PATH").ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    path.is_absolute().then_some(path)
}

fn next_request_id() -> String {
    format!(
        "cli-{}-{}",
        now_nanos(),
        NONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn next_file_nonce() -> String {
    format!(
        "{}-{}-{}",
        now_nanos(),
        std::process::id(),
        NONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn send_socket_request(socket_path: &Path, method: &str, params: Value) -> CliResult<Value> {
    send_socket_request_with_timeout(socket_path, method, params, SOCKET_TIMEOUT)
}

fn read_stdin_text() -> CliResult<String> {
    let mut stdin = io::stdin();
    if stdin.is_terminal() {
        return Ok(String::new());
    }
    read_text_from_reader(&mut stdin, MAX_STDIN_TEXT_BYTES, "stdin")
}

fn read_text_file_or_stdin(path: &str, label: &str) -> CliResult<String> {
    if path == "-" {
        return read_stdin_text();
    }
    let mut file = File::open(path)
        .map_err(|err| CliError::new(format!("failed to open {label} file {path}: {err}")))?;
    read_text_from_reader(&mut file, MAX_STDIN_TEXT_BYTES, label)
}

fn read_text_from_reader(
    reader: &mut impl Read,
    max_bytes: usize,
    source: &str,
) -> CliResult<String> {
    let mut bytes = Vec::new();
    let mut limited = reader.take(max_bytes as u64 + 1);
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(CliError::new(format!(
            "{source} exceeds {max_bytes} byte limit"
        )));
    }
    String::from_utf8(bytes).map_err(|err| CliError::new(err.to_string()))
}

fn read_optional_stdin_json() -> CliResult<Value> {
    let text = read_stdin_text()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(trimmed).or_else(|_| Ok(json!({ "raw": trimmed })))
}

/// Write one line of command output, treating a consumer that closed the pipe
/// (`forktty list --json | head -1`) as normal termination instead of a panic,
/// matching the `stream_events` convention.
fn write_output_line(out: &mut impl Write, text: &str) -> CliResult<()> {
    match writeln!(out, "{text}") {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn write_stdout_line(text: &str) -> CliResult<()> {
    write_output_line(&mut io::stdout().lock(), text)
}

fn write_stdout_text(text: &str) -> CliResult<()> {
    let mut out = io::stdout().lock();
    match out.write_all(text.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn print_json(value: &Value) -> CliResult<()> {
    write_stdout_line(&serde_json::to_string_pretty(value)?)
}

fn print_result_or_json(
    context: &CliContext,
    text: impl AsRef<str>,
    json_value: Value,
) -> CliResult<()> {
    if context.json {
        print_json(&json_value)
    } else {
        write_stdout_line(text.as_ref())
    }
}

fn handle_help(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    if args.is_empty() {
        print_help();
        return Ok(());
    }
    if args.len() > 1 {
        return Err(CliError::new(format!(
            "help: unexpected argument {}",
            args[1]
        )));
    }
    match args[0].as_str() {
        "team" | "teams" => write_stdout_text(TEAM_HELP_TEXT),
        "status" | "context" | "context-snapshot" => write_stdout_text(STATUS_HELP_TEXT),
        "agent" | "agents" => write_stdout_text(AGENT_HELP_TEXT),
        "workflow" | "workflows" => write_stdout_text(WORKFLOW_HELP_TEXT),
        "examples" => write_stdout_text(EXAMPLES_TEXT),
        other => Err(CliError::new(format!("help: unknown topic {other}"))),
    }
}

fn handle_examples(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "examples")?;
    write_stdout_text(EXAMPLES_TEXT)
}

fn handle_completions(_context: &CliContext, args: Vec<String>) -> CliResult<()> {
    if args.len() != 1 {
        return Err(CliError::new("completions requires bash, zsh, or fish"));
    }
    let script = completion_script(&args[0])?;
    write_stdout_text(&script)
}

fn completion_script(shell: &str) -> CliResult<String> {
    let commands = COMPLETION_COMMANDS.join(" ");
    let team_subcommands = TEAM_SUBCOMMANDS.join(" ");
    let status_subcommands = STATUS_SUBCOMMANDS.join(" ");
    Ok(match shell {
        "bash" => format!(
            r#"_forktty()
{{
    local cur prev
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    case "$prev" in
        team) COMPREPLY=( $(compgen -W "{team_subcommands}" -- "$cur") ); return 0 ;;
        status) COMPREPLY=( $(compgen -W "{status_subcommands}" -- "$cur") ); return 0 ;;
        completions) COMPREPLY=( $(compgen -W "bash zsh fish" -- "$cur") ); return 0 ;;
    esac
    COMPREPLY=( $(compgen -W "{commands}" -- "$cur") )
    return 0
}}
complete -F _forktty forktty
"#
        ),
        "zsh" => format!(
            r#"#compdef forktty
# Source this file directly, or install it as an fpath/autoloaded _forktty completion.
_forktty() {{
  local -a commands team_subcommands status_subcommands
  commands=({commands})
  team_subcommands=({team_subcommands})
  status_subcommands=({status_subcommands})
  if [[ $CURRENT -eq 2 ]]; then
    _describe 'forktty command' commands
  elif [[ $words[2] == team ]]; then
    _describe 'team subcommand' team_subcommands
  elif [[ $words[2] == status ]]; then
    _describe 'status subcommand' status_subcommands
  fi
}}
_forktty "$@"
"#
        ),
        "fish" => format!(
            r#"complete -c forktty -f -a "{commands}"
complete -c forktty -n "__fish_seen_subcommand_from team" -f -a "{team_subcommands}"
complete -c forktty -n "__fish_seen_subcommand_from status" -f -a "{status_subcommands}"
complete -c forktty -n "__fish_seen_subcommand_from completions" -f -a "bash zsh fish"
"#
        ),
        other => {
            return Err(CliError::new(format!(
                "unsupported completion shell {other}; expected bash, zsh, or fish"
            )));
        }
    })
}

#[cfg(test)]
fn completion_script_for_test(shell: &str) -> CliResult<String> {
    completion_script(shell)
}

fn handle_ping(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "ping")?;
    let result = send_socket_request(&context.socket_path, "system.ping", json!({}))?;
    if context.json {
        print_json(&json!({ "result": result }))
    } else {
        write_stdout_line(result.as_str().unwrap_or("pong"))
    }
}

fn handle_capabilities(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "capabilities")?;
    let result = send_socket_request(&context.socket_path, "system.capabilities", json!({}))?;
    if context.json {
        return print_json(&result);
    }
    for line in format_capabilities_lines(&result) {
        write_stdout_line(&line)?;
    }
    Ok(())
}

fn format_capabilities_lines(result: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(version) = string_field(result, "version") {
        lines.push(format!("version {version}"));
    }
    if let Some(methods) = result.get("methods").and_then(Value::as_array) {
        for method in methods {
            if let Some(name) = method.as_str() {
                lines.push(sanitize_for_terminal(name));
            }
        }
    }
    if let Some(policy) = result.get("team_provider_policy") {
        let default = safe_string_field(policy, "default_agent").unwrap_or_else(|| "auto".into());
        let order = string_array_field(policy, "provider_order").join(", ");
        let disabled = string_array_field(policy, "disabled_agents");
        let disabled = if disabled.is_empty() {
            "none".to_string()
        } else {
            disabled.join(", ")
        };
        let fallback = if policy
            .get("auto_fallback")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            "on"
        } else {
            "off"
        };
        lines.push(format!(
            "team providers default {default} fallback {fallback} order {order} disabled {disabled}"
        ));
    }
    if let Some(providers) = result
        .get("provider_capabilities")
        .and_then(Value::as_object)
    {
        let mut names = Vec::new();
        for name in ["codex", "claude", "pi", "opencode", "antigravity"] {
            if providers.contains_key(name) {
                names.push(name.to_string());
            }
        }
        for name in providers.keys() {
            if !names.iter().any(|known| known == name) {
                names.push(name.to_string());
            }
        }
        for name in names {
            let Some(provider) = providers.get(&name) else {
                continue;
            };
            let disabled = provider
                .get("disabled_by_config")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let available = provider
                .get("available_on_path")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let launchable = provider
                .get("launchable")
                .and_then(Value::as_bool)
                .unwrap_or(available);
            let configured = provider
                .get("configured_command")
                .and_then(Value::as_str)
                .is_some();
            let status = if disabled {
                "disabled"
            } else if launchable && configured {
                "configured"
            } else if launchable || available {
                "found"
            } else {
                "missing"
            };
            let detail = if disabled {
                safe_string_field(provider, "unavailable_reason")
                    .or_else(|| safe_string_field(provider, "program"))
            } else if launchable || available {
                safe_string_field(provider, "executable")
                    .or_else(|| safe_string_field(provider, "program"))
            } else {
                safe_string_field(provider, "unavailable_reason")
                    .or_else(|| safe_string_field(provider, "program"))
            };
            if let Some(detail) = detail.filter(|detail| !detail.is_empty()) {
                lines.push(format!("provider {name} {status} {detail}"));
            } else {
                lines.push(format!("provider {name} {status}"));
            }
        }
    }
    if let Some(pty) = result.get("pty_persistence") {
        let config_enabled = pty
            .get("config_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let active = pty.get("active").and_then(Value::as_bool).unwrap_or(false);
        let available = pty
            .get("available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let broker = safe_string_field(pty, "broker").unwrap_or_else(|| "dtach".to_string());
        let status = if active {
            "active"
        } else if config_enabled && !available {
            "configured-missing"
        } else if available {
            "available"
        } else {
            "off"
        };
        let detail = if active || available {
            safe_string_field(pty, "broker_executable").unwrap_or(broker)
        } else if config_enabled {
            safe_string_field(pty, "unavailable_reason")
                .unwrap_or_else(|| "broker_not_found".to_string())
        } else {
            broker
        };
        lines.push(format!("pty persistence {status} {detail}"));
    }
    lines
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(sanitize_for_terminal)
        .collect()
}

fn handle_identify(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let params = identify_params(args, "identify")?;
    let result = send_socket_request(
        &context.socket_path,
        "system.identify",
        Value::Object(params),
    )?;
    if context.json {
        return print_json(&result);
    }
    write_stdout_line(&format_identify_line(&result))
}

fn identify_params(args: Vec<String>, command: &str) -> CliResult<Map<String, Value>> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
        ],
        command,
    )?;
    require_no_args(&parsed.positionals, command)?;
    let mut params = workspace_surface_target_params_from_options(&parsed.options, command, false)?;
    insert_caller_env_params(&mut params);
    Ok(params)
}

fn handle_wait(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Err(CliError::new("wait requires subcommand agent-status"));
    };
    match subcommand.as_str() {
        "agent-status" | "agent_status" => handle_wait_agent_status(context, rest.to_vec()),
        other => Err(CliError::new(format!(
            "wait: unknown subcommand {other}; expected agent-status"
        ))),
    }
}

fn handle_wait_agent_status(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "agent",
            "status",
            "timeout-ms",
            "interval-ms",
        ],
        "wait agent-status",
    )?;
    require_no_args(&parsed.positionals, "wait agent-status")?;
    let mut snapshot_params =
        workspace_surface_target_params_from_options(&parsed.options, "wait agent-status", true)?;
    let status = non_blank_string_option(&parsed.options, "status", "--status")?
        .ok_or_else(|| CliError::new("wait agent-status requires --status"))?;
    let target = agent_wait_status_from_cli(status)?;
    let timeout_ms = agent_wait_timeout_ms_from_options(&parsed.options)?;
    let interval_ms = agent_wait_interval_ms_from_options(&parsed.options)?;
    let agent_filter = non_blank_string_option(&parsed.options, "agent", "--agent")?;
    let surface_filter = snapshot_params
        .get("surface_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    snapshot_params.insert("tail_lines".to_string(), json!(0));

    let started = Instant::now();
    loop {
        let elapsed_ms = cli_elapsed_ms(started);
        let snapshot = send_socket_request(
            &context.socket_path,
            "context.snapshot",
            Value::Object(snapshot_params.clone()),
        )?;
        let mut result = agent_wait_result_from_snapshot(
            &snapshot,
            target,
            status.trim(),
            surface_filter.as_deref(),
            agent_filter,
            elapsed_ms,
        );
        let matched = result
            .get("matched")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if matched || elapsed_ms >= timeout_ms {
            if !matched {
                if let Some(object) = result.as_object_mut() {
                    object.insert("timed_out".to_string(), Value::Bool(true));
                }
            }
            if context.json {
                print_json(&result)?;
            } else {
                write_stdout_line(&format_agent_wait_line(&result))?;
            }
            if !matched {
                return Err(CliError::code("wait agent-status timed out", "timeout"));
            }
            return Ok(());
        }
        let sleep_ms = interval_ms.min(timeout_ms.saturating_sub(elapsed_ms));
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

#[derive(Clone, Copy)]
enum AgentWaitStatus {
    Running,
    Idle,
    NeedsInput,
    Suspended,
    Ended,
    Unknown,
    Done,
}

impl AgentWaitStatus {
    fn matches(self, lifecycle: &str) -> bool {
        match self {
            Self::Running => lifecycle == "running",
            Self::Idle => lifecycle == "idle",
            Self::NeedsInput => lifecycle == "needs_input",
            Self::Suspended => lifecycle == "suspended",
            Self::Ended => lifecycle == "ended",
            Self::Unknown => lifecycle == "unknown",
            Self::Done => matches!(lifecycle, "idle" | "ended"),
        }
    }
}

fn agent_wait_status_from_cli(value: &str) -> CliResult<AgentWaitStatus> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "running" | "working" => Ok(AgentWaitStatus::Running),
        "idle" => Ok(AgentWaitStatus::Idle),
        "needs_input" | "blocked" => Ok(AgentWaitStatus::NeedsInput),
        "suspended" => Ok(AgentWaitStatus::Suspended),
        "ended" | "closed" => Ok(AgentWaitStatus::Ended),
        "unknown" => Ok(AgentWaitStatus::Unknown),
        "done" => Ok(AgentWaitStatus::Done),
        _ => Err(CliError::new(
            "wait agent-status: --status must be running, working, idle, done, needs_input, blocked, suspended, ended, closed, or unknown",
        )),
    }
}

fn agent_wait_timeout_ms_from_options(options: &BTreeMap<String, FlagValue>) -> CliResult<u64> {
    let timeout_ms = parse_u64_option(options, "timeout-ms", "--timeout-ms")?
        .unwrap_or(DEFAULT_AGENT_WAIT_TIMEOUT_MS);
    if timeout_ms > MAX_AGENT_WAIT_TIMEOUT_MS {
        return Err(CliError::new(format!(
            "wait agent-status: --timeout-ms must be 0..={MAX_AGENT_WAIT_TIMEOUT_MS}"
        )));
    }
    Ok(timeout_ms)
}

fn agent_wait_interval_ms_from_options(options: &BTreeMap<String, FlagValue>) -> CliResult<u64> {
    let interval_ms = parse_u64_option(options, "interval-ms", "--interval-ms")?
        .unwrap_or(DEFAULT_AGENT_WAIT_INTERVAL_MS);
    if interval_ms == 0 || interval_ms > MAX_AGENT_WAIT_INTERVAL_MS {
        return Err(CliError::new(format!(
            "wait agent-status: --interval-ms must be 1..={MAX_AGENT_WAIT_INTERVAL_MS}"
        )));
    }
    Ok(interval_ms)
}

fn agent_wait_result_from_snapshot(
    snapshot: &Value,
    target: AgentWaitStatus,
    requested_status: &str,
    surface_filter: Option<&str>,
    agent_filter: Option<&str>,
    elapsed_ms: u64,
) -> Value {
    let rows = snapshot
        .get("agent_health")
        .and_then(Value::as_array)
        .or_else(|| snapshot.get("agents").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();
    let mut observed = None;
    let mut matched = None;
    for row in rows.into_iter().filter(|row| {
        surface_filter.is_none_or(|surface_id| {
            row.get("surface_id").and_then(Value::as_str) == Some(surface_id)
        }) && agent_filter
            .is_none_or(|agent| row.get("agent").and_then(Value::as_str) == Some(agent))
    }) {
        if observed.is_none() {
            observed = Some(row.clone());
        }
        let lifecycle = row
            .get("lifecycle")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if target.matches(lifecycle) {
            matched = Some(row);
            break;
        }
    }
    let agent = matched.or(observed).unwrap_or(Value::Null);
    let matched = agent
        .get("lifecycle")
        .and_then(Value::as_str)
        .is_some_and(|lifecycle| target.matches(lifecycle));
    let workspace_id = snapshot
        .get("workspace")
        .and_then(|workspace| workspace.get("id"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let surface_id = agent
        .get("surface_id")
        .and_then(Value::as_str)
        .map(|value| value.to_string())
        .or_else(|| surface_filter.map(str::to_string));

    json!({
        "matched": matched,
        "timed_out": false,
        "status": requested_status,
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "agent": agent,
        "elapsed_ms": elapsed_ms,
    })
}

fn cli_elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn workspace_surface_target_params_from_options(
    options: &BTreeMap<String, FlagValue>,
    command: &str,
    default_from_env: bool,
) -> CliResult<Map<String, Value>> {
    let selectors = target_selector_values(options)?;
    if selectors.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: cannot combine {}",
            format_option_names(selectors.iter().map(|(option, _)| option.as_str()))
        )));
    }
    let mut params = Map::new();
    if let Some((_, (field, value))) = selectors.first() {
        let field = if *field == "worktreeName" {
            "worktree_name"
        } else {
            *field
        };
        params.insert(field.to_string(), Value::String(value.clone()));
    }
    insert_optional_cli_string_param(&mut params, options, "surface-id", "surface_id")?;
    if default_from_env && selectors.is_empty() && !params.contains_key("surface_id") {
        if !params.contains_key("workspace_id") {
            if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
                params.insert("workspace_id".to_string(), Value::String(workspace_id));
            }
        }
        if !params.contains_key("surface_id") {
            if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
                params.insert("surface_id".to_string(), Value::String(surface_id));
            }
        }
    }
    Ok(params)
}

fn insert_caller_env_params(params: &mut Map<String, Value>) {
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert(
            "caller_workspace_id".to_string(),
            Value::String(workspace_id),
        );
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("caller_surface_id".to_string(), Value::String(surface_id));
    }
}

fn format_identify_line(value: &Value) -> String {
    let workspace = value.get("workspace").unwrap_or(&Value::Null);
    let surface = value.get("surface").unwrap_or(&Value::Null);
    let caller = value.get("caller").unwrap_or(&Value::Null);
    let name = safe_string_field(workspace, "name").unwrap_or_else(|| "(workspace)".to_string());
    let workspace_id = safe_string_field(workspace, "id").unwrap_or_default();
    let surface_id = safe_string_field(surface, "id").unwrap_or_else(|| "(surface)".to_string());
    let cwd = safe_string_field(surface, "effective_project_cwd")
        .or_else(|| safe_string_field(workspace, "effective_project_cwd"))
        .unwrap_or_else(|| "(cwd unknown)".to_string());
    let mut parts = vec![format!(
        "{name} [{workspace_id}] surface {surface_id} cwd {cwd}"
    )];
    if let Some(agent) = value.get("agent").filter(|agent| agent.is_object()) {
        if let Some(provider) = safe_string_field(agent, "agent") {
            let lifecycle =
                safe_string_field(agent, "lifecycle").unwrap_or_else(|| "unknown".to_string());
            parts.push(format!("agent {provider}:{lifecycle}"));
        }
    }
    if caller
        .get("matches_surface")
        .and_then(Value::as_bool)
        .is_some_and(|matches| !matches)
    {
        parts.push("caller differs from target surface".to_string());
    }
    parts.join(" | ")
}

fn format_agent_wait_line(value: &Value) -> String {
    let matched = value
        .get("matched")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let timed_out = value
        .get("timed_out")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let elapsed = value.get("elapsed_ms").and_then(Value::as_u64).unwrap_or(0);
    let agent = value.get("agent").unwrap_or(&Value::Null);
    let provider = safe_string_field(agent, "agent").unwrap_or_else(|| "(agent)".to_string());
    let lifecycle = safe_string_field(agent, "lifecycle").unwrap_or_else(|| "unknown".to_string());
    if matched {
        format!("matched {provider}:{lifecycle} after {elapsed}ms")
    } else {
        debug_assert!(timed_out);
        format!("timed out after {elapsed}ms; last {provider}:{lifecycle}")
    }
}

fn handle_events(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let mut replay = true;
    for arg in &args {
        match arg.as_str() {
            "--no-replay" => replay = false,
            other => {
                return Err(CliError::new(format!(
                    "events: unexpected argument: {other}"
                )));
            }
        }
    }
    stream_events(&context.socket_path, replay)
}

fn target_selector_values(
    options: &BTreeMap<String, FlagValue>,
) -> CliResult<Vec<(String, (&'static str, String))>> {
    let mut out = Vec::new();
    for (option, field) in [
        ("workspace-id", "workspace_id"),
        ("workspace-name", "workspace_name"),
        ("worktree-name", "worktreeName"),
    ] {
        if let Some(value) = non_blank_string_option(options, option, &format!("--{option}"))? {
            out.push((option.to_string(), (field, value.trim().to_string())));
        }
    }
    Ok(out)
}

fn build_target_params(
    options: &BTreeMap<String, FlagValue>,
    command: &str,
) -> CliResult<Map<String, Value>> {
    let selectors = target_selector_values(options)?;
    if selectors.len() > 1 {
        return Err(CliError::new(format!(
            "{command}: cannot combine {}",
            format_option_names(selectors.iter().map(|(option, _)| option.as_str()))
        )));
    }
    let mut params = Map::new();
    if let Some((_, (field, value))) = selectors.first() {
        params.insert((*field).to_string(), Value::String(value.clone()));
    } else if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    Ok(params)
}

fn format_option_names<'a>(options: impl Iterator<Item = &'a str>) -> String {
    let formatted = options
        .map(|option| format!("--{option}"))
        .collect::<Vec<_>>();
    match formatted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [one, two] => format!("{one} and {two}"),
        _ => format!(
            "{}, and {}",
            formatted[..formatted.len() - 1].join(", "),
            formatted.last().unwrap()
        ),
    }
}

fn handle_notify(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &[]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "body",
            "kind",
            "title",
        ],
        "notify",
    )?;
    let stdin = if should_read_stdin(&parsed.options, &parsed.positionals, "body") {
        read_stdin_text()?
    } else {
        String::new()
    };
    let body = match string_option(&parsed.options, "body", "--body")? {
        Some(body) => body.to_string(),
        None if !parsed.positionals.is_empty() => parsed.positionals.join(" "),
        None => stdin.trim().to_string(),
    };
    let title = non_blank_string_option(&parsed.options, "title", "--title")?.unwrap_or("ForkTTY");
    let kind = non_blank_string_option(&parsed.options, "kind", "--kind")?.unwrap_or("info");
    if !matches!(kind, "prompt" | "error" | "info" | "custom") {
        return Err(CliError::new(format!("Invalid kind: {kind}")));
    }
    let mut params = build_target_params(&parsed.options, "notify")?;
    params.insert("title".to_string(), Value::String(title.to_string()));
    params.insert("body".to_string(), Value::String(body));
    params.insert("kind".to_string(), Value::String(kind.to_string()));
    send_socket_request(
        &context.socket_path,
        "notification.create",
        Value::Object(params),
    )?;
    print_result_or_json(
        context,
        format!("Sent {kind} notification"),
        json!({ "result": true }),
    )
}

fn should_read_stdin(
    options: &BTreeMap<String, FlagValue>,
    positionals: &[String],
    text_option: &str,
) -> bool {
    !matches!(options.get(text_option), Some(FlagValue::String(_))) && positionals.is_empty()
}

fn required_positionals(
    positionals: &[String],
    command: &str,
    labels: &[&str],
) -> CliResult<Vec<String>> {
    if positionals.len() < labels.len() {
        return Err(CliError::new(format!(
            "{command} requires {}",
            labels[positionals.len()]
        )));
    }
    if positionals.len() > labels.len() {
        return Err(CliError::new(format!(
            "{command}: unexpected argument {}",
            positionals[labels.len()]
        )));
    }
    positionals
        .iter()
        .zip(labels.iter())
        .map(|(value, label)| trimmed_positional(value, command, label))
        .collect()
}

fn trimmed_positional(value: &str, command: &str, label: &str) -> CliResult<String> {
    let value = value.trim();
    if value.is_empty() {
        Err(CliError::new(format!("{command} requires {label}")))
    } else {
        Ok(value.to_string())
    }
}

fn insert_optional_cli_string_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    option: &str,
    field: &str,
) -> CliResult<()> {
    if let Some(value) = non_blank_string_option(options, option, &format!("--{option}"))? {
        params.insert(field.to_string(), Value::String(value.trim().to_string()));
    }
    Ok(())
}

fn insert_optional_cli_raw_string_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    option: &str,
    field: &str,
) -> CliResult<()> {
    if let Some(value) = string_option(options, option, &format!("--{option}"))? {
        params.insert(field.to_string(), Value::String(value.to_string()));
    }
    Ok(())
}

fn insert_optional_cli_u64_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    option: &str,
    field: &str,
) -> CliResult<()> {
    if let Some(value) = parse_u64_option(options, option, &format!("--{option}"))? {
        params.insert(field.to_string(), json!(value));
    }
    Ok(())
}

fn insert_optional_cli_bool_param(
    params: &mut Map<String, Value>,
    options: &BTreeMap<String, FlagValue>,
    option: &str,
    field: &str,
) -> CliResult<()> {
    let Some(raw) = options.get(option) else {
        return Ok(());
    };
    let value = match raw {
        FlagValue::Bool => true,
        FlagValue::String(value) if value == "true" => true,
        FlagValue::String(value) if value == "false" => false,
        FlagValue::String(_) => {
            return Err(CliError::new(format!("--{option} expects true or false")));
        }
    };
    params.insert(field.to_string(), Value::Bool(value));
    Ok(())
}

fn comma_list_option(
    options: &BTreeMap<String, FlagValue>,
    key: &str,
    option_name: &str,
) -> CliResult<Option<Vec<String>>> {
    let Some(raw) = non_blank_string_option(options, key, option_name)? else {
        return Ok(None);
    };
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(CliError::new(format!("{option_name} requires a value")));
    }
    Ok(Some(values))
}

fn surface_id_from_args(parsed: &ParsedFlags, command: &str) -> CliResult<Option<String>> {
    let option = non_blank_string_option(&parsed.options, "surface-id", "--surface-id")?
        .map(|value| value.trim().to_string());
    let positional = parsed.positionals.first().map(|value| value.trim());
    if parsed
        .positionals
        .first()
        .is_some_and(|_| positional == Some(""))
    {
        return Err(CliError::new("surface id requires a value"));
    }
    if option.is_some() && positional.is_some_and(|value| !value.is_empty()) {
        return Err(CliError::new(format!(
            "{command}: cannot combine --surface-id with a positional surface id"
        )));
    }
    Ok(option
        .or_else(|| positional.map(str::to_string))
        .or_else(|| trimmed_env("FORKTTY_SURFACE_ID")))
}

fn resolve_focused_surface_id(context: &CliContext) -> CliResult<Option<String>> {
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    Ok(surface_id_from_workspace_list(&workspaces))
}

fn surface_id_from_workspace_list(workspaces: &Value) -> Option<String> {
    let items = workspaces.as_array()?;
    let active = items
        .iter()
        .find(|workspace| {
            workspace
                .get("active")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| {
            items.iter().find(|workspace| {
                workspace.get("focused_surface_id").is_some()
                    || workspace.get("focusedSurfaceId").is_some()
            })
        })?;
    string_field(active, "focused_surface_id")
        .or_else(|| string_field(active, "focusedSurfaceId"))
        .map(str::to_string)
}

#[cfg(any(feature = "browser", test))]
fn resolve_active_workspace_id(context: &CliContext) -> CliResult<String> {
    let workspaces = send_socket_request(&context.socket_path, "workspace.list", json!({}))?;
    active_workspace_id_from_list(&workspaces).ok_or_else(|| {
        CliError::new("browser open requires --workspace-id (no active workspace found)")
    })
}

#[cfg(any(feature = "browser", test))]
fn active_workspace_id_from_list(workspaces: &Value) -> Option<String> {
    let items = workspaces.as_array()?;
    let active = items
        .iter()
        .find(|w| w.get("active").and_then(Value::as_bool).unwrap_or(false))
        .or_else(|| items.first())?;
    string_field(active, "id").map(str::to_string)
}

fn handle_socket_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    require_no_args(&args, "doctor")?;
    let report = build_socket_doctor_report(context);
    if context.json {
        return print_json(&report);
    }
    write_stdout_text(&format_socket_doctor_text(&report))
}

fn format_socket_doctor_text(report: &Value) -> String {
    let mut lines = Vec::new();
    lines.push("ForkTTY doctor".to_string());
    lines.push(format!(
        "socket source: {}",
        report["socket"]["source"].as_str().unwrap_or("default")
    ));
    lines.push(format_doctor_path("socket", &report["socket"]["inspect"]));
    if let Some(info) = report["executable"]["forktty"].as_object() {
        lines.push(format_doctor_path("forktty", &Value::Object(info.clone())));
    }
    lines.push("environment:".to_string());
    if let Some(env) = report["env"].as_object() {
        for (key, value) in env {
            lines.push(format!("  {key}={}", value.as_str().unwrap_or("(unset)")));
        }
    }
    lines.push("hook configs:".to_string());
    if let Some(configs) = report["hookConfigs"].as_object() {
        for (agent, info) in configs {
            lines.push(format!("  {}", format_doctor_path(agent, info)));
        }
    }
    lines.push("mcp configs:".to_string());
    if let Some(configs) = report["mcpConfigs"].as_object() {
        for (agent, info) in configs {
            lines.push(format!("  {}", format_doctor_path(agent, info)));
        }
    }
    lines.push("skill dirs:".to_string());
    if let Some(dirs) = report["skillDirs"].as_object() {
        for (target, info) in dirs {
            lines.push(format!("  {}", format_doctor_path(target, info)));
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

fn build_socket_doctor_report(context: &CliContext) -> Value {
    let socket = inspect_path(&context.socket_path);
    let launcher = stable_hook_launcher_path();
    let launcher_info = launcher.as_ref().map(|path| inspect_path(path));
    let mut hook_configs = Map::new();
    for spec in AGENTS {
        hook_configs.insert(spec.key.to_string(), inspect_path(&(spec.config_path)()));
    }
    let mut mcp_configs = Map::new();
    for spec in MCP_AGENTS {
        mcp_configs.insert(spec.key.to_string(), inspect_path(&(spec.config_path)()));
    }
    let mut skill_dirs = Map::new();
    for spec in SKILL_TARGETS {
        skill_dirs.insert(spec.key.to_string(), inspect_skill_target(spec));
    }
    json!({
        "socket": {
            "path": context.socket_path,
            "source": if context.socket_explicit { "argument" } else if socket_path_from_env().is_some() { "FORKTTY_SOCKET_PATH" } else { "default" },
            "inspect": socket,
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
        "hookConfigs": hook_configs,
        "mcpConfigs": mcp_configs,
        "skillDirs": skill_dirs,
    })
}

fn parse_finite_number(raw: &str, option: &str) -> CliResult<f64> {
    if raw.trim().is_empty() {
        return Err(CliError::new(format!("{option} requires a value")));
    }
    raw.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| CliError::new(format!("Invalid {option}: expected finite number")))
}

fn is_supported_status_color(color: &str) -> bool {
    matches!(color, "green" | "yellow" | "red" | "blue" | "muted") || is_hex_status_color(color)
}

fn is_hex_status_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn safe_string_field(value: &Value, key: &str) -> Option<String> {
    string_field(value, key).map(sanitize_for_terminal)
}

fn format_json_scalar(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        text.to_string()
    } else {
        value.to_string()
    }
}

fn format_terminal_safe_json_scalar(value: &Value) -> String {
    sanitize_for_terminal(&format_json_scalar(value))
}

fn trimmed_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn sanitize_for_terminal(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\x{:02x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out
}

fn handle_hooks(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn handle_mcp(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn handle_skills(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    match args.first().map(String::as_str) {
        Some("setup") | Some("install") => handle_skills_setup(context, args[1..].to_vec()),
        Some("remove") | Some("uninstall") => handle_skills_remove(context, args[1..].to_vec()),
        Some("help") | Some("--help") | Some("-h") => write_stdout_line(
            "Usage: forktty skills setup [agents|codex|pi|claude] | forktty skills remove [agents|codex|pi|claude]\nDefault setup targets: agents, claude. codex and pi alias the interoperable agents target.",
        ),
        Some(other) => Err(CliError::new(format!("skills: unknown subcommand {other}"))),
        None => Err(CliError::new(
            "skills requires a subcommand: setup or remove",
        )),
    }
}

fn supported_agent_keys() -> String {
    AGENTS
        .iter()
        .map(|spec| spec.key)
        .collect::<Vec<_>>()
        .join(", ")
}

fn handle_hooks_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn handle_hooks_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn handle_skills_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "skills setup")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "skills setup: --dry-run must be true or false",
        ));
    };
    let targets = if parsed.positionals.is_empty() {
        default_skill_setup_targets()
    } else {
        supported_skill_targets(&parsed.positionals)?
    };

    let mut plans = Vec::new();
    for target in targets {
        plans.push(build_skill_setup_plan(target)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            backup_path = backup_skill_dir(&plan.skill_dir)?;
            for (path, content) in &plan.files {
                ensure_parent_dir(path)?;
                atomic_write_file(path, content.as_bytes())?;
            }
        }
        summaries.push(skill_setup_summary(&plan, dry_run, backup_path));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let target = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["target"].as_str().unwrap_or("target"));
        let skill_dir = summary["skillDir"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would install"
        } else if changed {
            "installed"
        } else {
            "already installed"
        };
        write_stdout_line(&format!("{target}: {verb} ForkTTY skill at {skill_dir}"))?;
        if let Some(status) = summary["status"].as_str() {
            write_stdout_line(&format!("  status: {status}"))?;
        }
        if let Some(repair) = summary["repairCommand"].as_str() {
            write_stdout_line(&format!("  repair: {repair}"))?;
        }
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
    }
    Ok(())
}

fn handle_skills_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["dry-run"]);
    reject_unknown_options(&parsed.options, &["dry-run"], "skills remove")?;
    let Some(dry_run) = bool_option(&parsed.options, "dry-run") else {
        return Err(CliError::new(
            "skills remove: --dry-run must be true or false",
        ));
    };
    let targets = supported_skill_targets(&parsed.positionals)?;

    let mut plans = Vec::new();
    for target in targets {
        plans.push(build_skill_remove_plan(target)?);
    }

    let mut summaries = Vec::new();
    for plan in plans {
        let mut backup_path = None;
        if plan.changed && !dry_run {
            backup_path = backup_skill_dir(&plan.skill_dir)?;
        }
        summaries.push(json!({
            "target": plan.spec.key,
            "agent": plan.spec.key,
            "label": plan.spec.label,
            "skillDir": plan.skill_dir,
            "configPath": plan.skill_dir,
            "changed": plan.changed,
            "backupPath": backup_path,
            "dryRun": dry_run,
        }));
    }

    if context.json {
        return print_json(&Value::Array(summaries));
    }
    for summary in summaries {
        let target = summary["label"]
            .as_str()
            .unwrap_or_else(|| summary["target"].as_str().unwrap_or("target"));
        let skill_dir = summary["skillDir"].as_str().unwrap_or("");
        let changed = summary["changed"].as_bool().unwrap_or(false);
        let dry_run = summary["dryRun"].as_bool().unwrap_or(false);
        let verb = if changed && dry_run {
            "would remove"
        } else if changed {
            "removed"
        } else {
            "not installed"
        };
        write_stdout_line(&format!("{target}: {verb} ForkTTY skill at {skill_dir}"))?;
        if let Some(backup) = summary["backupPath"].as_str() {
            write_stdout_line(&format!("  backup: {backup}"))?;
        }
    }
    Ok(())
}

fn handle_mcp_setup(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn handle_mcp_remove(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

struct HookSetupPlan {
    spec: &'static AgentSpec,
    config_path: PathBuf,
    changed: bool,
    content: String,
    /// Generated wrapper scripts written alongside the config (Antigravity
    /// only: its hook `command` is a bare executable path with no arguments).
    scripts: Vec<(PathBuf, String)>,
}

enum HookRemoveAction {
    Write(String),
    DeleteFile,
    None,
}

struct HookRemovePlan {
    spec: &'static AgentSpec,
    config_path: PathBuf,
    changed: bool,
    action: HookRemoveAction,
    /// ForkTTY-owned generated scripts directory deleted on removal.
    scripts_dir: Option<PathBuf>,
}

struct McpSetupPlan {
    spec: &'static McpAgentSpec,
    config_path: PathBuf,
    changed: bool,
    content: String,
}

enum McpRemoveAction {
    Write(String),
    DeleteFile,
    None,
}

struct McpRemovePlan {
    spec: &'static McpAgentSpec,
    config_path: PathBuf,
    changed: bool,
    action: McpRemoveAction,
}

struct SkillSetupPlan {
    spec: &'static SkillTargetSpec,
    skill_dir: PathBuf,
    changed: bool,
    status: &'static str,
    source_checksum: String,
    installed_checksum: String,
    repair_command: Option<String>,
    files: Vec<(PathBuf, String)>,
}

struct SkillRemovePlan {
    spec: &'static SkillTargetSpec,
    skill_dir: PathBuf,
    changed: bool,
}

fn build_hook_setup_plan(spec: &'static AgentSpec, launcher: &Path) -> CliResult<HookSetupPlan> {
    build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Lifecycle)
}

fn build_hook_setup_plan_with_profile(
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

fn build_hook_remove_plan(
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

const MCP_SERVER_NAME: &str = "forktty";
const MCP_MANAGED_ENV: &str = "FORKTTY_MCP_MANAGED";

fn build_mcp_setup_plan(spec: &'static McpAgentSpec, launcher: &Path) -> CliResult<McpSetupPlan> {
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

fn build_mcp_remove_plan(spec: &'static McpAgentSpec) -> CliResult<McpRemovePlan> {
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

fn build_skill_setup_plan(spec: &'static SkillTargetSpec) -> CliResult<SkillSetupPlan> {
    let skill_dir = (spec.skill_dir)();
    let skill_path = skill_dir.join("SKILL.md");
    let metadata_path = skill_dir.join("agents").join("openai.yaml");
    let files = vec![
        (skill_path, AGENT_SKILL_MD.to_string()),
        (metadata_path, AGENT_SKILL_OPENAI_YAML.to_string()),
    ];
    let source_checksum = skill_content_checksum(AGENT_SKILL_MD, Some(AGENT_SKILL_OPENAI_YAML));
    let (status, installed_checksum, _) =
        skill_target_status(&skill_dir, &source_checksum, spec.key);
    if status == "unmanaged" {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite unmanaged skill at {}",
            skill_dir.display()
        )));
    }
    if status == "invalid" && installed_checksum.is_none() {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite invalid skill at {} because the ForkTTY-managed marker could not be verified",
            skill_dir.display()
        )));
    }
    let changed = status != "up_to_date";
    Ok(SkillSetupPlan {
        spec,
        skill_dir,
        changed,
        status,
        source_checksum,
        installed_checksum: installed_checksum.unwrap_or_default(),
        repair_command: changed.then(|| format!("forktty skills setup {}", spec.key)),
        files,
    })
}

fn skill_setup_summary(
    plan: &SkillSetupPlan,
    dry_run: bool,
    backup_path: Option<PathBuf>,
) -> Value {
    let applied = plan.changed && !dry_run;
    let installed_checksum = if applied {
        Value::String(plan.source_checksum.clone())
    } else if plan.installed_checksum.is_empty() {
        Value::Null
    } else {
        Value::String(plan.installed_checksum.clone())
    };
    json!({
        "target": plan.spec.key,
        "agent": plan.spec.key,
        "label": plan.spec.label,
        "skillDir": plan.skill_dir,
        "configPath": plan.skill_dir,
        "changed": plan.changed,
        "status": if applied { "up_to_date" } else { plan.status },
        "sourceChecksum": plan.source_checksum,
        "installedChecksum": installed_checksum,
        "repairCommand": if applied { None } else { plan.repair_command.clone() },
        "backupPath": backup_path,
        "dryRun": dry_run,
    })
}

fn inspect_skill_target(spec: &'static SkillTargetSpec) -> Value {
    let skill_dir = (spec.skill_dir)();
    let mut value = inspect_path(&skill_dir);
    let source_checksum = skill_content_checksum(AGENT_SKILL_MD, Some(AGENT_SKILL_OPENAI_YAML));
    let (status, installed_checksum, repair_command) =
        skill_target_status(&skill_dir, &source_checksum, spec.key);
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_string(), Value::String(status.to_string()));
        object.insert("sourceChecksum".to_string(), Value::String(source_checksum));
        object.insert(
            "installedChecksum".to_string(),
            installed_checksum.map(Value::String).unwrap_or(Value::Null),
        );
        object.insert(
            "repairCommand".to_string(),
            repair_command.map(Value::String).unwrap_or(Value::Null),
        );
    }
    value
}

fn skill_target_status(
    skill_dir: &Path,
    source_checksum: &str,
    target: &str,
) -> (&'static str, Option<String>, Option<String>) {
    let repair = || Some(format!("forktty skills setup {target}"));
    let unrepairable_invalid = || ("invalid", None, None);
    let meta = match fs::symlink_metadata(skill_dir) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return ("missing", None, repair());
        }
        Err(_) => return unrepairable_invalid(),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return unrepairable_invalid();
    }
    let skill_path = skill_dir.join("SKILL.md");
    let skill = match read_managed_skill_file_for_doctor(&skill_path, "agent skill") {
        Ok(Some(skill)) => skill,
        Ok(None) | Err(_) => return unrepairable_invalid(),
    };
    if !skill.contains(AGENT_SKILL_MARKER) {
        return (
            "unmanaged",
            Some(skill_content_checksum(&skill, None)),
            None,
        );
    }
    let metadata_dir = skill_dir.join("agents");
    if let Err(_err) = validate_managed_skill_metadata_dir_for_doctor(&metadata_dir) {
        return (
            "invalid",
            Some(skill_content_checksum(&skill, None)),
            repair(),
        );
    }
    let metadata_path = metadata_dir.join("openai.yaml");
    let metadata = match read_managed_skill_file_for_doctor(&metadata_path, "agent skill metadata")
    {
        Ok(metadata) => metadata,
        Err(_) => {
            return (
                "invalid",
                Some(skill_content_checksum(&skill, None)),
                repair(),
            );
        }
    };
    let installed_checksum = skill_content_checksum(&skill, metadata.as_deref());
    if installed_checksum == source_checksum {
        ("up_to_date", Some(installed_checksum), None)
    } else {
        ("update_available", Some(installed_checksum), repair())
    }
}

fn validate_managed_skill_metadata_dir_for_doctor(path: &Path) -> CliResult<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(CliError::new(format!(
            "agent skill metadata directory exists but is not a directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn read_managed_skill_file_for_doctor(path: &Path, label: &str) -> CliResult<Option<String>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    let file = File::open(path)?;
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    if stat.len() > MAX_AGENT_SKILL_FILE_BYTES {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            MAX_AGENT_SKILL_FILE_BYTES
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(MAX_AGENT_SKILL_FILE_BYTES + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > MAX_AGENT_SKILL_FILE_BYTES {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            MAX_AGENT_SKILL_FILE_BYTES
        )));
    }
    Ok(Some(text))
}

fn skill_content_checksum(skill: &str, metadata: Option<&str>) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    fn feed(hash: &mut u64, bytes: &[u8]) {
        for byte in bytes {
            *hash ^= u64::from(*byte);
            *hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    feed(&mut hash, b"SKILL.md\0");
    feed(&mut hash, skill.as_bytes());
    feed(&mut hash, b"\0agents/openai.yaml\0");
    if let Some(metadata) = metadata {
        feed(&mut hash, metadata.as_bytes());
    }
    format!("fnv1a64:{hash:016x}")
}

fn build_skill_remove_plan(spec: &'static SkillTargetSpec) -> CliResult<SkillRemovePlan> {
    let skill_dir = (spec.skill_dir)();
    let changed = read_managed_skill_dir(&skill_dir)?.is_some();
    Ok(SkillRemovePlan {
        spec,
        skill_dir,
        changed,
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

fn json_mcp_server_config(launcher: &Path) -> Value {
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

fn supported_agents(names: &[String]) -> CliResult<Vec<&'static AgentSpec>> {
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

fn supported_hook_remove_agents(names: &[String]) -> CliResult<Vec<&'static AgentSpec>> {
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

fn default_hook_setup_agents() -> Vec<&'static AgentSpec> {
    DEFAULT_HOOK_SETUP_AGENT_KEYS
        .iter()
        .map(|key| agent_spec(key).expect("default hook setup agent exists"))
        .collect()
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

fn supported_skill_targets(names: &[String]) -> CliResult<Vec<&'static SkillTargetSpec>> {
    if names.is_empty() {
        return Ok(SKILL_TARGETS.iter().collect());
    }
    let mut out = Vec::new();
    for name in names {
        let normalized = normalize_skill_target_name(name);
        let spec = skill_target_spec(&normalized)
            .ok_or_else(|| CliError::new(format!("Unsupported skills target: {name}")))?;
        if !out
            .iter()
            .any(|existing: &&SkillTargetSpec| existing.key == spec.key)
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

fn default_skill_setup_targets() -> Vec<&'static SkillTargetSpec> {
    DEFAULT_SKILL_SETUP_TARGET_KEYS
        .iter()
        .map(|key| skill_target_spec(key).expect("default skill setup target exists"))
        .collect()
}

fn agent_spec(agent: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|spec| spec.key == agent)
}

fn mcp_agent_spec(agent: &str) -> Option<&'static McpAgentSpec> {
    MCP_AGENTS.iter().find(|spec| spec.key == agent)
}

fn skill_target_spec(target: &str) -> Option<&'static SkillTargetSpec> {
    SKILL_TARGETS.iter().find(|spec| spec.key == target)
}

fn normalize_agent_name(agent: &str) -> String {
    match agent.to_lowercase().as_str() {
        "claude-code" | "claude_code" => "claude".to_string(),
        "open-code" | "open_code" => "opencode".to_string(),
        "agy" => "antigravity".to_string(),
        other => other.to_string(),
    }
}

fn normalize_skill_target_name(target: &str) -> String {
    match target.to_lowercase().as_str() {
        "agent" | "agents" | "agent-skills" | "agent_skills" | "open-agent" | "open_agent"
        | "openagents" | "codex" | "pi" | "pi-agent" | "pi_agent" => "agents".to_string(),
        "claude-code" | "claude_code" => "claude".to_string(),
        other => other.to_string(),
    }
}

fn stable_hook_launcher_path() -> Option<PathBuf> {
    let current_exe = std::env::current_exe().ok();
    stable_hook_launcher_path_from_env(
        current_exe.as_deref(),
        [
            // Launched directly through the appimage runtime.
            (std::env::var_os("APPIMAGE"), std::env::var_os("APPDIR")),
            // Shells spawned inside the AppImage app: the runtime's own vars
            // are stripped from child environments, but AppRun exports these
            // and puts the mounted usr/bin first in PATH — so a plain
            // `forktty` resolves to the mounted binary whose /tmp/.mount_*
            // path dies with the next remount. Hooks must reference the
            // stable .AppImage path instead.
            (
                std::env::var_os("FORKTTY_APPIMAGE"),
                std::env::var_os("FORKTTY_APPIMAGE_DIR"),
            ),
        ],
    )
}

/// The launcher path hooks should invoke: the .AppImage file when (and only
/// when) the running binary is the one mounted from it, otherwise the binary
/// itself.
fn stable_hook_launcher_path_from_env(
    current_exe: Option<&Path>,
    appimage_candidates: [(Option<OsString>, Option<OsString>); 2],
) -> Option<PathBuf> {
    for (appimage, appdir) in appimage_candidates {
        if let (Some(appimage), Some(appdir), Some(current_exe)) =
            (appimage, appdir, current_exe.as_ref())
        {
            let appimage = PathBuf::from(appimage);
            let appdir = PathBuf::from(appdir);
            if appimage.is_absolute() && appdir.is_absolute() && current_exe.starts_with(appdir) {
                return Some(appimage);
            }
        }
    }
    current_exe.map(Path::to_path_buf)
}

fn home_dir() -> PathBuf {
    trimmed_env("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn codex_home_dir() -> PathBuf {
    trimmed_env("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".codex"))
}

fn codex_config_path() -> PathBuf {
    codex_home_dir().join("hooks.json")
}

fn codex_mcp_config_path() -> PathBuf {
    codex_home_dir().join("config.toml")
}

fn claude_config_path() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("settings.json")
}

fn claude_mcp_config_path() -> PathBuf {
    home_dir().join(".claude.json")
}

fn agent_skills_dir() -> PathBuf {
    home_dir()
        .join(".agents")
        .join("skills")
        .join(AGENT_SKILL_NAME)
}

fn claude_skill_dir() -> PathBuf {
    trimmed_env("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
        .join("skills")
        .join(AGENT_SKILL_NAME)
}

fn legacy_gemini_config_path() -> PathBuf {
    home_dir().join(".gemini/settings.json")
}

fn legacy_gemini_mcp_config_path() -> PathBuf {
    legacy_gemini_config_path()
}

// Antigravity CLI loads user-level hooks from ~/.gemini/config/hooks.json
// (verified against agy 1.0.3; the workspace-level .agents/hooks.json is
// intentionally not managed so hooks work from any project).
fn antigravity_root_dir() -> PathBuf {
    home_dir().join(".gemini")
}

fn antigravity_config_dir() -> PathBuf {
    antigravity_root_dir().join("config")
}

fn antigravity_config_path() -> PathBuf {
    antigravity_config_dir().join("hooks.json")
}

fn antigravity_mcp_config_path() -> PathBuf {
    antigravity_config_dir().join("mcp_config.json")
}

fn antigravity_scripts_dir() -> PathBuf {
    antigravity_config_dir().join("forktty-hooks.generated")
}

fn antigravity_script_path(hook_event_name: &str) -> PathBuf {
    antigravity_scripts_dir().join(format!("{hook_event_name}.sh"))
}

fn opencode_plugin_path() -> PathBuf {
    trimmed_env("OPENCODE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config/opencode"))
        .join("plugins/forktty.generated.js")
}

fn build_hook_shell_command(launcher: &Path, spec: &AgentSpec, event: &str) -> String {
    format!(
        "[ \"${{{}:-}}\" != \"1\" ] && {} hooks {} {} || echo '{}'",
        spec.disabled_env,
        shell_quote(&launcher.display().to_string()),
        spec.key,
        event,
        HOOK_CONTINUE_JSON.trim_end()
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn build_hook_entry(spec: &AgentSpec, command: String, timeout: u64) -> Value {
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

fn merge_hook_config(
    existing: &Value,
    spec: &AgentSpec,
    launcher: &Path,
) -> CliResult<(bool, Value)> {
    merge_hook_config_with_profile(existing, spec, launcher, HookSetupProfile::Full)
}

fn merge_hook_config_with_profile(
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

fn hook_entry_enabled_for_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    !(spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name))
}

fn hook_entry_removed_by_setup(
    spec: &AgentSpec,
    profile: HookSetupProfile,
    entry_spec: &HookEntrySpec,
) -> bool {
    spec.key == "claude"
        && profile == HookSetupProfile::Lifecycle
        && is_claude_high_frequency_event(entry_spec.event_name)
}

fn is_claude_high_frequency_event(event_name: &str) -> bool {
    CLAUDE_HIGH_FREQUENCY_HOOK_ENTRIES
        .iter()
        .any(|entry| entry.event_name == event_name)
}

fn hook_setup_profile_name(profile: HookSetupProfile) -> &'static str {
    match profile {
        HookSetupProfile::Lifecycle => "lifecycle",
        HookSetupProfile::Full => "full",
    }
}

fn remove_hook_config(
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

fn is_forktty_managed_entry(entry: &Value) -> bool {
    entry.get("forkttySource").and_then(Value::as_str) == Some(FORKTTY_HOOK_TAG)
}

/// ForkTTY owns this named hook group in Antigravity's hooks.json; other
/// top-level groups belong to the user and are never touched.
const ANTIGRAVITY_HOOK_GROUP: &str = "forktty";
const ANTIGRAVITY_SCRIPT_TAG: &str = "forktty-managed-antigravity-hook";

/// Antigravity executes `command` as one executable path (no argv splitting,
/// no shell), so each event points at a generated wrapper script that runs
/// the launcher with the usual guard line. PreToolUse is a gating hook, so its
/// disabled/failed fallback explicitly approves tool use; non-gating events
/// fall back to `{}` because Antigravity rejects unknown response fields like
/// `continue` under strict protojson unmarshaling.
fn build_antigravity_hook_script(launcher: &Path, spec: &AgentSpec, event: &str) -> String {
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

fn merge_antigravity_hook_config(existing: &Value, spec: &AgentSpec) -> CliResult<(bool, Value)> {
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

fn is_antigravity_flat_hook_event(event_name: &str) -> bool {
    matches!(event_name, "PreInvocation" | "PostInvocation" | "Stop")
}

fn is_legacy_forktty_hook_command(
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

fn read_agent_config(spec: &AgentSpec, path: &Path) -> CliResult<Value> {
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

fn read_json_file(path: &Path) -> CliResult<Value> {
    read_json_file_with_limit(path, MAX_HOOK_CONFIG_SIZE_BYTES, "hook config")
}

fn read_json_file_with_limit(path: &Path, max_bytes: u64, label: &str) -> CliResult<Value> {
    let link_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(json!({})),
        Err(err) => return Err(err.into()),
    };
    let followed = if link_meta.file_type().is_symlink() {
        match fs::metadata(path) {
            Ok(meta) => meta,
            // Treat a broken symlink the same as a missing file: the
            // subsequent write replaces the dangling link with a real file.
            // Previously this aborted `hooks setup` with a confusing
            // "path is a broken symlink" error.
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                eprintln!(
                    "warning: {} is a broken symlink; replacing with a fresh file",
                    path.display()
                );
                return Ok(json!({}));
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        link_meta
    };
    // Reject non-regular files before open(2): opening a FIFO blocks until a
    // peer shows up, which used to hang `forktty hooks setup` forever.
    if !followed.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    let file = File::open(path)?;
    // TOCTOU backstop: re-check the opened file, not just the pre-open stat.
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new("path exists but is not a regular file"));
    }
    if stat.len() > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            max_bytes
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(max_bytes + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            max_bytes
        )));
    }
    if text.trim().is_empty() {
        Ok(json!({}))
    } else {
        serde_json::from_str(&text).map_err(Into::into)
    }
}

fn read_text_config(path: &Path, label: &str) -> CliResult<Option<String>> {
    read_text_config_with_limit(path, label, MAX_HOOK_CONFIG_SIZE_BYTES)
}

fn read_text_config_with_limit(
    path: &Path,
    label: &str,
    max_bytes: u64,
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
    if !followed.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    let file = File::open(path)?;
    let stat = file.metadata()?;
    if !stat.is_file() {
        return Err(CliError::new(format!(
            "{label} exists but is not a regular file"
        )));
    }
    if stat.len() > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            stat.len(),
            max_bytes
        )));
    }
    let mut text = String::new();
    let mut limited = file.take(max_bytes + 1);
    limited.read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(CliError::new(format!(
            "{label} is too large ({} bytes; max {} bytes)",
            text.len(),
            max_bytes
        )));
    }
    Ok(Some(text))
}

fn read_managed_skill_dir(path: &Path) -> CliResult<Option<String>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(CliError::new(format!(
            "skills: refusing symlinked skill directory {}",
            path.display()
        )));
    }
    if !meta.is_dir() {
        return Err(CliError::new(format!(
            "skills: {} exists but is not a directory",
            path.display()
        )));
    }
    let skill_path = path.join("SKILL.md");
    let Some(text) = read_text_config(&skill_path, "agent skill")? else {
        return Err(CliError::new(format!(
            "skills: {} exists but has no SKILL.md",
            path.display()
        )));
    };
    if !text.contains(AGENT_SKILL_MARKER) {
        return Err(CliError::new(format!(
            "skills: refusing to overwrite unmanaged skill at {}",
            path.display()
        )));
    }
    Ok(Some(text))
}

fn read_opencode_plugin_file(spec: &AgentSpec, path: &Path) -> CliResult<Option<String>> {
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

fn build_opencode_plugin(launcher: &Path) -> CliResult<String> {
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

fn hook_config_write_path(path: &Path) -> CliResult<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => Ok(resolved),
            // Broken symlink: rename will replace the dangling link with the
            // newly written hook config.
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
            Err(err) => Err(err.into()),
        },
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err.into()),
    }
}

fn ensure_parent_dir(path: &Path) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn ensure_private_antigravity_hook_dirs() -> CliResult<()> {
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

fn backup_file(path: &Path) -> CliResult<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    loop {
        let backup = path.with_file_name(format!(
            "{}.bak-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("config"),
            next_file_nonce()
        ));
        match copy_file_exclusive(path, &backup) {
            Ok(()) => return Ok(Some(backup)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    }
}

fn backup_skill_dir(path: &Path) -> CliResult<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    loop {
        let backup = path.with_file_name(format!(
            "{}.bak-{}",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(AGENT_SKILL_NAME),
            next_file_nonce()
        ));
        match fs::rename(path, &backup) {
            Ok(()) => return Ok(Some(backup)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        }
    }
}

fn copy_file_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    let mut src = File::open(from)?;
    let mut dst = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)?;
    io::copy(&mut src, &mut dst)?;
    dst.sync_all()
}

fn atomic_write_file(path: &Path, content: &[u8]) -> CliResult<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let base = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let tmp = dir.join(format!(".{base}.tmp-{}", next_file_nonce()));
    let result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}

fn handle_hooks_doctor(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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
fn hooks_health_exit(command: &str, agent: &str, healthy: bool) -> CliResult<()> {
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

fn record_hook_check(
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

fn format_codex_trust_check(check: &Value) -> Option<String> {
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

fn describe_launcher_check(
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

fn describe_claude_installed_profile(config_path: &Path) -> &'static str {
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

fn config_has_forktty_hook(config: &Value, spec: &AgentSpec, entry_spec: &HookEntrySpec) -> bool {
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
fn describe_codex_hook_trust(config_path: &Path) -> Value {
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

fn codex_hook_trust_report(
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

fn camel_to_snake_event_name(event: &str) -> String {
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
fn hook_setup_reminder_message_for_statuses<'a>(
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

fn extract_launcher_from_opencode_plugin(text: &str) -> Option<String> {
    let marker = "const FORKTTY_LAUNCHER = ";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find(';')?;
    serde_json::from_str(rest[..end].trim()).ok()
}

fn format_launcher_check(check: &Value) -> Option<String> {
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

fn extract_managed_launcher_from_config(spec: &AgentSpec, config: &Value) -> Option<String> {
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

fn parse_launcher_from_managed_command(command: &str, spec: &AgentSpec) -> Option<String> {
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

fn test_system_ping(context: &CliContext, checks: &mut Vec<Value>) {
    let ping = match send_socket_request(&context.socket_path, "system.ping", json!({})) {
        Ok(value) if value.as_str() == Some("pong") => Ok(value),
        Ok(value) => Err(CliError::new(format!(
            "system.ping returned {value}, expected \"pong\""
        ))),
        Err(err) => Err(err),
    };
    record_hook_check(checks, "system.ping", ping);
}

fn test_metadata_set_status(
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

fn test_metadata_log(
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

fn test_notification_create(
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

fn test_metadata_clear_status(
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

fn test_notification_clear(
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

fn print_hook_test_report(
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

fn hook_check_error_for_terminal(check: &Value) -> String {
    sanitize_for_terminal(check["error"].as_str().unwrap_or("unknown error"))
}

fn handle_hooks_test(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn single_agent_command(
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

fn handle_hook_event(context: &CliContext, args: Vec<String>) -> CliResult<()> {
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

fn is_supported_hook_event(event: &str) -> bool {
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

fn should_send_hook_actions(context: &CliContext) -> bool {
    context.socket_explicit || socket_path_from_env().is_some()
}

fn hook_debug(context: &CliContext, message: &str) {
    if context.verbose || is_truthy_env("FORKTTY_HOOK_DEBUG") {
        eprintln!("ForkTTY hook debug: {}", sanitize_for_terminal(message));
    }
}

fn is_truthy_env(key: &str) -> bool {
    trimmed_env(key)
        .map(|value| matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Hook event ordering must survive wall-clock steps (NTP, manual `date`):
/// orders are compared across short-lived CLI processes, so use
/// CLOCK_BOOTTIME — system-wide, monotonic, and advancing across suspend —
/// instead of `SystemTime`, which previously dropped every hook update issued
/// after the clock stepped backwards.
fn next_hook_event_order() -> String {
    boottime_nanos().to_string()
}

fn boottime_nanos() -> u128 {
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

fn increment_hook_event_order(order: &str) -> String {
    order
        .parse::<u128>()
        .map(|value| value.saturating_add(1).to_string())
        .unwrap_or_else(|_| next_hook_event_order())
}

fn hook_target_params() -> Map<String, Value> {
    let mut params = Map::new();
    if let Some(workspace_id) = trimmed_env("FORKTTY_WORKSPACE_ID") {
        params.insert("workspace_id".to_string(), Value::String(workspace_id));
    }
    if let Some(surface_id) = trimmed_env("FORKTTY_SURFACE_ID") {
        params.insert("surface_id".to_string(), Value::String(surface_id));
    }
    params
}

fn add_hook_metadata(
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

fn hook_session_cwd_for_metadata(spec: &AgentSpec, payload: &Value) -> Option<String> {
    if spec.key == "antigravity" {
        return extract_antigravity_workspace_cwd(payload);
    }
    std::env::current_dir()
        .ok()
        .filter(|cwd| !cwd.as_os_str().is_empty())
        .map(|cwd| cwd.to_string_lossy().into_owned())
}

fn extract_antigravity_workspace_cwd(payload: &Value) -> Option<String> {
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

fn extract_first_string_array_item(payload: &Value, keys: &[&str]) -> Option<String> {
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

fn valid_hook_session_cwd(value: &str) -> Option<String> {
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
fn permission_mode_color(spec: &AgentSpec, mode: &str) -> &'static str {
    if !matches!(spec.key, "claude" | "codex") {
        return "muted";
    }
    match mode {
        "bypassPermissions" => "red",
        "acceptEdits" | "auto" | "dontAsk" => "yellow",
        _ => "muted",
    }
}

struct HookActionBuilder<'a> {
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

fn build_hook_actions(
    spec: &AgentSpec,
    event: &str,
    payload: &Value,
    order: &str,
) -> Vec<(String, Value)> {
    HookActionBuilder::new(spec, event, payload, order).build()
}

struct HookEnrichments {
    token_usage: Option<TokenUsage>,
    workspace: Option<HookWorkspaceContext>,
}

#[derive(Clone)]
struct HookWorkspaceContext {
    name: String,
    git_branch: Option<String>,
}

#[derive(Clone, Copy)]
struct TokenUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
}

impl TokenUsage {
    fn input_total(self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }
}

fn gather_hook_enrichments(
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

fn hook_workspace_context(context: &CliContext) -> Option<HookWorkspaceContext> {
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

fn build_token_progress_action(
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

fn build_hook_response(
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

fn extract_hook_message(payload: &Value) -> String {
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

fn extract_hook_source(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["source", "trigger", "reason"])
        .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

fn extract_hook_compact_trigger(payload: &Value) -> Option<String> {
    extract_first_string_like(
        payload,
        &["trigger", "compact_trigger", "compactTrigger", "reason"],
    )
    .map(|value| sanitize_for_terminal(&value).chars().take(32).collect())
}

fn extract_hook_tool_name(payload: &Value) -> Option<String> {
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

fn extract_hook_tool_error(payload: &Value) -> bool {
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

fn object_signals_tool_error(value: &Value) -> bool {
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

fn extract_hook_permission_mode(payload: &Value) -> Option<String> {
    extract_first_string_like(payload, &["permission_mode", "permissionMode"])
        .map(|value| {
            sanitize_for_terminal(&value)
                .chars()
                .take(64)
                .collect::<String>()
        })
        .filter(|value| !value.is_empty())
}

fn extract_hook_session_id(payload: &Value) -> Option<String> {
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

fn extract_hook_turn_id(event: &str, payload: &Value) -> Option<String> {
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

fn extract_first_string(payload: &Value, keys: &[&str]) -> Option<String> {
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

fn extract_first_string_like(payload: &Value, keys: &[&str]) -> Option<String> {
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

fn short_hash(value: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn read_token_usage_from_transcript(path: &Path) -> Option<TokenUsage> {
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

fn resolve_token_ceiling() -> u64 {
    trimmed_env("FORKTTY_HOOK_TOKEN_CEILING")
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(HOOK_TOKEN_CEILING_DEFAULT)
}

fn format_token_usage_block(usage: TokenUsage) -> String {
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

fn format_thousands(value: u64) -> String {
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

fn path_access(path: &Path, mode: libc::c_int) -> bool {
    let Ok(cstr) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(cstr.as_ptr(), mode) == 0 }
}

fn inspect_path(path: &Path) -> Value {
    let mut result = Map::new();
    result.insert(
        "path".to_string(),
        Value::String(path.display().to_string()),
    );
    result.insert("exists".to_string(), Value::Bool(false));
    result.insert("kind".to_string(), Value::String("missing".to_string()));
    result.insert("readable".to_string(), Value::Bool(false));
    result.insert("writable".to_string(), Value::Bool(false));
    result.insert("executable".to_string(), Value::Bool(false));
    result.insert("mode".to_string(), Value::Null);
    result.insert("error".to_string(), Value::Null);
    match fs::symlink_metadata(path) {
        Ok(stat) => {
            result.insert("exists".to_string(), Value::Bool(true));
            result.insert(
                "mode".to_string(),
                Value::String(format!("0{:o}", stat.permissions().mode() & 0o777)),
            );
            let kind = if stat.file_type().is_symlink() {
                "symlink"
            } else if stat.is_file() {
                "file"
            } else if stat.is_dir() {
                "directory"
            } else if stat.file_type().is_socket() {
                "socket"
            } else {
                "other"
            };
            result.insert("kind".to_string(), Value::String(kind.to_string()));
            // Only probe regular files: open(2) on a FIFO blocks until a peer
            // shows up, which used to hang `forktty --json doctor` forever.
            // Non-regular paths keep the readable/writable defaults (false).
            let followed_is_file = if stat.file_type().is_symlink() {
                fs::metadata(path)
                    .map(|meta| meta.is_file())
                    .unwrap_or(false)
            } else {
                stat.is_file()
            };
            if followed_is_file {
                result.insert(
                    "readable".to_string(),
                    Value::Bool(File::open(path).is_ok()),
                );
                result.insert(
                    "writable".to_string(),
                    Value::Bool(OpenOptions::new().write(true).open(path).is_ok()),
                );
            } else {
                // access(2) answers the permission question without open(2),
                // which would block on FIFOs and fail on sockets even when
                // the caller has full permissions (the doctor used to report
                // a healthy owner-only socket as "not readable, not
                // writable").
                result.insert(
                    "readable".to_string(),
                    Value::Bool(path_access(path, libc::R_OK)),
                );
                result.insert(
                    "writable".to_string(),
                    Value::Bool(path_access(path, libc::W_OK)),
                );
            }
            result.insert(
                "executable".to_string(),
                Value::Bool(stat.permissions().mode() & 0o111 != 0),
            );
        }
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                result.insert("error".to_string(), Value::String(err.to_string()));
            }
        }
    }
    Value::Object(result)
}

fn format_doctor_path(label: &str, info: &Value) -> String {
    let kind = string_field(info, "kind").unwrap_or("missing");
    let exists = if info.get("exists").and_then(Value::as_bool).unwrap_or(false) {
        "exists"
    } else {
        "missing"
    };
    let readable = if info
        .get("readable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "readable"
    } else {
        "not readable"
    };
    let writable = if info
        .get("writable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "writable"
    } else {
        "not writable"
    };
    let executable = if info
        .get("executable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "executable"
    } else {
        "not executable"
    };
    let mode = string_field(info, "mode")
        .map(|mode| format!(", {mode}"))
        .unwrap_or_default();
    let error = string_field(info, "error")
        .map(|error| format!(" ({error})"))
        .unwrap_or_default();
    format!(
        "{label}: {} [{kind}, {exists}, {readable}, {writable}, {executable}{mode}]{error}",
        string_field(info, "path").unwrap_or("")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{with_current_dir, with_env};
    use std::thread;

    #[test]
    fn inspect_path_reports_owned_socket_as_accessible() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("probe.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        let info = inspect_path(&socket_path);

        assert_eq!(info["kind"], "socket");
        assert_eq!(info["readable"], true, "owner socket must probe readable");
        assert_eq!(info["writable"], true, "owner socket must probe writable");
    }

    #[test]
    fn lagged_detection_matches_the_notice_but_not_embedded_payloads() {
        assert_eq!(
            lagged_dropped_count(r#"{"event":"lagged","dropped":15}"#),
            Some(15)
        );
        assert_eq!(
            lagged_dropped_count(r#"{"dropped":7,"event":"lagged"}"#),
            Some(7)
        );
        // A title that embeds the notice text arrives with escaped quotes.
        assert_eq!(
            lagged_dropped_count(
                r#"{"event":"surface_title_changed","title":"{\"event\":\"lagged\"}"}"#
            ),
            None
        );
        assert_eq!(lagged_dropped_count(r#"{"event":"subscribed"}"#), None);
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|value| (*value).to_string()).collect()
    }

    fn os_strings(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn assert_err_contains<T>(result: CliResult<T>, expected: &str) {
        match result {
            Ok(_) => panic!("expected error containing {expected:?}"),
            Err(err) => assert!(
                err.message.contains(expected),
                "expected {:?} to contain {:?}",
                err.message,
                expected
            ),
        }
    }

    fn with_socket_response(
        response: impl FnOnce(&Value) -> String + Send + 'static,
        test: impl FnOnce(&Path),
    ) -> Value {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(line.trim()).unwrap();
            stream.write_all(response(&request).as_bytes()).unwrap();
            request
        });
        test(&socket_path);
        handle.join().unwrap()
    }

    /// Serves a fixed number of requests, each on its own connection (the CLI
    /// opens a fresh connection per `send_socket_request`). The responder maps a
    /// request to a response string; every received request is returned in order.
    fn with_socket_server(
        request_count: usize,
        responder: impl Fn(&Value) -> String + Send + 'static,
        test: impl FnOnce(&Path),
    ) -> Vec<Value> {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let handle = thread::spawn(move || {
            let mut requests = Vec::with_capacity(request_count);
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(line.trim()).unwrap();
                stream.write_all(responder(&request).as_bytes()).unwrap();
                requests.push(request);
            }
            requests
        });
        test(&socket_path);
        handle.join().unwrap()
    }

    fn with_socket_server_until_done(
        responder: impl FnMut(&Value) -> String + Send + 'static,
        test: impl FnOnce(&Path),
    ) -> Vec<Value> {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done_thread = done.clone();
        let handle = thread::spawn(move || {
            let mut responder = responder;
            let mut requests = Vec::new();
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let mut line = String::new();
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        reader.read_line(&mut line).unwrap();
                        let request: Value = serde_json::from_str(line.trim()).unwrap();
                        stream.write_all(responder(&request).as_bytes()).unwrap();
                        requests.push(request);
                    }
                    Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                        if done_thread.load(std::sync::atomic::Ordering::SeqCst) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => panic!("socket test server failed: {err}"),
                }
            }
            requests
        });
        let test_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            test(&socket_path);
        }));
        done.store(true, std::sync::atomic::Ordering::SeqCst);
        let requests = handle.join().unwrap();
        if let Err(payload) = test_result {
            std::panic::resume_unwind(payload);
        }
        requests
    }

    fn test_context() -> CliContext {
        CliContext {
            json: true,
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            socket_explicit: true,
            verbose: false,
        }
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn backup_count(dir: &Path, prefix: &str) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with(prefix))
            })
            .count()
    }

    #[test]
    fn doctor_report_includes_agent_integration_paths() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let codex_home = dir.path().join("codex");
        let claude_dir = dir.path().join("claude");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&claude_dir).unwrap();

        let home_s = home.display().to_string();
        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        with_env(
            &[
                ("HOME", Some(home_s.as_str())),
                ("CODEX_HOME", Some(codex_home_s.as_str())),
                ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
            ],
            || {
                let report = build_socket_doctor_report(&test_context());

                assert_eq!(
                    report["mcpConfigs"]["codex"]["path"],
                    json!(codex_home.join("config.toml").display().to_string())
                );
                assert_eq!(
                    report["mcpConfigs"]["claude"]["path"],
                    json!(home.join(".claude.json").display().to_string())
                );
                assert_eq!(
                    report["mcpConfigs"]["antigravity"]["path"],
                    json!(home
                        .join(".gemini/config/mcp_config.json")
                        .display()
                        .to_string())
                );
                assert_eq!(
                    report["skillDirs"]["agents"]["path"],
                    json!(home
                        .join(".agents/skills/forktty-agent-orchestration")
                        .display()
                        .to_string())
                );
                assert_eq!(
                    report["skillDirs"]["claude"]["path"],
                    json!(claude_dir
                        .join("skills/forktty-agent-orchestration")
                        .display()
                        .to_string())
                );

                let text = format_socket_doctor_text(&report);
                assert!(text.contains("mcp configs:\n"));
                assert!(text.contains("  codex:"));
                assert!(text.contains("  claude:"));
                assert!(text.contains("  antigravity:"));
                assert!(text.contains("skill dirs:\n"));
                assert!(text.contains("  agents:"));
            },
        );
    }

    #[test]
    fn parse_global_flags_after_command() {
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", None),
                ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            ],
            || {
                let parsed = parse_global_args(strings(&[
                    "ping",
                    "--socket",
                    "/tmp/forktty.sock",
                    "--json",
                ]))
                .unwrap();
                assert_eq!(parsed.args, vec!["ping"]);
                assert!(parsed.json);
                assert!(parsed.socket_explicit);
                assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty.sock"));

                let parsed = parse_global_args(strings(&[
                    "worktree-create",
                    "feature/x",
                    "--cwd",
                    "/repo",
                    "--socket=/tmp/forktty-2.sock",
                ]))
                .unwrap();
                assert_eq!(
                    parsed.args,
                    vec!["worktree-create", "feature/x", "--cwd", "/repo"]
                );
                assert!(parsed.socket_explicit);
                assert_eq!(parsed.socket_path, PathBuf::from("/tmp/forktty-2.sock"));

                let parsed = parse_global_args(strings(&[
                    "send-text",
                    "--",
                    "--socket",
                    "literal",
                    "--json",
                ]))
                .unwrap();
                assert_eq!(
                    parsed.args,
                    vec!["send-text", "--", "--socket", "literal", "--json"]
                );
                assert!(!parsed.json);
                assert!(!parsed.socket_explicit);
                assert_eq!(
                    parsed.socket_path,
                    PathBuf::from("/run/user/1000/forktty.sock")
                );

                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket="])),
                    "--socket requires a value",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket", "--json"])),
                    "--socket requires a value",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket", "relative.sock"])),
                    "--socket requires an absolute path",
                );
                assert_err_contains(
                    parse_global_args(strings(&["ping", "--socket=relative.sock"])),
                    "--socket requires an absolute path",
                );
            },
        );
    }

    #[test]
    #[cfg(not(feature = "browser"))]
    fn browser_command_is_disabled_after_global_options_without_feature() {
        assert_err_contains(
            run_inner(os_strings(&[
                "--json",
                "browser",
                "open",
                "https://example.com",
            ])),
            "--features browser",
        );
    }

    #[test]
    fn parse_flags_handles_boolean_options_and_terminator() {
        let parsed = parse_flags(strings(&["--dry-run", "codex"]), &["dry-run"]);
        assert_eq!(parsed.options.get("dry-run"), Some(&FlagValue::Bool));
        assert_eq!(parsed.positionals, vec!["codex"]);

        let parsed = parse_flags(strings(&["--title", "Heads up", "--", "--body"]), &[]);
        assert_eq!(
            parsed.options.get("title"),
            Some(&FlagValue::String("Heads up".to_string()))
        );
        assert_eq!(parsed.positionals, vec!["--body"]);

        let parsed = parse_flags(strings(&["--", "--help", "--literal"]), &[]);
        assert!(parsed.options.is_empty());
        assert_eq!(parsed.positionals, vec!["--help", "--literal"]);
    }

    #[test]
    fn default_socket_path_prefers_absolute_env_then_runtime_dir() {
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", Some("relative.sock")),
                ("XDG_RUNTIME_DIR", Some(" /run/user/1000 ")),
            ],
            || {
                assert_eq!(
                    default_socket_path(),
                    PathBuf::from("/run/user/1000/forktty.sock")
                );
            },
        );
        with_env(
            &[
                ("FORKTTY_SOCKET_PATH", Some(" /tmp/explicit.sock ")),
                ("XDG_RUNTIME_DIR", Some("/run/user/1000")),
            ],
            || {
                assert_eq!(default_socket_path(), PathBuf::from("/tmp/explicit.sock"));
            },
        );
    }

    #[test]
    fn socket_error_messages_keep_path_and_reason() {
        let missing = format_socket_connect_error(
            io::Error::new(io::ErrorKind::NotFound, "connect ENOENT"),
            Path::new("/tmp/forktty.sock"),
        );
        assert!(missing
            .message
            .contains("Cannot reach ForkTTY at /tmp/forktty.sock"));
        assert!(missing
            .message
            .contains("FORKTTY_SOCKET_PATH to an absolute path"));

        let denied = format_socket_connect_error(
            io::Error::new(io::ErrorKind::PermissionDenied, "connect EACCES"),
            Path::new("/run/user/1000/forktty.sock"),
        );
        assert!(denied.message.contains("Cannot access ForkTTY socket"));
        assert!(denied.message.contains("/run/user/1000/forktty.sock"));

        let reset = format_socket_connect_error(
            io::Error::new(io::ErrorKind::ConnectionReset, "read ECONNRESET"),
            Path::new("/tmp/forktty.sock"),
        );
        assert!(reset
            .message
            .contains("ForkTTY socket error at /tmp/forktty.sock"));
        assert!(reset.message.contains("read ECONNRESET"));
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_workspace_not_found() {
        with_socket_response(
            |request| {
                format!(
                    "{}\n",
                    json!({
                        "id": request["id"],
                        "ok": false,
                        "error": { "code": "not_found", "message": "Workspace not found" }
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(
                    socket_path,
                    "workspace.select",
                    json!({ "id": "missing" }),
                )
                .unwrap_err();
                assert_eq!(err.code.as_deref(), Some("not_found"));
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("workspace.select"));
                assert!(err.message.contains("not_found: Workspace not found"));
            },
        );
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_stale_response() {
        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": "stale-response",
                        "ok": true,
                        "result": "pong"
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("system.ping"));
                assert!(err.message.contains("response id mismatch"));
                assert!(err.message.contains("stale-response"));
            },
        );
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_payload_too_large() {
        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": null,
                        "ok": false,
                        "error": { "code": "payload_too_large", "message": "Request exceeds 1 MiB" }
                    })
                )
            },
            |socket_path| {
                let err =
                    send_socket_request(socket_path, "surface.send_text", json!({ "text": "x" }))
                        .unwrap_err();
                assert_eq!(err.code.as_deref(), Some("payload_too_large"));
                assert!(err.message.contains("surface.send_text"));
                assert!(err
                    .message
                    .contains("payload_too_large: Request exceeds 1 MiB"));
                assert!(!err.message.contains("response id mismatch"));
            },
        );
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_server_busy() {
        with_socket_response(
            |_| {
                format!(
                    "{}\n",
                    json!({
                        "id": null,
                        "ok": false,
                        "error": { "code": "server_busy", "message": "Too many active socket connections" }
                    })
                )
            },
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert_eq!(err.code.as_deref(), Some("server_busy"));
                assert!(err.message.contains("system.ping"));
                assert!(err
                    .message
                    .contains("server_busy: Too many active socket connections"));
                assert!(!err.message.contains("response id mismatch"));
            },
        );
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_invalid_json() {
        with_socket_response(
            |_| "not json\n".to_string(),
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert!(err.message.contains(&socket_path.display().to_string()));
                assert!(err.message.contains("system.ping"));
                assert!(err.message.contains("Invalid socket response"));
            },
        );
    }

    #[test]
    fn socket_response_errors_preserve_method_path_and_codes_response_too_large() {
        with_socket_response(
            |_| format!("{}\n", "x".repeat(MAX_SOCKET_RESPONSE_BYTES + 1)),
            |socket_path| {
                let err = send_socket_request(socket_path, "system.ping", json!({})).unwrap_err();
                assert_eq!(err.code.as_deref(), Some("response_too_large"));
                assert!(err.message.contains("socket response exceeds"));
            },
        );
    }

    #[test]
    fn socket_response_accepts_valid_large_official_response() {
        let result = "x".repeat(2 * 1024 * 1024);
        with_socket_response(
            move |request| {
                format!(
                    "{}\n",
                    json!({
                        "id": request["id"],
                        "ok": true,
                        "result": result
                    })
                )
            },
            |socket_path| {
                let value = send_socket_request(socket_path, "metadata.list_logs", json!({}))
                    .expect("large official response should be readable");
                assert_eq!(value.as_str().unwrap().len(), 2 * 1024 * 1024);
            },
        );
    }

    #[test]
    fn worktree_status_rejects_cwd_without_value() {
        assert_err_contains(
            handle_worktree_status(&test_context(), strings(&["--cwd"])),
            "--cwd requires a value",
        );
    }

    #[test]
    fn worktree_status_rejects_ambiguous_path_options() {
        assert_err_contains(
            handle_worktree_status(
                &test_context(),
                strings(&["--path", "/tmp/a", "--cwd", "/tmp/b"]),
            ),
            "worktree-status: cannot combine --path and --cwd",
        );
    }

    #[test]
    fn worktree_status_rejects_positional_combined_with_path_or_cwd() {
        assert_err_contains(
            handle_worktree_status(&test_context(), strings(&["--path", "/tmp/a", "/tmp/b"])),
            "worktree-status: cannot combine a positional path with --path or --cwd",
        );
        assert_err_contains(
            handle_worktree_status(&test_context(), strings(&["--cwd", "/tmp/a", "/tmp/b"])),
            "worktree-status: cannot combine a positional path with --path or --cwd",
        );
    }

    #[test]
    fn write_output_line_treats_closed_pipe_as_success() {
        let (mut writer, reader) = std::os::unix::net::UnixStream::pair().unwrap();
        drop(reader);
        // Prove the transport reports BrokenPipe once the reader is gone (the
        // kernel may buffer an initial write before failing)…
        let mut raw = None;
        for _ in 0..64 {
            if let Err(err) = writer.write_all(b"x\n") {
                raw = Some(err);
                break;
            }
        }
        assert_eq!(
            raw.expect("write to closed pipe must fail").kind(),
            io::ErrorKind::BrokenPipe
        );
        // …and that the helper converts it to silent success, so
        // `forktty list --json | head -1` exits 0 instead of panicking.
        assert!(write_output_line(&mut writer, "payload").is_ok());

        // Other write errors still surface as CLI errors.
        struct FailWriter;
        impl Write for FailWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("disk full"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert_err_contains(write_output_line(&mut FailWriter, "payload"), "disk full");
    }

    #[test]
    fn hook_event_order_is_monotone_non_decreasing() {
        let mut previous = next_hook_event_order()
            .parse::<u128>()
            .expect("order is numeric");
        for _ in 0..100 {
            let next = next_hook_event_order()
                .parse::<u128>()
                .expect("order is numeric");
            assert!(next >= previous, "{next} went backwards from {previous}");
            previous = next;
        }
    }

    #[test]
    fn hooks_typos_and_missing_subcommands_error_instead_of_exiting_zero() {
        assert_err_contains(
            handle_hooks(&test_context(), strings(&["setupp"])),
            "Unsupported hooks subcommand or agent: setupp",
        );
        assert_err_contains(
            handle_hooks(&test_context(), Vec::new()),
            "hooks requires a subcommand",
        );
    }

    #[test]
    fn hooks_keep_lenient_continue_json_for_future_events_of_known_agents() {
        // Generated hook templates can outlive this binary: an event added by
        // a newer template must not fail the agent's hook invocation.
        handle_hooks(&test_context(), strings(&["claude", "some-future-event"]))
            .expect("unknown event for a known agent stays lenient");
    }

    #[test]
    fn read_json_file_rejects_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("hooks.json");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        // Watchdog: before the metadata-first check, open(2) blocked forever
        // waiting for a FIFO peer, so a regression would hang the suite.
        let (sender, receiver) = std::sync::mpsc::channel();
        let probe = thread::spawn(move || {
            sender.send(read_json_file(&fifo).map(|_| ())).ok();
        });
        let result = receiver
            .recv_timeout(Duration::from_secs(10))
            .expect("read_json_file must not block on a FIFO");
        assert_err_contains(result, "path exists but is not a regular file");
        probe.join().unwrap();
    }

    #[test]
    fn bounded_connect_reaches_an_accepting_listener() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ok.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            stream.write_all(b"pong\n").unwrap();
            line
        });

        let mut stream =
            connect_unix_stream_with_timeout(&socket_path, Duration::from_secs(5)).unwrap();
        stream.write_all(b"ping\n").unwrap();
        // The blocking read also proves O_NONBLOCK was cleared after connect.
        let mut reply = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut reply)
            .unwrap();
        assert_eq!(reply, "pong\n");
        assert_eq!(server.join().unwrap(), "ping\n");
    }

    #[test]
    fn bounded_connect_errors_within_the_timeout_when_backlog_is_full() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("busy.sock");
        let (addr, addr_len) = unix_socket_address(&socket_path).unwrap();
        // Hand-rolled listener with a zero backlog that never accepts.
        // SAFETY: plain socket(2); the result is checked before use.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        assert!(fd >= 0, "{}", io::Error::last_os_error());
        // SAFETY: freshly created descriptor owned by no one else.
        let listener = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: `addr` is a valid sockaddr_un of `addr_len` bytes.
        let bound = unsafe {
            libc::bind(
                listener.as_raw_fd(),
                &addr as *const libc::sockaddr_un as *const libc::sockaddr,
                addr_len,
            )
        };
        assert_eq!(bound, 0, "{}", io::Error::last_os_error());
        // SAFETY: listen(2) on the bound descriptor.
        assert_eq!(unsafe { libc::listen(listener.as_raw_fd(), 0) }, 0);

        // Saturate the backlog (listen(0) still admits a pending connection or
        // two); hold the streams so they stay queued.
        let mut held = Vec::new();
        let saturated = loop {
            match connect_unix_stream_with_timeout(&socket_path, Duration::from_millis(200)) {
                Ok(stream) => held.push(stream),
                Err(_) => break true,
            }
            if held.len() > 16 {
                break false;
            }
        };
        assert!(saturated, "accept backlog never filled");

        // `UnixStream::connect` would block here forever; the bounded variant
        // must fail within its timeout.
        let start = Instant::now();
        let result = connect_unix_stream_with_timeout(&socket_path, Duration::from_millis(300));
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "bounded connect took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn notify_rejects_blank_title_option() {
        assert_err_contains(
            handle_notify(&test_context(), strings(&["--title=", "--body", "body"])),
            "--title requires a value",
        );
    }

    #[test]
    fn status_color_validation_requires_hex_digits() {
        assert!(is_supported_status_color("green"));
        assert!(is_supported_status_color("#abc"));
        assert!(is_supported_status_color("#a1B2c3"));
        assert!(is_supported_status_color("#a1B2c3D4"));
        assert!(!is_supported_status_color("#"));
        assert!(!is_supported_status_color("#12"));
        assert!(!is_supported_status_color("#nothex"));

        assert_err_contains(
            handle_set_status(
                &test_context(),
                strings(&[
                    "--key",
                    "agent:codex",
                    "--value",
                    "Running",
                    "--color",
                    "#12",
                ]),
            ),
            "Unsupported status color: #12",
        );
    }

    #[test]
    fn formatting_and_target_helpers_match_cli_contract() {
        assert_eq!(
            format_notification_line(&json!({
                "read": false,
                "kind": "info",
                "title": "Smoke",
                "body": "GTK"
            })),
            "[unread] global · info · Smoke — GTK"
        );

        let options = BTreeMap::from([("body".to_string(), FlagValue::String(String::new()))]);
        assert!(should_read_stdin(&BTreeMap::new(), &[], "text"));
        assert!(!should_read_stdin(
            &BTreeMap::from([("text".to_string(), FlagValue::String("echo ok".to_string()))]),
            &[],
            "text"
        ));
        assert!(!should_read_stdin(
            &BTreeMap::new(),
            &["echo".to_string()],
            "text"
        ));
        assert!(!should_read_stdin(&options, &[], "body"));

        with_env(&[("FORKTTY_WORKSPACE_ID", Some(" ws-1 "))], || {
            let params = build_target_params(&BTreeMap::new(), "set-status").unwrap();
            assert_eq!(params["workspace_id"], Value::String("ws-1".to_string()));
        });

        let selectors = BTreeMap::from([
            (
                "workspace-id".to_string(),
                FlagValue::String("ws-1".to_string()),
            ),
            (
                "workspace-name".to_string(),
                FlagValue::String("main".to_string()),
            ),
        ]);
        assert_err_contains(
            build_target_params(&selectors, "set-progress"),
            "set-progress: cannot combine --workspace-id and --workspace-name",
        );

        let snapshot_params = context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-team-details".to_string(),
                "--include-workflow-details".to_string(),
                "--include-feed-trace".to_string(),
            ],
            "context-snapshot",
        )
        .unwrap();
        assert_eq!(snapshot_params["include_team_details"], Value::Bool(true));
        assert_eq!(
            snapshot_params["include_workflow_details"],
            Value::Bool(true)
        );
        assert_eq!(snapshot_params["include_feed_trace"], Value::Bool(true));
        let compact_snapshot_params = context_snapshot_params(
            vec![
                "--surface-id".to_string(),
                "surface-1".to_string(),
                "--include-team-details=false".to_string(),
                "--include-workflow-details=false".to_string(),
                "--include-feed-trace=false".to_string(),
            ],
            "context-snapshot",
        )
        .unwrap();
        assert_eq!(
            compact_snapshot_params["include_team_details"],
            Value::Bool(false)
        );
        assert_eq!(
            compact_snapshot_params["include_workflow_details"],
            Value::Bool(false)
        );
        assert_eq!(
            compact_snapshot_params["include_feed_trace"],
            Value::Bool(false)
        );
        assert_err_contains(
            context_snapshot_params(
                vec![
                    "--surface-id".to_string(),
                    "surface-1".to_string(),
                    "--include-team-details=maybe".to_string(),
                ],
                "context-snapshot",
            ),
            "--include-team-details expects true or false",
        );
        assert_err_contains(
            context_snapshot_params(
                vec![
                    "--surface-id".to_string(),
                    "surface-1".to_string(),
                    "--include-workflow-details=maybe".to_string(),
                ],
                "context-snapshot",
            ),
            "--include-workflow-details expects true or false",
        );
        assert_err_contains(
            context_snapshot_params(
                vec![
                    "--surface-id".to_string(),
                    "surface-1".to_string(),
                    "--include-feed-trace=maybe".to_string(),
                ],
                "context-snapshot",
            ),
            "--include-feed-trace expects true or false",
        );
    }

    #[test]
    fn stdin_reader_rejects_oversized_text() {
        let mut accepted = std::io::Cursor::new(b"abc".to_vec());
        assert_eq!(
            read_text_from_reader(&mut accepted, 3, "stdin").unwrap(),
            "abc"
        );

        let mut oversized = std::io::Cursor::new(b"abcd".to_vec());
        assert_err_contains(
            read_text_from_reader(&mut oversized, 3, "stdin"),
            "stdin exceeds 3 byte limit",
        );
    }

    #[test]
    fn hook_actions_cover_attention_status_tools_and_shutdown() {
        with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-1")),
                ("FORKTTY_SURFACE_ID", Some("surface-1")),
            ],
            || {
                let claude = agent_spec("claude").unwrap();
                let actions = build_hook_actions(
                    claude,
                    "notification",
                    &json!({ "message": "Review needed" }),
                    "12345",
                );
                assert_eq!(actions.len(), 3);
                assert_eq!(actions[0].0, "metadata.log");
                assert_eq!(actions[0].1["workspace_id"], "ws-1");
                assert_eq!(actions[0].1["surface_id"], "surface-1");
                assert_eq!(actions[0].1["level"], "warn");
                assert_eq!(actions[0].1["message"], "Review needed");
                assert_eq!(actions[1].0, "metadata.set_status");
                assert_eq!(actions[1].1["key"], "agent:claude");
                assert_eq!(actions[1].1["value"], "Needs input");
                assert_eq!(actions[1].1["color"], "yellow");
                assert_eq!(actions[1].1["hook_event_name"], "notification");
                assert_eq!(actions[2].0, "notification.create");
                assert_eq!(actions[2].1["title"], "Claude needs input");
                assert_eq!(actions[2].1["kind"], "prompt");

                let actions = build_hook_actions(
                    claude,
                    "pre-tool",
                    &json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } }),
                    "77",
                );
                assert_eq!(actions[0].1["message"], "Claude running Bash");
                assert_eq!(actions[1].1["value"], "Running");
                assert_eq!(actions[1].1["hook_event_order"], "77");

                let actions = build_hook_actions(
                    claude,
                    "post-tool",
                    &json!({ "tool_name": "Bash", "tool_response": { "is_error": true } }),
                    "78",
                );
                assert_eq!(actions.len(), 2);
                assert_eq!(actions[0].1["level"], "error");
                assert!(actions[0].1["message"]
                    .as_str()
                    .unwrap()
                    .contains("Bash reported an error"));
                assert_eq!(actions[1].0, "notification.create");
                assert_eq!(actions[1].1["kind"], "error");
            },
        );
    }

    #[test]
    fn hook_payload_extraction_sanitizes_and_hashes_sensitive_text() {
        assert_eq!(
            sanitize_for_terminal("bad\u{1b}[31m\nnext"),
            "bad\\x1b[31m\\nnext"
        );
        assert_eq!(
            hook_check_error_for_terminal(
                &json!({ "method": "system.ping", "ok": false, "error": "bad\u{1b}]0;title\u{7}\nnext" })
            ),
            "bad\\x1b]0;title\\x07\\nnext"
        );
        assert_eq!(
            extract_hook_tool_name(&json!({ "tool_name": "Bash\u{1b}[31m" })).unwrap(),
            "Bash\\x1b[31m"
        );
        let long = "a".repeat(120);
        let tool = extract_hook_tool_name(&json!({ "tool_name": long })).unwrap();
        assert_eq!(tool.chars().count(), HOOK_TOOL_LABEL_MAX);
        assert!(tool.ends_with("..."));

        // Documented top-level signals inside the tool result are detected.
        assert!(extract_hook_tool_error(
            &json!({ "tool_response": { "is_error": true } })
        ));
        assert!(extract_hook_tool_error(
            &json!({ "tool_response": { "isError": true } })
        ));
        assert!(extract_hook_tool_error(
            &json!({ "tool_response": { "error": { "message": "bad" } } })
        ));
        assert!(!extract_hook_tool_error(
            &json!({ "tool_response": { "is_error": false } })
        ));
        // Regression: Codex PostToolUse payloads carry nested tool output that
        // can legitimately contain `error` keys on success. A non-error result
        // with a deeply nested `error` object must NOT be flagged (previously a
        // recursive scan produced spurious sidebar errors on routine use).
        assert!(!extract_hook_tool_error(&json!({
            "tool_name": "my_mcp_tool",
            "tool_response": {
                "isError": false,
                "structuredContent": { "result": { "error": { "code": "NONE", "message": "no error" } } }
            }
        })));
        // A deeply nested error outside the tool result is likewise ignored.
        assert!(!extract_hook_tool_error(
            &json!({ "result": { "error": { "message": "bad" } } })
        ));

        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "prompt-submit",
            &json!({ "prompt": "ship the secret feature" }),
            "12345",
        );
        let turn_id = actions[1].1["hook_turn_id"].as_str().unwrap();
        assert!(turn_id.starts_with("prompt:"));
        assert!(!turn_id.contains("secret"));

        assert_eq!(
            extract_hook_source(&json!({ "source": "resume" })).as_deref(),
            Some("resume")
        );
        assert_eq!(
            extract_hook_compact_trigger(&json!({ "compactTrigger": "manual" })).as_deref(),
            Some("manual")
        );
    }

    #[test]
    fn human_formatters_escape_socket_payload_control_sequences() {
        let workspace_line = format_workspace_line(&json!({
            "active": true,
            "name": "bad\u{1b}[31m\nname",
            "id": "workspace\u{1b}",
            "gitBranch": "main\tbranch",
            "workingDir": "/tmp\rdir",
            "surfaces": 1,
        }));
        assert!(workspace_line.contains("bad\\x1b[31m\\nname"));
        assert!(workspace_line.contains("main\\tbranch"));
        assert!(!workspace_line.contains('\u{1b}'));
        assert!(!workspace_line.contains('\n'));

        let surface_line = format_surface_line(&json!({
            "id": "surface\u{1b}",
            "workspace_id": "workspace\n",
            "title": "build\r",
            "cwd": "/tmp\tforktty",
        }));
        assert!(surface_line.contains("surface\\x1b"));
        assert!(surface_line.contains("workspace\\n"));
        assert!(surface_line.contains("/tmp\\tforktty"));
        assert!(!surface_line.contains('\u{1b}'));

        let status_line = format_status_line(&json!({
            "label": "agent\u{1b}",
            "value": "running\n",
            "color": "red\r",
        }));
        assert_eq!(status_line, "agent\\x1b: running\\n (red\\r)");

        let progress_line = format_progress_line(&json!({
            "label": "task\u{1b}",
            "value": "5\n",
            "total": "10\r",
        }));
        assert_eq!(progress_line, "task\\x1b: 5\\n/10\\r");

        let notification_line = format_notification_line(&json!({
            "workspaceName": "main\u{1b}",
            "kind": "prompt\n",
            "title": "Needs\rinput",
            "body": "Run\ttool",
        }));
        assert!(notification_line.contains("main\\x1b"));
        assert!(notification_line.contains("prompt\\n"));
        assert!(notification_line.contains("Needs\\rinput"));
        assert!(notification_line.contains("Run\\ttool"));
        assert!(!notification_line.contains('\u{1b}'));
    }

    #[test]
    fn token_usage_feeds_claude_context_without_notification_text() {
        let usage = TokenUsage {
            input: 1_000,
            cache_read: 4_000,
            cache_creation: 500,
            output: 250,
        };
        let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
            format_token_usage_block(usage)
        });
        assert!(block.contains("5,500 / 200,000 input tokens"));
        assert!(block.contains("input=1000"));
        assert!(block.contains("cache_read=4000"));

        let block = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", Some("50000"))], || {
            format_token_usage_block(TokenUsage {
                input: 1_000,
                cache_read: 9_000,
                cache_creation: 0,
                output: 0,
            })
        });
        assert!(block.contains("10,000 / 50,000 input tokens"));
        assert!(block.contains("20%"));

        let progress = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-A")),
                ("FORKTTY_HOOK_TOKEN_CEILING", Some("12345")),
            ],
            || {
                build_token_progress_action(
                    agent_spec("claude").unwrap(),
                    &HookEnrichments {
                        token_usage: Some(TokenUsage {
                            input: 100,
                            cache_read: 200,
                            cache_creation: 50,
                            output: 10,
                        }),
                        workspace: None,
                    },
                    "prompt-submit",
                    "77",
                )
                .unwrap()
            },
        );
        assert_eq!(progress["workspace_id"], "ws-A");
        assert_eq!(progress["key"], "agent:claude:tokens");
        assert_eq!(progress["value"], 350);
        assert_eq!(progress["total"], 12345);
        assert_eq!(progress["hook_event_order"], "77");
    }

    #[test]
    fn token_usage_totals_saturate_on_extreme_transcript_values() {
        let usage = TokenUsage {
            input: u64::MAX,
            cache_read: 1,
            cache_creation: 1,
            output: 0,
        };

        // The ceiling env var must be held steady: concurrent tests set it via
        // with_env, and an unguarded read races with them.
        let (block, progress) = with_env(&[("FORKTTY_HOOK_TOKEN_CEILING", None)], || {
            (
                format_token_usage_block(usage),
                build_token_progress_action(
                    agent_spec("claude").unwrap(),
                    &HookEnrichments {
                        token_usage: Some(usage),
                        workspace: None,
                    },
                    "prompt-submit",
                    "88",
                )
                .unwrap(),
            )
        });
        assert!(block.contains("18,446,744,073,709,551,615 / 200,000 input tokens"));
        assert_eq!(progress["value"], json!(u64::MAX));
    }

    #[test]
    fn hook_response_adds_context_only_for_supported_claude_events() {
        let response = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("ws-4")),
                ("FORKTTY_SURFACE_ID", Some("surface-9")),
                ("FORKTTY_SOCKET_PATH", Some("/tmp/forktty.sock")),
            ],
            || {
                build_hook_response(
                    agent_spec("claude").unwrap(),
                    "session-start",
                    &HookEnrichments {
                        token_usage: None,
                        workspace: Some(HookWorkspaceContext {
                            name: "Feature Shell".to_string(),
                            git_branch: Some("feature/mcp".to_string()),
                        }),
                    },
                )
                .unwrap()
            },
        );
        assert_eq!(response["continue"], true);
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        let context = response["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("ForkTTY"));
        assert!(context.contains("ws-4"));
        assert!(context.contains("surface-9"));
        assert!(context.contains("forktty.sock"));
        assert!(context.contains("Feature Shell on branch feature/mcp"));
        assert!(context.contains("context_snapshot gives a compact read-only view"));
        assert!(context.contains("workspace_list, surface_list, topology_tree"));
        assert!(context.contains("surface_read_text"));
        assert!(context.contains("worktree_create creates an isolated git worktree"));
        assert!(context.contains("SSH remote inventory"));
        assert!(context.contains("remote_list/status"));
        assert!(context.contains(
            "For ordinary edits in the current repo, work normally; do not call ForkTTY tools just to edit files."
        ));
        assert_eq!(
            response["hookSpecificOutput"]["sessionTitle"],
            "Feature Shell"
        );
        assert!(!context.contains("ForkTTY pending notifications:"));

        let response = build_hook_response(
            agent_spec("claude").unwrap(),
            "prompt-submit",
            &HookEnrichments {
                token_usage: Some(TokenUsage {
                    input: 500,
                    cache_read: 1_000,
                    cache_creation: 0,
                    output: 50,
                }),
                workspace: None,
            },
        )
        .unwrap();
        assert_eq!(
            response["hookSpecificOutput"]["hookEventName"],
            "UserPromptSubmit"
        );
        let context = response["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(!context.contains("ForkTTY pending notifications:"));
        assert!(context.contains("1,500 / 200,000 input tokens"));

        let plain = build_hook_response(
            agent_spec("claude").unwrap(),
            "prompt-submit",
            &HookEnrichments {
                token_usage: None,
                workspace: None,
            },
        )
        .unwrap();
        assert_eq!(
            plain,
            serde_json::from_str::<Value>(HOOK_CONTINUE_JSON.trim()).unwrap()
        );
        let codex = build_hook_response(
            agent_spec("codex").unwrap(),
            "session-start",
            &HookEnrichments {
                token_usage: None,
                workspace: None,
            },
        )
        .unwrap();
        assert_eq!(codex, plain);
    }

    #[test]
    fn antigravity_pre_tool_response_explicitly_allows_tool_use() {
        let response = build_hook_response(
            agent_spec("antigravity").unwrap(),
            "pre-tool",
            &HookEnrichments {
                token_usage: None,
                workspace: None,
            },
        )
        .unwrap();
        assert_eq!(response, json!({ "decision": "approve" }));

        let response = build_hook_response(
            agent_spec("antigravity").unwrap(),
            "post-tool",
            &HookEnrichments {
                token_usage: None,
                workspace: None,
            },
        )
        .unwrap();
        assert_eq!(response, json!({}));
    }

    #[test]
    fn antigravity_pre_tool_wrapper_fallback_explicitly_allows_tool_use() {
        let spec = agent_spec("antigravity").unwrap();
        let pre_tool =
            build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "pre-tool");
        assert!(pre_tool.contains("printf '%s\\n' '{\"decision\":\"approve\"}'"));

        let post_tool =
            build_antigravity_hook_script(Path::new("/usr/bin/forktty"), spec, "post-tool");
        assert!(post_tool.contains("printf '%s\\n' '{}'"));
    }

    #[test]
    fn permission_mode_publishes_separate_status_for_codex_and_claude() {
        // Providers can emit permission state in lifecycle payloads. Keep it
        // as a sibling status so it never collides with `agent:<key>`
        // activity.
        let claude_payload = json!({
            "session_id": "sess-claude-1",
            "permission_mode": "acceptEdits",
            "transcript_path": "/tmp/transcript.jsonl"
        });
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-start",
            &claude_payload,
            "1",
        );
        assert_eq!(actions.len(), 3);
        let permission = &actions[2];
        assert_eq!(permission.0, "metadata.set_status");
        assert_eq!(permission.1["key"], "agent:claude:permission");
        assert_eq!(permission.1["label"], "Claude mode");
        assert_eq!(permission.1["value"], "acceptEdits");
        // Claude's acceptEdits auto-accepts file writes -> documented risk.
        assert_eq!(permission.1["color"], "yellow");
        assert_eq!(permission.1["hook_session_id"], "sess-claude-1");

        let codex_payload = json!({
            "session_id": "sess-codex-9",
            "permission_mode": "on-request",
            "model": "gpt-5",
        });
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "prompt-submit",
            &codex_payload,
            "2",
        );
        assert_eq!(actions.len(), 3);
        let permission = &actions[2];
        assert_eq!(permission.1["key"], "agent:codex:permission");
        assert_eq!(permission.1["label"], "Codex mode");
        assert_eq!(permission.1["value"], "on-request");
        assert_eq!(permission.1["hook_session_id"], "sess-codex-9");
    }

    #[test]
    fn hook_status_metadata_includes_current_working_directory() {
        let project_dir = tempfile::tempdir().unwrap();
        let payload = json!({
            "session_id": "sess-codex-9",
            "model": "gpt-5",
        });

        let actions = with_current_dir(project_dir.path(), || {
            build_hook_actions(agent_spec("codex").unwrap(), "prompt-submit", &payload, "2")
        });

        let status = actions
            .iter()
            .find(|(method, params)| {
                method == "metadata.set_status" && params["key"] == "agent:codex"
            })
            .expect("codex status action");
        assert_eq!(
            status.1["hook_session_cwd"],
            project_dir.path().to_string_lossy().as_ref()
        );
    }

    #[test]
    fn antigravity_hook_status_metadata_uses_workspace_paths_instead_of_wrapper_cwd() {
        let wrapper_dir = tempfile::tempdir().unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let payload = json!({
            "common": {
                "conversationId": "agy-session-1",
                "workspacePaths": [project_dir.path().to_string_lossy()],
            },
            "preToolHookArgs": {
                "toolCall": { "name": "shell" },
            },
        });

        let actions = with_current_dir(wrapper_dir.path(), || {
            build_hook_actions(
                agent_spec("antigravity").unwrap(),
                "pre-tool",
                &payload,
                "3",
            )
        });

        let status = actions
            .iter()
            .find(|(method, params)| {
                method == "metadata.set_status" && params["key"] == "agent:antigravity"
            })
            .expect("antigravity status action");
        assert_eq!(status.1["hook_session_id"], "agy-session-1");
        assert_eq!(
            status.1["hook_session_cwd"],
            project_dir.path().to_string_lossy().as_ref()
        );
    }

    #[test]
    fn antigravity_hook_status_metadata_omits_wrapper_cwd_without_workspace_paths() {
        let wrapper_dir = tempfile::tempdir().unwrap();
        let payload = json!({
            "conversationId": "agy-session-2",
            "toolName": "shell",
        });

        let actions = with_current_dir(wrapper_dir.path(), || {
            build_hook_actions(
                agent_spec("antigravity").unwrap(),
                "pre-tool",
                &payload,
                "4",
            )
        });

        let status = actions
            .iter()
            .find(|(method, params)| {
                method == "metadata.set_status" && params["key"] == "agent:antigravity"
            })
            .expect("antigravity status action");
        assert_eq!(status.1["hook_session_id"], "agy-session-2");
        assert!(status.1.get("hook_session_cwd").is_none());
    }

    #[test]
    fn claude_permission_mode_colors_track_documented_risk() {
        // Claude Code docs enumerate permission_mode as
        // default|plan|acceptEdits|auto|dontAsk|bypassPermissions.
        // bypassPermissions is the most dangerous and should surface in
        // red; modes that suppress per-action consent surface in yellow;
        // default/plan remain muted.
        let claude = agent_spec("claude").unwrap();
        assert_eq!(permission_mode_color(claude, "bypassPermissions"), "red");
        for warn in ["acceptEdits", "auto", "dontAsk"] {
            assert_eq!(permission_mode_color(claude, warn), "yellow");
        }
        for safe in ["default", "plan"] {
            assert_eq!(permission_mode_color(claude, safe), "muted");
        }
        // Unknown enum value stays muted instead of guessing risk.
        assert_eq!(permission_mode_color(claude, "futureMode"), "muted");
    }

    #[test]
    fn codex_permission_mode_colors_track_documented_risk() {
        let codex = agent_spec("codex").unwrap();
        assert_eq!(permission_mode_color(codex, "bypassPermissions"), "red");
        for warn in ["acceptEdits", "auto", "dontAsk"] {
            assert_eq!(permission_mode_color(codex, warn), "yellow");
        }
        for mode in ["default", "plan", "on-request", "futureMode"] {
            assert_eq!(permission_mode_color(codex, mode), "muted");
        }
    }

    #[test]
    fn build_hook_actions_paints_bypass_permissions_red_for_documented_providers() {
        let claude_actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-start",
            &json!({ "permission_mode": "bypassPermissions" }),
            "1",
        );
        let claude_permission = claude_actions.last().expect("permission action");
        assert_eq!(claude_permission.1["key"], "agent:claude:permission");
        assert_eq!(claude_permission.1["color"], "red");

        let codex_actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "session-start",
            &json!({ "permission_mode": "bypassPermissions" }),
            "1",
        );
        let codex_permission = codex_actions.last().expect("permission action");
        assert_eq!(codex_permission.1["key"], "agent:codex:permission");
        assert_eq!(codex_permission.1["color"], "red");
    }

    #[test]
    fn permission_status_omitted_when_payload_has_no_permission_mode() {
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "session-start",
            &json!({ "session_id": "sess-codex-no-mode" }),
            "1",
        );
        assert_eq!(actions.len(), 2);
        for (_, params) in &actions {
            assert_ne!(params["key"], "agent:codex:permission");
        }
    }

    #[test]
    fn stop_preserves_permission_status_when_session_end_hook_exists() {
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "stop",
            &json!({ "session_id": "sess-claude-stop" }),
            "11",
        );
        assert_eq!(actions.len(), 2);
        for (method, params) in &actions {
            assert_ne!(method, "metadata.clear_status");
            assert_ne!(params["key"], "agent:claude:permission");
        }
    }

    #[test]
    fn stop_clears_permission_status_when_no_session_end_hook_exists() {
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "stop",
            &json!({ "session_id": "sess-codex-stop" }),
            "11",
        );
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[2].0, "metadata.clear_status");
        assert_eq!(actions[2].1["key"], "agent:codex:permission");
        assert_eq!(actions[2].1["hook_session_id"], "sess-codex-stop");
    }

    #[test]
    fn session_end_clears_activity_and_permission_status() {
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "session-end",
            &json!({ "session_id": "sess-claude-end" }),
            "9",
        );
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[1].0, "metadata.clear_status");
        assert_eq!(actions[1].1["key"], "agent:claude");
        assert_eq!(actions[2].0, "metadata.clear_status");
        assert_eq!(actions[2].1["key"], "agent:claude:permission");
        // hook_session_id rides on every metadata action so the daemon can
        // correlate the clear with its originating session.
        assert_eq!(actions[1].1["hook_session_id"], "sess-claude-end");
        assert_eq!(actions[2].1["hook_session_id"], "sess-claude-end");
    }

    #[test]
    fn hook_metadata_includes_codex_turn_id_extension() {
        // Codex CLI hook payloads add `turn_id` to turn-scoped events.
        let actions = build_hook_actions(
            agent_spec("codex").unwrap(),
            "pre-tool",
            &json!({
                "session_id": "sess-codex-turn",
                "turn_id": "turn-42",
                "tool_name": "shell",
            }),
            "5",
        );
        assert_eq!(actions[1].0, "metadata.set_status");
        let turn = actions[1].1["hook_turn_id"]
            .as_str()
            .expect("hook_turn_id encoded");
        assert!(turn.starts_with("id:"));
        // Claude's PreToolUse payload uses `tool_use_id` and `tool_input`.
        // Claude documents `tool_use_id` as per-tool-call (not per-turn), so
        // we deliberately don't promote it to hook_turn_id; instead
        // session_id rides on every metadata action so the daemon can
        // correlate logs and statuses across the tool invocation.
        let actions = build_hook_actions(
            agent_spec("claude").unwrap(),
            "pre-tool",
            &json!({
                "session_id": "sess-claude-tool",
                "tool_use_id": "toolu_abc",
                "tool_name": "Bash",
                "tool_input": { "command": "ls" },
            }),
            "6",
        );
        assert_eq!(actions[1].0, "metadata.set_status");
        assert_eq!(actions[1].1["hook_session_id"], "sess-claude-tool");
        assert_eq!(actions[1].1["value"], "Running");
        assert!(actions[1].1.get("hook_turn_id").is_none());
    }

    #[test]
    fn doctor_supported_events_track_installed_entries_per_provider() {
        let codex_events: Vec<&str> = agent_spec("codex")
            .unwrap()
            .hook_entries
            .iter()
            .map(|entry| entry.event_name)
            .collect();
        assert_eq!(
            codex_events,
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PermissionRequest",
                "PreCompact",
                "PostCompact",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ]
        );
        let claude_events: Vec<&str> = agent_spec("claude")
            .unwrap()
            .hook_entries
            .iter()
            .map(|entry| entry.event_name)
            .collect();
        assert_eq!(
            claude_events,
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "UserPromptExpansion",
                "Setup",
                "PreToolUse",
                "PermissionRequest",
                "PermissionDenied",
                "PostToolUse",
                "PostToolUseFailure",
                "PostToolBatch",
                "SubagentStart",
                "SubagentStop",
                "TaskCreated",
                "TaskCompleted",
                "Elicitation",
                "ElicitationResult",
                "PreCompact",
                "PostCompact",
                "Stop",
                "StopFailure",
                "TeammateIdle",
                "Notification",
                "ConfigChange",
                "InstructionsLoaded",
                "CwdChanged",
                "FileChanged",
                "WorktreeCreate",
                "WorktreeRemove",
                "SessionEnd",
            ]
        );
        // Codex docs do not list Notification or SessionEnd, so the Codex
        // installer must never target them.
        assert!(!codex_events.contains(&"Notification"));
        assert!(!codex_events.contains(&"SessionEnd"));

        let opencode_events: Vec<&str> = agent_spec("opencode")
            .unwrap()
            .hook_entries
            .iter()
            .map(|entry| entry.event_name)
            .collect();
        assert!(opencode_events.contains(&"tool.execute.before"));
        assert!(opencode_events.contains(&"permission.asked"));
    }

    #[test]
    fn installed_hook_events_are_supported_and_render_actions() {
        for spec in AGENTS {
            for entry in spec.hook_entries {
                assert!(
                    is_supported_hook_event(entry.hook_event_name),
                    "{} installs unsupported hook event {}",
                    spec.key,
                    entry.hook_event_name
                );

                let actions = build_hook_actions(
                    spec,
                    entry.hook_event_name,
                    &json!({ "session_id": "sess-installed-hook" }),
                    "42",
                );
                assert!(
                    !actions.is_empty(),
                    "{} {} produced no actions",
                    spec.key,
                    entry.hook_event_name
                );
            }
        }
    }

    #[test]
    fn transcript_usage_reader_returns_latest_usage() {
        let dir = tempfile::tempdir().unwrap();
        let transcript = dir.path().join("transcript.jsonl");
        fs::write(
            &transcript,
            format!(
                "{}\n{}\n{}\n",
                json!({ "type": "user", "message": { "content": "hi" } }),
                json!({
                    "type": "assistant",
                    "message": {
                        "usage": {
                            "input_tokens": 1200,
                            "output_tokens": 80,
                            "cache_read_input_tokens": 4500,
                            "cache_creation_input_tokens": 300
                        }
                    }
                }),
                json!({ "type": "tool_use" })
            ),
        )
        .unwrap();
        let usage = read_token_usage_from_transcript(&transcript).unwrap();
        assert_eq!(usage.input, 1200);
        assert_eq!(usage.output, 80);
        assert_eq!(usage.cache_read, 4500);
        assert_eq!(usage.cache_creation, 300);
        assert!(read_token_usage_from_transcript(&dir.path().join("missing.jsonl")).is_none());
        assert!(read_token_usage_from_transcript(dir.path()).is_none());
    }

    #[test]
    fn hook_setup_writes_all_agent_configs_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex home");
        let claude_dir = dir.path().join("claude config");
        let home = dir.path().join("home dir");
        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = home.display().to_string();

        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["codex", "claude", "opencode", "codex"]))
                    .unwrap();

                let codex_path = codex_home.join("hooks.json");
                let claude_path = claude_dir.join("settings.json");
                let opencode_path = home
                    .join(".config/opencode")
                    .join("plugins/forktty.generated.js");
                let codex = read_json(&codex_path);
                assert!(codex["hooks"]["SessionStart"].is_array());
                assert!(codex["hooks"]["PermissionRequest"].is_array());
                assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks codex pre-tool"));
                assert!(!codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains("forktty.mjs"));

                let claude = read_json(&claude_path);
                for event in [
                    "PermissionRequest",
                    "SubagentStart",
                    "SubagentStop",
                    "PreCompact",
                    "PostCompact",
                    "StopFailure",
                    "SessionEnd",
                ] {
                    assert!(claude["hooks"][event].is_array(), "missing {event}");
                }
                for event in [
                    "PreToolUse",
                    "PostToolUse",
                    "PostToolUseFailure",
                    "PostToolBatch",
                ] {
                    assert!(
                        claude["hooks"].get(event).is_none(),
                        "default Claude setup should omit {event}"
                    );
                }
                assert!(
                    claude["hooks"]["PermissionRequest"][0]["hooks"][0]["command"]
                        .as_str()
                        .unwrap()
                        .contains(" hooks claude permission-request")
                );
                assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

                assert!(!home.join(".gemini/settings.json").exists());
                let opencode = fs::read_to_string(&opencode_path).unwrap();
                assert!(opencode.contains(OPENCODE_PLUGIN_TAG));
                assert!(opencode.contains("hooks\", \"opencode\""));
                assert!(opencode.contains("\"tool.execute.before\""));
                assert!(opencode.contains("const MAX_INPUT_BYTES = 1048576"));
                assert!(opencode.contains("const MAX_SANITIZE_NODES = 4096"));
                assert!(opencode.contains("function makeBudget"));
                assert!(opencode.contains("function sanitizeJson"));
                assert!(opencode.contains("input: hookInput(body)"));

                let first = fs::read_to_string(&codex_path).unwrap();
                handle_hooks_setup(&context, strings(&["codex"])).unwrap();
                assert_eq!(fs::read_to_string(&codex_path).unwrap(), first);
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
            },
        );
    }

    #[test]
    fn hook_setup_rejects_removed_gemini_target() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex home");
        let claude_dir = dir.path().join("claude config");
        let home = dir.path().join("home dir");
        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = home.display().to_string();

        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, Vec::new()).unwrap();

                assert!(codex_home.join("hooks.json").exists());
                assert!(claude_dir.join("settings.json").exists());
                assert!(home
                    .join(".config/opencode")
                    .join("plugins/forktty.generated.js")
                    .exists());
                assert!(home.join(".gemini/config/hooks.json").exists());
                assert!(!home.join(".gemini/settings.json").exists());

                let err = handle_hooks_setup(&context, strings(&["gemini"])).unwrap_err();
                assert!(err.message.contains("Unsupported agent: gemini"), "{err:?}");
                assert!(!home.join(".gemini/settings.json").exists());
            },
        );
    }

    #[test]
    fn hook_remove_cleans_legacy_gemini_config_without_enabling_setup() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let gemini_path = home.path().join(".gemini/settings.json");
            ensure_parent_dir(&gemini_path).unwrap();
            fs::write(
                &gemini_path,
                serde_json::to_string_pretty(&json!({
                    "hooks": {
                        "SessionStart": [
                            {
                                "hooks": [{
                                    "type": "command",
                                    "command": "[ \"${FORKTTY_GEMINI_HOOKS_DISABLED:-}\" != \"1\" ] && '/usr/bin/forktty' hooks gemini session-start || echo '{\"continue\":true,\"suppressOutput\":false}'"
                                }]
                            },
                            {
                                "hooks": [{
                                    "type": "command",
                                    "command": "custom-gemini-hook"
                                }]
                            }
                        ]
                    },
                    "mcpServers": {
                        "forktty": {
                            "command": "/usr/bin/forktty",
                            "args": ["mcp"],
                            "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                        }
                    },
                    "theme": "dark"
                }))
                .unwrap(),
            )
            .unwrap();

            assert_err_contains(
                handle_hooks_setup(&context, strings(&["gemini"])),
                "Unsupported agent: gemini",
            );
            handle_hooks_remove(&context, strings(&["gemini"])).unwrap();

            let config = read_json(&gemini_path);
            let entries = config["hooks"]["SessionStart"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0]["hooks"][0]["command"],
                Value::String("custom-gemini-hook".to_string())
            );
            assert!(config["mcpServers"].get("forktty").is_some());
            assert_eq!(config["theme"], "dark");
        });
    }

    #[test]
    fn claude_hook_setup_profiles_migrate_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join("claude config");
        let home = dir.path().join("home dir");
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = home.display().to_string();

        with_env(
            &[
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                let claude_path = claude_dir.join("settings.json");

                handle_hooks_setup(&context, strings(&["claude"])).unwrap();
                let lifecycle = read_json(&claude_path);
                assert!(lifecycle["hooks"]["SessionStart"].is_array());
                assert!(lifecycle["hooks"]["PermissionRequest"].is_array());
                assert!(lifecycle["hooks"].get("PreToolUse").is_none());
                assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

                handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
                let full = read_json(&claude_path);
                for event in [
                    "PreToolUse",
                    "PostToolUse",
                    "PostToolUseFailure",
                    "PostToolBatch",
                ] {
                    assert!(full["hooks"][event].is_array(), "missing full {event}");
                }
                assert!(full["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
                    .as_str()
                    .unwrap()
                    .contains(" hooks claude pre-tool"));
                assert_eq!(describe_claude_installed_profile(&claude_path), "full");

                handle_hooks_setup(&context, strings(&["claude"])).unwrap();
                let migrated = read_json(&claude_path);
                assert!(migrated["hooks"]["SessionStart"].is_array());
                assert!(migrated["hooks"]["PermissionRequest"].is_array());
                for event in [
                    "PreToolUse",
                    "PostToolUse",
                    "PostToolUseFailure",
                    "PostToolBatch",
                ] {
                    assert!(
                        migrated["hooks"].get(event).is_none(),
                        "default rerun should remove {event}"
                    );
                }
                assert_eq!(describe_claude_installed_profile(&claude_path), "lifecycle");

                handle_hooks_setup(&context, strings(&["--full", "claude"])).unwrap();
                handle_hooks_remove(&context, strings(&["claude"])).unwrap();
                let removed = read_json(&claude_path);
                assert!(removed.get("hooks").is_none());
                assert_eq!(
                    describe_claude_installed_profile(&claude_path),
                    "not_installed"
                );
            },
        );
    }

    #[test]
    fn claude_hook_setup_plan_profiles_control_tool_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let claude_dir = dir.path().join("claude config");
        let home = dir.path().join("home dir");
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = home.display().to_string();

        with_env(
            &[
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let spec = agent_spec("claude").unwrap();
                let launcher = Path::new("/usr/bin/forktty");
                let default_plan = build_hook_setup_plan(spec, launcher).unwrap();
                let default_config: Value = serde_json::from_str(&default_plan.content).unwrap();
                assert!(default_config["hooks"]["SessionStart"].is_array());
                assert!(default_config["hooks"].get("PreToolUse").is_none());

                let full_plan =
                    build_hook_setup_plan_with_profile(spec, launcher, HookSetupProfile::Full)
                        .unwrap();
                let full_config: Value = serde_json::from_str(&full_plan.content).unwrap();
                for event in [
                    "PreToolUse",
                    "PostToolUse",
                    "PostToolUseFailure",
                    "PostToolBatch",
                ] {
                    assert!(full_config["hooks"][event].is_array(), "missing {event}");
                }
            },
        );
    }

    #[test]
    fn hook_remove_deletes_only_forktty_managed_entries_and_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let home_s = dir.path().display().to_string();
        let codex_home_s = codex_home.display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("HOME", Some(&home_s)),
                ("OPENCODE_CONFIG_DIR", None),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["codex", "opencode"])).unwrap();
                let codex_path = codex_home.join("hooks.json");
                let opencode_path = dir
                    .path()
                    .join(".config/opencode")
                    .join("plugins/forktty.generated.js");
                let mut codex = read_json(&codex_path);
                codex["hooks"]["SessionStart"] = json!([
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "custom-wrapper hooks codex session-start"
                        }]
                    },
                    codex["hooks"]["SessionStart"][0].clone()
                ]);
                atomic_write_file(
                    &codex_path,
                    format!("{}\n", serde_json::to_string_pretty(&codex).unwrap()).as_bytes(),
                )
                .unwrap();

                handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();

                let codex = read_json(&codex_path);
                let entries = codex["hooks"]["SessionStart"].as_array().unwrap();
                assert_eq!(entries.len(), 1);
                assert_eq!(
                    entries[0]["hooks"][0]["command"],
                    Value::String("custom-wrapper hooks codex session-start".to_string())
                );
                assert!(codex["hooks"].get("PreToolUse").is_none());
                assert!(!opencode_path.exists());

                handle_hooks_remove(&context, strings(&["codex", "opencode"])).unwrap();
                assert!(opencode_path
                    .parent()
                    .unwrap()
                    .read_dir()
                    .unwrap()
                    .any(|entry| entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .contains(".bak-")));
            },
        );
    }

    #[test]
    fn hook_remove_dry_run_and_option_errors_do_not_write_configs() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[("CODEX_HOME", Some(&codex_home_s)), ("HOME", Some(&home_s))],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["codex"])).unwrap();
                let codex_path = codex_home.join("hooks.json");
                let before = fs::read_to_string(&codex_path).unwrap();

                handle_hooks_remove(&context, strings(&["--dry-run", "codex"])).unwrap();
                assert_eq!(fs::read_to_string(&codex_path).unwrap(), before);

                assert_err_contains(
                    handle_hooks_remove(&context, strings(&["--dry-run=yes", "codex"])),
                    "hooks remove: --dry-run must be true or false",
                );
                assert_err_contains(
                    handle_hooks_remove(&context, strings(&["--dryrun", "codex"])),
                    "hooks remove: unknown option --dryrun",
                );
            },
        );
    }

    #[test]
    fn mcp_tools_default_targets_from_forktty_env() {
        with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some(" ws-env ")),
                ("FORKTTY_SURFACE_ID", Some(" surface-env ")),
            ],
            || {
                let (_, params) = crate::mcp_server::build_socket_call_for_test(
                    "surface_send_text",
                    json!({ "text": "cargo test\n" }),
                )
                .unwrap();
                assert_eq!(params["surface_id"], "surface-env");
                assert_eq!(params["text"], "cargo test\n");

                let (_, params) =
                    crate::mcp_server::build_socket_call_for_test("surface_read_text", json!({}))
                        .unwrap();
                assert_eq!(params["surface_id"], "surface-env");

                let (_, params) = crate::mcp_server::build_socket_call_for_test(
                    "surface_capture_tail",
                    json!({ "lines": 20 }),
                )
                .unwrap();
                assert_eq!(params["surface_id"], "surface-env");
                assert_eq!(params["lines"], 20);

                let (_, params) = crate::mcp_server::build_socket_call_for_test(
                    "status_set",
                    json!({ "key": "agent:codex", "value": "Running" }),
                )
                .unwrap();
                assert_eq!(params["workspace_id"], "ws-env");
                assert_eq!(params["surface_id"], "surface-env");
                assert_eq!(params["label"], "agent:codex");

                let (_, params) =
                    crate::mcp_server::build_socket_call_for_test("surface_list", json!({}))
                        .unwrap();
                assert_eq!(params["workspace_id"], "ws-env");

                let (_, params) = crate::mcp_server::build_socket_call_for_test(
                    "notification_create",
                    json!({ "workspace_id": "explicit-ws", "body": "done" }),
                )
                .unwrap();
                assert_eq!(params["workspace_id"], "explicit-ws");
                assert!(params.get("surface_id").is_none());
            },
        );
    }

    #[test]
    fn mcp_setup_plans_write_agent_configs_and_are_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let codex_home = tempfile::tempdir().unwrap();
        let home = home.path().to_string_lossy().to_string();
        let codex_home = codex_home.path().to_string_lossy().to_string();
        with_env(
            &[
                ("HOME", Some(home.as_str())),
                ("CODEX_HOME", Some(codex_home.as_str())),
            ],
            || {
                let launcher = Path::new("/usr/bin/forktty");
                for agent in ["codex", "claude", "antigravity"] {
                    let spec = mcp_agent_spec(agent).unwrap();
                    let plan = build_mcp_setup_plan(spec, launcher).unwrap();
                    assert!(plan.changed, "{agent} initial plan should change");
                    ensure_parent_dir(&plan.config_path).unwrap();
                    fs::write(&plan.config_path, &plan.content).unwrap();
                    let replanned = build_mcp_setup_plan(spec, launcher).unwrap();
                    assert!(!replanned.changed, "{agent} setup should be idempotent");
                }

                let codex: toml::Table = fs::read_to_string(codex_mcp_config_path())
                    .unwrap()
                    .parse()
                    .unwrap();
                let codex_server = &codex["mcp_servers"]["forktty"];
                assert_eq!(codex_server["command"].as_str(), Some("/usr/bin/forktty"));
                assert_eq!(
                    codex_server["args"].as_array().unwrap()[0].as_str(),
                    Some("mcp")
                );
                assert_eq!(
                    codex_server["env"][MCP_MANAGED_ENV].as_str(),
                    Some(MCP_SERVER_NAME)
                );
                assert!(codex_server["env_vars"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value.as_str() == Some("FORKTTY_SOCKET_PATH")));

                for (agent, path) in [
                    ("claude", claude_mcp_config_path()),
                    ("antigravity", antigravity_mcp_config_path()),
                ] {
                    let config = read_json(&path);
                    let server = &config["mcpServers"]["forktty"];
                    assert_eq!(server["command"], "/usr/bin/forktty", "{agent}");
                    assert_eq!(server["args"][0], "mcp", "{agent}");
                    assert_eq!(server["env"][MCP_MANAGED_ENV], MCP_SERVER_NAME, "{agent}");
                }
            },
        );
    }

    #[test]
    fn mcp_setup_rejects_removed_gemini_target() {
        let home = tempfile::tempdir().unwrap();
        let codex_home = tempfile::tempdir().unwrap();
        let home = home.path().to_string_lossy().to_string();
        let codex_home = codex_home.path().to_string_lossy().to_string();
        with_env(
            &[
                ("HOME", Some(home.as_str())),
                ("CODEX_HOME", Some(codex_home.as_str())),
            ],
            || {
                let context = test_context();
                handle_mcp_setup(&context, Vec::new()).unwrap();

                assert!(codex_mcp_config_path().exists());
                assert!(claude_mcp_config_path().exists());
                assert!(antigravity_mcp_config_path().exists());
                let legacy_gemini_path = Path::new(&home).join(".gemini/settings.json");
                assert!(!legacy_gemini_path.exists());

                let err = handle_mcp_setup(&context, strings(&["gemini"])).unwrap_err();
                assert!(
                    err.message.contains("Unsupported mcp agent: gemini"),
                    "{err:?}"
                );
                assert!(!legacy_gemini_path.exists());
            },
        );
    }

    #[test]
    fn mcp_remove_cleans_legacy_gemini_config_without_enabling_setup() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let gemini_path = home.path().join(".gemini/settings.json");
            ensure_parent_dir(&gemini_path).unwrap();
            fs::write(
                &gemini_path,
                serde_json::to_string_pretty(&json!({
                    "mcpServers": {
                        "forktty": {
                            "command": "/usr/bin/forktty",
                            "args": ["mcp"],
                            "env": { MCP_MANAGED_ENV: MCP_SERVER_NAME }
                        },
                        "foreign": {
                            "command": "/bin/true"
                        }
                    },
                    "hooks": {
                        "SessionStart": [{
                            "hooks": [{
                                "type": "command",
                                "command": "custom-gemini-hook"
                            }]
                        }]
                    }
                }))
                .unwrap(),
            )
            .unwrap();

            assert_err_contains(
                handle_mcp_setup(&context, strings(&["gemini"])),
                "Unsupported mcp agent: gemini",
            );
            handle_mcp_remove(&context, strings(&["gemini"])).unwrap();

            let config = read_json(&gemini_path);
            assert!(config["mcpServers"].get("forktty").is_none());
            assert_eq!(config["mcpServers"]["foreign"]["command"], "/bin/true");
            assert!(config["hooks"].get("SessionStart").is_some());
        });
    }

    #[test]
    fn skill_setup_writes_default_targets_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let claude_dir = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();
        let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

        with_env(
            &[
                ("HOME", Some(home_s.as_str())),
                ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
            ],
            || {
                let context = test_context();
                handle_skills_setup(&context, Vec::new()).unwrap();

                let agents_skill = agent_skills_dir();
                let claude_skill = claude_skill_dir();
                for path in [&agents_skill, &claude_skill] {
                    let skill = fs::read_to_string(path.join("SKILL.md")).unwrap();
                    assert!(skill.contains(AGENT_SKILL_MARKER));
                    assert!(skill.contains("context_snapshot"));
                    assert!(skill.contains("team_message_dispatch"));
                    let metadata =
                        fs::read_to_string(path.join("agents").join("openai.yaml")).unwrap();
                    assert!(metadata.contains("value: \"forktty\""));
                    assert!(metadata.contains("allow_implicit_invocation: true"));
                }

                let first = fs::read_to_string(agents_skill.join("SKILL.md")).unwrap();
                handle_skills_setup(&context, Vec::new()).unwrap();
                assert_eq!(
                    fs::read_to_string(agents_skill.join("SKILL.md")).unwrap(),
                    first
                );
                assert_eq!(
                    backup_count(
                        agents_skill.parent().unwrap(),
                        "forktty-agent-orchestration.bak-"
                    ),
                    0
                );
            },
        );
    }

    #[test]
    fn bundled_agent_skill_documents_team_preflight_roles_and_qa_policy() {
        for expected in [
            "## Team Preflight",
            "## Worker Role Templates",
            "## Worktree Policy",
            "## Isolated Integration QA",
            "workflow_upsert",
            "workflow_plan_set",
            "team_task_upsert",
            "agent_health",
            "lifecycle_evidence",
            "include_feed_trace",
            "effective_project_cwd",
            "final_state",
            "source/installed checksums",
            "forktty hooks test codex",
            "FORKTTY_SOCKET_PATH",
            "separate temporary instance",
        ] {
            assert!(
                AGENT_SKILL_MD.contains(expected),
                "agent skill should document {expected}"
            );
        }
    }

    #[test]
    fn skill_setup_pi_alias_targets_interoperable_agents_dir() {
        let home = tempfile::tempdir().unwrap();
        let claude_dir = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();
        let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

        with_env(
            &[
                ("HOME", Some(home_s.as_str())),
                ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
            ],
            || {
                handle_skills_setup(&test_context(), strings(&["pi"])).unwrap();

                assert!(agent_skills_dir().join("SKILL.md").exists());
                assert!(!claude_skill_dir().exists());
            },
        );
    }

    #[test]
    fn skill_setup_rejects_removed_gemini_target() {
        let home = tempfile::tempdir().unwrap();
        let claude_dir = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();
        let claude_dir_s = claude_dir.path().to_string_lossy().to_string();

        with_env(
            &[
                ("HOME", Some(home_s.as_str())),
                ("CLAUDE_CONFIG_DIR", Some(claude_dir_s.as_str())),
            ],
            || {
                let err = handle_skills_setup(&test_context(), strings(&["gemini"])).unwrap_err();
                assert!(
                    err.message.contains("Unsupported skills target: gemini"),
                    "{err:?}"
                );
                assert!(!agent_skills_dir().exists());
                assert!(!claude_skill_dir().exists());
            },
        );
    }

    #[test]
    fn skill_setup_refuses_unmanaged_existing_skill() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n",
            )
            .unwrap();

            assert_err_contains(
                handle_skills_setup(&test_context(), strings(&["agents"])),
                "refusing to overwrite unmanaged skill",
            );
            assert_eq!(
                fs::read_to_string(skill_dir.join("SKILL.md")).unwrap(),
                "---\nname: forktty-agent-orchestration\ndescription: custom\n---\ncustom\n"
            );
        });
    }

    #[test]
    fn skill_setup_and_doctor_report_managed_skill_drift_checksums() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.join("agents")).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("{AGENT_SKILL_MARKER}\n# stale\n"),
            )
            .unwrap();
            fs::write(
                skill_dir.join("agents").join("openai.yaml"),
                "stale: true\n",
            )
            .unwrap();

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let plan = build_skill_setup_plan(spec).unwrap();
            assert_eq!(plan.status, "update_available");
            assert!(plan.changed);
            assert_ne!(plan.installed_checksum, plan.source_checksum);

            let report = build_socket_doctor_report(&test_context());
            assert_eq!(report["skillDirs"]["agents"]["status"], "update_available");
            assert_ne!(
                report["skillDirs"]["agents"]["installedChecksum"],
                report["skillDirs"]["agents"]["sourceChecksum"]
            );
            assert_eq!(
                report["skillDirs"]["agents"]["repairCommand"],
                "forktty skills setup agents"
            );
        });
    }

    #[test]
    fn doctor_reports_symlinked_skill_file_as_unrepairable_invalid() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.join("agents")).unwrap();
            let target = home.path().join("external-skill.md");
            fs::write(&target, AGENT_SKILL_MD).unwrap();
            std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
            fs::write(
                skill_dir.join("agents").join("openai.yaml"),
                AGENT_SKILL_OPENAI_YAML,
            )
            .unwrap();

            let report = build_socket_doctor_report(&test_context());

            assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
            assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
        });
    }

    #[test]
    fn skill_setup_refuses_symlinked_skill_file_without_verified_marker() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.join("agents")).unwrap();
            let target = home.path().join("external-skill.md");
            fs::write(&target, AGENT_SKILL_MD).unwrap();
            std::os::unix::fs::symlink(&target, skill_dir.join("SKILL.md")).unwrap();
            fs::write(
                skill_dir.join("agents").join("openai.yaml"),
                AGENT_SKILL_OPENAI_YAML,
            )
            .unwrap();

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let error = match build_skill_setup_plan(spec) {
                Ok(_) => panic!("expected symlinked SKILL.md to be refused"),
                Err(error) => error,
            };
            assert!(error
                .message
                .contains("ForkTTY-managed marker could not be verified"));

            let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
            assert!(error
                .message
                .contains("ForkTTY-managed marker could not be verified"));

            let report = build_socket_doctor_report(&context);
            assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
            assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
        });
    }

    #[test]
    fn skill_setup_repairs_invalid_metadata_after_verified_marker() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.join("agents")).unwrap();
            fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
            let target = home.path().join("external-openai.yaml");
            fs::write(&target, AGENT_SKILL_OPENAI_YAML).unwrap();
            std::os::unix::fs::symlink(&target, skill_dir.join("agents").join("openai.yaml"))
                .unwrap();

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let plan = build_skill_setup_plan(spec).unwrap();
            assert_eq!(plan.status, "invalid");
            assert!(plan.changed);

            handle_skills_setup(&context, strings(&["agents"])).unwrap();

            let metadata =
                fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
            assert!(metadata.is_file());
            assert!(!metadata.file_type().is_symlink());
            assert_eq!(
                backup_count(
                    skill_dir.parent().unwrap(),
                    "forktty-agent-orchestration.bak-"
                ),
                1
            );
            let report = build_socket_doctor_report(&context);
            assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
        });
    }

    #[test]
    fn skill_setup_repairs_symlinked_metadata_dir_after_verified_marker() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(skill_dir.join("SKILL.md"), AGENT_SKILL_MD).unwrap();
            let target = home.path().join("external-agents");
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("openai.yaml"), AGENT_SKILL_OPENAI_YAML).unwrap();
            std::os::unix::fs::symlink(&target, skill_dir.join("agents")).unwrap();

            let report = build_socket_doctor_report(&context);
            assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
            assert_eq!(
                report["skillDirs"]["agents"]["repairCommand"],
                "forktty skills setup agents"
            );

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let plan = build_skill_setup_plan(spec).unwrap();
            assert_eq!(plan.status, "invalid");
            assert!(plan.changed);

            handle_skills_setup(&context, strings(&["agents"])).unwrap();

            let metadata_dir = fs::symlink_metadata(skill_dir.join("agents")).unwrap();
            assert!(metadata_dir.is_dir());
            assert!(!metadata_dir.file_type().is_symlink());
            let metadata =
                fs::symlink_metadata(skill_dir.join("agents").join("openai.yaml")).unwrap();
            assert!(metadata.is_file());
            assert!(!metadata.file_type().is_symlink());
            let report = build_socket_doctor_report(&context);
            assert_eq!(report["skillDirs"]["agents"]["status"], "up_to_date");
        });
    }

    #[test]
    fn skill_setup_refuses_symlinked_skill_directory_without_verified_marker() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.parent().unwrap()).unwrap();
            let target = home.path().join("external-skill-dir");
            std::os::unix::fs::symlink(&target, &skill_dir).unwrap();

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let error = match build_skill_setup_plan(spec) {
                Ok(_) => panic!("expected symlinked skill directory to be refused"),
                Err(error) => error,
            };
            assert!(error
                .message
                .contains("ForkTTY-managed marker could not be verified"));

            let error = handle_skills_setup(&context, strings(&["agents"])).unwrap_err();
            assert!(error
                .message
                .contains("ForkTTY-managed marker could not be verified"));

            let skill_dir_meta = fs::symlink_metadata(&skill_dir).unwrap();
            assert!(skill_dir_meta.file_type().is_symlink());
            let report = build_socket_doctor_report(&context);
            assert_eq!(report["skillDirs"]["agents"]["status"], "invalid");
            assert!(report["skillDirs"]["agents"]["repairCommand"].is_null());
        });
    }

    #[test]
    fn skill_setup_summary_reports_repaired_state_after_install() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let skill_dir = agent_skills_dir();
            fs::create_dir_all(skill_dir.join("agents")).unwrap();
            fs::write(
                skill_dir.join("SKILL.md"),
                format!("{AGENT_SKILL_MARKER}\n# stale\n"),
            )
            .unwrap();
            fs::write(
                skill_dir.join("agents").join("openai.yaml"),
                "stale: true\n",
            )
            .unwrap();

            let spec = supported_skill_targets(&strings(&["agents"])).unwrap()[0];
            let plan = build_skill_setup_plan(spec).unwrap();
            assert_eq!(plan.status, "update_available");
            let summary = skill_setup_summary(&plan, false, Some(PathBuf::from("backup")));

            assert_eq!(summary["changed"], true);
            assert_eq!(summary["status"], "up_to_date");
            assert_eq!(summary["installedChecksum"], summary["sourceChecksum"]);
            assert!(summary["repairCommand"].is_null());
        });
    }

    #[test]
    fn skill_remove_moves_managed_skill_to_backup() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            let context = test_context();
            handle_skills_setup(&context, strings(&["agents"])).unwrap();
            let skill_dir = agent_skills_dir();
            assert!(skill_dir.exists());

            handle_skills_remove(&context, strings(&["agents"])).unwrap();

            assert!(!skill_dir.exists());
            assert_eq!(
                backup_count(
                    skill_dir.parent().unwrap(),
                    "forktty-agent-orchestration.bak-"
                ),
                1
            );
        });
    }

    #[test]
    fn skill_setup_dry_run_and_option_errors_do_not_write() {
        let home = tempfile::tempdir().unwrap();
        let home_s = home.path().to_string_lossy().to_string();

        with_env(&[("HOME", Some(home_s.as_str()))], || {
            handle_skills_setup(&test_context(), strings(&["--dry-run", "agents"])).unwrap();
            assert!(!agent_skills_dir().exists());

            assert_err_contains(
                handle_skills_setup(&test_context(), strings(&["--dry-run=yes", "agents"])),
                "skills setup: --dry-run must be true or false",
            );
            assert_err_contains(
                handle_skills_setup(&test_context(), strings(&["--dryrun", "agents"])),
                "skills setup: unknown option --dryrun",
            );
            assert!(!agent_skills_dir().exists());
        });
    }

    #[test]
    fn mcp_codex_setup_and_remove_preserve_comments_and_formatting() {
        let codex_home = tempfile::tempdir().unwrap();
        let codex_home_s = codex_home.path().to_string_lossy().to_string();
        with_env(&[("CODEX_HOME", Some(codex_home_s.as_str()))], || {
            let original = "# my model choice\nmodel = \"gpt-5.2-codex\" # keep high\n\n[mcp_servers.foreign]\ncommand = \"/bin/true\"\n";
            let path = codex_mcp_config_path();
            ensure_parent_dir(&path).unwrap();
            fs::write(&path, original).unwrap();

            let spec = mcp_agent_spec("codex").unwrap();
            let launcher = Path::new("/usr/bin/forktty");
            let plan = build_mcp_setup_plan(spec, launcher).unwrap();
            assert!(plan.changed);
            assert!(plan.content.contains("# my model choice"));
            assert!(plan
                .content
                .contains("model = \"gpt-5.2-codex\" # keep high"));
            assert!(plan.content.contains("[mcp_servers.foreign]"));
            assert!(plan.content.contains("[mcp_servers.forktty]"));
            fs::write(&path, &plan.content).unwrap();
            assert!(!build_mcp_setup_plan(spec, launcher).unwrap().changed);

            let remove = build_mcp_remove_plan(spec).unwrap();
            let McpRemoveAction::Write(content) = remove.action else {
                panic!("codex remove should rewrite config");
            };
            assert_eq!(content, original);
        });
    }

    #[test]
    fn mcp_codex_setup_allows_config_above_hook_config_limit() {
        let codex_home = tempfile::tempdir().unwrap();
        let codex_home_s = codex_home.path().to_string_lossy().to_string();
        with_env(&[("CODEX_HOME", Some(codex_home_s.as_str()))], || {
            let path = codex_mcp_config_path();
            ensure_parent_dir(&path).unwrap();
            let original = format!(
                "# {}\nmodel = \"gpt-5.2-codex\"\n",
                "x".repeat(MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1)
            );
            fs::write(&path, &original).unwrap();

            let spec = mcp_agent_spec("codex").unwrap();
            let plan = build_mcp_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();

            assert!(plan.changed);
            assert!(plan.content.contains("model = \"gpt-5.2-codex\""));
            assert!(plan.content.contains("[mcp_servers.forktty]"));
        });
    }

    #[test]
    fn mcp_remove_preserves_foreign_servers_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let codex_home = tempfile::tempdir().unwrap();
        let home = home.path().to_string_lossy().to_string();
        let codex_home = codex_home.path().to_string_lossy().to_string();
        with_env(
            &[
                ("HOME", Some(home.as_str())),
                ("CODEX_HOME", Some(codex_home.as_str())),
            ],
            || {
                let launcher = Path::new("/usr/bin/forktty");
                let codex = mcp_agent_spec("codex").unwrap();
                let codex_plan = build_mcp_setup_plan(codex, launcher).unwrap();
                ensure_parent_dir(&codex_plan.config_path).unwrap();
                fs::write(
                    &codex_plan.config_path,
                    format!(
                        "{}\n[mcp_servers.foreign]\ncommand = \"/bin/true\"\n",
                        codex_plan.content
                    ),
                )
                .unwrap();
                let remove = build_mcp_remove_plan(codex).unwrap();
                let McpRemoveAction::Write(content) = remove.action else {
                    panic!("codex remove should rewrite config");
                };
                assert!(!content.contains("[mcp_servers.forktty]"));
                assert!(content.contains("[mcp_servers.foreign]"));
                fs::write(codex_mcp_config_path(), content).unwrap();
                assert!(!build_mcp_remove_plan(codex).unwrap().changed);

                let claude = mcp_agent_spec("claude").unwrap();
                let path = claude_mcp_config_path();
                ensure_parent_dir(&path).unwrap();
                fs::write(
                    &path,
                    serde_json::to_string_pretty(&json!({
                        "mcpServers": {
                            "foreign": { "command": "/bin/true" },
                            "forktty": json_mcp_server_config(launcher),
                        },
                        "theme": "dark",
                    }))
                    .unwrap(),
                )
                .unwrap();
                let remove = build_mcp_remove_plan(claude).unwrap();
                let McpRemoveAction::Write(content) = remove.action else {
                    panic!("claude remove should rewrite config");
                };
                let value: Value = serde_json::from_str(&content).unwrap();
                assert!(value["mcpServers"].get("forktty").is_none());
                assert_eq!(value["mcpServers"]["foreign"]["command"], "/bin/true");
                assert_eq!(value["theme"], "dark");
                fs::write(path, content).unwrap();
                assert!(!build_mcp_remove_plan(claude).unwrap().changed);
            },
        );
    }

    #[test]
    fn hook_setup_dry_run_and_option_errors_do_not_write_configs() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["--dry-run", "codex"])).unwrap();
                assert!(!codex_home.join("hooks.json").exists());

                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["--dry-run=yes", "codex"])),
                    "hooks setup: --dry-run must be true or false",
                );
                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["--dryrun", "codex"])),
                    "hooks setup: unknown option --dryrun",
                );
                assert!(!codex_home.join("hooks.json").exists());
            },
        );
    }

    #[test]
    fn hook_setup_preflights_all_configs_and_prevents_partial_writes() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let claude_dir = dir.path().join("claude");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&claude_dir).unwrap();
        let codex_path = codex_home.join("hooks.json");
        let claude_path = claude_dir.join("settings.json");
        fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();
        fs::write(&claude_path, "{ not json ::: ").unwrap();

        let codex_home_s = codex_home.display().to_string();
        let claude_dir_s = claude_dir.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", Some(&claude_dir_s)),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                assert_err_contains(
                    handle_hooks_setup(&context, strings(&["codex", "claude"])),
                    "failed to read claude hook config",
                );
                assert_eq!(
                    fs::read_to_string(&codex_path).unwrap(),
                    "{\"customKey\":{\"keepMe\":true}}\n"
                );
            },
        );
    }

    #[test]
    fn hook_setup_preserves_unrelated_json() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        fs::create_dir_all(&codex_home).unwrap();
        let codex_path = codex_home.join("hooks.json");
        fs::write(&codex_path, "{\"customKey\":{\"keepMe\":true}}\n").unwrap();

        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                let context = test_context();
                handle_hooks_setup(&context, strings(&["codex"])).unwrap();
                let parsed = read_json(&codex_path);
                assert_eq!(parsed["customKey"]["keepMe"], true);
                assert!(parsed["hooks"]["SessionStart"].is_array());
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 1);
            },
        );
    }

    #[test]
    fn hook_setup_updates_symlink_targets_without_replacing_link() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        let managed_dir = dir.path().join("managed-codex");
        fs::create_dir_all(&codex_home).unwrap();
        fs::create_dir_all(&managed_dir).unwrap();
        let target_path = managed_dir.join("hooks.json");
        let config_path = codex_home.join("hooks.json");
        fs::write(&target_path, "{\"customKey\":\"managed\"}\n").unwrap();
        std::os::unix::fs::symlink(&target_path, &config_path).unwrap();

        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                handle_hooks_setup(&test_context(), strings(&["codex"])).unwrap();
                assert!(fs::symlink_metadata(&config_path)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                assert_eq!(fs::read_link(&config_path).unwrap(), target_path);
                let parsed = read_json(&target_path);
                assert_eq!(parsed["customKey"], "managed");
                assert!(parsed["hooks"]["SessionStart"].is_array());
                assert_eq!(backup_count(&managed_dir, "hooks.json.bak-"), 1);
                assert_eq!(backup_count(&codex_home, "hooks.json.bak-"), 0);
            },
        );
    }

    #[test]
    fn hook_setup_replaces_broken_symlink_with_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let codex_home = dir.path().join("codex");
        fs::create_dir_all(&codex_home).unwrap();
        let config_path = codex_home.join("hooks.json");
        std::os::unix::fs::symlink(codex_home.join("missing-target.json"), &config_path).unwrap();

        let codex_home_s = codex_home.display().to_string();
        let home_s = dir.path().display().to_string();
        with_env(
            &[
                ("CODEX_HOME", Some(&codex_home_s)),
                ("CLAUDE_CONFIG_DIR", None),
                ("HOME", Some(&home_s)),
            ],
            || {
                handle_hooks_setup(&test_context(), strings(&["codex"]))
                    .expect("setup through broken symlink should succeed");
                let stat = fs::symlink_metadata(&config_path).unwrap();
                assert!(
                    stat.is_file(),
                    "broken symlink should be replaced by a regular file"
                );
                let parsed = read_json(&config_path);
                assert!(parsed["hooks"]["SessionStart"].is_array());
            },
        );
    }

    #[test]
    fn hook_config_reader_rejects_unsafe_or_invalid_paths() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("codex").unwrap();

        let whitespace = dir.path().join("whitespace.json");
        fs::write(&whitespace, "   \n\t  \n").unwrap();
        assert_eq!(read_agent_config(spec, &whitespace).unwrap(), json!({}));

        let array = dir.path().join("array.json");
        fs::write(&array, "[]\n").unwrap();
        assert_err_contains(
            read_agent_config(spec, &array),
            "expected a JSON object at the top level",
        );
        assert_eq!(fs::read_to_string(&array).unwrap(), "[]\n");

        let directory = dir.path().join("directory.json");
        fs::create_dir(&directory).unwrap();
        assert_err_contains(
            read_agent_config(spec, &directory),
            "path exists but is not a regular file",
        );
        assert!(directory.is_dir());

        let oversized = dir.path().join("oversized.json");
        fs::write(
            &oversized,
            vec![b' '; MAX_HOOK_CONFIG_SIZE_BYTES as usize + 1],
        )
        .unwrap();
        assert_err_contains(
            read_agent_config(spec, &oversized),
            "hook config is too large",
        );

        let broken = dir.path().join("broken.json");
        std::os::unix::fs::symlink(dir.path().join("missing.json"), &broken).unwrap();
        assert_eq!(read_agent_config(spec, &broken).unwrap(), json!({}));
        assert!(fs::symlink_metadata(&broken)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn atomic_write_replaces_target_and_cleans_temp_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("atomic.json");
        atomic_write_file(&target, b"first\n").unwrap();
        atomic_write_file(&target, b"second\n").unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "second\n");
        assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);

        let target_in_missing_dir = dir.path().join("missing").join("atomic.json");
        assert!(atomic_write_file(&target_in_missing_dir, b"content\n").is_err());
        assert!(!target_in_missing_dir.exists());
        assert_eq!(backup_count(dir.path(), ".atomic.json.tmp-"), 0);
    }

    fn runtime_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
        [
            (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
            (None, None),
        ]
    }

    fn forktty_vars(appimage: &str, appdir: &str) -> [(Option<OsString>, Option<OsString>); 2] {
        [
            (None, None),
            (Some(OsString::from(appimage)), Some(OsString::from(appdir))),
        ]
    }

    #[test]
    fn stable_hook_launcher_uses_appimage_only_for_appdir_binary() {
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
                runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
            ),
            Some(PathBuf::from("/home/me/forktty.AppImage"))
        );
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/usr/bin/forktty")),
                runtime_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
            ),
            Some(PathBuf::from("/usr/bin/forktty"))
        );
    }

    // Shells inside the AppImage app see FORKTTY_APPIMAGE/FORKTTY_APPIMAGE_DIR
    // (exported by AppRun) instead of the runtime's APPIMAGE/APPDIR, which are
    // stripped from child environments. Hooks set up from such a shell used to
    // embed the volatile /tmp/.mount_* binary path, which broke on remount.
    #[test]
    fn stable_hook_launcher_uses_forktty_appimage_vars_from_child_shells() {
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/tmp/.mount_forktty/usr/bin/forktty")),
                forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
            ),
            Some(PathBuf::from("/home/me/forktty.AppImage"))
        );
        // A dev binary invoked explicitly from inside an AppImage shell must
        // keep its own path: it is not the mounted binary.
        assert_eq!(
            stable_hook_launcher_path_from_env(
                Some(Path::new("/home/me/forktty/target/release/forktty")),
                forktty_vars("/home/me/forktty.AppImage", "/tmp/.mount_forktty"),
            ),
            Some(PathBuf::from("/home/me/forktty/target/release/forktty"))
        );
    }

    #[test]
    fn parse_launcher_extracts_path_from_managed_command() {
        let spec = agent_spec("claude").unwrap();
        let command = build_hook_shell_command(
            Path::new("/home/me/ForkTTY/forktty.AppImage"),
            spec,
            "session-start",
        );
        assert_eq!(
            parse_launcher_from_managed_command(&command, spec).as_deref(),
            Some("/home/me/ForkTTY/forktty.AppImage")
        );
    }

    #[test]
    fn parse_launcher_handles_apostrophe_in_path() {
        let spec = agent_spec("codex").unwrap();
        let command =
            build_hook_shell_command(Path::new("/home/o'connor/forktty"), spec, "session-start");
        assert_eq!(
            parse_launcher_from_managed_command(&command, spec).as_deref(),
            Some("/home/o'connor/forktty")
        );
    }

    #[test]
    fn parse_launcher_rejects_unrelated_commands() {
        let spec = agent_spec("codex").unwrap();
        assert_eq!(parse_launcher_from_managed_command("echo hi", spec), None);
        assert_eq!(
            parse_launcher_from_managed_command(
                "[ x ] && '/usr/bin/forktty' hooks claude session-start || true",
                spec,
            ),
            None,
            "wrong agent key must not match"
        );
    }

    #[test]
    fn extract_managed_launcher_returns_first_forktty_entry() {
        let spec = agent_spec("claude").unwrap();
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        assert_eq!(
            extract_managed_launcher_from_config(spec, &config).as_deref(),
            Some("/usr/bin/forktty")
        );
    }

    #[test]
    fn extract_managed_launcher_skips_unmanaged_entries() {
        let spec = agent_spec("codex").unwrap();
        let config = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "echo unrelated",
                    }]
                }]
            }
        });
        assert_eq!(extract_managed_launcher_from_config(spec, &config), None);
    }

    #[test]
    fn describe_launcher_check_flags_stale_path() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("claude").unwrap();
        let config_path = dir.path().join("settings.json");
        let (_, config) = merge_hook_config(&json!({}), spec, Path::new("/old/forktty")).unwrap();
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let check = describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
        assert_eq!(check["status"], Value::String("stale".to_string()));
        assert_eq!(
            check["installedLauncher"],
            Value::String("/old/forktty".to_string())
        );
        assert_eq!(
            check["currentLauncher"],
            Value::String("/new/forktty".to_string())
        );
    }

    #[test]
    #[cfg(unix)]
    fn describe_launcher_check_treats_working_recorded_launcher_as_ok() {
        use std::os::unix::fs::PermissionsExt;

        // The recorded launcher exists and is executable, but the process now
        // runs from a different path (e.g. an AppImage version-suffixed copy).
        // The hooks still work, so this must not be reported as stale.
        let dir = tempfile::tempdir().unwrap();
        let installed_launcher = dir.path().join("forktty.appimage");
        fs::write(&installed_launcher, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&installed_launcher, fs::Permissions::from_mode(0o755)).unwrap();

        let spec = agent_spec("claude").unwrap();
        let config_path = dir.path().join("settings.json");
        let (_, config) = merge_hook_config(&json!({}), spec, &installed_launcher).unwrap();
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();

        let current = dir.path().join("forktty.appimage_0_2_0-alpha_7.appimage");
        let check = describe_launcher_check(spec, &config_path, Some(&current));
        assert_eq!(check["status"], Value::String("ok".to_string()));
        assert_eq!(
            check["installedLauncher"],
            Value::String(installed_launcher.display().to_string())
        );
    }

    #[test]
    fn describe_launcher_check_marks_matching_path_as_ok() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("codex").unwrap();
        let config_path = dir.path().join("hooks.json");
        let (_, config) =
            merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
        fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], Value::String("ok".to_string()));
    }

    #[test]
    fn describe_launcher_check_marks_missing_config_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        let spec = agent_spec("opencode").unwrap();
        let config_path = dir.path().join("forktty.generated.js");
        let check =
            describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
        assert_eq!(check["status"], Value::String("not_installed".to_string()));
    }

    #[test]
    fn antigravity_setup_plan_writes_named_group_and_wrapper_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().display().to_string();
        with_env(&[("HOME", Some(home.as_str()))], || {
            let spec = agent_spec("antigravity").unwrap();
            let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
            assert!(plan.changed);
            assert_eq!(plan.config_path, antigravity_config_path());

            let config: Value = serde_json::from_str(&plan.content).unwrap();
            let group = &config[ANTIGRAVITY_HOOK_GROUP];
            assert_eq!(group["enabled"], Value::Bool(true));
            // Model lifecycle hooks use flat handlers; tool events use
            // matcher wrappers.
            assert_eq!(
                group["PreInvocation"][0]["command"],
                antigravity_script_path("before-model")
                    .display()
                    .to_string()
            );
            assert!(group["PreInvocation"][0].get("hooks").is_none());
            assert!(group["PreInvocation"][0].get("matcher").is_none());
            assert_eq!(group["PreToolUse"][0]["matcher"], json!("*"));
            assert_eq!(group["PostToolUse"][0]["matcher"], json!("*"));

            assert_eq!(plan.scripts.len(), 3);
            for (event, provider_event) in [
                ("before-model", "PreInvocation"),
                ("pre-tool", "PreToolUse"),
                ("post-tool", "PostToolUse"),
            ] {
                let script_path = antigravity_script_path(event);
                let command = if provider_event == "PreInvocation" {
                    group[provider_event][0]["command"].as_str().unwrap()
                } else {
                    group[provider_event][0]["hooks"][0]["command"]
                        .as_str()
                        .unwrap()
                };
                assert_eq!(command, script_path.display().to_string());
                let (_, content) = plan
                    .scripts
                    .iter()
                    .find(|(path, _)| path == &script_path)
                    .unwrap();
                assert!(content.starts_with("#!/bin/sh\n"));
                assert!(content.contains(ANTIGRAVITY_SCRIPT_TAG));
                assert!(content.contains(&format!("'/usr/bin/forktty' hooks antigravity {event}")));
                assert!(content.contains("FORKTTY_ANTIGRAVITY_HOOKS_DISABLED"));
            }
        });
    }

    #[test]
    fn antigravity_setup_preserves_foreign_groups_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().display().to_string();
        with_env(&[("HOME", Some(home.as_str()))], || {
            let spec = agent_spec("antigravity").unwrap();
            let config_path = antigravity_config_path();
            fs::create_dir_all(config_path.parent().unwrap()).unwrap();
            fs::write(
                &config_path,
                r#"{"safety-gate":{"enabled":true,"PreToolUse":[{"matcher":"run_command","hooks":[{"type":"command","command":"/usr/local/bin/guard.sh"}]}]}}"#,
            )
            .unwrap();

            let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
            assert!(plan.changed);
            let config: Value = serde_json::from_str(&plan.content).unwrap();
            assert_eq!(
                config["safety-gate"]["PreToolUse"][0]["hooks"][0]["command"],
                json!("/usr/local/bin/guard.sh")
            );
            assert!(config[ANTIGRAVITY_HOOK_GROUP].is_object());

            // Simulate a full setup run, then re-plan: nothing changes.
            fs::write(&config_path, &plan.content).unwrap();
            for (script_path, content) in &plan.scripts {
                fs::create_dir_all(script_path.parent().unwrap()).unwrap();
                fs::write(script_path, content).unwrap();
            }
            let replanned = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
            assert!(!replanned.changed);
        });
    }

    #[test]
    fn antigravity_setup_hardens_config_and_wrapper_directories() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().display().to_string();
        with_env(&[("HOME", Some(home.as_str()))], || {
            let root_dir = antigravity_root_dir();
            let config_dir = antigravity_config_dir();
            let scripts_dir = antigravity_scripts_dir();
            fs::create_dir_all(&scripts_dir).unwrap();
            fs::set_permissions(&root_dir, fs::Permissions::from_mode(0o777)).unwrap();
            fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o777)).unwrap();
            fs::set_permissions(&scripts_dir, fs::Permissions::from_mode(0o777)).unwrap();

            handle_hooks_setup(&test_context(), strings(&["antigravity"])).unwrap();

            assert_eq!(
                fs::metadata(&root_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&scripts_dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for event in ["before-model", "pre-tool", "post-tool"] {
                let script_path = antigravity_script_path(event);
                assert_eq!(
                    fs::metadata(script_path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
        });
    }

    #[test]
    fn antigravity_setup_rejects_symlinked_hook_directories() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let target = dir.path().join("target");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o777)).unwrap();
        std::os::unix::fs::symlink(&target, home.join(".gemini")).unwrap();
        let home = home.display().to_string();

        with_env(&[("HOME", Some(home.as_str()))], || {
            let err = handle_hooks_setup(&test_context(), strings(&["antigravity"]))
                .expect_err("symlinked Antigravity hook root must be rejected");
            assert!(err.message.contains("refusing symlinked hook directory"));
        });

        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o777
        );
    }

    #[test]
    fn antigravity_remove_plan_deletes_group_scripts_and_solely_owned_file() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().display().to_string();
        with_env(&[("HOME", Some(home.as_str()))], || {
            let spec = agent_spec("antigravity").unwrap();
            let config_path = antigravity_config_path();
            fs::create_dir_all(config_path.parent().unwrap()).unwrap();
            let plan = build_hook_setup_plan(spec, Path::new("/usr/bin/forktty")).unwrap();
            fs::write(&config_path, &plan.content).unwrap();
            for (script_path, content) in &plan.scripts {
                fs::create_dir_all(script_path.parent().unwrap()).unwrap();
                fs::write(script_path, content).unwrap();
            }

            let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
            assert!(remove.changed);
            assert_eq!(remove.scripts_dir, Some(antigravity_scripts_dir()));
            assert!(matches!(remove.action, HookRemoveAction::DeleteFile));

            // With a foreign group present, only the forktty group is removed.
            let mut config: Value = serde_json::from_str(&plan.content).unwrap();
            config["safety-gate"] = json!({"enabled": true});
            fs::write(&config_path, serde_json::to_string(&config).unwrap()).unwrap();
            let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
            assert!(remove.changed);
            match &remove.action {
                HookRemoveAction::Write(content) => {
                    let next: Value = serde_json::from_str(content).unwrap();
                    assert!(next.get(ANTIGRAVITY_HOOK_GROUP).is_none());
                    assert!(next["safety-gate"].is_object());
                }
                _ => panic!("expected a rewrite that keeps the foreign group"),
            }

            // Nothing installed: nothing to do.
            fs::remove_file(&config_path).unwrap();
            fs::remove_dir_all(antigravity_scripts_dir()).unwrap();
            let remove = build_hook_remove_plan(spec, Some(Path::new("/usr/bin/forktty"))).unwrap();
            assert!(!remove.changed);
        });
    }

    #[test]
    fn antigravity_launcher_check_reads_wrapper_scripts() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().display().to_string();
        with_env(&[("HOME", Some(home.as_str()))], || {
            let spec = agent_spec("antigravity").unwrap();
            let config_path = antigravity_config_path();
            let check =
                describe_launcher_check(spec, &config_path, Some(Path::new("/usr/bin/forktty")));
            assert_eq!(check["status"], json!("not_installed"));

            let plan = build_hook_setup_plan(spec, Path::new("/old/forktty")).unwrap();
            for (script_path, content) in &plan.scripts {
                fs::create_dir_all(script_path.parent().unwrap()).unwrap();
                fs::write(script_path, content).unwrap();
            }
            let check =
                describe_launcher_check(spec, &config_path, Some(Path::new("/new/forktty")));
            assert_eq!(check["status"], json!("stale"));
            assert_eq!(check["installedLauncher"], json!("/old/forktty"));
        });
    }

    #[test]
    fn antigravity_session_id_comes_from_conversation_id() {
        assert_eq!(
            extract_hook_session_id(&json!({"conversationId": "032316f4-2fae"})),
            Some("032316f4-2fae".to_string())
        );
    }

    #[test]
    fn codex_trust_report_classifies_recorded_and_unrecorded_events() {
        let hooks_json = Path::new("/home/me/.codex/hooks.json");
        let config_toml = Path::new("/home/me/.codex/config.toml");
        let state: toml::Table = r#"
            ["/home/me/.codex/hooks.json:pre_tool_use:0:0"]
            trusted_hash = "sha256:abc"
            ["/other/hooks.json:stop:0:0"]
            trusted_hash = "sha256:def"
        "#
        .parse()
        .unwrap();
        let report =
            codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, Some(&state));
        assert_eq!(report["status"], json!("partial"));
        assert_eq!(report["recordedEvents"], json!(["PreToolUse"]));
        assert!(report["unrecordedEvents"]
            .as_array()
            .unwrap()
            .contains(&json!("Stop")));

        let report = codex_hook_trust_report(config_toml, hooks_json, CODEX_HOOK_ENTRIES, None);
        assert_eq!(report["status"], json!("none_recorded"));
    }

    #[test]
    fn hook_setup_reminder_only_prompts_when_all_missing_or_stale() {
        assert!(
            hook_setup_reminder_message_for_statuses(["not_installed", "not_installed"])
                .unwrap()
                .contains("Install ForkTTY agent hooks")
        );
        assert!(hook_setup_reminder_message_for_statuses(["ok", "not_installed"]).is_none());
        assert!(hook_setup_reminder_message_for_statuses(["ok", "stale"])
            .unwrap()
            .contains("Refresh ForkTTY agent hooks"));
        assert!(
            hook_setup_reminder_message_for_statuses(["current_launcher_unknown"])
                .unwrap()
                .contains("Refresh ForkTTY agent hooks")
        );
    }

    #[test]
    fn hook_setup_command_uses_forktty_launcher_without_node() {
        let spec = agent_spec("codex").unwrap();
        let command = build_hook_shell_command(
            Path::new("/home/me/ForkTTY/forktty.AppImage"),
            spec,
            "session-start",
        );
        assert!(command.contains("'/home/me/ForkTTY/forktty.AppImage' hooks codex session-start"));
        assert!(!command.contains("node"));
        assert!(command.contains("FORKTTY_CODEX_HOOKS_DISABLED"));
    }

    #[test]
    fn merge_hook_config_strips_legacy_script_entry() {
        let spec = agent_spec("codex").unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "[ \"${FORKTTY_CODEX_HOOKS_DISABLED:-}\" != \"1\" ] && node '/old/scripts/forktty.mjs' hooks codex session-start || echo '{\"continue\":true,\"suppressOutput\":false}'",
                        "timeout": 5000
                    }]
                }]
            },
            "custom": true
        });
        let (_, merged) =
            merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
        let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let command = entries[0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("'/usr/bin/forktty' hooks codex session-start"));
        assert!(!command.contains("forktty.mjs"));
        assert_eq!(merged["custom"], Value::Bool(true));
    }

    #[test]
    fn merge_hook_config_installs_current_codex_observability_events() {
        let (_, codex) = merge_hook_config(
            &json!({}),
            agent_spec("codex").unwrap(),
            Path::new("/usr/bin/forktty"),
        )
        .unwrap();
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "PreCompact",
            "PostCompact",
            "SubagentStart",
            "SubagentStop",
            "Stop",
        ] {
            assert!(codex["hooks"][event].is_array(), "missing Codex {event}");
        }
        assert!(codex["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("hooks codex pre-tool"));
    }

    #[test]
    fn hook_templates_match_native_installer_specs() {
        for (agent, template) in [
            ("codex", "codex-hooks.json"),
            ("claude", "claude-settings.json"),
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("hooks")
                .join(template);
            let template_json: Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let spec = agent_spec(agent).unwrap();
            let profile = if agent == "claude" {
                HookSetupProfile::Lifecycle
            } else {
                HookSetupProfile::Full
            };
            let (_, generated) = merge_hook_config_with_profile(
                &json!({}),
                spec,
                Path::new("{{FORKTTY_LAUNCHER}}"),
                profile,
            )
            .unwrap();
            assert_eq!(
                template_json,
                generated_without_installer_tags(generated),
                "{template} is out of sync with the native hook installer"
            );
        }
    }

    #[test]
    fn opencode_plugin_plan_is_idempotent_and_protects_unmanaged_files() {
        let dir = tempfile::tempdir().unwrap();
        let home_s = dir.path().display().to_string();
        with_env(
            &[("HOME", Some(&home_s)), ("OPENCODE_CONFIG_DIR", None)],
            || {
                let spec = agent_spec("opencode").unwrap();
                let launcher = Path::new("/usr/bin/forktty");
                let first = build_hook_setup_plan(spec, launcher).unwrap();
                assert!(first.changed);
                assert!(first.content.contains(OPENCODE_PLUGIN_TAG));
                assert!(first.content.contains("const HOOK_TIMEOUT_MS = 30000;"));
                assert!(first.content.contains("timeout: HOOK_TIMEOUT_MS,"));
                assert_eq!(
                    extract_launcher_from_opencode_plugin(&first.content).as_deref(),
                    Some("/usr/bin/forktty")
                );

                ensure_parent_dir(&first.config_path).unwrap();
                atomic_write_file(&first.config_path, first.content.as_bytes()).unwrap();
                let second = build_hook_setup_plan(spec, launcher).unwrap();
                assert!(!second.changed);

                atomic_write_file(
                    &first.config_path,
                    b"export const Mine = async () => ({})\n",
                )
                .unwrap();
                assert_err_contains(
                    build_hook_setup_plan(spec, launcher),
                    "refusing to overwrite unmanaged plugin file",
                );
            },
        );
    }

    fn generated_without_installer_tags(mut value: Value) -> Value {
        if let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) {
            for entries in hooks.values_mut().filter_map(Value::as_array_mut) {
                for entry in entries {
                    if let Some(object) = entry.as_object_mut() {
                        object.remove("forkttySource");
                    }
                }
            }
        }
        value
    }

    #[test]
    fn merge_hook_config_preserves_unrelated_hook_commands() {
        let spec = agent_spec("codex").unwrap();
        let existing = json!({
            "hooks": {
                "SessionStart": [{
                    "hooks": [{
                        "type": "command",
                        "command": "custom-wrapper hooks codex session-start"
                    }]
                }]
            }
        });
        let (_, merged) =
            merge_hook_config(&existing, spec, Path::new("/usr/bin/forktty")).unwrap();
        let entries = merged["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0]["hooks"][0]["command"],
            Value::String("custom-wrapper hooks codex session-start".to_string())
        );
    }

    #[test]
    fn codex_and_claude_hook_timeouts_are_seconds_within_provider_budget() {
        // Codex docs treat `timeout` as seconds (default 600). Claude Code
        // hooks reference also documents seconds (default 600, 30 for
        // UserPromptSubmit). The installer must emit a value that is
        // measured in seconds and stays under the smaller of the two
        // provider defaults so we never block the agent loop longer than a
        // local round-trip needs.
        assert_eq!(HOOK_ENTRY_TIMEOUT_SECS, 30);
        for spec in [agent_spec("codex").unwrap(), agent_spec("claude").unwrap()] {
            let (_, config) =
                merge_hook_config(&json!({}), spec, Path::new("/usr/bin/forktty")).unwrap();
            let hooks = config["hooks"].as_object().expect("hooks object");
            for entries in hooks.values() {
                for entry in entries.as_array().expect("entry array") {
                    if !is_forktty_managed_entry(entry) {
                        continue;
                    }
                    for hook in entry["hooks"].as_array().expect("hooks array") {
                        let timeout = hook["timeout"].as_u64().expect("integer timeout");
                        assert_eq!(
                            timeout, HOOK_ENTRY_TIMEOUT_SECS,
                            "{} entry must encode timeout in seconds",
                            spec.key
                        );
                        assert!(
                            timeout < 600,
                            "{} timeout must stay under provider default",
                            spec.key
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn surface_id_prefers_active_workspace() {
        let workspaces = json!([
            { "id": "a", "active": false, "focused_surface_id": "surface-a" },
            { "id": "b", "active": true, "focused_surface_id": "surface-b" }
        ]);
        assert_eq!(
            surface_id_from_workspace_list(&workspaces),
            Some("surface-b".to_string())
        );
    }

    fn ctx_for(socket_path: &Path) -> CliContext {
        CliContext {
            json: false,
            socket_path: socket_path.to_path_buf(),
            socket_explicit: true,
            verbose: false,
        }
    }

    #[test]
    fn capabilities_requests_system_capabilities() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": { "version": "9.9.9", "methods": ["system.ping"] },
                })
                .to_string()
            },
            |socket_path| {
                handle_capabilities(&ctx_for(socket_path), vec![]).unwrap();
            },
        );
        assert_eq!(request["method"], "system.capabilities");
    }

    #[test]
    fn capabilities_formatter_includes_team_provider_policy() {
        let lines = format_capabilities_lines(&json!({
            "version": "9.9.9",
            "methods": ["system.ping"],
            "team_provider_policy": {
                "default_agent": "auto",
                "provider_order": ["codex", "pi"],
                "auto_fallback": true,
                "disabled_agents": ["claude"]
            },
            "provider_capabilities": {
                "codex": {
                    "program": "codex",
                    "available_on_path": true,
                    "launchable": true,
                    "executable": "/usr/bin/codex",
                    "disabled_by_config": false
                },
                "claude": {
                    "program": "claude",
                    "available_on_path": true,
                    "launchable": false,
                    "executable": "/usr/bin/claude",
                    "disabled_by_config": true,
                    "unavailable_reason": "disabled by team.disabled_agents"
                },
                "pi": {
                    "program": "pi",
                    "available_on_path": false,
                    "launchable": false,
                    "disabled_by_config": false,
                    "unavailable_reason": "pi not found on PATH"
                },
                "opencode": {
                    "program": "/opt/opencode/bin/opencode",
                    "default_program": "opencode",
                    "configured_command": "/opt/opencode/bin/opencode",
                    "available_on_path": false,
                    "launchable": true,
                    "executable": "/opt/opencode/bin/opencode",
                    "disabled_by_config": false
                }
            },
            "pty_persistence": {
                "config_enabled": true,
                "active": false,
                "available": false,
                "broker": null,
                "broker_executable": null,
                "scope": "plain_terminal_surfaces",
                "unavailable_reason": "broker_not_found"
            }
        }));

        assert_eq!(
            lines,
            vec![
                "version 9.9.9",
                "system.ping",
                "team providers default auto fallback on order codex, pi disabled claude",
                "provider codex found /usr/bin/codex",
                "provider claude disabled disabled by team.disabled_agents",
                "provider pi missing pi not found on PATH",
                "provider opencode configured /opt/opencode/bin/opencode",
                "pty persistence configured-missing broker_not_found",
            ]
        );
    }

    #[test]
    fn identify_requests_system_identify_with_env_caller_context() {
        let request = with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
                ("FORKTTY_SURFACE_ID", Some("surface-env")),
            ],
            || {
                with_socket_response(
                    |req| {
                        json!({
                            "id": req["id"],
                            "ok": true,
                            "result": {
                                "workspace": {"id": "workspace-env", "name": "main"},
                                "surface": {"id": "surface-env"},
                                "caller": {}
                            },
                        })
                        .to_string()
                    },
                    |socket_path| {
                        handle_identify(&ctx_for(socket_path), vec![]).unwrap();
                    },
                )
            },
        );
        assert_eq!(request["method"], "system.identify");
        assert_eq!(request["params"]["caller_workspace_id"], "workspace-env");
        assert_eq!(request["params"]["caller_surface_id"], "surface-env");
        assert!(request["params"].get("workspace_id").is_none());
        assert!(request["params"].get("surface_id").is_none());
    }

    #[test]
    fn identify_explicit_target_does_not_mix_env_workspace_selector() {
        with_env(
            &[
                ("FORKTTY_WORKSPACE_ID", Some("workspace-env")),
                ("FORKTTY_SURFACE_ID", Some("surface-env")),
            ],
            || {
                let by_surface =
                    identify_params(strings(&["--surface-id", "surface-other"]), "identify")
                        .unwrap();
                assert_eq!(by_surface["surface_id"], "surface-other");
                assert!(by_surface.get("workspace_id").is_none());
                assert_eq!(by_surface["caller_workspace_id"], "workspace-env");
                assert_eq!(by_surface["caller_surface_id"], "surface-env");

                let by_name =
                    identify_params(strings(&["--workspace-name", "named"]), "identify").unwrap();
                assert_eq!(by_name["workspace_name"], "named");
                assert!(by_name.get("workspace_id").is_none());
                assert!(by_name.get("surface_id").is_none());
                assert_eq!(by_name["caller_workspace_id"], "workspace-env");
                assert_eq!(by_name["caller_surface_id"], "surface-env");

                let by_worktree =
                    identify_params(strings(&["--worktree-name", "feature"]), "identify").unwrap();
                assert_eq!(by_worktree["worktree_name"], "feature");
                assert!(by_worktree.get("workspace_id").is_none());
                assert!(by_worktree.get("surface_id").is_none());
                assert_eq!(by_worktree["caller_workspace_id"], "workspace-env");
                assert_eq!(by_worktree["caller_surface_id"], "surface-env");
            },
        );
    }

    #[test]
    fn wait_agent_status_aliases_match_expected_lifecycles() {
        for alias in ["running", "working"] {
            let status = agent_wait_status_from_cli(alias).unwrap();
            assert!(status.matches("running"));
            assert!(!status.matches("idle"));
        }
        for alias in ["needs_input", "needs-input", "blocked"] {
            let status = agent_wait_status_from_cli(alias).unwrap();
            assert!(status.matches("needs_input"));
            assert!(!status.matches("running"));
        }
        let done = agent_wait_status_from_cli("done").unwrap();
        assert!(done.matches("idle"));
        assert!(done.matches("ended"));
        assert!(!done.matches("running"));
        let closed = agent_wait_status_from_cli("closed").unwrap();
        assert!(closed.matches("ended"));
        assert!(!closed.matches("idle"));
    }

    #[test]
    fn wait_agent_status_rejects_timeout_and_interval_out_of_bounds() {
        let timeout_options = parse_flags(strings(&["--timeout-ms", "120001"]), &[]).options;
        let timeout_error = agent_wait_timeout_ms_from_options(&timeout_options).unwrap_err();
        assert!(timeout_error
            .message
            .contains("--timeout-ms must be 0..=120000"));

        let zero_interval_options = parse_flags(strings(&["--interval-ms", "0"]), &[]).options;
        let zero_interval_error =
            agent_wait_interval_ms_from_options(&zero_interval_options).unwrap_err();
        assert!(zero_interval_error
            .message
            .contains("--interval-ms must be 1..=5000"));

        let high_interval_options = parse_flags(strings(&["--interval-ms", "5001"]), &[]).options;
        let high_interval_error =
            agent_wait_interval_ms_from_options(&high_interval_options).unwrap_err();
        assert!(high_interval_error
            .message
            .contains("--interval-ms must be 1..=5000"));
    }

    #[test]
    fn wait_agent_status_polls_context_snapshot_until_match() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let requests = with_socket_server(
            2,
            move |req| {
                let call = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let lifecycle = if call == 0 { "running" } else { "needs_input" };
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agent_health": [{
                            "workspace_id": "w1",
                            "surface_id": "surface-1",
                            "agent": "codex",
                            "lifecycle": lifecycle
                        }]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_wait(
                    &ctx_for(socket_path),
                    strings(&[
                        "agent-status",
                        "--surface-id",
                        "surface-1",
                        "--agent",
                        "codex",
                        "--status",
                        "needs_input",
                        "--timeout-ms",
                        "1000",
                        "--interval-ms",
                        "1",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request["method"], "context.snapshot");
            assert_eq!(request["params"]["surface_id"], "surface-1");
            assert_eq!(request["params"]["tail_lines"], 0);
            assert!(request["params"].get("status").is_none());
            assert!(request["params"].get("timeout_ms").is_none());
        }
    }

    #[test]
    fn wait_agent_status_times_out_nonzero() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agent_health": [{
                            "workspace_id": "w1",
                            "surface_id": "surface-1",
                            "agent": "codex",
                            "lifecycle": "running"
                        }]
                    },
                })
                .to_string()
            },
            |socket_path| {
                let error = handle_wait(
                    &ctx_for(socket_path),
                    strings(&[
                        "agent-status",
                        "--surface-id",
                        "surface-1",
                        "--status",
                        "idle",
                        "--timeout-ms",
                        "0",
                    ]),
                )
                .unwrap_err();
                assert_eq!(error.code.as_deref(), Some("timeout"));
            },
        );
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["surface_id"], "surface-1");
        assert_eq!(request["params"]["tail_lines"], 0);
    }

    #[test]
    fn agents_requests_agent_list_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                handle_agents(&ctx_for(socket_path), strings(&["--workspace-id", "w1"])).unwrap();
            },
        );
        assert_eq!(request["method"], "agent.list");
        assert_eq!(request["params"]["workspace_id"], "w1");
    }

    #[test]
    fn agent_health_requests_agent_health_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                handle_agent_health(&ctx_for(socket_path), strings(&["--workspace-id", "w1"]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "agent.health");
        assert_eq!(request["params"]["workspace_id"], "w1");
    }

    #[test]
    fn agent_health_formatter_escapes_control_sequences() {
        let line = format_agent_health_line(&json!({
            "agent": "codex\u{1b}",
            "session_id": "session\n1",
            "surface_id": "surface\t1",
            "workspace_id": "workspace\r1",
            "lifecycle": "ended",
            "last_activity_ms": 5678,
            "resume_cwd": "/tmp/project\u{1b}",
            "ready": false,
            "reason": "program_not_found\u{1b}",
            "program": "codex",
        }));

        assert!(line.contains("codex\\x1b"));
        assert!(line.contains("session\\n1"));
        assert!(line.contains("surface\\t1"));
        assert!(line.contains("workspace\\r1"));
        assert!(line.contains("ended"));
        assert!(line.contains("last_activity 5678ms"));
        assert!(line.contains("resume_cwd /tmp/project\\x1b"));
        assert!(line.contains("program_not_found\\x1b"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn agent_reclaim_plan_requests_plan_with_workspace_selector_and_policy() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "policy": {"now_ms": 10_000, "min_idle_ms": 5_000},
                        "candidates": [],
                        "protected": [],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_agent_reclaim_plan(
                    &ctx_for(socket_path),
                    strings(&["--workspace-id", "w1", "--min-idle-ms", "5000"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "agent.reclaim.plan");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["min_idle_ms"], 5000);
    }

    #[test]
    fn agent_reclaim_plan_formatter_escapes_control_sequences() {
        let line = format_agent_reclaim_plan_line(&json!({
            "policy": {"now_ms": 10_000, "min_idle_ms": 5_000},
            "candidates": [{
                "agent": "codex\u{1b}",
                "session_id": "session\n1",
                "surface_id": "surface\t1",
                "workspace_id": "workspace\r1",
                "idle_ms": 9_000,
            }],
            "protected": [{
                "agent": "claude_code",
                "session_id": "session\u{1b}2",
                "surface_id": "surface2",
                "protect_reason": "needs_input\n",
            }],
        }));

        assert!(line.contains("candidates codex\\x1b:session\\n1@surface\\t1 idle 9000ms"));
        assert!(line.contains("protected claude_code:session\\x1b2@surface2 needs_input\\n"));
        assert!(line.contains("min_idle_ms 5000"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn hibernate_agent_requests_agent_hibernate_with_surface_id_and_policy() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface": {"id": "surface-1"},
                        "agent": "codex",
                        "session_id": "codex-session-1",
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_hibernate_agent(
                    &ctx_for(socket_path),
                    strings(&["--surface-id", "surface-1", "--min-idle-ms", "5000"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "agent.hibernate");
        assert_eq!(request["params"]["surface_id"], "surface-1");
        assert_eq!(request["params"]["min_idle_ms"], 5000);
    }

    #[test]
    fn agent_hibernate_formatter_escapes_control_sequences() {
        let line = format_agent_hibernate_line(&json!({
            "surface": {"id": "surface\n1"},
            "agent": "codex\u{1b}",
            "session_id": "session\t1",
        }));

        assert!(line.contains("surface\\n1"));
        assert!(line.contains("codex\\x1b"));
        assert!(line.contains("session\\t1"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn reclaim_agents_requests_agent_reclaim_with_workspace_selector_and_limit() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "policy": {"min_idle_ms": 5000},
                        "hibernated": [],
                        "protected": [],
                        "failed": [],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_reclaim_agents(
                    &ctx_for(socket_path),
                    strings(&[
                        "--workspace-id",
                        "w1",
                        "--min-idle-ms",
                        "5000",
                        "--limit",
                        "3",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "agent.reclaim");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["min_idle_ms"], 5000);
        assert_eq!(request["params"]["limit"], 3);
    }

    #[test]
    fn agent_reclaim_formatter_reports_counts() {
        let line = format_agent_reclaim_line(&json!({
            "hibernated": [{}, {}],
            "protected": [{}],
            "failed": [{}, {}, {}],
        }));

        assert_eq!(line, "hibernated 2 | protected 1 | failed 3");
    }

    #[test]
    fn resume_agent_requests_agent_resume_with_surface_id() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface": {"id": "surface-new"},
                        "agent": "codex",
                        "session_id": "codex-session-1",
                        "argv": ["codex", "resume", "codex-session-1"],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_resume_agent(
                    &ctx_for(socket_path),
                    strings(&["--surface-id", "surface-1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "agent.resume");
        assert_eq!(request["params"]["surface_id"], "surface-1");
    }

    #[test]
    fn agent_resume_formatter_escapes_control_sequences() {
        let line = format_agent_resume_line(&json!({
            "surface": {"id": "surface\nnew"},
            "agent": "codex\u{1b}",
            "session_id": "session\t1",
        }));

        assert!(line.contains("surface\\nnew"));
        assert!(line.contains("codex\\x1b"));
        assert!(line.contains("session\\t1"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn teams_requests_team_list_with_filters() {
        let request = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
            |socket_path| {
                handle_team_list(
                    &ctx_for(socket_path),
                    strings(&[
                        "--workspace-id",
                        "w1",
                        "--status",
                        "active",
                        "--query",
                        "ship",
                        "--limit",
                        "10",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.list");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["status"], "active");
        assert_eq!(request["params"]["query"], "ship");
        assert_eq!(request["params"]["limit"], 10);
    }

    #[test]
    fn team_upsert_requests_team_upsert() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "team-1", "name": "Launch", "status": "active"},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_upsert(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "--workspace-id",
                        "w1",
                        "--leader-surface-id",
                        "s1",
                        "--name",
                        "Launch",
                        "--status",
                        "active",
                        "--goal",
                        "ship runtime",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.upsert");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["leader_surface_id"], "s1");
        assert_eq!(request["params"]["name"], "Launch");
        assert_eq!(request["params"]["status"], "active");
        assert_eq!(request["params"]["goal"], "ship runtime");
    }

    #[test]
    fn team_worker_heartbeat_requests_team_worker_heartbeat() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "worker-1", "status": "running"},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_worker_heartbeat(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "worker-1",
                        "--status",
                        "running",
                        "--assigned-task-id",
                        "task-1",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.worker.heartbeat");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["worker_id"], "worker-1");
        assert_eq!(request["params"]["status"], "running");
        assert_eq!(request["params"]["assigned_task_id"], "task-1");
    }

    #[test]
    fn team_worker_launch_requests_team_worker_launch() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-2"},
                        "argv": ["codex", "--model", "test"]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_team_worker_launch(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "worker-2",
                        "--agent",
                        "codex",
                        "--role",
                        "reviewer",
                        "--assigned-task-id",
                        "task-1",
                        "--args=--model,test",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.worker.launch");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["worker_id"], "worker-2");
        assert_eq!(request["params"]["agent"], "codex");
        assert_eq!(request["params"]["role"], "reviewer");
        assert_eq!(request["params"]["assigned_task_id"], "task-1");
        assert_eq!(request["params"]["args"], json!(["--model", "test"]));
    }

    #[test]
    fn team_worker_launch_without_agent_requests_auto_launch() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-2", "agent": "pi"},
                        "argv": ["pi"],
                        "selection": {"requested_agent": "auto", "selected_agent": "pi"}
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_team_worker_launch(
                    &ctx_for(socket_path),
                    strings(&["team-1", "worker-2", "--role", "reviewer"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.worker.launch");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["worker_id"], "worker-2");
        assert!(request["params"].get("agent").is_none());
        assert_eq!(request["params"]["role"], "reviewer");
    }

    #[test]
    fn team_worker_launch_formatter_surfaces_auto_selection_and_task_context() {
        let line = format_team_worker_launch_line(&json!({
            "surface": {"id": "surface-2"},
            "worker": {
                "id": "worker-2",
                "agent": "pi",
                "role": "reviewer",
                "assigned_task_id": "task-1"
            },
            "argv": ["pi", "--readonly"],
            "selection": {
                "requested_agent": "auto",
                "selected_agent": "pi",
                "reason": "first available provider"
            }
        }));

        assert_eq!(
            line,
            "Launched worker worker-2 agent pi role reviewer task task-1 in surface-2: pi --readonly selected pi (first available provider)"
        );
    }

    #[test]
    fn team_worker_health_requests_team_worker_health() {
        let request = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": {"workers": []}}).to_string(),
            |socket_path| {
                handle_team_worker_health(
                    &ctx_for(socket_path),
                    strings(&["team-1", "--stale-after-ms", "1000"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.worker.health");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["stale_after_ms"], 1000);
    }

    #[test]
    fn team_worker_text_actions_preserve_text() {
        let nudge = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
            |socket_path| {
                handle_team_worker_nudge(
                    &ctx_for(socket_path),
                    strings(&["team-1", "worker-2", "--text", "ping\r"]),
                )
                .unwrap();
            },
        );
        assert_eq!(nudge["method"], "team.worker.nudge");
        assert_eq!(nudge["params"]["text"], "ping\r");

        let shutdown = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
            |socket_path| {
                handle_team_worker_shutdown(
                    &ctx_for(socket_path),
                    strings(&["team-1", "worker-2"]),
                )
                .unwrap();
            },
        );
        assert_eq!(shutdown["method"], "team.worker.shutdown");
        assert_eq!(shutdown["params"]["worker_id"], "worker-2");

        let shutdown_options = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": {"sent": true}}).to_string(),
            |socket_path| {
                handle_team_worker_shutdown(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "worker-2",
                        "--text",
                        "stop",
                        "--no-submit",
                        "--close",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(shutdown_options["method"], "team.worker.shutdown");
        assert_eq!(shutdown_options["params"]["text"], "stop");
        assert_eq!(shutdown_options["params"]["submit"], false);
        assert_eq!(shutdown_options["params"]["close_surface"], true);
    }

    #[test]
    fn team_task_upsert_requests_team_task_upsert_with_dependencies() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "task-1", "status": "open"},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_task_upsert(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "task-1",
                        "--title",
                        "Build runtime",
                        "--status",
                        "open",
                        "--detail",
                        "control plane",
                        "--depends-on",
                        "task-0,task-base",
                        "--assigned-worker-id",
                        "worker-1",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.task.upsert");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["task_id"], "task-1");
        assert_eq!(request["params"]["title"], "Build runtime");
        assert_eq!(request["params"]["detail"], "control plane");
        assert_eq!(
            request["params"]["depends_on"],
            json!(["task-0", "task-base"])
        );
        assert_eq!(request["params"]["assigned_worker_id"], "worker-1");
    }

    #[test]
    fn team_message_send_requests_team_message_send_preserving_body() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "msg-1", "delivered": false},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_message_send(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "--message-id",
                        "msg-1",
                        "--from",
                        "leader",
                        "--to-worker-id",
                        "worker-1",
                        "--task-id",
                        "task-1",
                        "--body",
                        "  continue\n",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.message.send");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["message_id"], "msg-1");
        assert_eq!(request["params"]["from"], "leader");
        assert_eq!(request["params"]["to_worker_id"], "worker-1");
        assert_eq!(request["params"]["task_id"], "task-1");
        assert_eq!(request["params"]["body"], "  continue\n");
    }

    #[test]
    fn team_message_send_rejects_extra_args_when_body_flag_is_used() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_team_message_send(
                &ctx,
                strings(&["team-1", "ignored", "--from", "leader", "--body", "body"]),
            ),
            "unexpected argument ignored",
        );
    }

    #[test]
    fn team_message_dispatch_requests_team_message_dispatch() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"sent": true, "message": {"id": "msg-1"}},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_message_dispatch(
                    &ctx_for(socket_path),
                    strings(&["team-1", "msg-1", "--worker-id", "worker-1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.message.dispatch");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["message_id"], "msg-1");
        assert_eq!(request["params"]["worker_id"], "worker-1");
    }

    #[test]
    fn team_message_dispatch_submit_sends_submit_param() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"sent": true, "submitted": true, "message": {"id": "msg-1"}},
                })
                .to_string()
            },
            |socket_path| {
                handle_team_message_dispatch(
                    &ctx_for(socket_path),
                    strings(&["team-1", "msg-1", "--worker-id", "worker-1", "--submit"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.message.dispatch");
        assert_eq!(request["params"]["submit"], true);
    }

    fn with_team_ask_flow_server(test: impl FnOnce(&Path)) -> Vec<Value> {
        with_socket_server(
            6,
            |req| {
                let result = match req["method"].as_str().unwrap_or("") {
                    "team.upsert" => json!({"id": "team-1", "status": "active"}),
                    "team.worker.launch" => json!({
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-1"},
                    }),
                    "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                    "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                    "team.message.dispatch" => {
                        json!({"sent": true, "message": {"id": "msg-1"}})
                    }
                    other => panic!("unexpected method {other}"),
                };
                json!({"id": req["id"], "ok": true, "result": result}).to_string()
            },
            test,
        )
    }

    #[test]
    fn team_ask_runs_high_level_worker_flow() {
        let requests = with_team_ask_flow_server(|socket_path| {
            handle_team(
                &ctx_for(socket_path),
                strings(&[
                    "ask",
                    "team-1",
                    "worker-1",
                    "--agent",
                    "claude",
                    "--task-id",
                    "task-1",
                    "--prompt",
                    "Review this",
                    "--role",
                    "reviewer",
                    "--title",
                    "Review",
                    "--goal",
                    "Check command ergonomics",
                    "--submit=false",
                ]),
            )
            .unwrap();
        });

        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "team.upsert",
                "team.task.upsert",
                "team.worker.launch",
                "team.task.upsert",
                "team.message.send",
                "team.message.dispatch",
            ]
        );
        assert_eq!(requests[0]["params"]["team_id"], "team-1");
        assert_eq!(requests[0]["params"]["status"], "active");
        assert_eq!(requests[0]["params"]["goal"], "Check command ergonomics");
        assert!(!requests[1]["params"]
            .as_object()
            .unwrap()
            .contains_key("assigned_worker_id"));
        assert_eq!(requests[1]["params"]["status"], "open");
        assert_eq!(requests[1]["params"]["title"], "Review");
        assert_eq!(requests[1]["params"]["detail"], "Review this");
        assert_eq!(requests[2]["params"]["agent"], "claude");
        assert_eq!(requests[2]["params"]["role"], "reviewer");
        assert_eq!(requests[2]["params"]["assigned_task_id"], "task-1");
        assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
        assert_eq!(requests[3]["params"]["status"], "running");
        assert_eq!(requests[4]["params"]["from"], "leader");
        assert_eq!(requests[4]["params"]["to_worker_id"], "worker-1");
        assert_eq!(requests[4]["params"]["body"], "Review this");
        assert_eq!(requests[5]["params"]["message_id"], "msg-1");
        assert_eq!(requests[5]["params"]["worker_id"], "worker-1");
        assert!(requests[5]["params"].get("submit").is_none());
    }

    #[test]
    fn team_ask_creates_task_before_assigning_fresh_worker() {
        let mut worker_exists = false;
        let requests = with_socket_server_until_done(
            move |req| {
                let result = match req["method"].as_str().unwrap_or("") {
                    "team.upsert" => json!({"id": "team-1", "status": "active"}),
                    "team.task.upsert" => {
                        if req["params"].get("assigned_worker_id").is_some() && !worker_exists {
                            return json!({
                                "id": req["id"],
                                "ok": false,
                                "error": {
                                    "code": "not_found",
                                    "message": "worker not found",
                                },
                            })
                            .to_string();
                        }
                        json!({"id": "task-1", "status": "running"})
                    }
                    "team.worker.launch" => {
                        assert_eq!(req["params"]["assigned_task_id"], "task-1");
                        worker_exists = true;
                        json!({
                            "surface": {"id": "surface-2"},
                            "worker": {"id": "worker-1"},
                        })
                    }
                    "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                    "team.message.dispatch" => {
                        json!({"sent": true, "message": {"id": "msg-1"}})
                    }
                    other => panic!("unexpected method {other}"),
                };
                json!({"id": req["id"], "ok": true, "result": result}).to_string()
            },
            |socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "ask",
                        "team-1",
                        "worker-1",
                        "--agent",
                        "claude",
                        "--task-id",
                        "task-1",
                        "--prompt",
                        "Review this",
                        "--submit=false",
                    ]),
                )
                .unwrap();
            },
        );

        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "team.upsert",
                "team.task.upsert",
                "team.worker.launch",
                "team.task.upsert",
                "team.message.send",
                "team.message.dispatch",
            ]
        );
        assert!(!requests[1]["params"]
            .as_object()
            .unwrap()
            .contains_key("assigned_worker_id"));
        assert_eq!(requests[1]["params"]["status"], "open");
        assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
        assert_eq!(requests[3]["params"]["status"], "running");
    }

    #[test]
    fn team_ask_binds_team_to_current_surface_env() {
        let requests = with_env(
            &[
                ("FORKTTY_SURFACE_ID", Some(" surface-orchestrator ")),
                ("FORKTTY_WORKSPACE_ID", Some(" workspace-orchestrator ")),
            ],
            || {
                with_team_ask_flow_server(|socket_path| {
                    handle_team(
                        &ctx_for(socket_path),
                        strings(&[
                            "ask",
                            "team-1",
                            "worker-1",
                            "--agent",
                            "claude",
                            "--task-id",
                            "task-1",
                            "--prompt",
                            "Review this",
                            "--submit=false",
                        ]),
                    )
                    .unwrap();
                })
            },
        );

        let params = requests[0]["params"].as_object().unwrap();
        assert_eq!(params["leader_surface_id"], "surface-orchestrator");
        assert!(!params.contains_key("workspace_id"));
    }

    #[test]
    fn team_ask_binds_team_to_current_workspace_env_without_surface() {
        let requests = with_env(
            &[
                ("FORKTTY_SURFACE_ID", None),
                ("FORKTTY_WORKSPACE_ID", Some(" workspace-orchestrator ")),
            ],
            || {
                with_team_ask_flow_server(|socket_path| {
                    handle_team(
                        &ctx_for(socket_path),
                        strings(&[
                            "ask",
                            "team-1",
                            "worker-1",
                            "--agent",
                            "claude",
                            "--task-id",
                            "task-1",
                            "--prompt",
                            "Review this",
                            "--submit=false",
                        ]),
                    )
                    .unwrap();
                })
            },
        );

        let params = requests[0]["params"].as_object().unwrap();
        assert_eq!(params["workspace_id"], "workspace-orchestrator");
        assert!(!params.contains_key("leader_surface_id"));
    }

    #[test]
    fn team_review_builds_read_only_commit_prompt() {
        let requests = with_socket_server(
            6,
            |req| {
                let result = match req["method"].as_str().unwrap_or("") {
                    "team.upsert" => json!({"id": "team-1", "status": "active"}),
                    "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                    "team.worker.launch" => json!({
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-1"},
                    }),
                    "team.message.send" => json!({"id": "msg-1", "delivered": false}),
                    "team.message.dispatch" => {
                        json!({"sent": true, "message": {"id": "msg-1"}})
                    }
                    other => panic!("unexpected method {other}"),
                };
                json!({"id": req["id"], "ok": true, "result": result}).to_string()
            },
            |socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "review",
                        "team-1",
                        "worker-1",
                        "--agent",
                        "claude",
                        "--task-id",
                        "task-1",
                        "--commit",
                        "HEAD",
                    ]),
                )
                .unwrap();
            },
        );

        let body = requests[4]["params"]["body"].as_str().unwrap();
        assert!(body.contains("Review commit HEAD"));
        assert!(body.contains("read-only inspection"));
        assert!(body.contains("file/line references"));
        assert_eq!(requests[1]["params"]["status"], "open");
        assert!(!requests[1]["params"]
            .as_object()
            .unwrap()
            .contains_key("assigned_worker_id"));
        assert_eq!(requests[3]["params"]["assigned_worker_id"], "worker-1");
        assert_eq!(requests[3]["params"]["status"], "running");
        assert_eq!(requests[5]["params"]["submit"], true);
    }

    #[test]
    fn team_ask_flow_formatter_reports_submission_target() {
        let line = format_team_ask_flow_line(&json!({
            "worker": {
                "worker": {
                    "id": "worker-1",
                    "agent": "claude"
                },
                "surface": {"id": "surface-2"}
            },
            "task": {"id": "task-1"},
            "dispatch": {
                "submitted": true
            }
        }));

        assert_eq!(
            line,
            "Team prompt submitted to worker-1 agent claude task task-1 surface surface-2"
        );
    }

    #[test]
    fn team_ask_flow_formatter_reports_selected_agent_and_assigned_task() {
        let line = format_team_ask_flow_line(&json!({
            "worker": {
                "worker": {
                    "id": "worker-1",
                    "agent": "auto",
                    "assigned_task_id": "task-1"
                },
                "surface": {"id": "surface-2"},
                "selection": {
                    "requested_agent": "auto",
                    "selected_agent": "pi"
                }
            },
            "dispatch": {
                "submitted": false
            }
        }));

        assert_eq!(
            line,
            "Team prompt dispatched to worker-1 agent pi task task-1 surface surface-2"
        );
    }

    #[test]
    fn team_ask_labels_mid_flow_socket_failures() {
        let requests = with_socket_server(
            5,
            |req| {
                let response = match req["method"].as_str().unwrap_or("") {
                    "team.upsert" => json!({"id": "team-1", "status": "active"}),
                    "team.task.upsert" => json!({"id": "task-1", "status": "running"}),
                    "team.worker.launch" => json!({
                        "surface": {"id": "surface-2"},
                        "worker": {"id": "worker-1"},
                    }),
                    "team.message.send" => {
                        return json!({
                            "id": req["id"],
                            "ok": false,
                            "error": {"code": "error", "message": "queue failed"},
                        })
                        .to_string();
                    }
                    other => panic!("unexpected method {other}"),
                };
                json!({"id": req["id"], "ok": true, "result": response}).to_string()
            },
            |socket_path| {
                assert_err_contains(
                    handle_team(
                        &ctx_for(socket_path),
                        strings(&[
                            "ask",
                            "team-1",
                            "worker-1",
                            "--agent",
                            "claude",
                            "--task-id",
                            "task-1",
                            "--prompt",
                            "Review this",
                        ]),
                    ),
                    "team ask failed while queueing prompt after worker launch",
                );
            },
        );

        assert_eq!(
            requests
                .iter()
                .map(|request| request["method"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "team.upsert",
                "team.task.upsert",
                "team.worker.launch",
                "team.task.upsert",
                "team.message.send",
            ]
        );
    }

    #[test]
    fn team_watch_reads_summary_health_inbox_and_events() {
        let requests = with_socket_server(
            4,
            |req| {
                let result = match req["method"].as_str().unwrap_or("") {
                    "team.summary" => json!({
                        "team_id": "team-1",
                        "status": "active",
                        "workers_total": 1,
                        "workers_active": 1,
                        "tasks_total": 2,
                        "tasks_open": 1,
                        "messages_pending": 0,
                        "last_event_seq": 7,
                    }),
                    "team.worker.health" => json!({"team_id": "team-1", "workers": []}),
                    "team.inbox" => json!([]),
                    "team.events" => json!([]),
                    other => panic!("unexpected method {other}"),
                };
                json!({"id": req["id"], "ok": true, "result": result}).to_string()
            },
            |socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "watch",
                        "team-1",
                        "--stale-after-ms",
                        "5000",
                        "--limit",
                        "3",
                    ]),
                )
                .unwrap();
            },
        );

        assert_eq!(requests[0]["method"], "team.summary");
        assert_eq!(requests[0]["params"]["team_id"], "team-1");
        assert_eq!(requests[1]["method"], "team.worker.health");
        assert_eq!(requests[1]["params"]["stale_after_ms"], 5000);
        assert_eq!(requests[2]["method"], "team.inbox");
        assert_eq!(requests[2]["params"]["limit"], 3);
        assert_eq!(requests[3]["method"], "team.events");
        assert_eq!(requests[3]["params"]["limit"], 3);
    }

    #[test]
    fn team_summary_formatter_uses_server_field_names() {
        assert_eq!(
            format_team_summary_line(&json!({
                "team_id": "team-1",
                "status": "active",
                "workers_total": 3,
                "workers_active": 2,
                "tasks_total": 5,
                "tasks_open": 4,
                "messages_pending": 1,
                "last_event_seq": 9,
            })),
            "team-1 active workers 2/3 tasks 4/5 pending 1 last_event 9"
        );
    }

    #[test]
    fn team_worker_health_formatter_surfaces_final_state_and_runtime_readiness() {
        let line = format_team_worker_health_line(&json!({
            "worker_id": "worker-1",
            "lifecycle": "starting",
            "final_state": "starting",
            "status": "running",
            "surface_id": "surface-1",
            "surface_present": true,
            "surface_runtime_present": true,
            "surface_ready": false,
            "heartbeat_age_ms": 42,
        }));

        assert_eq!(
            line,
            "worker worker-1 starting final_state starting status running surface surface-1 runtime present/not-ready heartbeat_age_ms 42"
        );
    }

    #[test]
    fn team_worker_health_formatter_distinguishes_missing_runtime() {
        let line = format_team_worker_health_line(&json!({
            "worker_id": "worker-1",
            "lifecycle": "surface_missing",
            "final_state": "surface_missing",
            "status": "running",
            "surface_id": "surface-1",
            "surface_present": true,
            "surface_runtime_present": false,
            "surface_ready": false,
        }));

        assert_eq!(
            line,
            "worker worker-1 surface_missing final_state surface_missing status running surface surface-1 runtime missing"
        );
    }

    #[test]
    fn team_finish_requests_verified_finish_method() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"team_id": "team-1", "finished": true},
                })
                .to_string()
            },
            |socket_path| {
                handle_team(&ctx_for(socket_path), strings(&["finish", "team-1"])).unwrap();
            },
        );
        assert_eq!(request["method"], "team.finish");
        assert_eq!(request["params"]["team_id"], "team-1");
    }

    #[test]
    fn team_finish_passes_cleanup_options() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"team_id": "team-1", "finished": false, "dry_run": true},
                })
                .to_string()
            },
            |socket_path| {
                handle_team(
                    &ctx_for(socket_path),
                    strings(&[
                        "finish",
                        "team-1",
                        "--dry-run",
                        "--close-workers",
                        "--force",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.finish");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["dry_run"], true);
        assert_eq!(request["params"]["close_workers"], true);
        assert_eq!(request["params"]["force"], true);
    }

    #[test]
    fn team_inbox_requests_team_inbox_with_include_delivered() {
        let request = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
            |socket_path| {
                handle_team_inbox(
                    &ctx_for(socket_path),
                    strings(&[
                        "team-1",
                        "--worker-id",
                        "worker-1",
                        "--include-delivered",
                        "--limit",
                        "20",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.inbox");
        assert_eq!(request["params"]["team_id"], "team-1");
        assert_eq!(request["params"]["worker_id"], "worker-1");
        assert_eq!(request["params"]["include_delivered"], true);
        assert_eq!(request["params"]["limit"], 20);
    }

    #[test]
    fn team_inbox_include_delivered_false_is_not_sent_as_true() {
        let request = with_socket_response(
            |req| json!({"id": req["id"], "ok": true, "result": []}).to_string(),
            |socket_path| {
                handle_team_inbox(
                    &ctx_for(socket_path),
                    strings(&["team-1", "--include-delivered=false"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "team.inbox");
        assert!(request["params"].get("include_delivered").is_none());
    }

    #[test]
    fn agent_session_formatter_escapes_control_sequences() {
        let line = format_agent_session_line(&json!({
            "agent": "codex\u{1b}",
            "session_id": "session\n1",
            "surface_id": "surface\t1",
            "workspace_id": "workspace\r1",
            "lifecycle": "idle",
            "last_activity_ms": 1234,
            "resume_cwd": "/tmp/project\n",
            "title": "build\u{1b}[31m",
            "cwd": "/tmp/project",
        }));

        assert!(line.contains("codex\\x1b"));
        assert!(line.contains("session\\n1"));
        assert!(line.contains("surface\\t1"));
        assert!(line.contains("workspace\\r1"));
        assert!(line.contains("idle"));
        assert!(line.contains("last_activity 1234ms"));
        assert!(line.contains("resume_cwd /tmp/project\\n"));
        assert!(line.contains("build\\x1b[31m"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn statusline_requests_status_summary_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agents": [],
                        "status": [],
                        "progress": [],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_statusline(
                    &ctx_for(socket_path),
                    strings(&["--workspace-name", "main"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "status.summary");
        assert_eq!(request["params"]["workspace_name"], "main");
    }

    #[test]
    fn status_explain_requests_context_snapshot() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agents": [{
                            "agent": "claude",
                            "surface_id": "s1",
                            "lifecycle": "needs_input",
                        }],
                        "risk_flags": ["pending_approval"],
                        "terminal_tails": [{
                            "surface_id": "s1",
                            "text": "Do you want to proceed?",
                        }],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_status(
                    &ctx_for(socket_path),
                    strings(&[
                        "explain",
                        "--workspace-name",
                        "main",
                        "--tail-lines",
                        "12",
                        "--tail-max-bytes",
                        "2048",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["workspace_name"], "main");
        assert_eq!(request["params"]["tail_lines"], 12);
        assert_eq!(request["params"]["tail_max_bytes"], 2048);
    }

    #[test]
    fn status_explain_formatter_includes_agent_evidence() {
        let line = format_context_snapshot_explain_line(&json!({
            "workspace": {"id": "w1", "name": "main"},
            "agents": [{
                "agent": "codex",
                "session_id": "sess-1",
                "surface_id": "surface-1",
                "lifecycle": "running",
                "source": "persisted_agent_session",
                "age_ms": 1200,
                "permission_mode": "bypassPermissions",
                "effective_project_cwd": "/home/simone/forktty",
                "lifecycle_evidence": {
                    "status_key": "agent:codex",
                    "status_value": "Running",
                    "readiness_reason": "ready",
                    "ready": true
                }
            }],
            "risk_flags": ["permission_bypass"],
            "terminal_tails": []
        }));

        assert!(line.contains("codex@surface-1#running"));
        assert!(line.contains("session sess-1"));
        assert!(line.contains("source persisted_agent_session"));
        assert!(line.contains("age_ms 1200"));
        assert!(line.contains("status agent:codex=Running"));
        assert!(line.contains("ready true:ready"));
        assert!(line.contains("mode bypassPermissions"));
        assert!(line.contains("cwd /home/simone/forktty"));
    }

    #[test]
    fn status_watch_can_run_one_iteration() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agents": [],
                        "risk_flags": [],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_status(
                    &ctx_for(socket_path),
                    strings(&["watch", "--count", "1", "--interval-ms", "1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "context.snapshot");
    }

    #[test]
    fn status_watch_runs_requested_iteration_count() {
        let requests = with_socket_server(
            2,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace": {"id": "w1", "name": "main"},
                        "agents": [],
                        "risk_flags": [],
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_status(
                    &ctx_for(socket_path),
                    strings(&[
                        "watch",
                        "--count",
                        "2",
                        "--interval-ms",
                        "1",
                        "--tail-lines",
                        "0",
                    ]),
                )
                .unwrap();
            },
        );

        assert_eq!(requests.len(), 2);
        for request in requests {
            assert_eq!(request["method"], "context.snapshot");
            assert_eq!(request["params"]["tail_lines"], 0);
        }
    }

    #[test]
    fn status_watch_rejects_zero_interval_before_socket() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_status(&ctx, strings(&["watch", "--interval-ms", "0"])),
            "greater than 0",
        );
    }

    #[test]
    fn context_snapshot_alias_requests_context_snapshot() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"workspace": {"id": "w1", "name": "main"}},
                })
                .to_string()
            },
            |socket_path| {
                handle_context_snapshot(
                    &ctx_for(socket_path),
                    strings(&["--surface-id", "s1", "--tail-lines", "3"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["surface_id"], "s1");
        assert_eq!(request["params"]["tail_lines"], 3);
    }

    #[test]
    fn context_snapshot_surface_id_does_not_add_env_workspace_selector() {
        let request = with_env(&[("FORKTTY_WORKSPACE_ID", Some("workspace-env"))], || {
            with_socket_response(
                |req| {
                    json!({
                        "id": req["id"],
                        "ok": true,
                        "result": {"workspace": {"id": "w1", "name": "main"}},
                    })
                    .to_string()
                },
                |socket_path| {
                    handle_context_snapshot(
                        &ctx_for(socket_path),
                        strings(&["--surface-id", "s1"]),
                    )
                    .unwrap();
                },
            )
        });
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["surface_id"], "s1");
        assert!(request["params"].get("workspace_id").is_none());
    }

    #[test]
    fn context_snapshot_uses_env_workspace_without_surface_selector() {
        let request = with_env(&[("FORKTTY_WORKSPACE_ID", Some("workspace-env"))], || {
            with_socket_response(
                |req| {
                    json!({
                        "id": req["id"],
                        "ok": true,
                        "result": {"workspace": {"id": "workspace-env", "name": "main"}},
                    })
                    .to_string()
                },
                |socket_path| {
                    handle_context_snapshot(&ctx_for(socket_path), vec![]).unwrap();
                },
            )
        });
        assert_eq!(request["method"], "context.snapshot");
        assert_eq!(request["params"]["workspace_id"], "workspace-env");
    }

    #[test]
    fn help_examples_and_completions_do_not_require_socket() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        handle_help(&ctx, strings(&["team"])).unwrap();
        handle_examples(&ctx, vec![]).unwrap();
        handle_completions(&ctx, strings(&["zsh"])).unwrap();
    }

    #[test]
    fn help_and_completions_reject_unknown_or_extra_args_before_socket() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(handle_help(&ctx, strings(&["unknown"])), "unknown topic");
        assert_err_contains(
            handle_help(&ctx, strings(&["team", "status"])),
            "help: unexpected argument status",
        );
        assert_err_contains(
            handle_completions(&ctx, strings(&["powershell"])),
            "unsupported completion shell powershell",
        );
        assert_err_contains(
            handle_completions(&ctx, strings(&["bash", "zsh"])),
            "completions requires bash, zsh, or fish",
        );
    }

    #[test]
    fn completions_include_grouped_commands_and_subcommands() {
        let bash = completion_script_for_test("bash").unwrap();
        assert!(bash.contains("team"));
        assert!(bash.contains("workflow-loop-set"));
        assert!(bash.contains("ask review watch finish"));
        assert!(bash.contains("summary explain watch"));
        assert!(bash.contains("bash zsh fish"));

        let fish = completion_script_for_test("fish").unwrap();
        assert!(fish.contains("__fish_seen_subcommand_from team"));
        assert!(fish.contains("workflow-loop-set"));
        assert!(fish.contains("ask review watch finish"));
    }

    #[test]
    fn workflow_loop_set_is_advertised_in_help_text() {
        assert!(HELP_TEXT.contains("forktty workflow-loop-set <workflow-id>"));
        assert!(WORKFLOW_HELP_TEXT.contains("workflow-loop-set <workflow-id>"));
        assert!(EXAMPLES_TEXT.contains("forktty workflow-loop-set"));
    }

    #[test]
    fn team_ask_rejects_required_options_before_socket() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_team(&ctx, strings(&["ask", "team-1", "worker-1"])),
            "team ask requires --task-id",
        );
        assert_err_contains(
            handle_team(
                &ctx,
                strings(&["ask", "team-1", "worker-1", "--task-id", "task-1"]),
            ),
            "team ask requires --prompt",
        );
        assert_err_contains(
            handle_team(
                &ctx,
                strings(&[
                    "ask",
                    "team-1",
                    "worker-1",
                    "--task-id",
                    "task-1",
                    "--prompt",
                    "   ",
                ]),
            ),
            "team ask requires --prompt",
        );
    }

    #[test]
    fn feed_requests_feed_list_with_workspace_selector_and_limit() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [
                        {
                            "type": "approval",
                            "title": "Permission",
                            "body": "Run command?",
                            "kind": "prompt",
                            "workspace_id": "w1",
                            "surface_id": "s1",
                            "created_at_ms": 123
                        }
                    ],
                })
                .to_string()
            },
            |socket_path| {
                handle_feed(
                    &ctx_for(socket_path),
                    strings(&["--workspace-id", "w1", "--limit", "20"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "feed.list");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["limit"], 20);
    }

    #[test]
    fn workflows_request_workflow_list_with_filters() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                handle_workflows(
                    &ctx_for(socket_path),
                    strings(&[
                        "--workspace-id",
                        "w1",
                        "--surface-id",
                        "s1",
                        "--session-id",
                        "sess1",
                        "--query",
                        "goal",
                        "--limit",
                        "5",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.list");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["surface_id"], "s1");
        assert_eq!(request["params"]["session_id"], "sess1");
        assert_eq!(request["params"]["query"], "goal");
        assert_eq!(request["params"]["limit"], 5);
    }

    #[test]
    fn workflow_upsert_requests_workflow_upsert() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "workflow-1",
                        "mode": "review",
                        "status": "running"
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_workflow_upsert(
                    &ctx_for(socket_path),
                    strings(&[
                        "--workflow-id",
                        "workflow-1",
                        "--workspace-id",
                        "w1",
                        "--surface-id",
                        "s1",
                        "--agent",
                        "codex",
                        "--session-id",
                        "sess1",
                        "--mode",
                        "review",
                        "--status",
                        "running",
                        "--goal",
                        "Review",
                        "--memory",
                        "Keep context",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.upsert");
        assert_eq!(request["params"]["workflow_id"], "workflow-1");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["surface_id"], "s1");
        assert_eq!(request["params"]["agent"], "codex");
        assert_eq!(request["params"]["session_id"], "sess1");
        assert_eq!(request["params"]["mode"], "review");
        assert_eq!(request["params"]["status"], "running");
        assert_eq!(request["params"]["goal"], "Review");
        assert_eq!(request["params"]["memory"], "Keep context");
    }

    #[test]
    fn workflow_plan_set_requests_workflow_plan_set() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "workflow-1",
                        "mode": "review",
                        "status": "running",
                        "plan": [{"id": "inspect"}]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_workflow_plan_set(
                    &ctx_for(socket_path),
                    strings(&[
                        "workflow-1",
                        "--steps-json",
                        r#"[{"id":"inspect","title":"Inspect","status":"done"}]"#,
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.plan.set");
        assert_eq!(request["params"]["workflow_id"], "workflow-1");
        assert_eq!(request["params"]["steps"][0]["id"], "inspect");
    }

    #[test]
    fn workflow_loop_set_requests_workflow_loop_set() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "workflow-1",
                        "loop_recipe": "review-fix-verify",
                        "loop_stage": "verify",
                        "loop_iteration": 2,
                        "loop_max_iterations": 3,
                        "loop_gates": [{"id": "fmt"}]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_workflow_loop_set(
                    &ctx_for(socket_path),
                    strings(&[
                        "workflow-1",
                        "--recipe",
                        "review-fix-verify",
                        "--stage",
                        "verify",
                        "--iteration",
                        "2",
                        "--max-iterations",
                        "3",
                        "--gates-json",
                        r#"[{"id":"fmt","kind":"command","label":"cargo fmt --all --check","status":"passed"}]"#,
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.loop.set");
        assert_eq!(request["params"]["workflow_id"], "workflow-1");
        assert_eq!(request["params"]["recipe"], "review-fix-verify");
        assert_eq!(request["params"]["stage"], "verify");
        assert_eq!(request["params"]["iteration"], 2);
        assert_eq!(request["params"]["max_iterations"], 3);
        assert_eq!(request["params"]["gates"][0]["id"], "fmt");
    }

    #[test]
    fn workflow_evidence_add_requests_workflow_evidence_add() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "workflow-1",
                        "mode": "review",
                        "status": "running",
                        "evidence": [{"id": "tests"}]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_workflow_evidence_add(
                    &ctx_for(socket_path),
                    strings(&[
                        "workflow-1",
                        "--evidence-id",
                        "tests",
                        "--kind",
                        "test",
                        "--title",
                        "cargo test",
                        "--text",
                        "passed",
                        "--path",
                        "target/test.log",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.evidence.add");
        assert_eq!(request["params"]["workflow_id"], "workflow-1");
        assert_eq!(request["params"]["evidence_id"], "tests");
        assert_eq!(request["params"]["kind"], "test");
        assert_eq!(request["params"]["title"], "cargo test");
        assert_eq!(request["params"]["text"], "passed");
        assert_eq!(request["params"]["path"], "target/test.log");
    }

    #[test]
    fn workflow_replay_requests_workflow_replay() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [{
                        "seq": 3,
                        "workflow_id": "workflow-1",
                        "kind": "workflow.evidence.added",
                        "summary": "tests"
                    }],
                })
                .to_string()
            },
            |socket_path| {
                handle_workflow_replay(
                    &ctx_for(socket_path),
                    strings(&[
                        "--workflow-id",
                        "workflow-1",
                        "--query",
                        "evidence",
                        "--since-seq",
                        "2",
                        "--limit",
                        "10",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workflow.replay");
        assert_eq!(request["params"]["workflow_id"], "workflow-1");
        assert_eq!(request["params"]["query"], "evidence");
        assert_eq!(request["params"]["since_seq"], 2);
        assert_eq!(request["params"]["limit"], 10);
    }

    #[test]
    fn project_actions_request_action_list_with_cwd() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [{
                        "id": "test",
                        "label": "Run tests",
                        "argv": ["cargo", "test"],
                        "cwd": "."
                    }],
                })
                .to_string()
            },
            |socket_path| {
                handle_project_action_list(&ctx_for(socket_path), strings(&["--cwd", "/repo"]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "project.action.list");
        assert_eq!(request["params"]["cwd"], "/repo");
    }

    #[test]
    fn project_action_run_requests_action_run_with_id_and_cwd() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "test",
                        "label": "Run tests",
                        "surface_id": "surface-2",
                        "argv": ["cargo", "test"],
                        "cwd": "/repo"
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_project_action_run(
                    &ctx_for(socket_path),
                    strings(&["test", "--cwd", "/repo"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "project.action.run");
        assert_eq!(request["params"]["id"], "test");
        assert_eq!(request["params"]["cwd"], "/repo");
    }

    #[test]
    fn feed_respond_records_approval_decision() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "id": "notification-1",
                        "type": "approval",
                        "approval_state": "approved"
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_feed(
                    &ctx_for(socket_path),
                    strings(&["respond", "notification-1", "--decision", "approve"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "feed.approval.respond");
        assert_eq!(request["params"]["id"], "notification-1");
        assert_eq!(request["params"]["decision"], "approve");
    }

    #[test]
    fn feed_respond_rejects_invalid_decision_before_socket() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_feed(
                &ctx,
                strings(&["respond", "notification-1", "--decision", "later"]),
            ),
            "approve or deny",
        );
    }

    #[test]
    fn read_screen_requests_surface_read_text() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface_id": "surface-1",
                        "scope": "all",
                        "text": "",
                        "cols": 80,
                        "rows": 24,
                        "total_lines": 0,
                        "lines": 0,
                        "truncated": false,
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_read_screen(
                    &ctx_for(socket_path),
                    strings(&[
                        "--surface-id",
                        "surface-1",
                        "--scope",
                        "all",
                        "--max-bytes",
                        "4096",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "surface.read_text");
        assert_eq!(request["params"]["surface_id"], "surface-1");
        assert_eq!(request["params"]["scope"], "all");
        assert_eq!(request["params"]["max_bytes"], 4096);
    }

    #[test]
    fn capture_tail_requests_surface_capture_tail() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "surface_id": "surface-1",
                        "scope": "tail",
                        "text": "",
                        "cols": 80,
                        "rows": 24,
                        "total_lines": 0,
                        "lines": 0,
                        "truncated": false,
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_capture_tail(
                    &ctx_for(socket_path),
                    strings(&["surface-1", "--lines", "20", "--max-bytes", "2048"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "surface.capture_tail");
        assert_eq!(request["params"]["surface_id"], "surface-1");
        assert_eq!(request["params"]["lines"], 20);
        assert_eq!(request["params"]["max_bytes"], 2048);
    }

    #[test]
    fn tree_requests_topology_tree_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspaces": [{
                            "id": "workspace-1",
                            "name": "main",
                            "active": true,
                            "working_dir": "/tmp",
                            "focused_surface_id": "surface-1",
                            "surface_count": 1,
                            "surfaces": [{
                                "id": "surface-1",
                                "workspace_id": "workspace-1",
                                "title": "shell",
                                "cwd": "/tmp",
                                "unread": false
                            }]
                        }]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_tree(
                    &ctx_for(socket_path),
                    strings(&["--workspace-id", "workspace-1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "topology.tree");
        assert_eq!(request["params"]["workspace_id"], "workspace-1");
    }

    #[test]
    fn top_requests_system_top_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "totals": {
                            "workspaces": 1,
                            "surfaces": 1,
                            "unread_surfaces": 0,
                            "agents": 0
                        },
                        "workspaces": [{
                            "id": "workspace-1",
                            "name": "main",
                            "active": true,
                            "working_dir": "/tmp",
                            "focused_surface_id": "surface-1",
                            "surfaces": [{
                                "id": "surface-1",
                                "kind": "terminal",
                                "focused": true,
                                "unread": false,
                                "cwd": "/tmp"
                            }],
                            "status": [],
                            "progress": []
                        }]
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_top(
                    &ctx_for(socket_path),
                    strings(&["--workspace-id", "workspace-1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "system.top");
        assert_eq!(request["params"]["workspace_id"], "workspace-1");
    }

    #[test]
    fn status_summary_formatter_escapes_control_sequences() {
        let line = format_status_summary_line(&json!({
            "workspace": {
                "id": "workspace\n1",
                "name": "main\u{1b}",
            },
            "agents": [{
                "agent": "codex",
                "session_id": "session\t1",
                "surface_id": "surface\r1",
                "lifecycle": "needs_input",
            }],
            "status": [{
                "label": "Codex",
                "value": "Running\u{1b}",
            }],
            "progress": [{
                "label": "Build",
                "value": 2,
                "total": 4,
            }],
        }));

        assert!(line.contains("main\\x1b"));
        assert!(line.contains("workspace\\n1"));
        assert!(line.contains("session\\t1"));
        assert!(line.contains("surface\\r1"));
        assert!(line.contains("needs_input"));
        assert!(line.contains("Running\\x1b"));
        assert!(line.contains("Build=2/4"));
        assert!(!line.contains('\u{1b}'));
        assert!(!line.contains('\n'));
    }

    #[test]
    fn events_defaults_to_replay_true() {
        let request = with_socket_response(
            |_req| r#"{"event":"subscribed"}"#.to_string(),
            |socket_path| {
                handle_events(&ctx_for(socket_path), vec![]).unwrap();
            },
        );
        assert_eq!(request["method"], "events.subscribe");
        assert_eq!(request["params"]["replay"], json!(true));
    }

    #[test]
    fn events_no_replay_flag_disables_replay() {
        let request = with_socket_response(
            |_req| r#"{"event":"subscribed"}"#.to_string(),
            |socket_path| {
                handle_events(&ctx_for(socket_path), strings(&["--no-replay"])).unwrap();
            },
        );
        assert_eq!(request["params"]["replay"], json!(false));
    }

    #[test]
    fn events_rejects_unknown_arg() {
        assert_err_contains(
            handle_events(&test_context(), strings(&["--bogus"])),
            "unexpected argument",
        );
    }

    #[test]
    fn events_surfaces_jsonrpc_error_handshake() {
        // An over-capacity (or otherwise rejecting) server replies with a
        // JSON-RPC error line then closes; the CLI must report it, not print it
        // as an event.
        with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": false,
                    "error": { "code": "server_busy", "message": "Too many connections" },
                })
                .to_string()
            },
            |socket_path| {
                assert_err_contains(handle_events(&ctx_for(socket_path), vec![]), "server_busy");
            },
        );
    }

    #[test]
    fn events_errors_when_socket_closes_before_handshake() {
        with_socket_response(
            |_req| String::new(),
            |socket_path| {
                assert_err_contains(
                    handle_events(&ctx_for(socket_path), vec![]),
                    "Socket closed without response for events.subscribe",
                );
            },
        );
    }

    #[test]
    fn events_rejects_oversized_handshake_response() {
        with_socket_response(
            |_req| format!("{}\n", "x".repeat(MAX_SOCKET_RESPONSE_BYTES + 1)),
            |socket_path| {
                let err = handle_events(&ctx_for(socket_path), vec![]).unwrap_err();
                assert_eq!(err.code.as_deref(), Some("response_too_large"));
                assert!(err.message.contains("events.subscribe response exceeds"));
            },
        );
    }

    fn hook_test_ok_response(
        request: &Value,
        list_calls: &std::sync::atomic::AtomicUsize,
    ) -> String {
        let result = match request["method"].as_str().unwrap_or("") {
            "system.ping" => json!("pong"),
            "notification.create" => json!({ "id": "n1" }),
            "notification.list" => {
                if list_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    json!([])
                } else {
                    json!([{ "id": "n1" }])
                }
            }
            _ => json!({}),
        };
        format!(
            "{}\n",
            json!({ "id": request["id"], "ok": true, "result": result })
        )
    }

    #[test]
    fn hooks_test_green_path_runs_full_roundtrip() {
        let list_calls = std::sync::atomic::AtomicUsize::new(0);
        let requests = with_socket_server(
            8,
            move |request| hook_test_ok_response(request, &list_calls),
            |socket_path| {
                handle_hooks_test(&ctx_for(socket_path), strings(&["claude"])).unwrap();
            },
        );
        let methods = requests
            .iter()
            .filter_map(|request| request["method"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            methods,
            vec![
                "system.ping",
                "metadata.set_status",
                "metadata.log",
                "notification.list",
                "notification.create",
                "metadata.clear_status",
                "notification.list",
                "notification.clear",
            ]
        );
    }

    #[test]
    fn hooks_test_continues_after_failure_and_exits_nonzero() {
        let list_calls = std::sync::atomic::AtomicUsize::new(0);
        let requests = with_socket_server(
            6,
            move |request| {
                if request["method"] == "notification.create" {
                    format!(
                        "{}\n",
                        json!({
                            "id": request["id"],
                            "ok": false,
                            "error": { "code": "error", "message": "boom" }
                        })
                    )
                } else {
                    hook_test_ok_response(request, &list_calls)
                }
            },
            |socket_path| {
                let error =
                    handle_hooks_test(&ctx_for(socket_path), strings(&["claude"])).unwrap_err();
                assert_eq!(error.exit, 1);
                assert!(error.message.contains("hooks test"));
            },
        );
        // The cleanup call must still run after the failed method: the report
        // is per-method, not abort-on-first-error.
        assert_eq!(requests[5]["method"], "metadata.clear_status");
    }

    #[test]
    fn create_workspace_accepts_cwd_alias() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"w9","name":"scratch"},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_create_workspace(
                    &ctx,
                    strings(&["--name", "scratch", "--cwd", "/tmp/project"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workspace.create");
        assert_eq!(request["params"]["workingDir"], "/tmp/project");
    }

    #[test]
    fn create_workspace_rejects_both_cwd_spellings() {
        assert_err_contains(
            handle_create_workspace(
                &test_context(),
                strings(&["--cwd", "/tmp/a", "--working-dir", "/tmp/b"]),
            ),
            "not both",
        );
    }

    #[test]
    fn subcommand_help_lists_allowed_options_and_exits_zero() {
        let error = handle_create_workspace(&test_context(), strings(&["--help"])).unwrap_err();
        assert_eq!(error.exit, 0);
        assert!(error.message.contains("usage: forktty create-workspace"));
        assert!(error.message.contains("--working-dir"));
        assert!(error.message.contains("--cwd"));
    }

    #[test]
    fn ssh_sends_workspace_create_ssh() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"w2","name":"prod"},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_ssh(
                    &ctx,
                    strings(&[
                        "user@example.com",
                        "--name",
                        "prod",
                        "--cwd",
                        "/tmp/project",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "workspace.create_ssh");
        assert_eq!(request["params"]["host"], "user@example.com");
        assert_eq!(request["params"]["name"], "prod");
        assert_eq!(request["params"]["workingDir"], "/tmp/project");
    }

    #[test]
    fn remotes_requests_remote_list_with_workspace_selector() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [{
                        "workspace_id": "w1",
                        "workspace_name": "prod",
                        "surface_id": "s1",
                        "host": "user@example.com",
                        "connected": true
                    }],
                })
                .to_string()
            },
            |socket_path| {
                handle_remotes(&ctx_for(socket_path), strings(&["--workspace-id", "w1"])).unwrap();
            },
        );
        assert_eq!(request["method"], "remote.list");
        assert_eq!(request["params"]["workspace_id"], "w1");
    }

    #[test]
    fn remote_status_requests_remote_status_with_surface_id() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {
                        "workspace_id": "w1",
                        "workspace_name": "prod",
                        "surface_id": "s1",
                        "host": "user@example.com",
                        "connected": false
                    },
                })
                .to_string()
            },
            |socket_path| {
                handle_remote_status(&ctx_for(socket_path), strings(&["--surface-id", "s1"]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "remote.status");
        assert_eq!(request["params"]["surface_id"], "s1");
    }

    #[test]
    fn ssh_requires_host() {
        assert_err_contains(
            handle_ssh(&test_context(), Vec::new()),
            "ssh: missing required argument <user@host>",
        );
    }

    #[test]
    fn browser_open_sends_browser_open_with_url_and_workspace() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"s9","kind":{"type":"browser","url":"https://example.com"}},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["open", "example.com", "--workspace-id", "w1"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.open");
        assert_eq!(request["params"]["url"], "example.com");
        assert_eq!(request["params"]["workspace_id"], "w1");
    }

    #[test]
    fn browser_navigate_sends_browser_navigate_with_explicit_surface() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"navigated": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["navigate", "s9", "https://rust-lang.org"]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.navigate");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["url"], "https://rust-lang.org");
    }

    #[test]
    fn browser_rejects_unknown_subcommand() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(handle_browser(&ctx, strings(&["frobnicate"])), "browser");
    }

    #[test]
    fn browser_requires_subcommand() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(handle_browser(&ctx, strings(&[])), "subcommand");
    }

    #[test]
    fn socket_cli_compat_aliases_route_to_canonical_methods() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("surface.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string(),
                Some("surface.send_text") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"sent": true},
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": false,
                    "error": {"code": "method_not_found", "message": "unexpected method"},
                })
                .to_string(),
            },
            |socket_path| {
                let mut surface_args =
                    os_strings(&["surface:list", "--workspace-id", "w1", "--socket"]);
                surface_args.push(socket_path.as_os_str().to_os_string());
                run_inner(surface_args).unwrap();

                let mut send_text_args =
                    os_strings(&["send_text", "hello", "--surface-id", "s1", "--socket"]);
                send_text_args.push(socket_path.as_os_str().to_os_string());
                run_inner(send_text_args).unwrap();
            },
        );

        assert_eq!(requests[0]["method"], "surface.list");
        assert_eq!(requests[0]["params"]["workspace_id"], "w1");
        assert_eq!(requests[1]["method"], "surface.send_text");
        assert_eq!(requests[1]["params"]["surface_id"], "s1");
        assert_eq!(requests[1]["params"]["text"], "hello");
    }

    #[test]
    fn browser_rejects_blank_required_args_before_socket_use() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        for (args, expected) in [
            (
                strings(&["open", "   ", "--workspace-id", "w1"]),
                "browser open requires a URL",
            ),
            (
                strings(&["navigate", "   "]),
                "browser navigate requires a URL",
            ),
            (
                strings(&["click", "s9", "   "]),
                "browser click requires <surface-id> <ref>",
            ),
            (
                strings(&["profile", "create", "   "]),
                "browser profile create requires a <name>",
            ),
            (
                strings(&["history", "search", "   "]),
                "browser history search requires a <query>",
            ),
            (
                strings(&["bookmark", "add", "   "]),
                "browser bookmark add requires a <url>",
            ),
        ] {
            assert_err_contains(handle_browser(&ctx, args), expected);
        }
    }

    #[test]
    fn browser_navigate_rejects_surface_id_flag() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(
                &ctx,
                strings(&["navigate", "--surface-id", "s9", "https://x.com"]),
            ),
            "browser navigate",
        );
    }

    #[test]
    fn browser_click_rejects_extra_argument() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["click", "s9", "e3", "extra"])),
            "unexpected argument",
        );
    }

    #[test]
    fn browser_fill_rejects_extra_argument() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello", "extra"])),
            "unexpected argument",
        );
    }

    #[test]
    fn browser_open_resolves_active_workspace_when_id_omitted() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("workspace.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [
                        { "id": "ws-idle", "active": false },
                        { "id": "ws-active", "active": true }
                    ],
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id":"s1","kind":{"type":"browser","url":"https://example.com"}},
                })
                .to_string(),
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["open", "example.com"])).unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
        assert_eq!(requests[1]["method"], "browser.open");
        assert_eq!(requests[1]["params"]["workspace_id"], "ws-active");
        assert_eq!(requests[1]["params"]["url"], "example.com");
    }

    #[test]
    fn browser_navigate_resolves_focused_surface_when_id_omitted() {
        let requests = with_socket_server(
            2,
            |req| match req["method"].as_str() {
                Some("workspace.list") => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [
                        { "id": "a", "active": false, "focused_surface_id": "surface-a" },
                        { "id": "b", "active": true, "focused_surface_id": "surface-b" }
                    ],
                })
                .to_string(),
                _ => json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"navigated": true},
                })
                .to_string(),
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["navigate", "https://rust-lang.org"])).unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
        assert_eq!(requests[1]["method"], "browser.navigate");
        assert_eq!(requests[1]["params"]["surface_id"], "surface-b");
        assert_eq!(requests[1]["params"]["url"], "https://rust-lang.org");
    }

    #[test]
    fn browser_open_errors_when_no_active_workspace() {
        let requests = with_socket_server(
            1,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                assert_err_contains(
                    handle_browser(&ctx, strings(&["open", "example.com"])),
                    "no active workspace",
                );
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
    }

    #[test]
    fn browser_navigate_errors_when_no_focused_surface() {
        let requests = with_socket_server(
            1,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                assert_err_contains(
                    handle_browser(&ctx, strings(&["navigate", "https://x.com"])),
                    "surface id",
                );
            },
        );
        assert_eq!(requests[0]["method"], "workspace.list");
    }

    #[test]
    fn browser_snapshot_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"role": "root", "children": []},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["snapshot", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.snapshot");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_click_sends_ref() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["click", "s9", "e3"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.click");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["ref"], "e3");
    }

    #[test]
    fn browser_fill_sends_ref_and_value() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["fill", "s9", "e3", "hello world"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.fill");
        assert_eq!(request["params"]["surface_id"], "s9");
        assert_eq!(request["params"]["ref"], "e3");
        assert_eq!(request["params"]["value"], "hello world");
    }

    #[test]
    fn browser_fill_reads_value_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let value_path = dir.path().join("value.txt");
        fs::write(&value_path, "secret from file").unwrap();
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            move |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    vec![
                        "fill".to_string(),
                        "s9".to_string(),
                        "e3".to_string(),
                        "--value-file".to_string(),
                        value_path.display().to_string(),
                    ],
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.fill");
        assert_eq!(request["params"]["value"], "secret from file");
    }

    #[test]
    fn browser_back_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["back", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.back");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_forward_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["forward", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.forward");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_reload_sends_request() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"ok": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["reload", "s9"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.reload");
        assert_eq!(request["params"]["surface_id"], "s9");
    }

    #[test]
    fn browser_profile_list_sends_profile_list() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "list"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.list");
        assert_eq!(request["params"], json!({}));
    }

    #[test]
    fn browser_profile_create_sends_display_name() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "p1", "display_name": "Work"},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "create", "Work"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.create");
        assert_eq!(request["params"]["display_name"], "Work");
    }

    #[test]
    fn browser_profile_create_missing_name_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["profile", "create"])),
            "browser profile create requires a <name>",
        );
    }

    #[test]
    fn browser_profile_delete_sends_id() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"deleted": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["profile", "delete", "p-abc123"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.profile.delete");
        assert_eq!(request["params"]["id"], "p-abc123");
    }

    #[test]
    fn browser_open_with_profile_includes_profile_param() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"id": "s1", "kind": {"type": "browser", "url": "https://example.com"}},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "open",
                        "--workspace-id",
                        "w1",
                        "--profile",
                        "Work",
                        "https://example.com",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.open");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["workspace_id"], "w1");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_history_list_sends_correct_method() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "list"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.list");
        assert_eq!(request["params"], json!({}));
    }

    #[test]
    fn browser_history_list_trims_profile_and_numeric_limit() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["history", "list", "--profile", " Work ", "--limit", " 5 "]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.list");
        assert_eq!(request["params"]["profile"], "Work");
        assert_eq!(request["params"]["limit"], 5);
    }

    #[test]
    fn browser_history_limit_requires_non_blank_number() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "list", "--limit="])),
            "--limit requires a value",
        );
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "list", "--limit", "bad"])),
            "--limit must be a number",
        );
    }

    #[test]
    fn browser_import_cli_is_not_exposed() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["import", "discover"])),
            "browser: unknown subcommand import",
        );
    }

    #[test]
    fn browser_eval_cli_is_not_exposed() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["eval", "s9", "document.title"])),
            "browser: unknown subcommand eval",
        );
    }

    #[test]
    fn browser_history_search_sends_query() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "search", "foo"])).unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.search");
        assert_eq!(request["params"]["query"], "foo");
    }

    #[test]
    fn browser_history_clear_sends_trimmed_profile() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"cleared": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["history", "clear", "--profile", " Work "]))
                    .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.clear");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_history_search_missing_query_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(&ctx, strings(&["history", "search"])),
            "browser history search requires a <query>",
        );
    }

    #[test]
    fn browser_history_search_limit_is_numeric() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": [],
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["history", "search", "hello", "--limit", "5"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.history.search");
        assert_eq!(request["params"]["query"], "hello");
        assert_eq!(request["params"]["limit"], json!(5));
    }

    #[test]
    fn browser_history_search_invalid_limit_returns_error() {
        let ctx = ctx_for(Path::new("/tmp/forktty-nonexistent.sock"));
        assert_err_contains(
            handle_browser(
                &ctx,
                strings(&["history", "search", "hello", "--limit", "bad"]),
            ),
            "--limit must be a number",
        );
    }

    #[test]
    fn browser_bookmark_add_sends_url_and_title() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"added": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "add",
                        "https://example.com",
                        "--title",
                        "Example",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.add");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["title"], "Example");
    }

    #[test]
    fn browser_bookmark_add_trims_url_title_and_profile() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"added": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "add",
                        " https://example.com ",
                        "--title",
                        " Example ",
                        "--profile",
                        " Work ",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.add");
        assert_eq!(request["params"]["url"], "https://example.com");
        assert_eq!(request["params"]["title"], "Example");
        assert_eq!(request["params"]["profile"], "Work");
    }

    #[test]
    fn browser_bookmark_remove_sends_url() {
        let request = with_socket_response(
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": {"removed": true},
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(
                    &ctx,
                    strings(&["bookmark", "remove", "https://example.com"]),
                )
                .unwrap();
            },
        );
        assert_eq!(request["method"], "browser.bookmark.remove");
        assert_eq!(request["params"]["url"], "https://example.com");
    }

    #[test]
    fn browser_bookmark_list_and_remove_trim_profile_and_url() {
        let requests = with_socket_server(
            2,
            |req| {
                json!({
                    "id": req["id"],
                    "ok": true,
                    "result": if req["method"] == "browser.bookmark.list" {
                        json!([])
                    } else {
                        json!({"removed": true})
                    },
                })
                .to_string()
            },
            |socket_path| {
                let ctx = ctx_for(socket_path);
                handle_browser(&ctx, strings(&["bookmark", "list", "--profile", " Work "]))
                    .unwrap();
                handle_browser(
                    &ctx,
                    strings(&[
                        "bookmark",
                        "remove",
                        " https://example.com ",
                        "--profile",
                        " Work ",
                    ]),
                )
                .unwrap();
            },
        );
        assert_eq!(requests[0]["method"], "browser.bookmark.list");
        assert_eq!(requests[0]["params"]["profile"], "Work");
        assert_eq!(requests[1]["method"], "browser.bookmark.remove");
        assert_eq!(requests[1]["params"]["url"], "https://example.com");
        assert_eq!(requests[1]["params"]["profile"], "Work");
    }
}
