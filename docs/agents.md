# Agent Providers in ForkTTY

This document treats Claude Code, Codex, OpenCode, Gemini CLI, and custom CLIs as **agent providers** with explicit capabilities, not generic terminals.

This is a baseline taxonomy for safe future integration. It does not change how ForkTTY launches agents, writes hook config, gates permissions, or reports UI/socket state by itself.

## Documentation areas reviewed (May 29, 2026)

- Claude Code docs: hooks reference (`code.claude.com/docs/en/hooks`).
- OpenAI Codex docs: hooks reference (`developers.openai.com/codex/hooks`).
- OpenCode docs: config, plugins, and agents (`opencode.ai/docs/...`).
- Gemini CLI docs: configuration and hooks (`google-gemini/gemini-cli` docs).

## Capability matrix (documented-only)

| Provider | Install command (doc) | Launch command (doc) | Context files | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings | ForkTTY installs documented local settings hooks for the current lifecycle surface | Documented permission settings; ForkTTY colors documented risky modes | Documented by provider; verify per install | Not fully standardized publicly |
| Codex CLI | `npm install -g @openai/codex` | `codex` | `AGENTS.md` | ForkTTY installs documented `hooks.json` lifecycle hooks that are useful for terminal state | Approval/sandbox modes documented; ForkTTY colors documented risky modes | Documented by provider; control API is experimental | JSON/headless flows documented in Codex docs |
| OpenCode | See opencode install docs | `opencode` | rules/agent config docs | ForkTTY installs a generated local plugin under the OpenCode plugins directory | Documented agent permission boundaries; ForkTTY observes permission events | Documented by provider; not modeled here | CLI/server flows documented by provider |
| Gemini CLI | `npm install -g @google/gemini-cli` | `gemini` | `GEMINI.md` (hierarchical) | ForkTTY installs documented settings hooks for lifecycle/tool/model events | Safety/confirmation behavior in settings/docs; no automatic ForkTTY policy mapping | Yes (`mcpServers`) | Yes (`output.format = "json"`) |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `opencode`, `gemini`, `custom`).
2. **Normalized status surface**: future UI/socket code should consume normalized states (`idle`, `running`, `needs_input`, `permission_request`, `tool_running`, `tests_running`, `done`, `failed`, `cancelled`, `unknown`) without treating unknown provider strings as success or progress.
3. **Config discovery**: only probe documented locations/env overrides; avoid writing undocumented files.
4. **Hook installer**: only mutate providers with documented, local JSON config paths and validated hook schemas.
5. **Doctor checks**: local-only, no network, no telemetry, no mutating agent state.

## Unknowns / risks (left unimplemented intentionally)

- Provider hook surfaces are evolving; run `forktty hooks doctor <agent>` after agent upgrades to confirm installed event coverage.
- OpenCode integration is plugin-based, so ForkTTY intentionally writes a generated plugin file instead of mutating `opencode.json`.
- Cross-provider “progress stream” formats are not standardized; normalization should remain best-effort and conservative.

## Current ForkTTY integration notes

- Hook setup currently targets Codex, Claude, and Gemini local settings files, plus a generated OpenCode plugin.
- `forktty doctor` reports hook config path state and misconfiguration risks.
- Status normalization is now centralized in `forktty-core` for reuse by UI/socket/script layers.
