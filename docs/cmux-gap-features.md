# cmux Gap Features

Features present in [cmux](https://github.com/manaflow-ai/cmux) that ForkTTY
does not yet match. This inventory is based on the cmux repository inspected on
2026-06-12 at commit `aeb8847` (`Sources/`, `CLI/`, and docs including
`cli-contract.md`, `agent-hooks.md`, `feed.md`, `workspace-groups.md`,
`dock.md`, `custom-sidebars.md`, and `remote-daemon-spec.md`).

Related agent-orchestration projects inspected on the same pass:

- [oh-my-codex](https://github.com/Yeachan-Heo/oh-my-codex) at commit
  `0332e47` (2026-06-09), focused on Codex CLI workflows, hooks, teams, HUD,
  `.omx/` state, tmux runtime adapters, and skills.
- [oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode) at
  commit `deee3a4` (2026-06-09), focused on Claude Code plugin/runtime
  orchestration, hooks, teams, HUD, `.omc/` state, notepad/project memory, and
  worktree-backed workers.
- [Zellij](https://github.com/zellij-org/zellij) at commit `b6a5ad0`,
  focused on active/resurrectable session inventory, recency sorting, and
  attach/resurrect UX.
- [WezTerm](https://github.com/wez/wezterm) at commit `891bed3`, focused on
  mux domains, pane metadata, local/Unix/SSH attach semantics, and status-safe
  metadata exposure.
- [agent-deck](https://github.com/asheshgoplani/agent-deck) at commit
  `e38969c`, focused on multi-agent session state, conductor/fleet status,
  provider session bindings, and restart/resume policy.

Browser-specific work is intentionally not the focus of this file. ForkTTY's
browser feature remains source-only and tracked separately in `ROADMAP.md`.

## Current Parity Snapshot

- Sidebar PR and listening-port hints: done for the local GTK sidebar.
- Socket events and capabilities: done; raw rpc/topology parity is still
  incomplete.
- Browser panes/import/profiles: partial and source-only; excluded here except
  where a non-browser cmux feature depends on the same infrastructure.
- SSH remote workspaces: partial. ForkTTY can spawn `ssh <host>` surfaces and
  restore them, but cmux has deeper remote-daemon semantics.
- Agent hooks/status: partial. ForkTTY has hook templates, status/progress/log
  metadata, notifications, and now per-surface persisted agent session ids,
  lifecycle, last activity, resume health, explicit resume, and read-only
  reclaim planning.

## Priority Non-Browser Gaps

### 1. Agent Resume And Hibernation

- **Impact**: high. **Cost**: medium-high. **Status**: partial.
- cmux has first-class agent session models, hibernation/reclaim paths, resume
  controls, and provider wrappers around Claude/Codex/OpenCode workflows.
- OMX/OMC also persist provider session ids and workflow state to make Stop
  continuation/resume decisions across hooks, CLI, and HUD.
- ForkTTY now persists `{agent, session_id, resume_cwd, lifecycle,
  last_activity_ms}` on a surface when hook `metadata.set_status` carries
  `hook_session_id` (and `resume_cwd` when the hook reports an existing cwd), exposes
  that inventory and hook-derived lifecycle through `agent.list`,
  `forktty agents`, and MCP `agent_list`, reports local resume readiness through
  `agent.health`, `forktty agent-health`, and MCP `agent_health`, can explicitly
  resume a persisted Codex/Claude/Gemini/OpenCode/Antigravity session into a
  new tab through argv-only provider commands, auto-resumes supported persisted
  agent terminal surfaces during session restore using the saved resume cwd as a
  provider flag where available or as the child process cwd otherwise (Codex can
  also infer cwd from local `session_meta` JSONL for older ForkTTY session
  files), exposes read-only reclaim candidates through `agent.reclaim.plan`,
  `forktty agent-reclaim-plan`, and MCP `agent_reclaim_plan`, and can now
  hibernate/reclaim idle locally resumable sessions through `agent.hibernate`,
  `agent.reclaim`, `forktty hibernate-agent`, `forktty reclaim-agents`, and MCP
  `agent_hibernate`/`agent_reclaim`. Hibernated sessions persist an explicit
  `suspended` lifecycle and do not auto-respawn; explicit resume still uses the
  argv-only provider path.
- Remaining scope: provider-side stale-session checks beyond local command/PATH
  readiness and richer UI controls.

### 2. Workflow State, Goals, And Memory

- **Impact**: high. **Cost**: medium. **Status**: control-plane done.
- OMX/OMC treat project-local `.omx/` / `.omc/` directories as a control plane:
  per-mode/per-session state, goal/spec artifacts, ledgers, session search,
  notepads, project memory, wiki/session capture, and compaction recovery.
- ForkTTY now has a provider-neutral, bounded `workflow-v1.json` state store
  with per-workspace/surface/session/mode bindings, durable goal/status/memory
  fields, replaceable plan steps, bounded evidence entries, and replayable
  workflow events. It is exposed through `workflow.*` socket methods,
  `forktty workflows` / `forktty workflow-*` CLI commands, and MCP
  `workflow_*` tools for agent use.
- Remaining scope: richer UI panels, project-local wiki/notepad file trees,
  deep prompt/spec capture, and parity-side search/navigation surfaces.

### 3. Team Orchestration Runtime

- **Impact**: high. **Cost**: high.
- cmux has teammate/provider launchers; OMX/OMC go deeper with leader/worker
  state, task DAGs, mailbox/dispatch, worker heartbeat/status, tmux-backed
  workers, idle nudges, shutdown, and optional worktree-backed workers.
- ForkTTY now owns a minimal provider-neutral team control plane through
  `team.*`, `forktty team-*`, and MCP `team_*`: leader/workspace metadata,
  worker records, task DAG validation, mailbox/inbox, heartbeats, summaries,
  event polling, optional surface/worktree references, provider worker launch
  into tabs, dispatch confirmations that write to worker panes, worker health
  snapshots, idle nudges, and safe shutdown requests.
- Remaining scope: parity UI for supervising teams.

### 4. Feed And Approval Bridge

- **Impact**: high. **Cost**: medium.
- cmux has a feed model for agent activity and approvals, not just a latest
  status string.
- OMC adds permission handlers, subagent tracking, persistent-mode Stop
  enforcement, and action-oriented hook context.
- ForkTTY has notifications, metadata logs, a notification panel, bounded
  durable feed history, workspace filtering, and approval decision state through
  `feed.approval.respond`.
- Remaining scope: provider-specific permission replies, richer provider
  filtering, and UI surfaces over the feed.

### 5. Remote Daemon And SSH Depth

- **Impact**: high for remote users. **Cost**: high.
- cmux remote support includes a remote daemon spec, reconnect/disconnect
  behavior, persistent remote PTY/session ownership, and CLI relay concepts.
- ForkTTY currently launches plain local `ssh <host>` surfaces.
- Scope: remote helper lifecycle, auth/ownership checks, reconnection model,
  remote command routing, remote session metadata, and failure recovery.

### 6. Project Actions And Layout Config

- **Impact**: medium. **Cost**: low-medium.
- cmux reads project config for actions and layout/workspace behavior.
- OMX/OMC also use project-local workflow files for prompts, skills, agents,
  plans, specs, notes, and runtime configuration.
- ForkTTY now has a bounded repo-local `forktty.json` action manifest with
  argv-only commands exposed through socket and CLI.
- Remaining scope: command-palette entries, per-workspace layout hints, skills,
  prompts, and a deliberately small project guidance manifest before any plugin
  API.

### 7. Right Sidebar, Dock, And Custom Sidebars

- **Impact**: medium-high. **Cost**: high.
- cmux has a richer side-panel ecosystem: files/find/vault/sessions/feed/dock
  style areas plus custom sidebar concepts.
- ForkTTY has a workspace sidebar, notification panel, settings, and worktree
  dialog, but no extensible right sidebar or dock model.
- Scope: panel container, persisted active panel, file/find/feed panels first,
  then extension points only after concrete built-in panels prove the API.

### 8. Workspace Organization

- **Impact**: medium. **Cost**: medium.
- cmux supports workspace grouping, pinning/collapse/reorder style workflows.
- ForkTTY supports workspace order and selection, but not groups or pin/collapse
  semantics.
- Scope: model-level groups, sidebar interactions, session persistence, socket
  verbs, and event stream changes.

### 9. Multi-Window And Routing

- **Impact**: medium. **Cost**: high.
- cmux can route work across multiple windows/views.
- ForkTTY is currently a DBus single-instance GTK app with one primary window.
- Scope: window identity in the model, focus routing, socket target selectors,
  session persistence, and safe single-instance behavior.

### 10. CLI / Topology / tmux Parity

- **Impact**: medium. **Cost**: medium-high.
- cmux CLI has broader topology and terminal-control verbs (`read-screen`,
  `send-key`, `top`, `tree`, move/reorder/split-off/capture/pipe/wait/swap/join
  style operations).
- OMX defines an adapter-neutral mux boundary around resolve-target,
  send-input, capture-tail, inspect-liveness, attach, and detach, with tmux only
  as the first adapter.
- ForkTTY covers core workspace/surface/pane-tab/metadata/worktree/browser
  verbs, plus read-only `topology.tree`, `system.top`, terminal `read-screen`,
  and `capture-tail` primitives. It still lacks `send-key`, move/reorder,
  join/swap/split-off, buffers, pipe/wait, and broader tmux-compatible
  manipulation.
- Scope: add mutating verbs only with clear model invariants and tests.

### 11. Prompt Composer / TextBox

- **Impact**: medium. **Cost**: medium.
- cmux has TextBox/prompt-composition surfaces.
- OMX/OMC add prompt/workflow skills such as deep interview, planning,
  ultragoal, and provider/team launchers.
- ForkTTY sends text directly to terminals and has no first-class draft prompt
  composer.
- Scope: reusable prompt editor, target surface selection, send/append actions,
  and session-safe drafts if needed.

### 12. Agent/Skill Catalog And Guidance Packs

- **Impact**: medium. **Cost**: medium.
- OMX/OMC ship installable agents, skills, commands, prompt templates, model
  routing hints, and project/user-scoped reusable guidance.
- ForkTTY installs hooks/MCP registrations, but it has no curated workflow
  catalog or project-scoped skill manifest.
- Scope: start with import/discovery of project guidance, then optional
  provider-specific workflow packs. Keep execution argv-safe and avoid making
  ForkTTY depend on one agent vendor.

### 13. HUD / Statusline Export

- **Impact**: medium. **Cost**: low-medium.
- OMC has a statusline HUD for active mode, agents, token/context health, tasks,
  and update/install diagnostics.
- ForkTTY has native in-app status, sidebar metadata, a read-only
  agent-session inventory, a GTK Agent HUD, and a compact
  workspace/status/progress/session summary through `status.summary`,
  `forktty statusline`, and MCP `status_summary`.
- Remaining scope: active-mode, worker, token, health, and notification fields
  when hooks provide them, plus provider-specific statusline packaging.

### 14. File And Review Panels

- **Impact**: medium. **Cost**: medium-high.
- cmux includes file explorer/preview, markdown, diff, and comment/review
  modules.
- ForkTTY intentionally focuses on terminal/worktree/socket today.
- Scope: file explorer and markdown preview before diff/comment review, with
  git/worktree safety checks reused from existing code.

### 15. Polish And Configuration Depth

- **Impact**: medium. **Cost**: ongoing.
- cmux has deeper shortcut/theme/preferences/update/product plumbing.
- ForkTTY has native settings and imports Ghostty terminal appearance config,
  but full theme customization, richer shortcut editing, and broader Ghostty
  app-option parity are still backlog.

## Non-Goals

- macOS and Windows support.
- Cloud VM backend, account/auth, hosted vault, telemetry, and product-service
  update checks.
- Bundling browser support into release artifacts before the source-only
  browser feature is considered stable.
- Treating the local user-owned socket as a hard same-user security boundary.
