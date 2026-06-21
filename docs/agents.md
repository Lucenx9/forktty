# Agent Providers in ForkTTY

This document treats Claude Code, Codex, Antigravity CLI, OpenCode, and custom
CLIs as **agent providers** with explicit capabilities, not generic terminals.

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

| Provider | Install command (doc) | Launch command (doc) | Context files / skills | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings; `forktty skills setup claude` installs `~/.claude/skills/forktty-agent-orchestration` | ForkTTY installs the documented local settings hooks in the lifecycle profile by default; `--full` adds high-frequency per-tool hooks | Documented permission settings; ForkTTY colors risky modes and preserves exact `bypassPermissions` resumes with the documented bypass flag | Registered in `~/.claude.json` as `mcpServers.forktty` | Not fully standardized publicly |
| Codex CLI | See Codex install docs | `codex` | `AGENTS.md`, user/project `config.toml` layers; `forktty skills setup agents` installs `~/.agents/skills/forktty-agent-orchestration` | ForkTTY installs documented `hooks.json` lifecycle hooks; Codex requires per-hook trust approval via `/hooks` before non-managed hooks run | Approval/sandbox modes documented; ForkTTY colors risky modes and preserves exact `bypassPermissions` resumes with the documented yolo/dangerous flag | Registered in `$CODEX_HOME/config.toml` / `~/.codex/config.toml` as `[mcp_servers.forktty]` | JSON/headless flows documented in Codex docs |
| Antigravity CLI | See Antigravity CLI docs | `agy` | Antigravity workspace customization and user config | ForkTTY installs the verified `PreInvocation`, `PreToolUse`, and `PostToolUse` hooks via a ForkTTY-owned group plus generated wrapper scripts | Hook responses are conservative: `PreToolUse` explicitly approves; other events return `{}` | Registered in `~/.gemini/config/mcp_config.json` as `mcpServers.forktty` | CLI behavior documented by Antigravity docs |
| OpenCode | See OpenCode install docs | `opencode` | `AGENTS.md`, OpenCode config, plugins | ForkTTY installs a generated local plugin under the OpenCode plugins directory instead of mutating `opencode.json` | OpenCode permission/event payloads are observed and bounded before forwarding | OpenCode supports MCP, but ForkTTY does not yet manage an OpenCode MCP registration path | CLI/server flows documented by provider |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `antigravity`, `opencode`, `custom`); removed provider names such as `gemini` deserialize as `custom`.
2. **Normalized status surface**: future UI/socket code should consume normalized states (`idle`, `running`, `needs_input`, `permission_request`, `tool_running`, `tests_running`, `done`, `failed`, `cancelled`, `unknown`) without treating unknown provider strings as success or progress.
3. **Config discovery**: only probe documented locations/env overrides; avoid writing undocumented files.
4. **Hook installer**: only mutate providers with documented, local JSON config paths and validated hook schemas.
5. **Doctor checks**: local-only, no network, no telemetry, no mutating agent state.

## Unknowns / risks (left unimplemented intentionally)

- Provider hook surfaces are evolving; run `forktty hooks doctor <agent>` after agent upgrades to confirm installed event coverage.
- OpenCode integration is plugin-based, so ForkTTY intentionally writes a generated plugin file instead of mutating `opencode.json`.
- OpenCode supports MCP servers, but ForkTTY has no verified OpenCode MCP registration path yet; keep this explicit instead of guessing a config shape.
- Cross-provider “progress stream” formats are not standardized; normalization should remain best-effort and conservative.

## Current ForkTTY integration notes

- Default hook setup currently targets Codex, Claude Code, Antigravity CLI, and
  OpenCode.
- Default MCP setup currently targets Codex, Claude Code, and Antigravity CLI.
  OpenCode MCP registration is not managed yet.
- Default skill setup installs the shared `forktty-agent-orchestration` skill
  to `~/.agents/skills` and `~/.claude/skills`; `codex` is an alias for the
  interoperable `agents` target.
- The managed skill directs hook/MCP/skill setup debugging through local
  `forktty doctor` diagnostics and setup dry runs before config writes.
- `system.capabilities` exposes a `provider_capabilities` matrix for supported
  launch/resume providers so socket and MCP clients can read provider support
  directly instead of probing failed operations.
- `forktty hooks doctor <agent>` reports hook config path state, launcher
  freshness, supported events, Claude profile, and Codex trust-record state.
- Status normalization is centralized in `forktty-core` for reuse by UI/socket/script layers.
