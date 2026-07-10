# Harness integration evidence from primary sources

Accessed: **2026-07-10** (Europe/Rome).

This note compares ForkTTY's current provider registry and integration installers
with first-party product documentation and first-party source repositories. It is
research evidence, not a compatibility promise: vendor CLIs change quickly, and
PATH discovery does not prove authentication, entitlement, quota, or runtime
health.

## Local scope reviewed

ForkTTY currently launches six canonical team-worker providers from
`crates/forktty-socket/src/provider_runtime.rs:19-86`: `codex`, `claude`,
`opencode`, `pi`, `antigravity` (`agy`), and `grok`. Resume argv is built in
`crates/forktty-core/src/agents.rs:98-141`; role-specific launch arguments in
`crates/forktty-socket/src/team_provider.rs:189-285`; provider-aware terminal
submission in `crates/forktty-socket/src/team_dispatch.rs:15-102`.

Managed integration surfaces are narrower:

- Hooks: Codex, Claude Code, Antigravity, and OpenCode
  (`crates/forktty-ui-gtk/src/socket_cli/hooks/specs.rs:434-475`).
- MCP registration: Codex, Claude Code, and Antigravity
  (`crates/forktty-ui-gtk/src/socket_cli/mcp.rs:39-60`).
- Skills: the interoperable `~/.agents/skills` target and Claude's
  `~/.claude/skills` target; `codex` and `pi` are aliases only
  (`crates/forktty-ui-gtk/src/socket_cli/skills.rs:33-71`).
- Gemini is not launchable and has no setup target. It is retained only for
  removing legacy ForkTTY-managed entries from `~/.gemini/settings.json`.

### Installed CLI snapshot

The following read-only checks were run locally on 2026-07-10 from this
checkout. They establish the installed parser/version surface only; no model
request, authentication check, provider configuration inspection, or
interactive-startup smoke test was performed.

| Executable | Installed version | Additional local evidence |
|---|---:|---|
| `codex` | `codex-cli 0.144.1` | `codex --version` |
| `claude` | `2.1.204 (Claude Code)` | The installed ELF contains `executeMessageDisplayHooks` and `MessageDisplay` schema/runtime strings |
| `pi` | `0.80.5` | `pi --version` |
| `opencode` | `1.14.50` | `opencode --help` exposes the top-level `--agent` option |
| `agy` | `1.0.16` | Static strings in the installed binary include `GetPostInvocationHookArgs`, corroborating that the current binary knows the documented `PostInvocation` event |
| `grok` | `0.2.93 (f00f96316d) [stable]` | `grok --help` exposes the top-level `--permission-mode` option and lists `plan` among its values |

## Compact matrix

Legend: **yes** means ForkTTY has a direct managed path; **indirect** means the
provider currently discovers a shared/compatibility path ForkTTY already writes;
**provider only** means the vendor supports the feature but ForkTTY does not
manage it; **no built-in** means the provider explicitly delegates the feature to
extensions.

