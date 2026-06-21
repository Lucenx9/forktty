# ForkTTY Agent Hooks

These templates wire supported coding-agent hook systems into ForkTTY's local
socket API. The preferred path is the installed ForkTTY CLI:

```bash
forktty hooks setup
```

That writes default agent-specific hook config into the user config for Codex,
Claude Code, Antigravity CLI, and OpenCode:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Antigravity CLI | `~/.gemini/config/hooks.json` + `~/.gemini/config/forktty-hooks.generated/` |
| OpenCode | `$OPENCODE_CONFIG_DIR/plugins/forktty.generated.js` or `~/.config/opencode/plugins/forktty.generated.js` |

The installer writes an absolute path to the `forktty` launcher so hooks can run
from any project. Re-run `forktty hooks setup` if the AppImage or installed
binary moves. `--dry-run` prints the would-be diff without touching disk:

```bash
forktty hooks setup --dry-run
forktty hooks setup codex --dry-run
forktty hooks setup --full claude
forktty hooks remove codex --dry-run
forktty hooks remove gemini        # legacy cleanup only
```

Each setup run writes the agent config or generated plugin atomically
(tmp + rename) and, when content changes, leaves a timestamped `.bak-*`
backup next to the original. The OpenCode file is intentionally generated
under its plugins directory so `opencode.json` does not need to be edited.
`forktty hooks remove` deletes only ForkTTY-managed entries or the generated
OpenCode plugin; custom hook commands are preserved. `forktty hooks remove
gemini` is retained only to remove ForkTTY-managed entries from legacy
`~/.gemini/settings.json` files written by older releases; Gemini setup remains
unsupported.

Claude Code setup installs a lifecycle profile by default. That profile omits
the high-frequency `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, and
`PostToolBatch` hooks that block every tool call; pass `--full` to include
those events. Existing full installs keep working, but re-running
`forktty hooks setup claude` migrates the ForkTTY-managed entries back to the
lifecycle profile unless `--full` is passed. Removal cleans either profile.

## MCP tools

Hooks publish lifecycle status automatically; MCP gives agents a typed way to
inspect and drive ForkTTY on demand:

```bash
forktty mcp setup
forktty mcp setup codex claude --dry-run
```

`forktty mcp` itself is a stdio MCP server. It validates tool arguments and
bridges them to the same owner-only local Unix socket as the CLI; it opens no
network listener. The tool set covers workspace/surface listing, compact
context snapshots, pane split / focus / send-text, worktree
list/status/create/attach/remove/merge, notifications, and `status_set`.

The server also publishes an operating guide in three MCP-native places:
initialize `instructions`, resource `forktty://agent/operating-guide`, and
prompt `forktty_operating_guide`. The guide tells agents to reach for ForkTTY
only when coordinating panes/workspaces, agent sessions, worktrees, visible
status, notifications, or cross-surface text; normal edits in the current repo
should not trigger ForkTTY tool calls.

