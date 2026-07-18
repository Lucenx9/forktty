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
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` | 26-event lifecycle profile by default; `--full` adds three high-frequency tool hooks for 29 total. |
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

Claude `SessionStart` enrichment requires complete ForkTTY provenance:
workspace ID, surface ID, and an absolute socket path. A partial tuple is
treated atomically as absent, returns the exact continue response, and performs
no socket I/O. The managed event counts are Codex 10, Claude 26 lifecycle / 29
full, Antigravity 3, and OpenCode 11. Claude lifecycle excludes only
`PreToolUse`, `PostToolUse`, and `PostToolUseFailure`; `PostToolBatch` remains.

`Suspended` is a durable tombstone. Late hooks after hibernate cannot revive the
session, publish side effects, or advance its event-order watermark; only an
explicit resume may replace that lifecycle. Prompt request/result correlation is
provider-, session-, kind-, target-, and order-scoped. A result keeps the
matching in-app prompt as read history and closes its desktop notification;
stale results and unrelated prompts remain untouched.

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
5. Keep hook doctor checks local-only and non-mutating. Doctor health requires a
   complete canonical managed plan, including exact regular executable
   Antigravity wrappers; `installationCheck.ok` gates the top-level result.
6. Preserve exact permission/resume metadata only when supplied by the
   provider; do not infer elevated safety from a friendly label.
7. Fail closed when persisted provider resume metadata is invalid: record a
   visible terminal error and never substitute an unrelated plain shell.

Provider hook contracts evolve. After a provider upgrade, run
`forktty hooks doctor <agent>` and `forktty hooks test <agent>` to verify the
installed launcher, supported events, and socket round-trip.
