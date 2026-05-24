# ForkTTY Agent Hooks

These templates wire supported coding-agent hook systems into ForkTTY's local
socket API. The preferred path is the installed ForkTTY CLI:

```bash
forktty hooks setup
```

That writes agent-specific hook config into the user config for:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Gemini CLI | `~/.gemini/settings.json` |

The installer writes an absolute path to the `forktty` launcher so hooks can run
from any project. Re-run `forktty hooks setup` if the AppImage or installed
binary moves. `--dry-run` prints the would-be diff without touching disk:

```bash
forktty hooks setup --dry-run
forktty hooks setup codex --dry-run
```

Each setup run writes the agent config atomically (tmp + rename) and, when
content changes, leaves a timestamped `.bak-*` backup next to the original.

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
event names ForkTTY installs hooks for (Codex: 5; Claude Code: 9; Gemini: 8).

## Status entries published by hooks

ForkTTY hooks publish the following status keys via `metadata.set_status`.
They render in the ForkTTY UI status row and can be inspected from the CLI:

| Key | Set on | Cleared on | Color semantics |
|---|---|---|---|
| `agent:<key>` | SessionStart, prompt-submit, pre/post-tool, stop, notification, pre-compact | session-end | `green` ready, `blue` running, `yellow` needs input / compacting, `red` error |
| `agent:<key>:permission` | SessionStart, prompt-submit | session-end | `muted` for documented-safe modes (or unknown / non-Claude providers), `yellow` for Claude `acceptEdits`/`auto`/`dontAsk`, `red` for Claude `bypassPermissions` |
| `agent:claude:tokens` | prompt-submit (Claude only, when a transcript is available) | not cleared automatically | progress against `FORKTTY_HOOK_TOKEN_CEILING` (default 200,000) |

Codex modes stay `muted` because Codex docs describe `permission_mode` only
as "string" without a published enum — ForkTTY does not invent risk levels
the provider hasn't published.

## Manual editing

Files in this directory are canonical examples for review or manual repair:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`

Replace `{{FORKTTY_LAUNCHER}}` with the absolute path to the `forktty` launcher
if you install these by hand; keep it shell-quoted. The installer handles this
quoting automatically.

The `timeout` field is provider-defined. Claude Code and Codex measure it in
**seconds** (Codex default 600 s; Claude default 600 s, 30 s for
`UserPromptSubmit`), and ForkTTY pins those entries at 30 s. Gemini templates
use `5000`, matching Gemini's millisecond-style hook timeout field. The intent
is the same for every provider: a hook should not block the agent loop longer
than a local socket round-trip needs.

Each command is guarded by a per-agent disable variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_GEMINI_HOOKS_DISABLED=1`
