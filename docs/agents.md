# Agent Providers in ForkTTY

This document treats Claude Code, Codex, OpenCode, Gemini CLI, and custom CLIs as **agent providers** with explicit capabilities, not generic terminals.

This is a baseline taxonomy for safe future integration. It does not change how ForkTTY launches agents, writes hook config, gates permissions, or reports UI/socket state by itself.

## Documentation areas reviewed (May 22, 2026)

- Claude Code docs: hooks reference and hooks guide (`code.claude.com/docs/en/hooks`, `.../hooks-guide`).
- OpenAI Codex repo/docs: `openai/codex` (`docs/config.md`, repository `AGENTS.md`).
- OpenCode docs: `opencode.ai/docs/agents/`.
- Gemini CLI docs: configuration, GEMINI.md, headless mode, MCP (`google-gemini.github.io/gemini-cli/...`).

## Capability matrix (documented-only)

| Provider | Install command (doc) | Launch command (doc) | Context files | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings | Documented event hooks; ForkTTY must still write only known local settings shapes | Documented permission settings; no automatic ForkTTY policy mapping | Documented by provider; verify per install | Not fully standardized publicly |
| Codex CLI | `npm install -g @openai/codex` | `codex` | `AGENTS.md` | Configurable hooks exist, but event coverage/stability should be validated per release | Approval/sandbox modes documented; no automatic ForkTTY policy mapping | Documented by provider; control API is experimental | JSON/headless flows documented in Codex docs |
| OpenCode | See opencode install docs | `opencode` | rules/agent config docs | No ForkTTY hook installer yet; verify schema before enabling | Documented agent permission boundaries; no automatic ForkTTY policy mapping | Documented by provider; not modeled here | Unclear whether stable event stream contract is public |
| Gemini CLI | `npm install -g @google/gemini-cli` | `gemini` | `GEMINI.md` (hierarchical) | Documented hooks exist; ForkTTY should validate exact schema before deeper automation | Safety/confirmation behavior in settings/docs; no automatic ForkTTY policy mapping | Yes (`mcpServers`) | Yes (`--output-format json`, headless mode docs) |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `opencode`, `gemini`, `custom`).
2. **Normalized status surface**: future UI/socket code should consume normalized states (`idle`, `running`, `needs_input`, `permission_request`, `tool_running`, `tests_running`, `done`, `failed`, `cancelled`, `unknown`) without treating unknown provider strings as success or progress.
3. **Config discovery**: only probe documented locations/env overrides; avoid writing undocumented files.
4. **Hook installer**: only mutate providers with documented, local JSON config paths and validated hook schemas.
5. **Doctor checks**: local-only, no network, no telemetry, no mutating agent state.

## Unknowns / risks (left unimplemented intentionally)

- OpenCode hook schema and stable event names need implementation-specific review before safe automatic hook installation.
- Gemini CLI hook APIs are documented, but ForkTTY should still validate schema compatibility before adding deeper automation.
- Cross-provider “progress stream” formats are not standardized; normalization should remain best-effort and conservative.

## Current ForkTTY integration notes

- Hook setup currently targets Codex, Claude, and Gemini local settings files.
- `forktty doctor` reports hook config path state and misconfiguration risks.
- Status normalization is now centralized in `forktty-core` for reuse by UI/socket/script layers.
