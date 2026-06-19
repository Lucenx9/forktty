# Agent Providers in ForkTTY

This document treats Claude Code, Codex, Antigravity CLI, OpenCode, legacy
Gemini CLI, and custom CLIs as **agent providers** with explicit capabilities,
not generic terminals.

This is a baseline taxonomy for safe future integration. It does not change how ForkTTY launches agents, writes hook config, gates permissions, or reports UI/socket state by itself.

## Documentation areas reviewed (June 19, 2026)

- OpenAI Codex docs: hooks and configuration references
  (`developers.openai.com/codex/hooks`, `developers.openai.com/codex/config-basic`).
- Claude Code docs: hooks, MCP, settings, and security references
  (`code.claude.com/docs/en/hooks`, `code.claude.com/docs/en/mcp`,
  `code.claude.com/docs/en/settings`, `code.claude.com/docs/en/security`).
- OpenCode docs: config, MCP servers, plugins, and agents
  (`opencode.ai/docs/config/`, `opencode.ai/docs/mcp-servers/`,
  `opencode.ai/docs/plugins/`, `opencode.ai/docs/agents/`).
- Google Antigravity docs: CLI overview, hooks, MCP, plugins/skills
  (`antigravity.google/docs/cli-overview`, `antigravity.google/docs/hooks`,
  `antigravity.google/docs/mcp`, `antigravity.google/docs/cli-plugins`).
- MCP specification: security principles, user consent, tool caution, and
  implementation guidance (`modelcontextprotocol.io/specification/2025-06-18`).

## Capability matrix (documented-only)

| Provider | Install command (doc) | Launch command (doc) | Context files | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings | ForkTTY installs the documented local settings hooks in the lifecycle profile by default; `--full` adds high-frequency per-tool hooks | Documented permission settings; ForkTTY colors documented risky modes as display-only metadata | Registered in `~/.claude.json` as `mcpServers.forktty` | Not fully standardized publicly |
| Codex CLI | See Codex install docs | `codex` | `AGENTS.md`, user/project `config.toml` layers | ForkTTY installs documented `hooks.json` lifecycle hooks; Codex requires per-hook trust approval via `/hooks` before non-managed hooks run | Approval/sandbox modes documented; ForkTTY colors documented risky modes as display-only metadata | Registered in `$CODEX_HOME/config.toml` / `~/.codex/config.toml` as `[mcp_servers.forktty]` | JSON/headless flows documented in Codex docs |
| Antigravity CLI | See Antigravity CLI docs | `agy` | Antigravity workspace customization and user config | ForkTTY installs the verified `PreInvocation`, `PreToolUse`, and `PostToolUse` hooks via a ForkTTY-owned group plus generated wrapper scripts | Hook responses are conservative: `PreToolUse` explicitly approves; other events return `{}` | Registered in `~/.gemini/config/mcp_config.json` as `mcpServers.forktty` | CLI behavior documented by Antigravity docs |
| OpenCode | See OpenCode install docs | `opencode` | `AGENTS.md`, OpenCode config, plugins | ForkTTY installs a generated local plugin under the OpenCode plugins directory instead of mutating `opencode.json` | OpenCode permission/event payloads are observed and bounded before forwarding | OpenCode supports MCP, but ForkTTY does not yet manage an OpenCode MCP registration path | CLI/server flows documented by provider |
| Gemini CLI | Legacy explicit target only | `gemini` | `GEMINI.md` (hierarchical) | ForkTTY can still install documented settings hooks when explicitly requested (`forktty hooks setup gemini`) | Safety/confirmation behavior in settings/docs; no automatic ForkTTY policy mapping | Explicit legacy registration only (`forktty mcp setup gemini`) | Yes (`output.format = "json"`) |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `antigravity`, `opencode`, `gemini`, `custom`).
2. **Normalized status surface**: future UI/socket code should consume normalized states (`idle`, `running`, `needs_input`, `permission_request`, `tool_running`, `tests_running`, `done`, `failed`, `cancelled`, `unknown`) without treating unknown provider strings as success or progress.
3. **Config discovery**: only probe documented locations/env overrides; avoid writing undocumented files.
4. **Hook installer**: only mutate providers with documented, local JSON config paths and validated hook schemas.
5. **Doctor checks**: local-only, no network, no telemetry, no mutating agent state.

## Unknowns / risks (left unimplemented intentionally)

- Provider hook surfaces are evolving; run `forktty hooks doctor <agent>` after agent upgrades to confirm installed event coverage.
- OpenCode integration is plugin-based, so ForkTTY intentionally writes a generated plugin file instead of mutating `opencode.json`.
- OpenCode supports MCP servers, but ForkTTY has no verified OpenCode MCP registration path yet; keep this explicit instead of guessing a config shape.
- Antigravity docs are newer than the legacy Gemini CLI docs. Treat Antigravity as the default Google provider and Gemini CLI as legacy opt-in.
- Cross-provider “progress stream” formats are not standardized; normalization should remain best-effort and conservative.

## Current ForkTTY integration notes

- Default hook setup currently targets Codex, Claude Code, Antigravity CLI, and
  OpenCode. Gemini CLI remains available only as an explicit legacy target.
- Default MCP setup currently targets Codex, Claude Code, and Antigravity CLI.
  Gemini CLI remains available only as an explicit legacy target; OpenCode MCP
  registration is not managed yet.
- `forktty hooks doctor <agent>` reports hook config path state, launcher
  freshness, supported events, Claude profile, and Codex trust-record state.
- Status normalization is now centralized in `forktty-core` for reuse by UI/socket/script layers.