| Harness | Launch / cwd / resume | Permission or plan surface | ForkTTY hooks | ForkTTY MCP | ForkTTY skill reach | Submit evidence | Current status |
|---|---|---|---|---|---|---|---|
| Codex CLI | `codex`; process cwd or global `-C`; `codex resume [-C PATH] SESSION_ID` is consistent with the [CLI reference](https://developers.openai.com/codex/cli/reference) | Approval and sandbox flags are documented; Codex has a TUI Plan mode, but the public CLI reference exposes no stable startup flag, so ForkTTY's launch-capability boolean is conservatively false | **yes**: the ten configured events match the current [Codex hooks reference](https://developers.openai.com/codex/hooks) | **yes**: `[mcp_servers.forktty]` in `~/.codex/config.toml` matches the [MCP reference](https://developers.openai.com/codex/mcp) | **yes**: `~/.agents/skills` is a documented [user skill location](https://developers.openai.com/codex/skills) | Public docs establish an interactive TUI, but do not specify whether injected body bytes and Enter must be separate writes | Actively maintained in the official [openai/codex releases](https://github.com/openai/codex/releases) |
| Claude Code | `claude`; process cwd; `claude --resume SESSION` is documented in the [CLI reference](https://code.claude.com/docs/en/cli-reference) | `--permission-mode` accepts `plan`, `dontAsk`, `auto`, and bypass modes; `auto` has account/model/provider/admin prerequisites in [permission modes](https://code.claude.com/docs/en/permission-modes) | **yes**: settings hooks and event schemas are documented in the [hooks reference](https://code.claude.com/docs/en/hooks) | **yes**: user/local servers live in `~/.claude.json`, per [MCP scopes](https://code.claude.com/docs/en/mcp) | **yes**: personal `~/.claude/skills` and project `.claude/skills` are documented in [skills](https://code.claude.com/docs/en/skills) | Enter submits in the [interactive-mode reference](https://code.claude.com/docs/en/interactive-mode); write separation is not a documented contract | Current docs describe Claude Code 2.1.x behavior and active feature evolution |
| OpenCode | `opencode`; process cwd or positional project; `opencode --session ID`; all are in the current [CLI reference](https://opencode.ai/docs/cli) | Built-in Build and Plan primary agents are documented, and `--agent` selects one ([agents](https://opencode.ai/docs/agents), [CLI](https://opencode.ai/docs/cli)) | **yes** via generated plugin; the event names used by ForkTTY appear in the current [plugin event list](https://opencode.ai/docs/plugins) | **provider only**: local/remote MCP is supported in `opencode.json`, per [MCP servers](https://opencode.ai/docs/mcp-servers) | **indirect**: OpenCode now explicitly scans `~/.agents/skills` and `.agents/skills`, per [Agent Skills](https://opencode.ai/docs/skills) | Default `input_submit` is `return`, per [keybinds](https://opencode.ai/docs/keybinds); combined text+CR remains a PTY implementation choice | Actively maintained; current releases are published at [anomalyco/opencode](https://github.com/anomalyco/opencode/releases) |
| Pi | `pi`; process cwd; `pi --session PATH_OR_ID`; documented in the official [coding-agent README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md) | A read-only review can be constrained with `--tools read,grep,find,ls`; Pi explicitly has no built-in permission popups or Plan mode in the same README | **no built-in managed path**; extensions can subscribe to events | **no built-in**: Pi says MCP can be supplied by an extension, not core | **yes**: Pi scans `~/.agents/skills` and `.agents/skills`, per its [README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md) | Enter submits or queues a steering message while busy, explicitly documented in the [message queue](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#message-queue) | Actively maintained after the repository/package move to Earendil Works; see [releases](https://github.com/earendil-works/pi/releases) |
| Antigravity CLI | `agy`; cwd scopes histories; `agy --conversation ID` is documented in [Managing conversations](https://antigravity.google/docs/cli-conversations) | Fine-grained allow/ask/deny policy and `--dangerously-skip-permissions` are documented in [CLI permissions](https://antigravity.google/docs/cli-permissions) and the official [CLI codelab](https://codelabs.developers.google.com/antigravity-cli-hands-on) | **yes, partial**: ForkTTY installs 3 of the 5 currently documented events in [hooks](https://antigravity.google/docs/hooks) | **yes**: `~/.gemini/config/mcp_config.json` with `mcpServers` is the documented [global CLI MCP path](https://antigravity.google/docs/mcp) | **workspace only through the repo**: `.agents/skills` is documented, but the documented global path is Antigravity-specific in [skills](https://antigravity.google/docs/skills) | Enter is the protected submit binding in [Using AGY CLI](https://antigravity.google/docs/cli-using); write separation is not specified | Current Google product; official CLI docs and codelabs were updated for the May 2026 Antigravity suite |
| Grok Build | `grok`; `--cwd PATH`; `--resume ID`; all documented in the [CLI reference](https://docs.x.ai/build/cli/reference) | Plan exists; docs describe `/plan`, Shift+Tab, and `--permission-mode plan`; installed `grok 0.2.93` exposes that option at top level, although interactive startup was not smoke-tested | **indirect/provider only**: native hooks exist and Grok also reads Claude Code hooks, per [skills/plugins/hooks](https://docs.x.ai/build/features/skills-plugins-marketplaces) and [hooks](https://docs.x.ai/build/features/hooks) | **indirect/provider only**: native MCP exists and Grok reads `~/.claude.json`, per [MCP servers](https://docs.x.ai/build/features/mcp-servers) | **indirect**: Grok explicitly scans `~/.agents/skills`, per [skills/plugins](https://docs.x.ai/build/features/skills-plugins-marketplaces) | Enter sends a prompt; Ctrl+Enter interjects while a turn is running, per [keyboard shortcuts](https://docs.x.ai/build/keyboard-shortcuts) | Active beta with first-party docs updated in July 2026; see [overview](https://docs.x.ai/build/overview) and [release notes](https://docs.x.ai/developers/release-notes) |
| Gemini CLI (legacy cleanup only) | Not a ForkTTY launch provider | Gemini has a documented plan approval mode, but ForkTTY intentionally has no active target | remove-only; `~/.gemini/settings.json` remains a real current config path ([settings](https://geminicli.com/docs/cli/settings/), [hooks](https://geminicli.com/docs/hooks/reference/)) | remove-only; Gemini still supports `mcpServers` in that same file ([MCP](https://geminicli.com/docs/tools/mcp-server/)) | ForkTTY's `agents` target would be discoverable because Gemini documents `~/.agents/skills` as an alias ([skills](https://geminicli.com/docs/cli/creating-skills/)) | Not audited because ForkTTY does not launch Gemini | Actively maintained upstream, but intentionally out of ForkTTY's active provider registry |

## Per-harness evidence and integration assessment

### Codex CLI

Confirmed alignment:

- The executable is `codex`; running it without a subcommand opens the TUI.
  `codex resume` is stable, accepts a session ID/name, and accepts the same
  global flags as `codex`, including `-C/--cd`. The dangerous bypass flag used
  by ForkTTY is also documented in the [CLI reference](https://developers.openai.com/codex/cli/reference).
- ForkTTY's ten hook names (`SessionStart`, `UserPromptSubmit`, `PreToolUse`,
  `PostToolUse`, `PermissionRequest`, `PreCompact`, `PostCompact`,
  `SubagentStart`, `SubagentStop`, `Stop`) are all present in the current
  [hooks reference](https://developers.openai.com/codex/hooks). Hook `timeout`
  is seconds, default 600 seconds, so ForkTTY's 30-second bound is dimensionally
  correct.
- `~/.codex/config.toml` and `[mcp_servers.<name>]` with `command`, `args`, and
  environment forwarding are the documented [MCP configuration](https://developers.openai.com/codex/mcp).
- Codex scans `.agents/skills` from cwd to repo root and `$HOME/.agents/skills`,
  exactly matching ForkTTY's interoperable skill target
  ([skills discovery](https://developers.openai.com/codex/skills)).

Alignment/unknowns:

- `provider_runtime.rs` reports `plan_mode: false`, but current first-party
  Codex material includes a Plan collaboration mode and the
  [configuration reference](https://developers.openai.com/codex/config-reference)
  documents a Plan-mode-specific reasoning override. The public CLI reference
  did not expose a stable `--plan` startup flag on the access date. Because the
  ForkTTY field controls deterministic launch behavior, `false` is conservative
  and is not a finding against the current integration.
- Public docs establish Enter-driven interactive use but do not specify the PTY
  byte-level rule behind ForkTTY's “body, settle, separate Enter” behavior.
  Keep this as runtime-tested compatibility, not a vendor-guaranteed protocol.

### Claude Code

Confirmed alignment:

- `claude --resume <session>` and the lack of a required cwd resume flag match
  ForkTTY's argv/process-cwd approach ([CLI reference](https://code.claude.com/docs/en/cli-reference)).
- `--permission-mode dontAsk`, `--allowedTools`, and
  `--dangerously-skip-permissions` are documented. `dontAsk` executes only
  pre-approved tools, so ForkTTY's `Read,Grep,Glob` review launch is a coherent
  conservative profile ([permission modes](https://code.claude.com/docs/en/permission-modes),
  [CLI flags](https://code.claude.com/docs/en/cli-reference)).
- The managed JSON hook shape, seconds-based timeout, current event names, and
  `~/.claude/settings.json` location agree with the
  [hooks reference](https://code.claude.com/docs/en/hooks).
- User/local MCP entries in `~/.claude.json` and stdio `command`/`args`/`env`
  agree with [Claude MCP scopes](https://code.claude.com/docs/en/mcp).
- Personal skills live under `~/.claude/skills`; project skills under
  `.claude/skills` ([skills reference](https://code.claude.com/docs/en/skills)).

Risk:

- ForkTTY unconditionally adds `--permission-mode auto` for every non-review
  Claude worker. Claude documents `auto` as conditional on account plan,
  owner/admin enablement, model, and provider; it may be unavailable or
  explicitly disabled ([Auto mode requirements](https://code.claude.com/docs/en/permission-modes#eliminate-permission-prompts-with-auto-mode)).
  PATH launchability cannot establish these prerequisites. A launch should not
  require `auto` unless readiness/capability is known, or it needs a fallback
  that does not weaken policy.
- **Low coverage drift, non-lifecycle:** current Claude
  [hook docs](https://code.claude.com/docs/en/hooks#messagedisplay) include
  `MessageDisplay`, which fires during assistant-message streaming and only
  changes displayed text, not the transcript or Claude's context. Installed
  Claude Code 2.1.204 contains `executeMessageDisplayHooks` and related schema
  strings, while `CLAUDE_HOOK_ENTRIES` remains 29 entries and does not include
  it (the README's lifecycle profile is 25 of those 29, excluding four
  high-frequency tool hooks). This is optional display coverage drift, not a
  lifecycle/coordination blocker.

### OpenCode

Confirmed alignment:

- `opencode [project]`, `--session/-s`, and `--agent` are current flags
  ([CLI reference](https://opencode.ai/docs/cli)). Process cwd is a valid way to
  choose the project; passing a project path is also supported. Installed
  `opencode 1.14.50 --help` also exposes `--agent`.
- ForkTTY's generated plugin destination is a documented auto-loaded location:
  `.opencode/plugins` project-wide or `~/.config/opencode/plugins` globally.
  The plugin event list includes every event ForkTTY forwards, including
  `session.created`, `session.status`, permission events, tool before/after,
  compaction, idle/error/deleted ([plugins](https://opencode.ai/docs/plugins)).
- `--session ID` matches the resume argv ForkTTY builds.
- The default input submit key is `return`, supporting ForkTTY's CR submit at a
  semantic level ([keybinds](https://opencode.ai/docs/keybinds)).

Drift/gaps:

- OpenCode has built-in Build and Plan primary agents and exposes `--agent` on
  TUI launch ([agents](https://opencode.ai/docs/agents),
  [CLI](https://opencode.ai/docs/cli)), while ForkTTY reports
  `plan_mode: false` and does not add `--agent plan` for reviewers.
- OpenCode supports local stdio and remote MCP servers in the documented
  `mcp` object ([MCP servers](https://opencode.ai/docs/mcp-servers)), but
  ForkTTY has no OpenCode MCP installer and the router conservatively maps
  `managed_mcp: false` to `supports_mcp: false`. This is incomplete provider
  metadata: “ForkTTY has no installer”, “the harness supports MCP”, and “MCP is
  configured in this local environment” are three distinct facts. The first
  two are established here; this research did not inspect the third.
- OpenCode now explicitly discovers the shared `~/.agents/skills` target
  ([skills](https://opencode.ai/docs/skills)); ForkTTY's default skill setup
  therefore reaches it even though CLI help exposes no `opencode` alias.

### Pi

Confirmed alignment:

- The current first-party package/repository is Earendil Works. The documented
  executable remains `pi`; `--session <path|id>` selects an existing session;
  `--tools <list>` is an allowlist. The built-in tools include exactly
  `read,bash,edit,write,grep,find,ls`, so ForkTTY's
  `--tools read,grep,find,ls` review profile is genuinely read-only
  ([official README](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#cli-reference)).
- Pi scans both `~/.agents/skills` and `.agents/skills`
  ([skills](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#skills)).
- Pi explicitly says core has no permission popups, built-in MCP, or Plan mode;
  those can be extensions. ForkTTY's false provider flags are therefore honest
  for core Pi ([philosophy](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#philosophy)).
- Pi explicitly documents that Enter queues a steering message while a turn is
  running. ForkTTY's separate Enter can therefore result in queued steering,
  not necessarily immediate interruption
  ([message queue](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md#message-queue)).

Unknown:

- ForkTTY assumes a uniform four parallel sessions for every launchable
  provider. Pi documents that multiple processes can be orchestrated externally
  but publishes no four-session capacity guarantee. This limit is a ForkTTY
  policy, not provider evidence.

### Antigravity CLI

Confirmed alignment:

- Google's installer and codelabs use the `agy` executable
  ([install/getting started](https://antigravity.google/docs/cli-install)).
- Conversations are scoped to the current working directory; direct resume is
  `agy --conversation <id>` ([Managing conversations](https://antigravity.google/docs/cli-conversations)).
  ForkTTY's child-cwd launch and resume argv match that contract.
- Global MCP lives at `~/.gemini/config/mcp_config.json`, workspace MCP at
  `.agents/mcp_config.json`, using `mcpServers` and stdio `command`/`args`/`env`
  ([MCP](https://antigravity.google/docs/mcp)). ForkTTY's managed global entry
  matches.
- `.agents/skills` is the documented workspace skill location
  ([skills](https://antigravity.google/docs/skills)).
- Enter submits and Shift+Enter/Alt+Enter/Ctrl+J insert a newline
  ([Using AGY CLI](https://antigravity.google/docs/cli-using)).

Drift/gaps:

- Current docs list five hook events: `PreToolUse`, `PostToolUse`,
  `PreInvocation`, `PostInvocation`, and `Stop`; ForkTTY installs only the first
  three of that list (omitting `PostInvocation` and `Stop`)
  ([hooks](https://antigravity.google/docs/hooks)). Static strings in installed
  `agy 1.0.16` also include `GetPostInvocationHookArgs`, corroborating current
  binary awareness of the omitted event without claiming it is configured.
- Current docs describe hook `command` as a command string and `timeout` as
  seconds with a default of 30. Local comments still say `agy 1.0.3` executes
  one bare executable path and that timeout is unverified. Generated wrapper
  scripts remain a safe valid command, but the explanation and omitted timeout
  are stale against current first-party docs.
- The skill docs name an Antigravity-specific global directory, while the
  migration page names a different Antigravity CLI global directory
  ([skills](https://antigravity.google/docs/skills),
  [Gemini CLI migration](https://antigravity.google/docs/gcli-migration)).
  Neither page promises `~/.agents/skills` as a global alias. ForkTTY's default
  user skill setup therefore reaches Antigravity only when a repo carries the
  project `.agents/skills` copy; a dedicated global target requires runtime
  verification against the installed `agy` version.

### Grok Build

Confirmed alignment:

- The first-party CLI is `grok`. `--cwd`, `--resume/-r`, `--continue/-c`, and
  headless `-p` are documented ([CLI reference](https://docs.x.ai/build/cli/reference),
  [headless scripting](https://docs.x.ai/build/cli/headless-scripting)).
  ForkTTY's `grok --cwd PATH --resume ID` is correct.
- Plan mode and permission modes exist. The first-party keyboard reference says
  Enter sends a prompt and Ctrl+Enter interjects during an active turn
  ([modes](https://docs.x.ai/build/modes-and-commands),
  [keyboard shortcuts](https://docs.x.ai/build/keyboard-shortcuts)).
- Grok has native hooks and MCP, reads Claude Code configuration for both, and
  reads `~/.agents/skills`
  ([skills/plugins/compatibility](https://docs.x.ai/build/features/skills-plugins-marketplaces),
  [MCP compatibility](https://docs.x.ai/build/features/mcp-servers),
  [hooks](https://docs.x.ai/build/features/hooks)). Local `grok 0.2.93 inspect
  --json` confirms this compatibility path in the current checkout: it reports
  seven ForkTTY command hooks with `vendor: "claude"` from `~/.claude`, the
  `forktty` stdio MCP server from `~/.claude.json`, and the project
  `.agents/skills/forktty-agent-orchestration/SKILL.md` skill. This proves local
  discovery, but not a successful hook or MCP round trip.
  The current-runtime provider binding logic is still necessary so those
  Claude-compatible hook keys do not relabel a Grok worker.

Risks/gaps:

- ForkTTY marks `managed_hooks=false` and `managed_mcp=false`, which is accurate
  for its own installers. The router conservatively converts those fields into
  generic `supports_hooks=false` / `supports_mcp=false`; that is incomplete
  provider metadata. Provider support, ForkTTY-managed setup, and locally
  detected configuration need separate fields. The local `grok inspect`
  evidence above establishes discovery on this machine, not runtime success.
- ForkTTY adds `--permission-mode plan` to an interactive Grok review worker.
  xAI documents `--permission-mode plan` in the enterprise/headless section,
  and installed `grok 0.2.93 --help` exposes it as a top-level option with
  `plan` among the accepted values. The parser surface therefore supports
  ForkTTY's argv; only an interactive no-network startup smoke remains undone.
- ForkTTY uses ordinary Enter for every Grok dispatch. xAI documents
  Ctrl+Enter as “interject while a turn is running”. A message dispatched to a
  busy Grok worker may therefore not have the immediate steering semantics the
  control plane implies. This needs a live PTY test; the docs do not say whether
  ordinary Enter queues, rejects, or defers while busy.
- `target/debug/forktty --json identify` could not reach
  `/run/user/1000/forktty.sock` during this snapshot. Consequently, the local
  discovery above did not verify a live hook callback or MCP socket round trip.

## Cross-provider findings for the router

1. **Managed setup is not provider support or local configuration.**
   `task_strategy_runtime.rs`
   currently maps `managed_hooks` directly to `supports_hooks` and
   `managed_mcp` directly to `supports_mcp`. This is a conservative but
   incomplete representation of OpenCode MCP and Grok hooks/MCP. The local
   `grok inspect` snapshot proves Grok discovery on this machine; static router
   flags alone would not prove configuration or runtime success.
2. **OpenCode Plan launch metadata has drifted.** OpenCode has a documented
   built-in Plan agent selectable with `--agent`, confirmed by installed
   `opencode 1.14.50 --help`, but ForkTTY reports `plan_mode: false` and does not
   select `--agent plan` for reviewers. Codex is not part of this finding:
   without a documented startup flag its launch-capability value is
   conservatively false. Grok's installed top-level help accepts
   `--permission-mode plan`.
3. **Claude `auto` is conditional.** Executable discovery cannot prove the
   entitlement/config prerequisites ForkTTY currently assumes for every
   non-review worker.
4. **Parallel capacity is ForkTTY policy, not vendor evidence.** Every
   launchable provider receives the same `DEFAULT_PROVIDER_MAX_PARALLEL_SESSIONS`
   (currently 4). None of the reviewed vendor docs promises that capacity.
5. **Submit acknowledgements remain transport-level.** Provider docs confirm
   Enter semantics, but not ForkTTY's PTY write timing. Pi explicitly queues
   Enter while busy; Grok reserves Ctrl+Enter for active-turn interjection.
   A successful terminal write cannot prove that a provider began the intended
   turn.

## Recommended verification order

These are the remaining evidence gaps after the read-only version/help and Grok
inspection snapshot recorded above:

1. Add argv-only tests for OpenCode `--agent plan`, Grok interactive
   `--permission-mode plan`, and a Claude `auto` fallback policy.
2. In throwaway HOME/config roots, run each ForkTTY setup `--dry-run` and compare
   the produced config with the provider's own inspect/doctor command:
   `codex mcp list`, `claude mcp list`, `opencode mcp list`, `grok inspect`, and
   Antigravity's `/mcp`/hook loader diagnostics where a non-network command is
   available.
3. In a controlled PTY, prove idle submit and busy-worker submit separately.
   For Grok, distinguish normal Enter from Ctrl+Enter interjection; for Pi,
   assert queued steering rather than immediate execution.
4. Keep auth/quota outside static capabilities. Confirm those only from a
   visible authenticated worker or a provider-supported local status command.

## Explicit unknowns

- No first-party public document was found that makes PTY body/Enter write
  separation a stable ABI for Codex, Claude, Pi, or Grok. The ForkTTY delay is a
  compatibility heuristic backed by local tests.
- No provider publishes a four-parallel-session guarantee matching ForkTTY's
  router constant.
- Antigravity's two current pages disagree on its global skill directory; do
  not add a managed global target without checking the installed CLI.
- The public Codex docs establish Plan mode but not a stable direct launch flag;
  because ForkTTY's field governs launch argv, `plan_mode: false` is a
  conservative value rather than a finding.
- Installed `grok 0.2.93 --help` accepts top-level `--permission-mode plan`, but
  an interactive no-network startup smoke was not run.
- Local `grok inspect --json` confirms ForkTTY hook, MCP, and skill discovery,
  but no runtime round trip was possible while the ForkTTY socket was offline.
