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

## Manual editing

Files in this directory are canonical examples for review or manual repair:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`

Replace `{{FORKTTY_LAUNCHER}}` with the absolute path to the `forktty` launcher
if you install these by hand; keep it shell-quoted. The installer handles this
quoting automatically. Each command is guarded by a per-agent disable variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_GEMINI_HOOKS_DISABLED=1`
