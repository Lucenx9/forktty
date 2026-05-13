# ForkTTY Agent Hooks

These templates wire supported coding-agent hook systems into ForkTTY's local
socket API. The preferred path is the repo-local installer:

```bash
./scripts/forktty.mjs hooks setup
```

That writes agent-specific hook config into the user config for:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Gemini CLI | `~/.gemini/settings.json` |

The installer writes an absolute path to `scripts/forktty.mjs` so hooks can run
from any project. Re-run `hooks setup` if the repo moves.

Files in this directory are canonical examples for review or manual repair:

- `codex-hooks.json`
- `claude-settings.json`
- `gemini-settings.json`

Replace `{{FORKTTY_SCRIPT}}` with the absolute path to `scripts/forktty.mjs` if
you install these by hand. Each command is guarded by a per-agent disable
variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_GEMINI_HOOKS_DISABLED=1`
