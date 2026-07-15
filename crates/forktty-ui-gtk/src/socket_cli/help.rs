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
  forktty worktree-doctor [--cwd <repo>] [--json]
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
  forktty context-snapshot [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--tail-max-bytes <n>]
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

pub(super) const STATUS_HELP_TEXT: &str = "\
ForkTTY status commands

  forktty status summary [workspace selectors]
      Alias for statusline/status.summary.

  forktty status explain [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--tail-max-bytes <n>]
      Read context.snapshot and explain agent status with session/source/age/readiness/cwd evidence plus risk flags.

  forktty status watch [workspace selectors] [--surface-id <id>] [--count <n>] [--interval-ms <ms>]
      Re-run status explain output. Omit --count to watch until interrupted; interval must be greater than 0.

  forktty context-snapshot [workspace selectors] [--surface-id <id>] [--tail-lines <n>] [--tail-max-bytes <n>]
      Direct CLI alias for the context.snapshot socket method.
";

pub(super) const HOOKS_HELP_TEXT: &str = "\
ForkTTY hook commands

  forktty hooks setup [--full] [codex] [claude] [antigravity] [opencode]
  forktty hooks remove [codex] [claude] [antigravity] [opencode] [gemini]
  forktty hooks doctor <codex|claude|antigravity|opencode>
  forktty hooks test <codex|claude|antigravity|opencode>
  forktty hooks <agent> <event>
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

pub(super) const EXAMPLES_TEXT: &str = "\
ForkTTY examples

  forktty status explain --tail-lines 20
  forktty identify --json
  forktty wait agent-status --status needs_input --timeout-ms 30000
  forktty context-snapshot --workspace-name main --tail-lines 0 --json
";

// Curated ergonomic command set, not every low-level socket alias.
pub(super) const COMPLETION_COMMANDS: &[&str] = &[
    "list",
    "surfaces",
    "agents",
    "agent-health",
    "status",
    "statusline",
    "context-snapshot",
    "identify",
    "wait",
    "tree",
    "top",
    "events",
    "capabilities",
    "examples",
    "completions",
    "help",
];

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
