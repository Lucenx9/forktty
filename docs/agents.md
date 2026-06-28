# Agent Providers in ForkTTY

This document treats Claude Code, Codex, Pi, Antigravity CLI, OpenCode, and
custom CLIs as **agent providers** with explicit capabilities, not generic
terminals.

This is a baseline taxonomy for safe future integration. It does not change how ForkTTY launches agents, writes hook config, gates permissions, or reports UI/socket state by itself.

## Source of truth

This file is the provider taxonomy and review map. Executable behavior lives in
the owning Rust modules and agent-facing setup docs:

- Provider identity, normalized status, and provider command defaults:
  `crates/forktty-core/src/agents.rs`.
- Hook setup, removal, doctor checks, and hook event payload handling:
  `crates/forktty-ui-gtk/src/socket_cli/hooks.rs`,
  `crates/forktty-ui-gtk/src/socket_cli/hooks/install.rs`, and
  `crates/forktty-ui-gtk/src/socket_cli/hooks/event.rs`.
- MCP setup and MCP tool exposure: `crates/forktty-ui-gtk/src/mcp_server.rs`
  plus the socket methods in `crates/forktty-socket/src/`.
- Task strategy routing: `crates/forktty-core/src/task_strategy.rs`,
  `crates/forktty-socket/src/task_strategy_runtime.rs`, and MCP/CLI wrappers.
  `task.strategy.plan` infers dirty git state from the selected surface or
  workspace cwd when `repo_dirty` is omitted, and infers likely user-visible
  edit intent plus clear fast/conservative/parallel/review-heavy router
  profiles from the goal when those hints are omitted. It also returns the
  selected router profile, ranked candidate strategy scores, and role-specific
  harness assignment scores with factor breakdowns, and accepts optional
  last-known-good plus per-harness cooldown/lockout signals so agents can
  inspect the router decision before applying it. When callers omit
  last-known-good, the planner can infer it from completed task-strategy
  workflows in the selected workspace. Last-known-good is a small advisory
  stickiness factor; cooldown is a soft assignment penalty; lockout is
  a hard task/mode exclusion. `task.strategy.apply` stages visible
  workflow/team/task/message state by default; with
  `submit=true`, supported team plans launch visible worker panes and dispatch
  prompts through the team mailbox; missing approvals can be
  published as Feed approvals before any workflow/team mutation; worktree-layer
  plans require `worktree_name` for an already-open ForkTTY worktree workspace.
  Worktree creation, push, merge, and destructive work remain separate explicit
  approvals.
- Managed agent skill content: `.agents/skills/forktty-agent-orchestration/`,
  embedded by `crates/forktty-ui-gtk/src/socket_cli/skills.rs`.
- User-facing hook/MCP/skill setup guidance: `hooks/README.md`, `README.md`,
  `SPEC.md`, and the separate `forktty-site` checkout when public docs change.

When provider behavior changes, update the owning module first, then keep this
taxonomy and the agent-facing docs aligned in the same change.

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
- Pi docs: quickstart, usage, sessions, tool options, and skills
  (`pi.dev/docs/latest/quickstart`, `pi.dev/docs/latest/usage`,
  `pi.dev/docs/latest/skills`).
- MCP specification: security principles, user consent, tool caution, and
  implementation guidance (`modelcontextprotocol.io/specification/2025-06-18`).

## Capability matrix (documented-only)

