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
binary moves.

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
