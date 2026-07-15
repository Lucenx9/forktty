# Agent Lifecycle in ForkTTY

ForkTTY treats coding agents primarily as terminal processes. Optional hooks
add a thin, provider-aware lifecycle layer so the workspace UI can show
attention, last activity, and native resume state without owning the agent's
task planning or coordination policy.

## Source of truth

- Provider identity and normalized lifecycle: `crates/forktty-core/src/agents.rs`.
- Hook setup, removal, diagnostics, and payload handling:
  `crates/forktty-ui-gtk/src/socket_cli/hooks/`.
- Socket lifecycle reads and explicit resume/hibernate/reclaim operations:
  `crates/forktty-socket/src/agent_runtime.rs`.
- User-facing setup behavior: `hooks/README.md`, `README.md`, and `SPEC.md`.

ForkTTY does not expose a built-in MCP server, managed agent skills, task
router, provider-neutral team/workflow store, or approval feed. External tools
may call the generic socket CLI when they need workspace, pane, notification,
metadata, worktree, or terminal-text primitives.

## Supported hook integrations

| Provider | Hook installation | Notes |
| --- | --- | --- |
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` | Changed hook definitions require review through `/hooks`; ForkTTY can detect trust records but cannot verify their hashes. |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` | Lifecycle profile by default; `--full` adds high-frequency tool hooks. |
| Antigravity CLI | `~/.gemini/config/hooks.json` plus generated wrappers | Uses a ForkTTY-owned hook group and direct wrapper executables. |
| OpenCode | Generated plugin under the OpenCode plugin directory | Avoids mutating `opencode.json`. |

Pi, Grok, and custom agents can run normally in panes but do not have a
ForkTTY-managed hook installer. Legacy Gemini entries can be removed but are no
longer installed.

## Lifecycle contract

Hooks can associate a provider session id, cwd, PID, permission mode, and
normalized lifecycle with a ForkTTY surface. Unknown provider strings remain
custom and unknown states remain conservative. Executable discovery or a hook
record proves neither authentication nor provider-side session validity.

The public agent socket family is limited to:

- `agent.list` and `agent.health` for observation;
- `agent.resume` for an explicit provider-native resume request;
- `agent.reclaim.plan`, `agent.hibernate`, and `agent.reclaim` for explicit
  maintenance of idle, locally restorable sessions.

These operations do not select a model, launch a team, assign work, or run an
autonomous loop. ForkTTY never installs or refreshes hooks automatically at GTK
startup. All writes require an explicit user action in the welcome flow,
settings, or the `forktty hooks setup` CLI.

## Safety rules

1. Probe only documented local config locations and environment overrides.
2. Write hook configuration atomically and preserve unrelated entries.
3. Treat terminal text and hook payloads as untrusted data.
4. Reject ambiguous cwd-to-surface matches instead of guessing active focus.
5. Keep hook doctor checks local-only and non-mutating.
6. Preserve exact permission/resume metadata only when supplied by the
   provider; do not infer elevated safety from a friendly label.

Provider hook contracts evolve. After a provider upgrade, run
`forktty hooks doctor <agent>` and `forktty hooks test <agent>` to verify the
installed launcher, supported events, and socket round-trip.
