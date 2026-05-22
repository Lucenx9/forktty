# Agent Providers in ForkTTY

This document treats Claude Code, Codex, OpenCode, Gemini CLI, and custom CLIs as **agent providers** with explicit capabilities, not generic terminals.

## Official sources reviewed (May 22, 2026)

- Claude Code docs: hooks reference and hooks guide (`code.claude.com/docs/en/hooks`, `.../hooks-guide`).
- OpenAI Codex repo/docs: `openai/codex` (`docs/config.md`, repository `AGENTS.md`).
- OpenCode docs: `opencode.ai/docs/agents/`.
- Gemini CLI docs: configuration, GEMINI.md, headless mode, MCP (`google-gemini.github.io/gemini-cli/...`).

## Capability matrix (documented-only)

| Provider | Install command (doc) | Launch command (doc) | Context files | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings | Yes (documented event hooks) | Yes (`/permissions`, settings) | Documented in ecosystem; verify per install | Not fully standardized publicly |
| Codex CLI | `npm install -g @openai/codex` | `codex` | `AGENTS.md` | Configurable hooks (repo docs) | Approval modes documented in Codex docs | Yes (Codex MCP docs/repo) | JSON/headless flows documented in Codex docs |
| OpenCode | See opencode install docs | `opencode` | rules/agent config docs | Partially documented | Documented agent permission boundaries | Documented | Unclear whether stable event stream contract is public |
| Gemini CLI | `npm install -g @google/gemini-cli` | `gemini` | `GEMINI.md` (hierarchical) | No stable generic hook contract found in official docs reviewed | Safety/confirmation behavior in settings/docs | Yes (`mcpServers`) | Yes (`--output-format json`, headless mode docs) |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `opencode`, `gemini`, `custom`).
2. **Normalized status surface**: UI/socket consume normalized states (`idle`, `running`, `needs_input`, `permission_request`, `tool_running`, `tests_running`, `done`, `failed`, `cancelled`, `unknown`).
3. **Config discovery**: only probe documented locations/env overrides; avoid writing undocumented files.
4. **Hook installer**: only mutate providers with documented, local JSON config paths and known hook schemas.
5. **Doctor checks**: local-only, no network, no telemetry, no mutating agent state.

## Unknowns / risks (left unimplemented intentionally)

- OpenCode hook schema and stable event names were not sufficiently documented for safe automatic hook installation.
- Gemini CLI hook APIs were not clearly documented as a stable provider hook/event contract in reviewed official pages.
- Cross-provider “progress stream” formats are not standardized; normalization should remain best-effort and conservative.

## Current ForkTTY integration notes

- Hook setup currently targets Codex, Claude, and Gemini local settings files.
- `forktty doctor` reports hook config path state and misconfiguration risks.
- Status normalization is now centralized in `forktty-core` for reuse by UI/socket/script layers.