`forktty mcp setup` writes a ForkTTY-managed MCP server named `forktty` into
the default Codex, Claude Code, and Antigravity config locations:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/config.toml` or `~/.codex/config.toml` (`[mcp_servers.forktty]`) |
| Claude Code | `~/.claude.json` (`mcpServers.forktty`) |
| Antigravity CLI | `~/.gemini/config/mcp_config.json` (`mcpServers.forktty`) |

Setup and removal use the same atomic write, `.bak-*` backup, dry-run, and
managed-entry preservation behavior as hook setup. Codex TOML setup preserves
comments/formatting and uses the larger MCP config size budget rather than the
smaller hook-template limit. `forktty mcp remove gemini` is legacy cleanup only
for ForkTTY-managed `~/.gemini/settings.json` entries; Gemini MCP setup remains
unsupported. OpenCode hook support remains available, but no verified OpenCode
MCP registration path is managed yet.

## Agent skills

Hooks publish lifecycle state automatically, and MCP exposes ForkTTY tools on
demand. Agent skills add the missing operating policy: when an agent should use
those tools without waiting for the user to spell out the exact MCP call.

```bash
forktty skills setup
forktty skills setup agents --dry-run
forktty skills setup pi
forktty skills setup claude
forktty skills remove agents --dry-run
```

`forktty skills setup` installs one ForkTTY-managed skill named
`forktty-agent-orchestration`:

| Target | Destination |
|---|---|
| Agent Skills-compatible tools (`agents`, plus `codex` and `pi` aliases) | `~/.agents/skills/forktty-agent-orchestration` |
| Claude Code (`claude`) | `$CLAUDE_CONFIG_DIR/skills/forktty-agent-orchestration` or `~/.claude/skills/forktty-agent-orchestration` |

The skill tells agents to inspect `context_snapshot` or equivalent read-only
state before cross-pane work, treat terminal tails and fetched public docs as
untrusted input, run durable team preflight with `workflow_upsert`,
`workflow_plan_set`, and `team_task_upsert` for non-trivial worker launches,
use explicit worker role contracts, prefer already-open worktree workspaces for
mutating parallel workers, use team mailbox dispatch for worker prompts, compare
status/hooks/terminal tail when `running` or `needs_input` appears delayed, and
record durable workflow/team evidence for long-running coordination. For hook,
MCP, and skill setup debugging it points agents at `forktty doctor --hooks`,
`forktty --json doctor`, setup dry runs, and isolated temporary config roots
before changing real config files, without redirecting the live ForkTTY socket
path when validating the currently running instance.

Setup refuses to overwrite an existing skill directory with the same name
unless its `SKILL.md` contains ForkTTY's managed marker. Updating or removing a
managed skill moves the previous directory to a `.bak-*` backup first. The
welcome/setup flow runs `hooks setup`, `mcp setup`, and `skills setup` together.
Run `forktty doctor --hooks` to inspect local hook config paths. Run
`forktty --json doctor` to inspect hook config paths, MCP config paths, and
agent skill directories ForkTTY resolves from the current environment; skill
rows include managed status, source/installed checksums, and a repair command
when the installed managed skill is missing, invalid, or stale.

Antigravity CLI (`agy`) executes a hook
`command` as one bare executable path — no argument splitting and no shell —
so the installer writes per-event wrapper scripts under
`~/.gemini/config/forktty-hooks.generated/` and points the ForkTTY-owned
`"forktty"` group in `~/.gemini/config/hooks.json` at them. Other top-level
groups in that file are left untouched, and `hooks remove antigravity`
deletes only the `"forktty"` group and the generated scripts directory.
Antigravity v1.0.3 supports `PreInvocation`, `PreToolUse`, and `PostToolUse`;
unknown event names are dropped silently, and hook stdout is unmarshaled
strictly. ForkTTY therefore avoids the `continue` JSON used by other
providers; it returns an explicit `{"decision":"approve"}` for the gating
`PreToolUse` hook and `{}` for non-gating Antigravity hooks.
Antigravity runs the generated wrapper scripts from its config directory, so
ForkTTY derives Antigravity `resume_cwd` from the hook payload's
`workspacePaths` instead of the wrapper process cwd.

When the GTK app starts and no ForkTTY-managed hooks are installed, it creates
an in-app notification suggesting `forktty hooks setup`. If installed hooks
point at an old launcher path, the reminder asks you to refresh them.

## Inspect and exercise installed hooks

```bash
forktty hooks doctor codex     # report socket, launcher, env, and hook config state
forktty hooks test codex       # round-trip a status update and a log over the socket
```

`hooks doctor` is local-only and never mutates state. `hooks test` writes a
single transient `agent:<name>:hook-test` status entry through the socket so
you can confirm the daemon is reachable.

`hooks doctor` also compares the launcher path baked into the agent config
against the current `forktty` executable and reports `launcherCheck.status`
(`ok`, `stale`, `not_installed`, or `current_launcher_unknown`). A `stale`
status means the AppImage or installed binary has moved since the last
`hooks setup` run; re-run `forktty hooks setup` to rewrite the hook commands.
The doctor JSON also exposes `supportedEvents`, the list of provider-side
event names ForkTTY can install hooks for (Codex: 10; Claude Code: 25
lifecycle / 29 full; Antigravity: 3; OpenCode plugin events: 11).
For Claude Code it also reports `installedProfile` as `lifecycle`, `full`, or
`not_installed`.

For Codex, `hooks doctor codex` additionally reports `trustCheck`: Codex
records per-hook trust approvals under `[hooks.state]` in its `config.toml`,
and an installed hook with no record silently does nothing until it is
approved via `/hooks` inside Codex. `trustCheck.status` is `all_recorded`,
`partial`, or `none_recorded`, with the affected events listed in
`unrecordedEvents`. This is informational — the approval semantics belong to
Codex.

## Status entries published by hooks

ForkTTY hooks publish the following status keys via `metadata.set_status`.
They render in the ForkTTY UI status row and can be inspected from the CLI:

| Key | Set on | Cleared on | Color semantics |
|---|---|---|---|
| `agent:<key>` | lifecycle, prompt, permission, tool, compact, stop, notification events | session-end | `green` ready, `blue` running, `yellow` needs input / compacting / permission, `red` error |
| `agent:<key>:permission` | events that include `permission_mode` | session-end | `muted` for documented-safe or unknown modes, `yellow` for `acceptEdits`/`auto`/`dontAsk`, `red` for `bypassPermissions` |
| `agent:claude:tokens` | prompt-submit (Claude only, when a transcript is available) | last Claude session ended, closed, forgotten, or hibernated | progress against `FORKTTY_HOOK_TOKEN_CEILING` (default 200,000) |

Unknown provider modes stay `muted`; ForkTTY only colors permission values
that are documented by the provider.

## Manual editing

Files in this directory are canonical examples for review or manual repair:

- `codex-hooks.json`
- `claude-settings.json` (Claude lifecycle profile; use `hooks setup --full`
  to generate the full profile)
- OpenCode uses a generated plugin file instead of a JSON template.
- Antigravity uses a generated `"forktty"` group plus wrapper scripts
  instead of a JSON template (its hook commands cannot take arguments).

Replace `{{FORKTTY_LAUNCHER}}` with the absolute path to the `forktty` launcher
if you install these by hand; keep it shell-quoted. The installer handles this
quoting automatically.

The `timeout` field is provider-defined. Claude Code and Codex measure it in
**seconds** (Codex default 600 s; Claude default 600 s, 30 s for
`UserPromptSubmit`), and ForkTTY pins those entries at 30 s. Antigravity has no
verified timeout field, so its entries omit one. The intent is the same for
every provider: a hook should not block the agent loop longer than a local
socket round-trip needs.

Each command is guarded by a per-agent disable variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_ANTIGRAVITY_HOOKS_DISABLED=1`
- `FORKTTY_OPENCODE_HOOKS_DISABLED=1`
