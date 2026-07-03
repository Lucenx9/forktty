//! Socket CLI help text and shell completion command registry.

pub(super) const HELP_TEXT: &str = "\
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
  forktty team-worker-launch <team-id> <worker-id> [--agent <agent>] [--role <role>] [--assigned-task-id <id>] [--worktree-name <name>] [--cwd <repo>] [--args <comma-list>] [--json]
  forktty team-worker-health <team-id> [--stale-after-ms <ms>] [--json]
  forktty team-worker-nudge <team-id> <worker-id> [--text <text>] [--json]
  forktty team-worker-shutdown <team-id> <worker-id> [--text <text>] [--no-submit] [--close] [--json]
  forktty team-task-upsert <team-id> <task-id> [--title <title>] [--status <status>] [--detail <text>] [--depends-on <comma-list>] [--assigned-worker-id <id>] [--json]
  forktty team-message-send <team-id> --from <id> --body <text> [--message-id <id>] [--to-worker-id <id>] [--task-id <id>] [--json]
  forktty team-message-dispatch <team-id> <message-id> [--worker-id <id>] [--submit] [--json]
  forktty team-message-ack <team-id> <message-id> [--worker-id <id>] [--json]
  forktty team-inbox <team-id> [--worker-id <id>] [--include-delivered] [--limit <n>] [--json]
      Read active pending messages; --include-delivered also includes delivered and superseded history.
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
  forktty task-plan <goal> [workspace selectors] [--surface-id <id>] [--cwd <repo>] [--strategy <strategy>] [--profile balanced|fast|conservative|parallel|review_heavy] [--last-known-good-json <json>] [--harness-signals-json <json>] [--repo-dirty] [--parallel] [--review] [--user-visible] [--json]
      Ask ForkTTY to choose a read-only strategy for a task before selecting team, loop, worktree, or harnesses. If --profile is omitted, ForkTTY infers a router profile when the goal clearly asks for speed, caution, parallel work, or review. ForkTTY can infer last-known-good stickiness from completed task-strategy workflows; --last-known-good-json may override or enrich it. --harness-signals-json may pass per-harness cooldown/lockout signals for scripts. If --repo-dirty or --user-visible is omitted, ForkTTY infers dirty state from --cwd inside an open ForkTTY workspace/surface repo or the selected surface/workspace cwd and edit intent from the goal when possible.
  forktty task-apply --run-id <id> --plan-json <json> [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--cwd <repo>] [--leader-surface-id <id>|--surface-id <id>] [--workflow-id <id>] [--team-id <id>] [--approved <ids>|--request-approval|--approval-id <id>] [--submit] <goal> [--json]
      Apply an approved strategy. Defaults to staged workflow/team state; --request-approval creates a Feed approval without starting, --approval-id consumes an approved request, and --submit launches visible workers for supported team plans. Pass --cwd when the repo target differs from the selected ForkTTY pane and is already represented by an open ForkTTY workspace/surface repo; worktree-layer apply requires an already-open worktree workspace.
  forktty cleanup orchestration [--dry-run|--apply] [--workspace-id <id>] [--json]
      Inspect or apply conservative cleanup for stale team/workflow orchestration records; dry-run is the default and live worker surfaces are reported for manual review.
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

pub(super) const TEAM_HELP_TEXT: &str = "\
ForkTTY team commands

High-level wrappers:
  forktty team ask <team-id> <worker-id> [--agent <agent>] --task-id <id> --prompt <text>
      Create/update the team, create the task, launch a fresh worker surface, assign the task, queue the prompt, and dispatch it.
      Omit --agent, or pass --agent auto, to use Settings > Agents team provider selection.
      Submit uses provider-aware terminal input; Codex/Claude/Pi/Grok get text, a short settle, then Enter.
      Freshly launched provider TUI workers wait briefly before the first prompt.
      Re-running ask/review launches another worker; use team-message-send + team-message-dispatch for follow-ups.
      Options: --agent <auto|codex|claude|pi|opencode|antigravity|grok>, --role <role>, --title <title>, --goal <text>, --worktree-name <name>,
               --args <comma-list>, --submit[=true|false] (default: true; pass --submit=false to stage only).

  forktty team review <team-id> <worker-id> [--agent <agent>] --task-id <id> [--commit <rev>]
      Same flow as ask, with a read-only commit review prompt.
      Options: --agent <auto|codex|claude|pi|opencode|antigravity|grok>, --role <role>, --worktree-name <name>, --args <comma-list>,
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
      Rejects superseded messages; with --submit, uses provider-aware terminal input and fresh provider TUI worker settle.