| Provider | Install command (doc) | Launch command (doc) | Context files / skills | Hooks/events | Permission controls | MCP | Headless/JSON |
|---|---|---|---|---|---|---|---|
| Claude Code | See Claude install docs | `claude` | `CLAUDE.md` / project settings; `forktty skills setup claude` installs `~/.claude/skills/forktty-agent-orchestration` | ForkTTY installs the documented local settings hooks in the lifecycle profile by default; `--full` adds high-frequency per-tool hooks | Documented permission settings; ForkTTY colors risky modes, starts team workers with documented permission-mode defaults, and preserves exact `bypassPermissions` resumes with the documented bypass flag | Registered in `~/.claude.json` as `mcpServers.forktty` | Not fully standardized publicly |
| Codex CLI | See Codex install docs | `codex` | `AGENTS.md`, user/project `config.toml` layers; `forktty skills setup agents` installs `~/.agents/skills/forktty-agent-orchestration` | ForkTTY installs documented `hooks.json` lifecycle hooks; Codex requires per-hook trust approval via `/hooks` before non-managed hooks run | Approval/sandbox modes documented; ForkTTY colors risky modes and preserves exact `bypassPermissions` resumes with the documented yolo/dangerous flag | Registered in `$CODEX_HOME/config.toml` / `~/.codex/config.toml` as `[mcp_servers.forktty]` | JSON/headless flows documented in Codex docs |
| Pi | `npm install -g --ignore-scripts @earendil-works/pi-coding-agent` | `pi` | `AGENTS.md`; `forktty skills setup pi` aliases the interoperable `~/.agents/skills/forktty-agent-orchestration` target that Pi scans | No verified managed hook path yet | Tool allowlists are documented; ForkTTY starts Pi review workers with read-only `--tools read,grep,find,ls` unless explicit tool args are supplied | ForkTTY does not manage a Pi MCP registration path | `--mode json`, `--mode rpc`, and `-p/--print` flows documented |
| Antigravity CLI | See Antigravity CLI docs | `agy` | Antigravity workspace customization and user config | ForkTTY installs the verified `PreInvocation`, `PreToolUse`, and `PostToolUse` hooks via a ForkTTY-owned group plus generated wrapper scripts | Hook responses are conservative: `PreToolUse` explicitly approves; other events return `{}` | Registered in `~/.gemini/config/mcp_config.json` as `mcpServers.forktty` | CLI behavior documented by Antigravity docs |
| OpenCode | See OpenCode install docs | `opencode` | `AGENTS.md`, OpenCode config, plugins | ForkTTY installs a generated local plugin under the OpenCode plugins directory instead of mutating `opencode.json` | OpenCode permission/event payloads are observed and bounded before forwarding | OpenCode supports MCP, but ForkTTY does not yet manage an OpenCode MCP registration path | CLI/server flows documented by provider |
| Custom | User-defined | User-defined | User-defined | Unknown | Unknown | Unknown | Unknown |

## Safe integration points for ForkTTY

1. **Provider identity**: keep `AgentKind` explicit (`claude_code`, `codex`, `pi`, `antigravity`, `opencode`, `custom`); removed provider names such as `gemini` deserialize as `custom`.
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
  Pi and OpenCode MCP registration are not managed yet.
- Default skill setup installs the shared `forktty-agent-orchestration` skill
  to `~/.agents/skills` and `~/.claude/skills`; `codex` and `pi` are aliases
  for the interoperable `agents` target.
- The managed skill directs hook/MCP/skill setup debugging through local
  `forktty doctor` diagnostics and setup dry runs before config writes.
- `system.capabilities` exposes a `provider_capabilities` matrix for supported
  launch/resume providers so socket and MCP clients can read provider support
  directly instead of probing failed operations.
- `task.strategy.plan`, CLI `forktty task-plan`, and MCP
  `task_strategy_plan` provide a read-only routing recommendation before an
  agent chooses solo work, workflow loops, reviewers, teams, worktrees, MCP,
  hooks, or harness roles. The planner uses the selected surface/workspace cwd
  to infer simple git dirty state when no explicit dirty hint is supplied,
  returns or infers a router profile (`balanced`, `fast`, `conservative`,
  `parallel`, or `review_heavy`), and scores harness assignments per role while
  using configured provider order as the tie-break. Callers with concrete
  runtime evidence can pass explicit last-known-good strategy/harness evidence
  plus per-harness cooldown/lockout signals; when they omit LKGP, ForkTTY can
  infer it from completed task-strategy workflow history. LKGP adds a small
  explainable score bias, cooldown lowers assignment score, and lockout excludes
  a harness from assignment. It does not launch workers or mutate orchestration
  state.
- `task.strategy.apply`, CLI `forktty task-apply`, and MCP
  `task_strategy_apply` apply an approved returned plan as visible
  workflow/team/task/message state with deterministic ids. The default path is
  staged and local. If approvals are missing, `request_approval` publishes a
  Feed approval and returns blocked without workflow/team mutation; an approved
  returned `approval_id` can later satisfy that same request-bound start-run
  approval. Apply recomputes dirty-repo edit isolation, worktree approvals, and
  multi-worker submit approvals from the selected target, requested operation,
  and effective plan shape before trusting the plan's approval list. `approved`
  is a caller attestation; use Feed `request_approval` when a separate human
  decision is required. With
  `submit=true`,
  supported team plans launch worker panes and dispatch role prompts;
  worktree-layer plans require `worktree_name` for an already-open ForkTTY
  worktree workspace and are rejected before mutation if that workspace is
  missing.
- `forktty hooks doctor <agent>` reports hook config path state, launcher
  freshness, supported events, Claude profile, and Codex trust-record state.
- Status normalization is centralized in `forktty-core` for reuse by UI/socket/script layers.