";

pub(super) const STATUS_HELP_TEXT: &str = "\
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

pub(super) const AGENT_HELP_TEXT: &str = "\
ForkTTY agent commands

  forktty agents [workspace selectors]
  forktty agent-health [workspace selectors]
  forktty agent-reclaim-plan [workspace selectors] [--min-idle-ms <ms>]
  forktty hibernate-agent [--surface-id <id>] [--min-idle-ms <ms>]
  forktty reclaim-agents [workspace selectors] [--min-idle-ms <ms>] [--limit <n>]
  forktty resume-agent [--surface-id <id>]
  forktty wait agent-status [workspace selectors] [--surface-id <id>] [--agent <agent>] --status <running|working|idle|done|needs_input|blocked|suspended|ended|closed|unknown>
";

pub(super) const WORKFLOW_HELP_TEXT: &str = "\
ForkTTY workflow commands

  forktty workflows [workspace selectors] [--surface-id <id>] [--session-id <id>] [--query <text>]
  forktty workflow-get <workflow-id>
  forktty workflow-upsert [--workflow-id <id>] [workspace selectors] [--goal <text>] [--memory <text>]
  forktty workflow-plan-set <workflow-id> --steps-json <json-array>
  forktty workflow-loop-set <workflow-id> [--recipe <text>] [--stage <text>] [--iteration <n>] [--max-iterations <n>] [--stop-reason <text>] [--gates-json <json-array>]
  forktty workflow-evidence-add <workflow-id> --kind <kind> --title <title> [--text <text>|--text-file <path>|--text-file -]
  forktty workflow-replay [--workflow-id <id>] [--query <text>] [--since-seq <n>]
";

pub(super) const EXAMPLES_TEXT: &str = "\
ForkTTY examples

  forktty status explain --tail-lines 20
  forktty identify --json
  forktty wait agent-status --status needs_input --timeout-ms 30000
  forktty context-snapshot --workspace-name main --tail-lines 0 --json
  forktty task-plan \"fix the failing test\" --cwd \"$PWD\" --json
  forktty task-apply --run-id task-router-1 --plan-json '<plan-json>' --request-approval \"fix the failing test\"
  forktty feed respond '<approval-id-from-request>' --decision approve
  forktty task-apply --run-id task-router-1 --plan-json '<plan-json>' --approval-id '<approval-id-from-request>' \"fix the failing test\"
  forktty task-apply --run-id task-router-1 --plan-json '<plan-json>' --approved start_run \"fix the failing test\"
  forktty task-apply --run-id task-router-1 --plan-json '<team-plan-json>' --approved start_run,launch_parallel_workers --submit \"review the implementation\"
  forktty task-apply --run-id task-router-2 --plan-json '<worktree-team-plan-json>' --approved start_run,create_worktree,launch_parallel_workers --worktree-name feature-x --submit \"implement in feature-x\"
  forktty team ask review-team review-worker --task-id review-head --prompt \"Review HEAD read-only\" --submit
  forktty team review review-team review-worker --task-id review-head --commit HEAD --submit
  forktty team watch review-team --stale-after-ms 120000 --limit 10
  forktty team finish review-team
  forktty workflows --query release --limit 5
  forktty workflow-loop-set loop-runtime --stage verify --iteration 2 --max-iterations 4
";

// Curated ergonomic command set, not every low-level socket alias.
pub(super) const COMPLETION_COMMANDS: &[&str] = &[
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
    "cleanup",
    "orchestration-cleanup",
    "task-plan",
    "task-apply",
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

pub(super) const TEAM_SUBCOMMANDS: &[&str] =
    &["ask", "review", "watch", "finish", "list", "get", "summary"];
pub(super) const STATUS_SUBCOMMANDS: &[&str] = &["summary", "explain", "watch"];

#[cfg(feature = "browser")]
pub(super) const BROWSER_HELP_TEXT: &str = "\
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

pub(super) fn print_help() {
    print!("{HELP_TEXT}");
    #[cfg(feature = "browser")]
    print!("{BROWSER_HELP_TEXT}");
}
