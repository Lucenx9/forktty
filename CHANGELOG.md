# Changelog

All notable changes to ForkTTY are documented here.

## [Unreleased]

### Added
- Added dry-run-first `orchestration.cleanup` over socket, MCP
  `orchestration_cleanup`, and `forktty cleanup orchestration` so stale
  team/workflow records can be inspected and conservatively closed without
  touching live worker surfaces.

### Changed
- Reduced the always-sent ForkTTY MCP initialize instructions and task-strategy
  tool descriptions so agents get the routing essentials without duplicating
  the full operating guide; the complete guide remains available through the
  MCP resource and prompt.
- `forktty skills setup` and `forktty skills remove` now retain only the three
  newest ForkTTY-managed skill backups per target and report pruned backups,
  while leaving unmanaged `.bak-*` directories untouched.
- Task strategy planning now accepts a normalized `task_kind` /
  `task_class_hint` so agents can pass clear user intent across languages
  without relying on ForkTTY keyword guessing.

### Fixed
- Fixed task strategy plan/apply input validation so oversized goals are
  rejected before they can be copied into workflow/team state or worker
  prompts.
- Fixed task strategy planning input validation so oversized routing reason
  hints are rejected before they can inflate plan score explanations.
- Fixed task strategy error reporting so unsupported routing enum values and
  malformed submitted plans cannot echo oversized caller-controlled strings.
- Fixed task strategy text validation so goals reject terminal control
  characters other than newline or tab, and routing reason hints reject control
  characters before reaching worker prompts or score explanations.
- Fixed task strategy text validation so terminal/control characters at trimmed
  string edges are rejected instead of silently dropped.
- Fixed task strategy last-known-good inference so invalid harness ids in old
  workflow plan text are ignored instead of inflating router explanations.
- Fixed task strategy apply so submitted plan reasons and score details are
  revalidated before they can inflate workflow/team state, approval
  fingerprints, or worker prompts.
- Fixed task strategy apply so submitted plans with more assignments than
  visible team worker capacity are rejected before creating partial
  workflow/team state.
- Fixed `orchestration.cleanup` so `dry_run=false` without `apply=true` is
  rejected instead of mutating stale orchestration records.
- Fixed `orchestration.cleanup` so non-terminal workers without a recorded
  surface are reported for manual review instead of being closed as stale.
- Fixed task strategy parallel worker prompts so repeated researcher lanes get
  deterministic, distinct scopes instead of duplicate broad instructions.
- Fixed task strategy dirty-repo isolation so apply-time safety also uses the
  normalized plan task class, not only English goal wording.
- Fixed task strategy dirty-repo isolation so a client-submitted `review_only`
  strategy cannot bypass worktree approval when the submitted task class or
  assignments are mutating.
- Fixed task strategy dirty-repo isolation so apply-time safety treats mutating
  strategy shapes as edit intent even if a client submits a stale read-only
  task class.
- Fixed the managed ForkTTY orchestration skill text so installed agent skills
  describe the same dirty-repo isolation inputs as the socket/MCP runtime.
- Fixed task strategy goal classification so unrelated words that merely start
  with "fix" or "bug" (such as "fixtures") no longer classify a goal as a
  focused bugfix.

## [0.2.0-alpha.17] - 2026-07-03

### Added
- Added app actions exposed in the Command Palette for moving the active
  workspace up or down, with toast feedback when the workspace moves or is
  already at an edge.
- Added a read-only task strategy planner for agents and CLI users so ForkTTY
  can recommend when to use solo work, workflow loops, reviewers, teams,
  worktrees, MCP, and hooks before launching visible agent work.
- Added staged task strategy apply over socket, CLI, and MCP so an approved
  plan can create visible workflow/team/task/message coordination state without
  launching hidden workers or sending terminal input.
- Added approved `task.strategy.apply` submit support for supported team plans
  so ForkTTY can launch visible worker panes and dispatch role prompts through
  the team mailbox, including worktree-layer plans when `worktree_name` names an
  already-open ForkTTY worktree workspace.
- Added Feed-backed task strategy approval requests so `task.strategy.apply`
  can publish a pending start-run approval, stay blocked without workflow/team
  mutation, and later consume the approved request-bound `approval_id`.
- Added task strategy safeguards so provider selection respects configured
  team provider order, reviewer strategies always include a reviewer role, and
  apply recomputes structural worktree and multi-worker submit approvals instead
  of trusting a client-provided plan approval list.
- Added task strategy context inference so `task.strategy.plan` can derive
  dirty git state from the selected surface/workspace cwd when callers omit an
  explicit `repo_dirty` hint, and can infer likely user-visible edit intent
  from goal wording when callers omit an explicit user-visible hint.
- Added an explicit task strategy planning `cwd` override for socket, CLI, and
  MCP callers whose real repository cwd differs from the selected ForkTTY pane.
- Added ranked candidate strategy scores to `task.strategy.plan` responses so
  agents can see why ForkTTY selected a mode and which alternatives were
  considered before applying a plan.
- Added role-aware harness assignment scores to `task.strategy.plan` responses
  so ForkTTY can prefer plan-mode reviewers and worktree-cwd-capable
  implementers while keeping configured provider order as a tie-break.
- Added task router profiles (`balanced`, `fast`, `conservative`, `parallel`,
  and `review_heavy`) so `task.strategy.plan`, MCP `task_strategy_plan`, and
  `forktty task-plan --profile` can reweight the same explainable scorer
  without making users manually choose team, loop, worktree, or harness modes.
- Added per-harness task routing signals so scripts and MCP agents can pass
  observed cooldowns as soft assignment penalties while hard task/mode lockouts
  exclude a harness from the selected plan.
- Added advisory last-known-good task routing so `task.strategy.plan`, MCP,
  and `forktty task-plan` can infer prior successful strategy/harness evidence
  from completed task-strategy workflows or accept explicit caller evidence,
  then apply only a small explainable score bias without overriding readiness,
  cooldown, lockout, task fit, or approval rules.
- Added Grok Build (`grok`) as a visible team/router harness with Settings,
  `system.capabilities`, `team.worker.launch`, and task-strategy routing
  support.

### Changed
- Reordered the main menu so the standard app items stay grouped, and the
  sidebar toggle now reports its shown/hidden state with a toast.
- Improved dark UI shortcut contrast in custom menus and the Command Palette,
  and disabled workspace move commands when the active workspace is already at
  an edge.
- Improved `forktty task-plan --help`, `forktty task-apply --help`, and
  `forktty feed respond --help` so task-router approval flows document their
  positional goals, plan JSON, `cwd`, and approval decisions directly.

### Fixed
- Fixed `cargo audit` failures by updating the Windows notification backend
  dependency so the lockfile no longer includes vulnerable `quick-xml` versions.
- Fixed CLI boolean parsing so commands such as
  `forktty task-plan --repo-dirty false --parallel false` treat the following
  `true`/`false` token as the option value instead of silently leaving it in the
  positional task goal.
- Fixed `team.summary` and compact context snapshots so workers whose terminal
  surfaces disappeared are no longer counted as active and now raise a
  consistency warning.
- Fixed task strategy worker prompts so launched workers are told that the
  leader already applied the plan and must not call `task.strategy.apply`,
  launch nested workers, or create nested worktrees unless separately directed.
- Fixed command argv validation so GNU `env` wrappers cannot hide shell
  trampolines behind `-- NAME=VALUE` assignments, `-a`/`--argv0` arguments,
  `-S`/`--split-string` values that begin with `--`, or attached/clustered
  `-Sstring` and `-vSstring` split-string forms.
- Fixed Feed persistence so corrupt `feed.json` files are quarantined instead
  of disabling the Feed on every launch, Feed saves are fsync-backed like the
  other stores, and approved approval rows are retained through notification
  churn so later `task.strategy.apply` retries can still consume them; prompt
  notification approval decisions are now preserved only for the same persisted
  prompt payload so a same-millisecond id collision after restart cannot inherit
  an old decision. Feed writes and recovery now also reject duplicate entry ids
  or inconsistent approval state instead of saving or loading ambiguous approval
  rows.
- Fixed team store validation/recovery and ack idempotency so legacy duplicate
  persisted message ids are repaired on load by retaining the first message
  while preserving delivered/superseded terminal state, strict saves reject
  invalid state, and repeated message acknowledgements do not emit duplicate
  `team.message.acked` events.
- Fixed team/workflow store mutation ordering so event sequence failures cannot
  leave in-memory plan, task, worker, or message updates partially applied
  before the store update is rejected.
- Fixed team/workflow store caps so creating a new record can evict the oldest
  terminal `done`/`closed`/`cancelled`/`finished` record instead of leaving the
  store permanently full until manual JSON editing.
- Fixed `team.finish --close-workers` so a worker surface close failure leaves
  team and worker store state unchanged instead of persisting partial shutdown
  requests or missing-surface worker cleanup before the error.
- Fixed `team.finish --close-workers` so closing multiple worker panes restores
  any already-closed runtime surface if a later worker surface close fails,
  avoiding a half-closed visible team after an error.
- Fixed `team.finish --close-workers` so closing a launch-owned worker pane that
  became the only pane in an inactive workspace still spawns the normal
  replacement terminal surface, and leaves the worker running if that
  replacement cannot be spawned.
- Fixed `team.worker.shutdown --close-surface` so a worker surface close
  failure leaves the worker store state unchanged instead of persisting a
  partial shutdown request.
- Fixed small GTK polish issues: failed worktree merge/remove notifications now
  appear as errors, multi-tab close confirmations say tab instead of pane,
  sidebar pane counts ignore extra tabs, context menu shortcut/copy icon hints
  are consistent, notification clear copy matches the action, and workspace
  popover buttons expose accessible names.
- Fixed task strategy routing so parallel research/experiment plans respect
  each harness's declared parallel session capacity instead of assigning
  multiple concurrent roles to a single-session harness.
- Fixed task strategy routing so a single launchable harness can provide
  multiple parallel research lanes when its declared session capacity permits
  it, instead of degrading to a non-parallel strategy.
- Fixed task strategy routing so parallel research/experiment plans do not
  launch an eager synthesizer worker before researcher workers can produce
  output.
- Fixed task strategy routing so review-requested implementer/reviewer plans
  no longer require worktree isolation in clean repositories unless dirty-repo
  edit isolation actually applies.
- Fixed provider capability reporting so plan-mode support is exposed through
  `system.capabilities` and used by task-strategy reviewer scoring.
- Fixed task strategy approval retries so an approved Feed request covering a
  superset of required approvals can still satisfy the remaining approvals when
  the caller also supplies explicit attestations for part of the same request.
- Fixed task strategy submit retries so an existing live worker is reused only
  when its harness, role, task, worktree, and active worker status match the
  current deterministic assignment; blocked, idle, or needs-input workers now
  return `conflict` before any prompt dispatch.
- Fixed task strategy apply so conflicting `surface_id` and `leader_surface_id`
  aliases are rejected before dirty-repo checks or workflow/team mutation.
- Fixed task strategy planning so explicit `cwd` context is limited to Git
  repositories already represented by an open ForkTTY workspace, surface, or
  effective project cwd.
- Fixed task strategy apply so explicit `cwd` launch targets must also be
  inside a Git repository already represented by an open ForkTTY workspace,
  surface, or effective project cwd before any workflow/team mutation.
- Fixed task strategy apply so explicit `cwd` launch targets are canonicalized
  before worker launch, prompt generation, and submit-retry compatibility
  checks, keeping retries idempotent when callers switch between symlinked and
  real paths for the same open repository.
- Fixed task strategy apply so camelCase workspace selector aliases such as
  `workspaceId` and `worktreeName` target the same workspace for dirty-repo
  inference and workflow/team mutation instead of falling back to the active
  workspace.
- Fixed task strategy submit retries so a live worker whose launch cwd or
  surface cwd differs from the selected target cwd is rejected even when the
  retry omits an explicit `cwd`.
- Fixed worktree-layer task strategy submit retries so a live worker is reused
  only when its surface cwd still matches the currently selected worktree
  workspace target.
- Fixed task strategy routing so review-primary goals that mention
  implementation terms still stay read-only review work and no longer force
  dirty-repo worktree isolation.
- Fixed task strategy dirty-repo edit inference so unrelated words such as
  "dropdown", "fixture", or "portable" no longer trigger worktree isolation
  through short edit-prefix matches.
- Fixed MCP `task_strategy_apply` so calls without explicit target selectors
  inherit the current ForkTTY workspace and leader surface from the MCP
  environment, matching the intended in-pane apply flow.
- Fixed task strategy submit apply so unlaunchable harness assignments are
  rejected after approvals but before workflow/team state is mutated.
- Fixed worktree-layer task strategy apply so staged task details and worker
  prompts name the selected worktree and effective repository cwd.
- Fixed task strategy submit retries so a persisted worker record with a closed
  or missing surface is relaunched before dispatching its still-pending role
  prompt.
- Fixed task strategy submit retries so a relaunched worker receives a fresh
  role prompt when the previous prompt was already delivered to the old closed
  surface.
- Fixed task strategy submit so staged prompts with a stale cwd, body, or
  worker target are not reused when launching workers for a later apply.
- Fixed staged task strategy apply retries so a changed cwd, body, or target
  queues the next deterministic role prompt instead of reusing a stale pending
  message.
- Fixed task strategy prompt replacement so older undelivered deterministic role
  prompts are marked superseded and no longer appear in normal `team.inbox` or
  pending-message counts after a later staged/apply retry changes the task body,
  target worker, or cwd.
- Fixed team message dispatch and ack so superseded prompts cannot still be
  delivered explicitly by id or marked acknowledged after replacement.
- Fixed team message storage so hitting the per-team message cap evicts only
  delivered or superseded history and rejects new sends rather than silently
  dropping pending instructions.
- Fixed MCP tool schemas for `workflow_evidence_add` and `worktree_status` so
  schema-driven clients see the same required alternatives enforced by the
  handlers.
- Fixed workflow evidence auto ids so omitted `evidence_id` values skip over
  existing explicit evidence ids instead of permanently colliding with them.
- Fixed workflow loop-state updates so invalid iteration/max-iteration payloads
  are rejected without leaving mutated in-memory workflow state behind.
- Fixed workflow upsert/evidence updates so failures while recording their
  audit event summary do not leave partially mutated in-memory workflow state.
- Fixed `forktty team-worker-launch` so the CLI exposes the documented `--cwd`
  worker launch target.
- Fixed workflow list/replay queries so `limit: 0` returns zero rows, matching
  the other bounded list APIs.
- Fixed team event persistence so stores with stale or missing
  `next_event_seq` mint new event sequence numbers after the highest persisted
  event, and legacy non-increasing event histories are repaired on load before
  strict validation.
- Fixed Codex team message submit so ForkTTY sends the task prompt and Enter as
  separate terminal actions instead of leaving fresh Codex worker prompts staged.
- Fixed team message dispatch for freshly launched Codex/Claude/Pi workers so
  ForkTTY waits briefly before the first prompt and uses a separate submit Enter.
- Fixed team message dispatch for freshly launched Codex, OpenCode, and
  Antigravity workers so ForkTTY waits briefly before sending the first task
  prompt.
- Fixed Codex/Claude/Pi team message dispatch retries so a failed separate submit
  Enter does not resend and duplicate the already-written prompt body.
- Fixed Feed approval responses so dismissed, stale, approved, or denied
  approvals cannot be re-approved and reused after the pending request is no
  longer active.
- Fixed task strategy classification so goals that mention review feedback but
  explicitly ask to fix a bug still route through the bugfix verify-loop path
  instead of being treated as read-only review work.
- Fixed task strategy routing and apply safety for review-first goals that
  mention bug fixes, common editing verbs on dirty repositories, and
  deterministic prompt ids with very large numeric suffixes.
- Fixed task strategy apply so a pending Feed approval request is dismissed
  when the same run is later applied with equivalent explicit `approved`
  attestations, including retries that also pass the superseded `approval_id`,
  avoiding stale `pending_approval` risk flags.
- Fixed task strategy routing from real provider capabilities so one launchable
  provider can still satisfy implementer-plus-reviewer plans by reusing the
  same visible harness for both roles.
- Fixed staged worktree-layer task strategy apply so it requires `worktree_name`
  for an already-open ForkTTY worktree workspace before mutating workflow/team
  state, matching submit-mode enforcement.
- Fixed task strategy submit runs with an explicit `cwd` so worker panes launch
  in that repository, role prompts name it, and submit retries reject live
  deterministic workers whose cwd no longer matches the assignment; dirty
  editing apply checks also treat that explicit cwd as a worktree-isolation
  target.

## [0.2.0-alpha.16] - 2026-06-27

### Fixed
- Fixed GTK window close so the embedded Unix-socket server receives a shutdown
  signal instead of keeping the ForkTTY GUI/AppImage process alive after the
  last window is closed.
- Fixed hook and MCP setup from AppImage launches so generated ForkTTY CLI
  commands set `APPIMAGE_EXTRACT_AND_RUN=1`, preventing short hook calls and
  persistent MCP servers from leaving FUSE AppImage runtime mounts behind.
- Fixed AppImage terminal launches with opt-in PTY process persistence so
  detached `dtach` brokers do not inherit AppImage runtime file descriptors and
  keep the FUSE mount alive after the GTK window closes.
- Fixed opt-in PTY process persistence cleanup so disabling the setting,
  starting with it disabled, or explicitly closing/restarting a pane terminates
  stale ForkTTY-managed `dtach` brokers and their child process trees instead
  of leaving detached terminals behind; closing the GTK window after disabling
  persistence now also cleans visible managed brokers.
- Fixed generated OpenCode hook plugins so they contain valid JavaScript
  constants instead of Rust visibility prefixes.
- Fixed official CLI/MCP socket clients timing out before slower server-side
  operations complete or rejecting valid bounded responses larger than 1 MiB.
- Fixed `team.message.dispatch` so concurrent or post-send/failed-ack retries
  of the same queued message do not write the prompt to a terminal twice.
- Fixed duplicate `team.worker.launch` calls for the same live launch-owned
  worker so the second call fails before leaving an orphaned worker pane.
- Fixed `workspace.close` and `worktree.remove` rollback when closing multiple
  surfaces fails after one or more terminal runtimes were already closed.
- Fixed `workspace.close` and `worktree.remove` surface-set races so tabs or
  splits started during a close cannot leave orphan terminal runtimes behind.
- Fixed team and workflow store updates to coordinate through per-store lock
  files, preventing lost updates when multiple ForkTTY processes share a state
  directory, and moved socket store I/O onto Tokio's blocking pool so slow
  filesystems do not stall unrelated socket requests.
- Fixed `events.subscribe` validation for non-boolean `replay` values and
  capped event subscribers separately from the general socket request budget.
- Fixed MCP tool metadata and `SPEC.md` drift for workflow loop state and
  non-idempotent team heartbeat/ack operations.
- Fixed Antigravity hook setup so the generated `PreInvocation` entry uses
  Antigravity's flat lifecycle-hook handler shape instead of the nested
  tool-hook matcher shape, allowing ForkTTY's before-model wrapper to load.

### Security
- Restricted opt-in PTY process persistence to explicitly plain interactive
  terminal shell spawns so project actions and team-worker provider commands
  cannot be wrapped in `dtach` and outlive their visible pane unexpectedly.

## [0.2.0-alpha.15] - 2026-06-23

### Added
- Opt-in real PTY/process persistence for generic terminal panes via a new
  `general.persist_terminal_processes` config flag (default off). When enabled
  and a detach/reattach broker (`dtach`) is on `PATH`, plain interactive
  terminals run under the broker so their process tree (shell, dev servers,
  REPLs, editors, long-running commands) survives a GTK UI restart; a relaunch
  re-attaches the same surface to its still-running processes, keyed by the
  persisted surface id. Agent panes (provider resume), SSH, and browser surfaces
  are unaffected, and behavior is unchanged when the flag is off or no broker is
  installed. The broker socket lives under the owner-only per-user runtime dir
  and ForkTTY never wraps a `sh -c` command. `system.capabilities` and
  `forktty capabilities` report whether the flag is configured and whether a
  broker is currently available. Explicit ForkTTY pane close/restart removes
  the per-surface broker socket so a later reused surface id starts fresh
  instead of re-attaching to a stale detached session. Settings > Worktrees now
  exposes a "Persist terminal processes" toggle and shows whether `dtach` is
  available from the running ForkTTY environment.
- CLI automation now includes high-level `forktty team ask/watch/finish/review`
  wrappers, `forktty status explain/watch`, a `context-snapshot` alias, grouped
  help/examples, and generated bash/zsh/fish completions.
- `forktty skills setup` now installs a ForkTTY agent orchestration skill for
  Agent Skills-compatible tools (`~/.agents/skills`) and Claude Code
  (`~/.claude/skills`) so agents have an explicit policy for proactively using
  ForkTTY MCP, hooks, context snapshots, and team workers.
- Socket `context.snapshot` and MCP `context_snapshot` now provide a compact
  read-only workspace snapshot with pane/surface state, status, agent health,
  compact workflow/team/feed/remote summaries, and bounded untrusted terminal
  tails. Full workflow and team records are opt-in.
- `system.capabilities` now advertises a provider capability matrix for the
  supported team/resume providers (`codex`, `claude`, `pi`, `opencode`, and
  `antigravity`) so socket and MCP clients do not need to infer provider support
  by trial and error.
- Agent rows returned by `agent.list`, `agent.health`, `status.summary`, and
  `context.snapshot` now include `lifecycle_evidence`, a compact diagnostic
  block that correlates the persisted lifecycle, freshness timestamps, current
  workspace/provider status row, permission mode, and resume-readiness reason
  where available.
- Socket/MCP/CLI automation now includes `system.identify`/`identify` for a
  compact canonical workspace/surface/effective-project-cwd read, and CLI
  automation adds `forktty wait agent-status` for bounded read-only polling of
  persisted agent lifecycle state through short `context.snapshot` reads.
- Pi is now a supported team/resume provider: team launch accepts `--agent pi`,
  agent resume uses `pi --session <id>`, `forktty skills setup pi` aliases the
  interoperable Agent Skills target, and Pi review workers default to read-only
  `--tools read,grep,find,ls` unless explicit Pi tool args are supplied.
- Team message dispatch now supports an explicit submit mode through socket
  `team.message.dispatch` (`submit: true`), MCP `team_message_dispatch`, and
  CLI `forktty team-message-dispatch --submit`, appending a carriage-return
  Enter to the dispatched terminal input when the message does not already end
  in carriage return.
- Socket/MCP/CLI automation now includes `team.finish` / `team_finish` /
  `forktty team finish` to verify team state, optionally close current-runtime
  launch-owned disposable worker panes, normalize missing worker surfaces, and
  mark the team done in one finalization step.
- Workflow automation now includes bounded closed-loop state through socket
  `workflow.loop.set`, MCP `workflow_loop_set`, and CLI
  `forktty workflow-loop-set`: agents can record a loop recipe, stage,
  iteration budget, stop reason, and verification gate counts without granting
  ForkTTY any background scheduler or automatic command execution.
- Team worker launch now supports configurable provider auto-selection:
  Settings > Agents exposes the default provider, fallback, provider order,
  disabled providers, PATH detection, and direct command overrides for
  non-default harness install locations; socket/MCP/CLI launches can omit
  `agent` or pass `auto`, and successful launches report the selected provider
  and considered candidates.

### Changed
- ForkTTY config no longer emits the obsolete `general.theme_source` key;
  existing files that still contain it continue to load, and terminal theme
  preferences remain owned by Ghostty config.
- Command Palette, GTK context menus, Keyboard Shortcuts, About ForkTTY, and
  Settings have been polished for clearer shortcut labeling, standard menu
  access (`F10` outside terminal focus), richer About links, and correct
  initial focus when opening directly to the Agents settings page.
- `AGENTS.md` now reflects the current MCP/team/workflow/skill/AppImage
  maintenance flow, including final-binary skill checksum verification.
- Agent HUD rows now show a compact workflow loop chip when a visible agent
  surface is bound to closed-loop workflow state, with gate failures and
  human-attention stops highlighted without adding a separate loop panel.
- Pane drag-and-drop now exposes a visible header grip and tooltip clarifying
  that dragging a pane header swaps panes.
- Tab drag-and-drop now starts from the tab grip instead of the whole tab,
  reducing accidental drags while selecting or closing tabs.
- Notification panel and Agent HUD polish now make attention states easier to
  scan: notifications group prompt/current-workspace history, avoid a duplicate
  global open action, and use quieter tonal cards/chips/action areas; agent
  rows show the current pane, group lifecycles, surface risky permission modes,
  compact ended sessions, calmer status chips, compact risky permission labels,
  and inline terminal previews.
- Agent-oriented workspace chrome now uses more human scan labels and targets:
  running agent rows/badges read as Working, ended rows read as Done, sidebar
  metadata suppresses standing permission-mode noise, workspace paths prefer a
  tracked agent `resume_cwd` when it differs from the launch directory,
  notification jump prioritizes unread prompts, and high-level team CLI output
  reports the worker, task, surface, provider, and submit state.
- GTK chrome micro-polish now aligns workspace badges, pane action hover/focus
  states, sidebar actions, status shortcuts, and pane hairlines with the quieter
  Agent HUD and notification panel treatment.

### Fixed
- AppImages now prefer the host GTK/libadwaita stack when it is available and
  keep the bundled GTK copy as a fallback/override
  (`FORKTTY_APPIMAGE_GTK_RUNTIME=bundled|host|auto`), preventing the Ubuntu
  release bundle's GTK 4.14 runtime from failing embedded Ghostty OpenGL context
  creation on newer desktop stacks.
- Notification cleanup now stays coherent across surfaces: clearing through the
  socket or notification panel closes matching desktop notifications, sends OSC
  99 close reports when the terminal requested them, and marks pending prompt
  approval feed rows as `dismissed`; stale prompt approvals whose target
  workspace/surface disappeared now report `approval_state: "stale"` and no
  longer raise `pending_approval` risk flags.
- Workspace/surface-targeted desktop notifications now expose a best-effort
  default Open action that focuses the relevant ForkTTY surface or workspace,
  while terminal notification action buttons carry accessible labels and
  app-authored notification chips use the clearer `App` label.
- Team message dispatch and worker shutdown submit mode now use
  provider-aware input: Codex, Claude, and Pi receive staged text, a short settle, and
  a separate Enter, while other providers keep text plus carriage-return Enter
  in one terminal input.
- Context snapshots now include compact `loop_summaries` rows for workflow
  loops, including stale surface-binding detection and risk flags for failed
  gates, blocked/needs-human stages, exhausted loop budgets, and stale
  workflow surface bindings; the rows omit full workflow goals and detailed
  gate notes so default snapshots stay compact.
- Workflow loop iteration updates now clear prior gate results and stop reasons
  unless the same request supplies replacements, preventing stale failed gates
  from carrying into a new verification pass.
- Workflow consistency warnings now treat `completed` plan steps as terminal,
  matching existing workflow records and avoiding false `done_with_open_plan_steps`
  risk flags in compact context snapshots.
- CLI routing now recognizes `forktty workflow-loop-set` and its socket-style
  aliases, so the documented wrapper reaches `workflow.loop.set` instead of
  being rejected as an unknown argument.
- Worktree socket/CLI/MCP operations now ignore hook-reported agent `resume_cwd`
  metadata for repository authorization and only trust visible workspace roots
  plus the recorded cwd of open surfaces, preventing spoofed hook metadata from
  authorizing hidden worktree operations in unopened repositories.
- Team worker launch no longer copies a leader's hook-reported `resume_cwd` into
  the new worker surface cwd, preventing spoofed agent metadata from being
  promoted into the worktree authorization boundary through a visible worker
  tab.
- CLI routing now accepts socket-style `worktree.*` and `project.action.*`
  aliases in addition to the documented dash/colon forms, keeping low-level
  worktree and project-action wrappers consistent with other socket methods.
- `team.summary` now flags active teams that have no active workers, open tasks,
  or pending messages as `active_without_open_work`, so stale orchestration
  records are visible before an agent mistakes them for still-running work.
- `team.worker.health` now reports `surface_present`,
  `surface_runtime_present`, `surface_ready`, and a `starting` final state for
  present but not-yet-ready worker runtimes, while still treating exited or
  lost worker panes as not live. `forktty team watch` now prints `final_state`
  and runtime readiness, and `forktty status explain` includes compact agent
  evidence such as source, age, session id, status row, readiness, permission
  mode, and effective project cwd.
- GTK workspace badges now distinguish status-driven `Input`, `Starting`, and
  `Missing` states instead of folding them into generic Exited/Running badges.
- Agent hook `PreToolUse` events now keep the primary agent status value as the
  compact lifecycle state `Running` while preserving the exact tool name in the
  hook log metadata, reducing noisy `Running <tool>` snapshots for agents.
- `team.worker.shutdown`, MCP `team_worker_shutdown`, and CLI
  `forktty team-worker-shutdown` now submit shutdown text by default, so custom
  shutdown messages no longer sit unexecuted in full-screen agent composers.
  They also support an explicit close option for surfaces created by the current
  ForkTTY runtime through `team.worker.launch`, giving team leaders a safe way
  to immediately close disposable worker panes without closing manually attached
  surfaces.
- `team.worker.shutdown` with close-surface cleanup now requires proof that the
  target worker surface was launched by the current ForkTTY runtime, preventing
  stale persisted team records from closing an unrelated pane whose surface id
  was reused after restart.
- `system.identify`, MCP `identify`, and `forktty identify` now treat ForkTTY
  pane environment ids as caller context rather than mandatory target selectors,
  so stale `FORKTTY_SURFACE_ID` values no longer make the compact identify read
  fail before returning caller validation fields.
- Polished small GTK UI details: suspended agents now have a proper lifecycle
  pill, truncated agent/notification text keeps full tooltips, pane status text
  has stronger contrast, and the Agents settings entry uses a more fitting
  icon.
- Agent HUD scrollbars now reserve their own gutter instead of overlaying row
  action buttons.
- Embedded Ghostty panes now force Ghostty's `wait-after-command` behavior for
  ForkTTY-managed surfaces, so a clean shell exit leaves an inspectable
  `Closed` pane with restart/scrollback parity instead of immediately removing
  the split.
- The GTK worktree dialog now uses the focused pane's live or recorded cwd when
  available, so worktree actions no longer fall back to the workspace launch
  directory when ForkTTY knows the pane is in a project checkout.
- Socket `context.snapshot` and MCP `context_snapshot` now cap terminal-tail
  snapshots by aggregate surface count and text bytes, preventing large
  workspaces from multiplying the per-surface tail limit into an oversized
  response.
- Sidebar workspace metadata now keeps status and progress rows for inactive
  tabs that are still open, so exited or failed background tabs continue to
  affect the workspace badge.
- The local `forktty doctor` hook diagnostics now include the default
  Antigravity hook config path alongside Codex, Claude Code, and OpenCode.
- Automatically named workspaces now keep the visible name aligned with the
  allocated workspace id, even after earlier workspaces are closed and the id
  counter has gaps.
- `forktty team ask/review` and MCP `team_upsert` now bind the team to the
  invoking ForkTTY surface or workspace from `FORKTTY_SURFACE_ID`/
  `FORKTTY_WORKSPACE_ID`, so launched workers open next to the orchestrator pane
  and inherit its working directory.
- `team.worker.launch` now honors an explicit `worktree_name` by opening the
  worker in that worktree workspace, validating the worktree name, and using
  that workspace directory instead of falling back to the team leader workspace.
- `forktty team ask/review` now create the task before launching the worker and
  assign it after launch, matching the socket server's task and worker
  validation order and avoiding a `running` unassigned task if worker launch
  fails.
- `forktty team ask/review --help` now describes the task creation and worker
  launch order correctly.
- Claude Code team workers now start with documented permission-mode defaults:
  review roles use non-interactive `dontAsk` with pre-approved built-in read
  tools (`Read`, `Grep`, and `Glob`), other Claude workers use `auto`, and
  explicit provider permission args are left untouched.
- Embedded Ghostty title updates now ignore launcher-wrapper titles such as
  `/usr/bin/env` from both terminal events and embedded title notifications,
  preventing new team or terminal panes from replacing useful pane titles with
  the wrapper executable path.
- Team message dispatch now brings the target worker workspace/tab to the
  foreground and waits briefly for the embedded terminal surface to become
  socket-ready before typing, fixing unattended `team ask/review` prompts that
  were queued for a newly launched but hidden worker tab.
- `forktty skills setup` and `forktty skills remove` now route through the
  top-level CLI parser instead of being rejected as unknown arguments.
- `forktty --json doctor` now inspects managed skill files through bounded
  regular-file reads and reports symlinked, non-regular, or oversized managed
  skill components as invalid instead of following or opening them directly;
  `forktty skills setup` repairs invalid managed copies only after verifying the
  ForkTTY-managed marker and otherwise refuses to overwrite unverified skill
  directories or `SKILL.md` entries.
- Team-wide message dispatch with an explicit worker target now marks the
  message delivered after the terminal accepts the text and optional submit
  input.
- Codex MCP setup/removal now reads `$CODEX_HOME/config.toml` and
  `~/.codex/config.toml` with the MCP config size limit, not the smaller hook
  config limit.
- Team message dispatch submit mode now treats LF-terminated prompt text as
  still requiring explicit submit input, fixing full-screen agent TUIs where a
  pasted newline is not equivalent to pressing Enter.
- MCP `team_message_dispatch` is now annotated as destructive and open-world,
  matching that it can type into an agent pane and optionally submit input.
- Command safety now rejects BusyBox shell applet trampolines such as
  `busybox sh -c ...` when validating provider launch arguments.
- Embedded Ghostty panes now synchronize runtime `cols`/`rows` metadata through
  Ghostty's bounded read-text ABI, so `surface.list`, `context.snapshot`,
  `topology.tree`, and `system.top` stop reporting the initial 80x24 size after
  the pane is resized on current embed builds.
- Agent lifecycle summaries now ignore stale hook events consistently with the
  visible status row, preventing delayed `running`/`needs input` updates from
  overriding newer agent state.
- Agent sessions attached to terminal panes are now marked `ended` when the
  terminal child exits, preventing stale `running`/`needs input` state after an
  agent process has already stopped.
- Closed, forgotten, hibernated, or ended agent panes now clear their provider
  status, permission mode, token progress, and closed-surface metadata when no
  same-provider session is still active in that workspace, preventing stale
  agent names in the workspace sidebar summary.
- The workspace sidebar now ignores stale surface/provider metadata for panes
  or managed agents that are no longer present, preventing old `Exited` badges
  or activity summaries from sticking after the active pane has moved on.
- AppImage packaging now preserves the `libgtk4-layer-shell` runtime SONAME
  alongside the unversioned development name, so embedded Ghostty panes can
  load on hosts without a system gtk4-layer-shell installation.
- Browser panes now refresh immediately after model-driven URL changes even
  when the pane has no terminal chrome or tab-strip entry.

### Changed
- `context.snapshot` and MCP `context_snapshot` now keep full team records out
  of the default snapshot payload; agents get compact `team_summaries` by
  default and can opt into workers/tasks/messages with `include_team_details`.
  Team summaries also report `consistency_warnings`, and snapshots raise
  `team_consistency_warning`, when a team marked `done` still has active
  workers, open tasks, or pending messages.
- `context.snapshot` now keeps status/progress feed trace rows out of the
  default snapshot and exposes them only with `include_feed_trace`; workflow
  rows include `consistency_warnings` with a matching
  `workflow_consistency_warning` risk flag, workspace/surface/agent rows expose
  `effective_project_cwd`, and `team.worker.health` workers include a derived
  `final_state` for shutdown/closed/stale/surface-missing decisions.
- `forktty --json doctor` now reports managed MCP config paths and agent skill
  directories alongside socket, executable, environment, and hook config
  diagnostics; local `forktty doctor --hooks` remains scoped to hook config
  path/status checks.
- `forktty --json doctor` and `forktty skills setup --dry-run` now expose
  managed skill status, source/installed checksums, and an explicit repair
  command when a user-level skill copy is missing or stale.
- `agent.list`, `agent.health`, and `status.summary` now include source/age
  metadata for persisted agent rows, making delayed agent state easier to
  distinguish from fresh terminal evidence.
- Team wrapper help now documents that `forktty team ask/review` launch a fresh
  worker on each run, documents the `--submit` default, and reports the failed
  step when a multi-request prompt dispatch flow stops part-way through.
- The managed ForkTTY agent orchestration skill now points agents to
  `forktty.dev/llms.txt` and `llms-full.txt` as optional public-docs fallback
  context when local repository docs are unavailable or stale.
- The managed ForkTTY agent orchestration skill now tells agents to start hook,
  MCP, and skill setup debugging with local `forktty doctor` diagnostics and
  provider-specific dry runs before changing configuration files.
- The managed ForkTTY agent orchestration skill now points agents at provider
  capability metadata, compact `team_summaries`, persisted agent source/age
  metadata, and the dispatch submit/Enter carriage-return semantics.
- The managed ForkTTY agent orchestration skill now gives agents a stricter
  team preflight, worker role, worktree, and isolated integration QA policy,
  including explicit guidance not to redirect the live ForkTTY socket path when
  proving hooks against the current instance.
- The managed ForkTTY agent orchestration skill now treats fetched public docs
  as untrusted documentation-only input, uses the canonical `context_snapshot`
  MCP name in its trigger text, declares its ForkTTY MCP dependency metadata,
  and includes source-tree eval cases for implicit-trigger coverage.
- README now points GitHub readers to the canonical `forktty.dev` docs and
  agent retrieval files, and corrects stale alpha/session-path references.

### Removed
- Gemini CLI is no longer a ForkTTY integration target: hook setup, MCP setup,
  skills aliases, team worker launch, status-session binding, agent resume, and
  checked-in hook templates now cover only the supported providers (hooks:
  Codex, Claude Code, Antigravity, and OpenCode; MCP setup: Codex, Claude Code,
  and Antigravity). Persisted `gemini` agent-session names deserialize as
  unsupported `custom` sessions instead of breaking session load. `hooks remove
  gemini` and `mcp remove gemini` remain available only to clean legacy
  ForkTTY-managed entries from `~/.gemini/settings.json`.

## [0.2.0-alpha.14] - 2026-06-19

### Fixed
- Embedded Ghostty terminal panes no longer force GTK's `cairo` software
  renderer, which leaked multi-GiB of live, `malloc_trim`-immune heap while
  compositing the embedded Ghostty `GtkGLArea` every frame (most visible during
  long full-screen agent TUI sessions such as Codex; idle and standalone
  Ghostty were unaffected). ForkTTY now defaults `GSK_RENDERER` to the GL
  renderer (`ngl`), which composites the GLArea natively with no such growth.
  An explicit `GSK_RENDERER` override is still honored for QA/debugging.
- Embedded Ghostty panes no longer cap their redraw tick at ~10fps during
  continuous output. That 100ms floor was a throttle against the old cairo
  software-renderer leak; with the GL renderer default it only added latency,
  so the tick now follows the 16ms wakeup-check cadence and GTK's frame clock.
- AppImage packaging now bundles and verifies the runtime dependencies of the
  embedded Ghostty GTK library itself, not only the main ForkTTY binary, so
  GitHub-built AppImages do not ship smaller artifacts whose terminal panes
  fail to start on distributions missing those libraries system-wide.
- Embedded Ghostty panes now follow Ghostty's `scrollback-limit` config, falling
  back to Ghostty's bounded default budget (10 MB per surface) instead of
  disabling retained history, so mouse-wheel scrollback works in freshly opened
  terminal panes.
- Embedded Ghostty panes now pack the raw Ghostty surface inside a GTK scrolled
  window and honor Ghostty's `scrollbar = system|never` config, so retained
  scrollback has the same visible vertical scrollbar behavior as standalone
  Ghostty.
- Agent health, explicit resume, and restore-time auto-resume now preserve
  hook-reported `bypassPermissions` sessions for Codex and Claude Code by
  rebuilding argv with the providers' documented dangerous-mode flags
  (`codex --dangerously-bypass-approvals-and-sandbox resume ...` and
  `claude --dangerously-skip-permissions --resume ...`) instead of silently
  resuming those panes back in prompted mode.

### Security
- Ordered embedded Ghostty AppImage environment unsets before assignments so
  `/usr/bin/env` cannot treat a cleanup flag as the terminal child command.
- Rejected control characters in restored session identifiers and embedded
  Ghostty command-spawn values so tampered session state cannot influence
  terminal child argv or environment setup.
- Hardened embedded Ghostty GTK library loading by canonicalizing candidate
  paths and rejecting relative paths, non-regular files, untrusted ownership,
  or group/other-writable files and parent directories before `dlopen`, while
  allowing packaged AppImage libraries below sticky `/tmp` mounts.
- Limited OSC99 terminal notification icon dimensions before decoding or
  forwarding image data to desktop notification servers.
- Kitty image snapshots now copy and downsample only the rendered pixel
  footprint for each placement and use fallible render-buffer allocation,
  preventing malicious terminal output from forcing full-source-image
  per-placement copies on every redraw.

### Changed
- Settings and newly saved configs no longer expose Ghostty-owned terminal
  appearance/runtime controls (`font_family`, `font_size`, `scrollback_lines`,
  `terminal_audible_bell`, `terminal_renderer`, `terminal_theme`). Those TOML
  keys still load for compatibility, but ForkTTY now leaves font, theme, bell,
  and terminal rendering behavior to Ghostty's config and keeps only
  ForkTTY-owned settings in the visible UI.
- ForkTTY documentation, package metadata, first-run privacy link, and the
  anonymous telemetry endpoint now use the canonical `https://forktty.dev`
  domain.
- Documented the `.deb` runtime baseline as Debian 13/Trixie+ and Ubuntu
  24.04 LTS+, and added a repeatable `piuparts` install/purge release check.
- Clarified privacy/security documentation around the anonymous daily ping,
  update checks, and packaged source/license availability.
- App dialogs now use tighter shared spacing, shorter copy, and calmer inline
  actions across the command palette, shortcuts, worktree, and notification
  panels.
- The Agent HUD now uses a calmer dense-card layout with compact actions,
  subtler status pills, and terminal-like output snippets.
- The About dialog now uses a more compact identity layout, calmer metadata
  rows, and lighter action buttons.
- App chrome now uses quieter top/status bars, subtler split-pane focus,
  softer per-pane tabs, and less intrusive pane/browser toolbars.
- Sidebar navigation, popovers, badges, and terminal empty/error states now use
  calmer density, shorter copy, and less dominant status styling.
- Settings now use a clamped, denser layout with calmer inline actions, and the
  Agents page presents one recommended integration action plus advanced
  per-component actions.
- The welcome dialog's agent setup action now opens Settings directly on the
  Agents page, so first-time setup shows installed/update state before writing
  provider configuration files.
- Settings no longer exposes a shell editor; advanced users can still set
  `general.shell` manually in `config.toml`, while the dialog focuses on
  ForkTTY-owned behavior and appearance.

### Fixed
- Embedded Ghostty panes now disable cursor blinking in the embedded runtime
  and drain Ghostty's app mailbox from a wakeup callback instead of polling
  `ghostty_gtk_context_tick()` while idle, preventing background memory growth
  even when no terminal output is being rendered.
- Embedded Ghostty panes now coalesce continuous wakeups before ticking
  Ghostty's GTK app mailbox, avoiding redundant GTK runtime work during bursty
  terminal output.
- AppImages now prefer the bundled `usr/lib/ghostty-gtk-embed.so` before any
  development checkout under `vendor/ghostty`, so package smoke tests and user
  runs exercise the same embedded Ghostty library unless explicitly overridden
  with `FORKTTY_GHOSTTY_GTK_LIB`.
- First-launch onboarding now describes the default workspace as the user's
  home directory instead of the process current directory, matching the actual
  startup behavior.
- AppImages now bundle `libgtk4-layer-shell.so` beside the embedded Ghostty
  GTK library, and Debian packages now declare `libgtk4-layer-shell0`, so
  terminal panes can start on systems such as Fedora that do not have
  gtk4-layer-shell installed globally.
- AppRun now always uses the bundled GTK/libadwaita userspace stack, while
  still leaving glibc, fontconfig/text shaping, display-server libraries, and
  GPU drivers host-side, so AppImages no longer depend on host GTK packages.
- Debian and AppImage packaging now include ForkTTY copyright/license text and
  third-party notices for the vendored Ghostty runtime artifacts.
- Embedded Ghostty terminals launched from the AppImage no longer leak the
  AppImage runtime environment (`LD_LIBRARY_PATH`, `APPDIR`/`APPIMAGE`/`OWD`,
  and GTK/GObject module search paths) into spawned children, so a child
  process such as git, an editor, or an agent links against the host's
  libraries instead of the AppImage's bundled copies. `XDG_DATA_DIRS` is left
  intact because Ghostty's own shell integration depends on it.
- AppImage and Debian packages now include Ghostty's bundled themes, so
  embedded Ghostty panes can resolve user configs such as
  `theme = Catppuccin Mocha` instead of falling back to the default colors.
- Embedded Ghostty panes now keep the cursor blink timer disabled while the
  rendered terminal state uses a steady cursor, preventing idle OpenGL redraws
  from steadily growing RSS for Ghostty configs such as
  `cursor-style-blink = false`.
- The embedded Ghostty GTK library probe now builds Ghostty with the stable
  `ReleaseSafe` optimization profile and a linker-compatible Blueprint helper,
  avoiding local Zig/GCC `.sframe` linker failures and `ReleaseFast`
  startup crashes when running ForkTTY from cargo.
- Refined the per-pane tab close button styling so hover changes only the X
  color instead of drawing a filled control around it.
- Embedded Ghostty panes now focus the terminal's internal focusable widget
  after new workspace, tab, split, or pane-header selection, so typing reaches
  the newly focused pane without an extra click inside the terminal.
- Dialogs now handle `Escape` in capture phase, so command palette and other
  dialogs close even when a search field or text entry has focus.
- Embedded Ghostty panes now use a native command-spawn ABI when the bundled
  library supports it, so per-surface `FORKTTY_*` environment setup no longer
  appears as a typed `exec /usr/bin/env ...` command in every new workspace.
  Older embedding libraries now start Ghostty's default shell without the
  ForkTTY environment instead of typing a bootstrap command into the terminal.
- Embedded Ghostty socket text reads now use a bounded GTK ABI when the
  embedding library supports it, so `read_text`/`capture_tail` requests do not
  ask Ghostty to materialize unbounded scrollback in ForkTTY before response
  truncation.
- Embedded Ghostty scrollback tail capture (`tail_text`) now returns an empty
  string for a zero-column terminal instead of erroring on an out-of-bounds grid
  reference, matching the existing guard in `visible_screen_rows`.
- Failed `agent.hibernate` close attempts now restore the previous unread bit
  and status entry instead of leaving the running surface shown as suspended.
- Pending embedded Ghostty spawns now lose their orphan-reaper protection
  after one reconciliation if their backend surface never appears in the
  model, preventing hidden PTY/widget processes from surviving a
  spawn/model-removal race.
- Bounded the `forktty remote-helper pty` stdin relay queue so
  non-draining PTY children apply backpressure instead of allowing unbounded
  memory growth.
- Feed approval rows now use durable notification feed IDs and avoid carrying
  approval decisions across newer entries with reused transient notification
  IDs.
- The socket CLI now writes all success output through the broken-pipe-aware
  writer, so piping a command into a consumer that closes early (for example
  `forktty ping | head -1`) terminates cleanly instead of panicking on the
  closed pipe.
- List socket methods (`team.list`, `team.inbox`, `team.events`,
  `workflow.list`, `workflow.replay`) now clamp an explicit `limit` to 10000,
  matching the existing browser-history cap, so no single request can ask for an
  unbounded result set.
- Ghostty Nushell shell integration now imports the bundled `ghostty.nu` module by absolute path and skips injection when that trusted module is missing, preventing workspace files from shadowing the startup import.

### Added
- Settings now includes an Agents page for installing or refreshing ForkTTY
  agent hooks and the local MCP bridge after first launch. ForkTTY also
  auto-refreshes already-managed hook/MCP entries on startup when a new build
  would update them, while leaving first-time setup explicit.
- Embedded Ghostty panes now snapshot their scrollback tail into
  the session (on child exit, on programmatic close/restart, and via a throttled
  poll) when
  `appearance.persistent_scrollback_lines > 0`, matching classic panes, so a
  later session save keeps recent embedded output. Restoring that scrollback on
  respawn/session restore is wired through an optional
  `ghostty_gtk_surface_restore_scrollback` embedding ABI (feeds Ghostty's VT
  stream, never the child PTY). The pinned Ghostty fork now exports the symbol
  (an IO-thread `inject_output` mailbox message routed to
  `Termio.processOutput`), so an embedding library built from the current pin
  restores embedded scrollback; libraries built before it degrade to a safe
  no-op. The Ghostty GTK Probe now builds the embedding `.so` on Ubuntu and
  verifies an embedded pane restart preserves a pre-restart scrollback marker in
  `capture_tail`. See `docs/ghostty-renderer-embedding-spike.md` for the
  Ghostty-side design.
- Bumped the vendored Ghostty pin to
  `31da31b65d1011b59e40932cd2b81cb9c69556bd`, which adds the
  `ghostty_gtk_surface_restore_scrollback`,
  `ghostty_gtk_surface_new_with_working_directory_and_command`, and
  `ghostty_gtk_surface_read_text_limited`,
  `ghostty_gtk_surface_new_with_working_directory_command_and_scrollback_limit`
  GTK embedding ABIs.
- ForkTTY now pins the full upstream Ghostty source as `vendor/ghostty` for the
  cmux-style renderer/widget integration path; release builds package the
  vendored Ghostty GTK embedding library for the default pane renderer.
- The Ghostty renderer spike is now documented: upstream's current public C
  surface embedding API is macOS/iOS-only, so ForkTTY's next renderer step is a
  minimal Ghostty-side GTK widget embedding API instead of more parity shims.
- `scripts/ghostty-gtk-build-probe.sh` now records the reduced upstream Ghostty
  GTK build used before attempting the Linux renderer embedding patch.
- A manual `Ghostty GTK Probe` GitHub Actions workflow can run that upstream
  Ghostty GTK build on Ubuntu without blocking the normal ForkTTY CI.
- `forktty ghostty-gtk-probe` can auto-exit with
  `FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS`, and the manual Ghostty GTK Probe
  workflow now smoke-tests the Rust GTK widget bridge under Xvfb after building
  the vendored Ghostty GTK embedding library.
- The vendored Ghostty GTK embedding library now avoids standalone-app theme
  startup when registered inside ForkTTY's host GTK application.
- The vendored Ghostty GTK embedding ABI now returns a sunk full widget
  reference so the Rust probe can parent the surface without premature dispose.
- The vendored Ghostty GTK embedding context now initializes Ghostty's GTK app
  state in-place so internal runtime pointers stay valid after context setup.
- ForkTTY can pack the vendored Ghostty GTK widget into terminal panes after
  `ghostty-gtk-embed.so` has been built; this is now the default renderer path,
  and terminal spawn records an error instead of falling back to the classic
  GTK/Pango/Cairo renderer if the embedded library cannot be loaded or a
  surface fails to spawn.
- The vendored Ghostty GTK embedding ABI can create surfaces with a working
  directory override so embedded Ghostty panes start in the ForkTTY
  surface cwd.
- The embedded Ghostty GTK pane mode can now forward ForkTTY socket
  `send_text` input to embedded Ghostty surfaces after the Ghostty core surface
  is initialized.
- The embedded Ghostty GTK pane mode can now service ForkTTY socket
  `read_text` and `capture_tail` requests by reading visible/full text through
  the vendored Ghostty GTK embedding ABI.
- Release CI now requires `ghostty-gtk-embed.so` before packaging, so the deb
  and AppImage ship the embedded Ghostty library for the default renderer path.
- The embedded Ghostty renderer is now the only terminal pane renderer in the
  GTK runtime. The temporary alpha `appearance.embedded_ghostty` opt-out key is
  accepted on load for compatibility but omitted from new saves and ignored at
  runtime. `forktty doctor` flags a missing embedding library because terminal
  panes cannot open without it.
- Embedded Ghostty panes now wire surface lifecycle into the
  ForkTTY model: title changes mirror into the model, child-process exit drops
  the surface from the ready set, sets a closed/`Exited (n)` status, and raises
  an abnormal-exit notification, and a Ghostty close-request tears the pane down
  cleanly so no stale pane is left behind. The embedding ABI gains
  `ghostty_gtk_surface_exit_code` so embedded panes report the real exit status;
  older libraries without the symbol fall back to the neutral "Closed".
- Embedded Ghostty panes now reach copy/paste/select-all/find
  parity with classic panes: the `Ctrl+Shift+C/V/A/F` accelerators (and the
  command palette equivalents) route to the focused embedded surface instead of
  no-opping, with find opening Ghostty's native search overlay. The embedding
  ABI gains `ghostty_gtk_surface_perform_action`, which performs a Ghostty
  keybinding action by name (e.g. `copy_to_clipboard`, `start_search`); older
  libraries without the symbol degrade to a logged no-op. Mouse selection
  already works natively inside the embedded surface.
- Embedded Ghostty panes now route ForkTTY zoom actions to
  Ghostty's native font-size actions, so `Ctrl+plus`, `Ctrl+minus`, reset zoom,
  and command-palette zoom affect embedded panes as well as classic panes.
- Embedded Ghostty panes now have child-PID ABI plumbing for
  listening-port discovery and the socket `surfaces` PID field. The embedding
  ABI gains `ghostty_gtk_surface_child_pid`, fed by a new `pid_available`
  surface mailbox message that hands the IO-thread-owned pid to the GTK main
  thread race-free; ForkTTY polls the getter briefly after spawn to record the
  PID, with a Linux direct-child process fallback while the embedded surface
  finishes startup, and the ABI falls back to Ghostty's PTY foreground PID while
  the mailbox value is not yet visible.
- The socket `surface.list` result, and therefore `forktty surfaces --json`,
  now includes live runtime fields (`shell`, `cols`, `rows`, and `pid` when
  known) in the same rows as the model metadata.
- Embedded Ghostty panes now back ForkTTY's Agent HUD tail reads
  and inline agent replies, so agent surfaces keep showing recent output and
  accepting panel replies when the embedded renderer is enabled.
- Embedded Ghostty panes now handle ForkTTY's reset/clear command
  by routing it to Ghostty's native `clear_screen` keybinding action. This
  clears the embedded surface through Ghostty; a full terminal-state reset
  still depends on Ghostty exposing a dedicated keybinding action.
- The Ghostty GTK Probe now requires the embedded `exit_code`, `child_pid`, and
  `perform_action` ABI symbols, and its smoke test verifies socket
  `capture-tail`, embedded pane startup, and that panes expose a positive PID
  through `forktty surfaces --json`. It also verifies that a clean child exit
  marks the pane non-writable with a `Closed` status.
- The deb and AppImage packagers now require `ghostty-gtk-embed.so` in
  `vendor/ghostty/zig-out/lib` and install it into `usr/lib`, so installed
  builds load the embedded Ghostty library via the binary RUNPATH
  (`$ORIGIN/../lib`) without needing `FORKTTY_GHOSTTY_GTK_LIB`.
- Team orchestration state is now available as a provider-neutral control
  plane through `team.*` socket methods, `forktty team-*` CLI commands, and MCP
  tools, covering leader/worker metadata, task DAGs, mailbox messages,
  heartbeats, provider worker launch into tabs, pane dispatch confirmations,
  worker health/lifecycle snapshots, idle nudges, safe shutdown requests,
  summaries, and event polling without adding parity UI yet.
- `agent.hibernate`, `agent.reclaim`, `forktty hibernate-agent`,
  `forktty reclaim-agents`, and MCP `agent_hibernate`/`agent_reclaim` can now
  close idle, locally resumable agent terminal processes, mark their persisted
  sessions `suspended`, and leave them resumable through the existing
  `agent.resume` path without adding parity UI panels.
- Workflow control-plane methods (`workflow.list`, `workflow.get`,
  `workflow.upsert`, `workflow.plan.set`, `workflow.evidence.add`,
  `workflow.replay`) plus `forktty workflows`/`workflow-*` CLI commands and MCP
  tools now persist bounded goal, mode/session memory, plan, evidence, and
  replay events without adding parity UI panels.
- Repo-local `forktty.json` project actions can now be listed and launched
  through `project.action.list` / `project.action.run` and the `forktty actions`
  / `forktty action-run` CLI. Actions are argv-only and limited to git repos
  already open in ForkTTY.
- `feed.list` and `forktty feed` now expose a minimal read-only feed snapshot
  that normalizes current notifications, approval prompts, status, and progress
  without adding durable feed history yet.
- Feed history now persists bounded notification, approval, status, and progress
  events to `feed.json`; `feed.approval.respond` / `forktty feed respond` can
  mark approval rows approved or denied for later workflow consumers.
- `forktty top` / `system.top` now return a read-only workspace and surface
  health snapshot with focus, unread, kind, cwd, shell, size, PID when known,
  agent lifecycle, status, and progress fields.
- `remote.list` / `remote.status`, `forktty remotes` /
  `forktty remote-status`, and MCP `remote_list` / `remote_status` now expose
  read-only SSH workspace inventory and connection state without adding a
  remote daemon yet.
- `forktty remote-helper hello` now prints a one-shot stdio JSON handshake for
  future SSH-launched remote helpers.
- `forktty remote-helper pty -- <program> [args...]` now runs an argv command
  under a PTY and relays stdin/stdout bytes over stdio as the first remote
  helper PTY path.
- `appearance.persistent_scrollback_lines` can opt into saving a bounded
  plain-text terminal tail per surface and restoring it with the session.
- OSC 99 terminal notifications can now keep activation/close report metadata,
  icon names, bounded icon data cache entries, and expose basic same-id buttons
  in the in-app notification panel.

### Changed
- Embedded Ghostty panes are now the sole terminal renderer path in the GTK
  runtime; failed embedded startup records a terminal spawn failure instead of
  falling back to the classic renderer.
- Settings no longer exposes terminal font family, font size, or terminal palette controls; GTK terminal panes now read font, color, and `scrollback-limit` appearance from Ghostty's config, including `config-file`, `theme`, named colors, and ANSI palette entries, while legacy ForkTTY appearance keys are loaded only for compatibility and omitted from new saves.
- Repeated Ghostty `font-family`, `font-family-bold`, `font-family-italic`, and `font-family-bold-italic` entries now build Pango fallback lists, and empty entries reset each list.
- Ghostty `font-feature` and `font-variation*` entries now apply to GTK terminal text through Pango.
- Ghostty `cell-foreground`/`cell-background` cursor and selection color references, plus legacy `cursor-invert-fg-bg` and `selection-invert-fg-bg`, are now honored by GTK terminal panes.
- Ghostty `bold-color` and legacy `bold-is-bright` are now honored by GTK terminal panes, including bright ANSI mapping for bold base-color text.
- Ghostty `cursor-opacity` now controls the GTK terminal cursor overlay.
- Ghostty `cursor-style` and `cursor-style-blink` now seed the GTK terminal
  cursor default for DECSCUSR-backed cursor styles.
- Ghostty `faint-opacity` now controls SGR faint text opacity in GTK terminal panes.
- Ghostty `selection-clear-on-typing` now controls whether typing after a
  scroll-to-bottom keeps or clears a finished terminal selection.
- Ghostty `selection-clear-on-copy` now controls whether copying clears the
  finished terminal selection.
- Ghostty `selection-word-chars` now controls double-click word boundaries in
  libghostty word selection and GTK fallback selection.
- Ghostty `clipboard-trim-trailing-spaces` now trims trailing whitespace from
  copied terminal lines.
- Ghostty `clipboard-codepoint-map` now maps configured Unicode codepoints or
  ranges while copying terminal text.
- Ghostty `copy-on-select` now controls selection publication: default/`true`
  keeps PRIMARY selection behavior, `false` disables it, and `clipboard`
  publishes to both PRIMARY and the regular clipboard.
- Ghostty `right-click-action` now controls terminal right-click behavior for
  context menu, copy, paste, copy-or-paste, and ignore.
- Ghostty `scroll-to-bottom` now controls whether input and/or new output snap
  the terminal viewport back to the bottom.
- Ghostty `mouse-reporting = false` now keeps mouse press/release/motion/scroll
  local even when terminal applications request mouse tracking.
- Ghostty `mouse-shift-capture` now controls whether Shift+click stays local
  for selection or can be forwarded to mouse-tracking applications, including
  XTSHIFTESCAPE runtime overrides for `true`/`false`.
- Ghostty `mouse-hide-while-typing` now hides the GTK terminal pointer after
  user typing or paste until mouse movement restores it.
- Ghostty `mouse-scroll-multiplier` now controls GTK terminal precision and discrete scroll distance.
- Ghostty `font-style*` and `font-synthetic-style` now control GTK terminal style selection and fallback synthesis.
- Ghostty `adjust-cell-width` and `adjust-cell-height` now adjust GTK terminal cell metrics using pixel or percentage values.
- Ghostty text metric adjustments now affect GTK terminal text baseline,
  underline/strikethrough/overline position and thickness, and cursor thickness/height.
- Ghostty `unfocused-split-opacity` and `unfocused-split-fill` now control ForkTTY's inactive split dim overlay.
- Terminal panes now support runtime zoom with `Ctrl++`/`Ctrl+=`, `Ctrl+-`, and `Ctrl+0` without adding persistent font settings.
- Terminal child shells now use Ghostty shell-integration resources when available, including upstream zsh/bash/fish/elvish/nushell startup injection and bundled Linux package resources.
- Linux packages now bundle Ghostty terminfo so packaged terminals can advertise `TERM=xterm-ghostty`.
- Ghostty-backed terminals now use Ghostty's 320MB Kitty image storage default instead of libghostty-vt's lower library default, enable Ghostty's file/temp/shared-memory Kitty image loading media, decode and draw Kitty PNG image uploads, and honor Ghostty `image-storage-limit`.
- Finished terminal selections now format their clipboard payload through `libghostty-vt`'s selection formatter, with the existing GTK frame extraction kept as a fallback.
- Double-click word selection now asks `libghostty-vt` for the word range first, falling back to the GTK frame logic when Ghostty has no selectable word.
- The GTK/Ghostty smoke script now verifies tab create/select/close, GTK action
  split/focus behavior, socket split readback, live pane close, restart with
  scrollback restore, and the socket notification create/list/clear flow.

### Fixed
- The `tree`/`topology-tree` and `team-worker-launch`/`team-worker-health`/
  `team-worker-nudge`/`team-worker-shutdown`/`team-message-dispatch` CLI
  subcommands (and their `:`/`.` spellings) now work when run as the first
  argument; previously `forktty tree` exited with `unknown argument: tree`
  even though the commands were dispatched and documented, because the CLI
  allow-list had drifted from the dispatch table.
- Embedded Ghostty surfaces (and per-pane action boxes) are no longer leaked
  when a pane is closed or restarted; their event-controller closures held a
  strong reference to the widget that owns the controller, forming a reference
  cycle that kept the surface alive forever.
- The durable workflow event store no longer wedges permanently when its
  on-disk `next_event_seq` is stale or absent (e.g. a hand-edited or
  partially-migrated file): event sequence numbers are now minted strictly
  above the highest existing event so saves keep validating.
- Per-workspace status and progress entries are now capped (like logs and
  notifications), so a misbehaving socket client posting many distinct keys can
  no longer grow ForkTTY memory without bound.
- The MCP `workflow_evidence_add` tool now rejects missing `kind`/`title`
  locally with a clear validation error, matching its published input schema
  and the socket server's requirements.
- Embedded Ghostty panes now start the requested `SpawnRequest` command and
  ForkTTY environment through Ghostty's native command-spawn ABI, so SSH panes,
  agent resume panes, configured shell arguments, and per-surface environment
  values no longer fall back to Ghostty's default shell in packaged builds.
- Embedded Ghostty panes now clear cached child PIDs when the child exits and
  ignore stale PID poll results after exit, close, or respawn, preventing exited
  panes from exposing or using reused PIDs for port discovery.
- `feed.list` now applies its response limit while reading live status and
  progress metadata, avoiding unbounded JSON aggregation and sorting under the
  workspace model lock when many metadata keys exist.
- ForkTTY and dialog titlebars now stay on the app's neutral dark chrome even
  when a GTK user theme defines a blue/purple titlebar color.
- Clicking a split pane's top bar now focuses that pane and routes subsequent
  input to it, matching clicks inside the embedded terminal area.
- Embedded Ghostty panes now intercept right-click before Ghostty's standalone
  window context menu opens, replacing the disabled native copy/paste entries
  with ForkTTY's context menu backed by the embedded Ghostty action ABI.
- Embedded Ghostty panes now capture ForkTTY terminal accelerators
  (`Ctrl+Shift+C/V/A/F`) on the pane wrapper before Ghostty's internal widget
  can consume the key event, so copy, paste, select-all, and find route through
  the embedded Ghostty action ABI reliably.
- Embedded Ghostty bounded text reads now use
  `ghostty_gtk_surface_read_text_limited` when the embedding library supports
  it, preventing long agent panes or socket reads from materializing unbounded
  scrollback before ForkTTY truncates the response.
- Embedded Ghostty panes now keep a just-spawned replacement surface protected
  from orphan reaping until the workspace model observes it, avoiding a race
  where socket-driven root-pane close/replacement could briefly close the new
  embedded shell as stale.
- Debian and AppImage packaging now invoke the shared Ghostty GTK library probe
  to build or locate `ghostty-gtk-embed.so`, verify the required embedded ABI
  symbols, and install the verified library into `usr/lib` before producing
  artifacts.
- The documented `forktty read-screen` and `forktty capture-tail` socket CLI
  commands now route through the top-level CLI instead of being rejected before
  reaching the socket client.
- Embedded Ghostty panes now apply Ghostty's GTK OpenGL context defaults before
  GTK initializes, disabling GLES/Vulkan context selection so packaged/AppImage
  panes do not show "Unable to acquire an OpenGL context for rendering" on
  affected GTK/driver combinations. The embedded Ghostty library also stays
  quiet on stderr by default unless `GHOSTTY_LOG` is explicitly set.
- GTK terminal panes now keep Ghostty steady cursor styles visible instead of
  hiding every focused cursor during the blink timer's off phase.
- Ghostty config and theme appearance loading now enforces the oversized-file guard before reading or applying colors.
- OSC 99 terminal notifications now decode `e=1` base64 title/body payloads and accumulate same-id `d=0`/`d=1` title/body chunks instead of dropping them as unsupported metadata.
- OSC 99 multipart title/body notifications now keep the title separate from later body updates instead of concatenating both into notification text.
- OSC 99 notification identifiers now follow the protocol identifier character set before ForkTTY tracks or echoes them in replies.
- OSC 99 payload types ForkTTY does not implement are now ignored instead of surfacing as terminal status text.
- OSC 99 terminal notifications with the same `i` identifier now update the existing ForkTTY notification, and `p=close` dismisses that tracked notification.
- OSC 99 single-part `p=title` notifications now use the payload as the notification title, same-id title/body updates preserve the prior title and terminal metadata, and no-id `p=close` closes the default `i=0` notification.
- OSC 99 terminal notification payloads, report ids, same-id routing entries, expiry timers, and icon data are now bounded before decode/allocation so malformed PTY output cannot grow memory indefinitely.
- OSC 99 desktop notifications now reuse the prior OS notification id on updates and close the tracked OS notification on `p=close` or in-app dismiss.
- OSC 99 notifications that request reports now send in-app Open/Dismiss/Clear All activation and close replies back to the source terminal.
- OSC 99 `n=` icon names now feed in-app and desktop notification icons instead of always showing the ForkTTY icon.
- OSC 99 `f=` application names now act as in-app and desktop icon fallbacks when no `n=` icon name is provided.
- OSC 99 `p=icon` binary payloads now render in the in-app notification panel when GTK can decode the image, respect `n=` icon-name precedence, and are passed to desktop notifications through bounded temporary image files under `$XDG_RUNTIME_DIR`.
- OSC 99 desktop icon data is now ignored unless it has a recognized PNG, JPEG, GIF, or WebP signature.
- OSC 99 `o=unfocused` and `o=invisible` notification occasions now suppress notifications for focused or already-visible terminal panes.
- OSC 99 `f` application name and `t` notification type metadata are now retained, can be blocked with `notifications.blocked_terminal_apps` / `blocked_terminal_types`, and are exposed to `notification_command` for external filtering.
- OSC 99 `u` urgency, `w` expiry, and base64 `s` sound metadata now feed desktop notification hints when supported, and positive `w` values also auto-dismiss in-app notifications.
- OSC 99 button payloads now accept base64 text and the spec's U+2028 separator.
- OSC 99 `p=?` and `p=alive` queries now reply to the source terminal with ForkTTY's supported notification capabilities and same-surface live notification ids.
- Sidebar badges, duplicate GTK spawns, closed-terminal status handling, corrupt tab leaves, and scrollback settings copy now handle stale or delayed terminal state without misleading UI or backend readiness loss.
- Persistent terminal scrollback is no longer deleted merely because restore is disabled, session saves now reject serialized data beyond the load cap, and scrollback snapshots are throttled while still flushing on child exit.
- Terminal panes now size rows from one shared widget-measured cell size plus a small vertical guard, preventing agent TUIs from being clipped after resizes without inflating terminal line spacing.
- Terminal styled text runs now fit the terminal cell grid, preventing colored inline-code spans from leaving visual gaps between words.
- Terminal text selection and mouse hit-testing now use GTK content-box coordinates directly, fixing selections that were offset from the pointer.
- Terminal shortcuts, Meta-key input, unread output tracking, OSC99 status updates, browser-pane cleanup, and uppercase HTTP(S) links now behave consistently from focused panes and delayed events.
- Terminal selection finalization now releases the render borrow before formatting through Ghostty, avoiding a RefCell panic when a pointer gesture is released or cancelled.
- Terminal drag selection now snaps at cell midpoints and preserves real one-cell drags, keeping highlights aligned with the pointer.
- Terminal selections now preserve selected whitespace, invalidate select-all payloads on new output, clear stale search highlights, keep adjacent OSC 8 links separated by URI, handle wide-character spacer tails, and avoid drawing scrollback indicators outside tiny panes.
- Terminal mouse release suppression is now tracked per button, avoiding spurious release forwarding during left/middle button chords.
- Agent HUD, workspace closing, worktree, restart, settings reset, and welcome telemetry flows now keep their captured context through delayed UI actions and failure paths.

## [0.2.0-alpha.13] - 2026-06-16

### Security
- Stop hooks now preserve agent permission-mode warnings when the provider has a later session-end cleanup, while providers without session-end hooks still clear the warning on final stop.
- Terminal smooth-scroll handling now rejects non-finite deltas and caps per-event line replay, preventing oversized synthetic scroll events from monopolizing the UI thread while mouse tracking is active.
- Browser automation now injects and evaluates its driver in an isolated WebKit script world, preventing visited pages from detecting or tampering with `window.__forktty`.
- Chromium cookie import now verifies the version-24+ encrypted host digest before accepting decrypted values, rejecting malformed or cross-host cookie rows.
- Agent resume PTY spawns now resolve bare provider commands using only absolute `PATH` entries before applying the recorded session cwd, preventing relative/empty `PATH` entries from executing project-local binaries during restore or resume.
- Socket hook correlation now rejects `hook_session_id` values larger than the metadata text limit before caching them, preventing a local client from retaining many near-request-size session IDs in memory.
- Socket-triggered notification dispatch and custom notification command reaping now use bounded queues instead of spawning unbounded OS threads per `notification.create`, preventing a local client from exhausting threads by flooding notifications.

### Added
- `forktty doctor` now accepts `--hooks`, `--socket`, and `--packaging` scopes for running only the relevant local diagnostics.
- First launch now shows a one-time welcome dialog: an informed (default-on) telemetry toggle linking to the privacy notice, and a one-click "Set up agent integration" button that runs `hooks setup` and `mcp setup`. The first anonymous ping is deferred until this dialog is dismissed, so the toggle is always seen before any data leaves the machine; the welcome is recorded in `$XDG_STATE_HOME/forktty/welcome-seen.json` and the update check is skipped on that first launch.
- The GTK app now sends at most one anonymous daily usage ping when `telemetry.anonymous_ping = true` (the default). The payload contains only schema/kind/app/version/date, can be disabled in Settings or config, and crash uploads remain unimplemented.
- The GTK app now checks GitHub Releases at most once per day when `updates.auto_check = true`, shows update availability in-app, opens the release page for non-AppImage installs, and can self-update writable AppImages after explicit confirmation by downloading the AppImage plus `SHA256SUMS`, verifying SHA256, and atomically replacing the current file.
- Release AppImages can now embed AppImage update information and ship a matching `.zsync` asset when `APPIMAGE_UPDATE_INFO=1` is used during packaging; release CI enables this and includes `.zsync` in `SHA256SUMS`.
- Agent HUD rows now include a Forget action with Undo, so stale tracked sessions can be removed from the HUD without closing the terminal or deleting provider data.
- Agent HUD rows now show an accent unread dot when an agent has produced output you have not viewed since last focusing it, and float those rows up within their lifecycle group — so a finished (idle) agent whose result is still unseen stands out instead of sinking to the bottom of the list.

### Fixed
- The terminal pane header now keeps the Close Pane button on the far right of split-pane headers instead of clustering it beside the split and new-tab actions.
- Worktree list/status reporting now treats registered worktree paths replaced by files, symlinks, or invalid directories as unknown instead of clean/dirty.
- Command palette and Settings selection states now use neutral row highlights instead of heavy accent rails.
- Settings toggles are now smaller and use a subtler checked state.
- Settings sidebar navigation is now more compact, with item descriptions kept in tooltips and accessibility labels instead of visible subtitles.
- Settings pages now use shorter page and section copy, hiding redundant section descriptions.
- Settings, Agent HUD, and Notifications now expose a maximize/restore titlebar control and enforce minimum window sizes, while Command Palette windows stay fixed-size.
- Settings now labels the privacy and reset page as Privacy instead of Advanced.
- Agent HUD now relies on its titlebar close button and Esc instead of showing a duplicate Close footer button.
- Agent HUD now opens shorter when sessions are present, keeps its empty state unclipped, uses flatter rows, and gives row actions clearer visual hierarchy.
- Worktree merge selected by worktree name now merges that worktree's branch even when an unrelated local branch shares the worktree's derived name (e.g. worktree `feat-a` for branch `feat/a` alongside a separate `feat-a` branch), matching the worktree the cleanliness check already validates instead of silently merging the wrong branch.
- Config loading now normalizes a `notification_command` that tokenizes to zero words (for example an inline shell comment like `"# disabled"`) to an empty command instead of rejecting it and quarantining the entire config, so a benign command value no longer resets every other setting to defaults.
- Nested worktree creation now appends `.worktrees/` to `.git/info/exclude` even when the existing exclude file contains non-UTF-8 bytes, instead of failing before creating the worktree.
- Agent session-end hooks now mark the persisted agent binding as ended when clearing its live status, so agents whose providers emit a session-end event no longer remain shown as running in the Agent HUD.
- Chromium bookmark import, browser bookmark loading, and browser profile metadata loading now reject or skip non-regular files before reading, preventing local FIFO/device paths from blocking the import or profile workflows.
- Socket metadata calls now reject stale explicit `surface_id` values even when `workspace_id` is valid, oversized request lines return the documented `payload_too_large` code, and invalid parameter errors use the documented `invalid_param` code.
- Config recovery now quarantines config paths that resolve to FIFOs without blocking application startup.
- Worktree create now propagates branch lookup errors other than `NotFound` instead of treating every libgit2 failure as a missing branch.
- Creating or attaching a worktree whose registration survived an external deletion of its working directory now prunes the stale registration and recreates the worktree in place, instead of failing with an unresolved-path error; `create` adopts the existing branch in this case and never deletes a pre-existing branch during cleanup.
- `ProfileStore::save` now creates the `browser_profiles` directory with owner-only (`0700`) permissions instead of inheriting the umask, so profile metadata is not world-readable on multi-user systems when the directory is first created via the socket API.
- `ProfileStore::save` now hardens an existing `browser_profiles` directory owned by the current uid/gid by removing group/other permission bits before writing profile metadata.
- Browser import spooling now uses anonymous (unlinked) temp files instead of named temp files, so spooled pre-read data is reclaimed automatically if the process is killed mid-import.
- Browser imports now spool pre-read source data to temporary files instead of retaining every selected profile in memory, preventing large all-source imports from exhausting memory while preserving all-or-nothing read validation before writes begin.
- Update checks now honor HTTP-date `Retry-After` headers from GitHub rate-limit responses instead of retrying before the requested deadline.
- Restarting an agent pane now resumes the agent session (provider resume argv and recorded cwd) instead of relaunching a plain shell, matching the session-restore and worktree spawn paths.
- Session restore now quarantines a session file containing invalid UTF-8 instead of returning an error that crashed startup on every launch.
- The first-run welcome dialog now stays open when persisting a telemetry opt-out fails (e.g. a read-only config directory), so it cannot silently fall back to the default-enabled ping without giving the user a chance to fix permissions or re-enable telemetry.
- Saving browser bookmarks now creates the profile directory with owner-only (`0700`) permissions instead of inheriting the umask, matching the history database directory, so bookmark URLs are not exposed when a profile's first write is a bookmark.
- Bookmark entries now bound the stored URL and title to the same size caps as history visits, preventing an oversized imported bookmark from growing `bookmarks.json` until it becomes unreadable.
- Shell trampoline detection now keeps scanning after shell options that take a value, so `bash -o vi -c ...` and `bash --rcfile file -c ...` notification commands are rejected instead of bypassing the `-c` guard.
- Shell trampoline detection now recognizes PowerShell's command grammar, so `pwsh -Command ...`, `pwsh -EncodedCommand ...`, and `pwsh -CommandWithArgs ...` notification commands (and their `-c`/`-e`/`-ec`/`-cwa` aliases) are rejected instead of bypassing the shell-command guard.
- `PtySession::read_until` now reports `UnexpectedEof` when a child exits before the requested bytes arrive, instead of returning partial output as success.
- `forktty hooks test` now sanitizes socket error text before rendering human-readable failures, preventing local socket responses from injecting terminal control sequences.
- Browser import is now limited to the in-app Settings workflow and is no longer advertised or accepted over the socket/CLI automation boundary, preventing local socket clients from using ForkTTY to read external browser profile data.
- Notification command validation now rejects `rbash -c` shell trampolines instead of allowing restricted Bash aliases to bypass the shell-command guard.
- Session restore now quarantines FIFO and other non-regular session paths without blocking application startup.
- Browser history now ignores oversized URLs and truncates oversized page titles before writing to SQLite, preventing web-controlled title or URL churn from causing unbounded history database growth.
- Browser imports now report oversized history URLs as skipped writes instead of counting them as imported rows.
- Browser automation CLI fill now supports `--value-file` (with `-` for stdin) so sensitive values do not have to be exposed in process arguments.
- Removed the raw `browser.eval` socket/CLI command so same-user socket clients can no longer execute arbitrary JavaScript inside browser panes.
- Agent HUD terminal-tail polling now formats only the bounded tail rows instead of dumping the full scrollback each second, preventing noisy agents from freezing the GTK UI while the HUD is open.
- Browser committed-URI synchronization now rejects URLs over the shared 8 KiB browser URL limit while preserving non-hierarchical URLs such as `about:blank`.
- OpenCode hooks now sanitize and size-bound plugin payloads before spawning the ForkTTY CLI, preventing oversized tool output from blocking or crashing the OpenCode process before the CLI stdin cap applies.
- Antigravity hook setup now hardens `~/.gemini`, the config directory, and generated wrapper directory to owner-only permissions before planning or writing executable hook scripts, preventing local users from replacing wrappers through group/world-writable directories.
- Browser profile storage directories are now created with private Unix permissions before WebKit persists cookies, local storage, and cache data.
- Terminal scrollback search now caps stored matches and shows a capped count, avoiding unbounded memory/CPU use on repetitive untrusted terminal output.
- MCP `surface_send_text` is now annotated as destructive and open-world, reflecting that terminal input can execute shell commands or interact with files and networks.
- Browser imports now read all selected source profiles before preparing destinations or writing data, avoiding partial imports when any later source is unreadable.
- Terminal-originated OSC 9/basic OSC 99 notifications are now rate-limited per surface, preventing untrusted terminal output from spamming desktop notifications or repeatedly spawning `notification_command`.
- Session locking now creates and hardens the state directory and lock file with private permissions, preventing other local users from reading or pre-locking `session.lock` to block startup.
- Atomic profile metadata saves now preserve an existing `profiles.json` file mode on Unix when ownership matches, and drop group/other bits when replacing with a temp inode owned by a different uid or gid.
- Browser history databases and SQLite WAL/SHM sidecars are now created with owner-only file permissions inside owner-only profile directories.
- Bookmark files and corrupt-bookmark backups are now saved with owner-only permissions to avoid exposing sensitive URLs to other local users.
- Stale Ghostty event batches from an old pane spawn are now discarded before they can mark a restarted pane not ready, overwrite its terminal status, or emit stale notifications.
- Browser imports now copy temporary SQLite databases and WAL/SHM sidecars into a private `0700` directory with newly-created `0600` files, preventing local temp-file races from exposing browser data.
- Browser-feature socket dispatch fuzz tests now isolate `XDG_DATA_HOME`, preventing adversarial method sweeps from clearing a developer's real browser history.
- `forktty events` now mirrors lag notices to stderr regardless of the JSON object key order used by the socket server.
- Browser import planning now lowercases Unicode titlecase profile names before matching and de-duplicating destinations, preventing duplicate-looking profile creation.
- Command palette shortcut searches such as `ctrl shift c` now match the intended shortcut instead of treating the key token as a fuzzy match against modifier text on earlier commands.
- Ctrl+keypad Home/End/PageUp/PageDown are now reserved for tab-navigation accelerators like their non-keypad equivalents instead of being consumed by terminal input handling.
- Retrying Worktree Create for an already-linked branch no longer deletes that existing worktree and branch if terminal spawning fails.
- Sidebar visibility persistence now rebases onto the latest config under a process-wide update lock, avoiding stale background saves that could overwrite newer settings-dialog changes.
- Malformed browser bookmark files are now moved aside after backup so repeated opens cannot create unbounded backup copies.
- `forktty doctor` once again exits 2 whenever the diagnostics report contains warnings, preserving the documented health-check behavior even without `--strict`.
- Ctrl+click terminal links now open only `http://` and `https://` targets, blocking terminal-controlled `file://` and custom URI handlers.
- AppImage self-updates now create downloaded replacement files with owner-only permissions before checksum verification, closing a local same-group temp-file tampering window under permissive umasks.
- Notification commands using SSH/mosh options that contain `-c` are no longer rejected as shell trampolines, and a ForkTTY binary built without `gtk-ghostty` now exits with failure when asked to launch the GTK app.
- Worktree and workspace rollback paths now close spawned replacement terminals instead of only forgetting bookkeeping entries, preventing untracked terminal processes after cleanup failures.
- OSC 8 hyperlink lookup now caps URI buffers at 8 KiB and fails closed for larger terminal-provided targets, avoiding attacker-controlled memory growth when resolving links.
- Panic logs are now created in a private state directory with owner-only file permissions, and older permissive logs are rotated before new panic entries are written.
- Terminal text snapshot truncation now treats a zero-byte internal limit as an empty, truncated result instead of disabling truncation.
- Terminal spawning now preserves non-UTF-8 working-directory bytes on Unix instead of converting the cwd through lossy UTF-8.
- Large PTY writes now keep waiting after `poll()` reports no writable fd before the per-write deadline, instead of treating the poll timeout as readiness.
- PTY `read_until` now reports `TimedOut` when the requested bytes do not arrive before its deadline.
- Metadata OSC parsing now aborts an unterminated OSC string on a bare `ESC`, so OSC 9 notifications and OSC 99 agent metadata that follow in the same PTY chunk are no longer swallowed.
- The Worktree dialog no longer overwrites a typed Create/Attach branch name when the asynchronous existing-worktree list finishes loading.
- Agent HUD Resume buttons are re-enabled after a failed resume attempt instead of staying disabled until the HUD is reopened.
- Releasing a terminal text selection after wheel-scrolling mid-drag now preserves the scroll-compensated selection endpoint unless the pointer actually moved.
- Bad config/session quarantine paths are now reserved atomically before rename, avoiding races between simultaneous ForkTTY instances.
- Update checks now strip only one leading `v` from GitHub release tags, so malformed tags like `vv1.2.3` are ignored instead of parsed as `1.2.3`.
- Worktree and branch names now reject leading dashes and control characters before reaching git APIs.
- OSC 8 hyperlink lookup now retries with a large enough buffer for long multibyte UTF-8 URIs.
- The MCP stdio server now returns a JSON-RPC parse error and continues after an invalid UTF-8 line instead of ending the session.
- Custom terminal theme colors are now re-applied when an OSC color reset (`OSC 104`/`110`/`111`) follows an aborted OSC sequence in the same output chunk; previously the reset was swallowed as payload of the aborted sequence and the pane kept the wrong colors.
- The MCP stdio server now reads incoming messages through a bounded buffer, so an oversized message is rejected at the 1 MiB limit without first allocating the entire message in memory.
- Clicking an unfocused terminal pane now focuses it *and* lets the same click start a text selection (or reach the application), instead of swallowing the first click so the drag was lost and had to be repeated.
- Scrolling the wheel or touchpad while dragging a selection now keeps the drag anchored to the same text, like drag-autoscroll already did, instead of silently dropping the in-progress selection; a finished selection is still cleared when the viewport scrolls.
- A terminal color reset (`OSC 110`/`111`/`104`) immediately followed by an explicit color set in the same output chunk now keeps the application's color, instead of clobbering it with the re-seeded theme color.
- Hook/MCP socket requests no longer reject a parameter sent as an explicit JSON `null` (e.g. `hook_session_id: null`) with a type error; `null` is now treated as absent, matching the numeric parameter handling.
- A completed worktree merge whose post-commit cleanup fails is now reported as success instead of failure, avoiding a retry that would create a duplicate merge commit.
- An agent hook event now still runs its later cleanup actions (clearing a stale status or permission marker) when an earlier action fails transiently, instead of stopping at the first error.
- The `appearance.terminal_renderer` validation error message now lists `vte`, which is an accepted value.
- Closing a terminal pane no longer risks freezing the UI: the dropped PTY session now reaps its killed child on a background thread instead of blocking the GTK main thread in `waitpid`, which a child stuck in uninterruptible sleep (D state on a dead NFS/FUSE mount) could otherwise wedge forever.
- The PTY read loop now retries a read interrupted by a signal (`EINTR`) instead of surfacing it as a spurious error on every pump tick.
- Large PTY writes now honor their overall deadline even when repeatedly interrupted by signals, instead of being able to retry indefinitely under a pathological signal rate.
- Socket `surface.read_text`/`surface.capture_tail` no longer block a tokio worker thread while waiting for the GTK main loop: the wait is offloaded via `block_in_place`, so many concurrent read requests (as agent hooks issue) can no longer starve the socket server and stall every other request.
- Removing a worktree now deletes its working-tree directory before deregistering it from git, so a failed directory removal leaves a recoverable (git-pruneable) registration instead of stranding the directory permanently with no way for git to find it.
- A failed fast-forward merge rollback now logs the underlying ref-reset/HEAD-restore errors instead of silently discarding them, making a wedged repository diagnosable.
- Re-running `worktree.create` for a branch that already has a ForkTTY-supported linked worktree now reopens that worktree instead of failing on the already-created branch, recovering the crash window between Git worktree registration and ForkTTY session persistence.
- Concurrent nested worktree creation now serializes updates to `.git/info/exclude`, keeping the `.worktrees/` entry idempotent.
- Closing a non-last tab now keeps the model locked through backend close and model removal, so concurrent UI/socket closes cannot observe a half-closed surface.
- Terminal copy, mouse selection, and Select All now omit invisible terminal cells, so escape-hidden text cannot be copied to the clipboard.

## [0.2.0-alpha.12] - 2026-06-13

### Added
- Terminal panes now show terse toasts for copy/paste failures and flash an accent border for visual bell events.
- Terminal panes now show a minimal overlay scrollback indicator while viewing history.
- Terminal content now has balanced 6px inner padding so text does not touch pane edges.
- Split terminal panes now dim unfocused panes slightly so the focused pane is easier to pick out.
- Ctrl+click opens links: OSC 8 hyperlinks and plain `http(s)://`/`file://` URLs in the output (also when wrapped across lines). Hovering with Ctrl held shows a pointer cursor and underlines the target.
- Middle-click pastes the PRIMARY selection (select text, middle-click to paste — the standard Linux flow); Shift+middle-click pastes even inside mouse-tracking apps.
- Shift+PageUp/PageDown page through the terminal scrollback, Shift+Home/End jump to its start/end; inside full-screen apps (vim, htop) the keys keep going to the app.
- Typing or pasting in a scrolled-up terminal now snaps the viewport back to the bottom, like other terminals; output arriving while you read scrollback still leaves the viewport where it is.
- Agent hook status updates now persist a per-surface agent session binding (`agent` + provider `session_id`) when `metadata.set_status` carries `hook_session_id`, plus the provider session cwd as `resume_cwd` when available, giving future resume work stable session state instead of a runtime-only hook cache.
- Persisted agent session bindings now carry lifecycle state (`running`, `idle`, `needs_input`, `ended`, or `unknown`) derived from hook events, giving future hibernation/reclaim work an explicit ended-vs-idle signal.
- Persisted agent session bindings now track hook-derived `last_activity_ms`, so automation can reason about idle age without scraping provider files.
- `agent.list`, `forktty agents`, and the read-only MCP tool `agent_list` now expose persisted per-surface agent session ids, resume cwd, lifecycle, and last activity, so resume/HUD automation can discover Codex/Claude/Gemini/OpenCode/Antigravity sessions without scraping `session-v2.json`.
- `agent.health`, `forktty agent-health`, and MCP `agent_health` now report whether persisted agent sessions have a supported argv-only resume command and provider executable on PATH before attempting a resume.
- `agent.reclaim.plan`, `forktty agent-reclaim-plan`, and MCP `agent_reclaim_plan` now provide a read-only reclaim plan that classifies old idle, locally-resumable agent sessions as candidates and protects running/input-needed/ended/recent/not-ready sessions with explicit reasons.
- `agent.resume`, `forktty resume-agent`, and MCP `agent_resume` now resume a persisted Codex, Claude Code, Gemini, OpenCode, or Antigravity session in a new ForkTTY tab using provider-specific argv-only commands.
- Restored terminal surfaces with a persisted supported agent session now respawn through the provider's argv-only resume command instead of opening a plain shell after a ForkTTY restart; Codex sessions with a persisted hook cwd, or a cwd found in Codex's local `session_meta` JSONL, use `codex resume -C <cwd> <id>` to avoid Codex's resume-directory prompt when the pane cwd differs from the session cwd, and providers without a cwd flag such as Claude Code are spawned with the recorded cwd as their process directory.
- `status.summary`, `forktty statusline`, and MCP `status_summary` now provide a compact read-only workspace summary with persisted agent sessions, status entries, and progress entries for agent statusline/HUD integrations.
- The GTK app now has an Agent HUD in the titlebar and command palette, showing persisted agent sessions across workspaces with lifecycle, last activity, cwd/session context, needs-input highlighting, focus, and resume actions.
- The Agent HUD updates live while open (one-second model re-snapshot that rebuilds rows only when they changed), shows each agent's last terminal output line refreshed in place (generation-gated so idle agents cost nothing), and its rows are keyboard-activatable — Enter or a click on a row focuses that agent's pane.
- Agent HUD needs-input rows now show what the agent is actually waiting on (the hook prompt message, e.g. a permission request) instead of the raw terminal tail, and gain an inline reply entry that types the answer (plus Enter) straight into the agent's terminal without leaving the HUD; the list never rebuilds while a reply is being typed.
- The MCP server now exposes a `forktty://agent/operating-guide` resource and a `forktty_operating_guide` prompt, plus matching initialize instructions, so agents can discover when ForkTTY tools are useful and when to keep working normally.
- `surface.read_text`, `surface.capture_tail`, `topology.tree`, CLI `forktty read-screen`/`capture-tail`/`tree`, and MCP `surface_read_text`/`surface_capture_tail`/`topology_tree` now give agents read-only terminal inspection primitives before they focus or drive another pane.
- `forktty --json hooks doctor <agent>` and `forktty --json hooks test <agent>` are now a stable machine-readable API (documented in SPEC.md): versioned report with an overall `ok`, per-method `{method, ok, error?}` results for `hooks test` (which keeps running after a failed method instead of aborting, so cleanup still happens and the report is complete), and exit code 0/1 reflecting overall health for CI gating. The Codex trust state stays a first-class field of the doctor report.
- The worktree open-workspace boundary rejection now carries the structured error code `precondition_failed` (documented in SPEC.md), and MCP tool errors with a known recovery carry machine-readable `remedy` and `suggested_tool` fields in `structuredContent` — the boundary error points at `workspace_create`, so an agent can recover without parsing prose.

### Changed
- Touchpad scrolling in terminal panes now accumulates smooth deltas instead of forcing chunky wheel ticks.
- The declared Rust MSRV is now 1.96, matching the current `rusqlite`/`libsqlite3-sys` dependency chain required by the workspace lockfile.
- The competitive gap inventory now includes the non-browser cmux gaps plus the additional control-plane gaps found in oh-my-codex and oh-my-claudecode: workflow state/artifacts, team runtime, HUD/statusline export, and agent/skill catalogs.
- Gemini CLI integrations are now legacy opt-in: default `forktty hooks setup` and `forktty mcp setup` skip Gemini and prefer Antigravity, while explicit Gemini setup/remove/doctor/test and persisted Gemini resume compatibility remain supported.
- Claude Code session-start context now includes the same concise ForkTTY tool-use policy as the MCP operating guide: use ForkTTY for panes, agents, worktrees, status, or cross-surface text, but avoid tool calls for ordinary single-repo edits.

### Fixed
- Agent resume now treats hook-reported permission modes as display-only metadata, so forged or stale `bypassPermissions` hook/status updates cannot add dangerous Claude Code or Codex resume flags.
- Copying a soft-wrapped line (a long command or paragraph the terminal wrapped to fit the width) no longer inserts a spurious newline at each wrap point: selection copy, the no-selection viewport copy, and select-all now rejoin soft-wrapped rows into their logical line, so pasting a wrapped command back into a shell runs it as one line instead of splitting it.
- Selecting text by clicking an unfocused terminal pane no longer leaves the selection stuck following the pointer: the focus gesture claiming that first click used to cancel the selection drag without a release, stranding the `selecting` flag, so the highlight kept extending on every mouse move with no button held. The drag now finalizes on gesture cancel and whenever button 1 is no longer physically down.
- Dragging a left-click selection in an agent pane (deferred local drag) no longer aborts the app with a `RefCell already borrowed` panic in the motion handler.
- Terminal pane edge polish: double/triple-clicks and Ctrl+click/Ctrl+hover a few pixels into a pane's trailing gutter now select the last row/word and resolve last-row links instead of doing nothing; the pane being searched no longer dims while its search entry holds focus; middle-click or paste with an empty clipboard no longer shows a spurious "Paste failed" toast; and copying with an active selection still works when the terminal backend fails to render a frame.
- Wayland touchpad scrolling in terminal panes no longer overscrolls roughly 30x: smooth-scroll deltas arrive in surface (logical pixel) units on Wayland and are now converted through the pane's cell height, while X11 smooth deltas and wheel ticks keep the 3-lines-per-tick mapping.
- Scrolling inside mouse-tracking applications (vim, htop, tmux) now forwards one wheel press per three accumulated lines — matching physical-wheel speed — instead of one press per line, and hi-resolution wheels' fractional ticks accumulate into whole presses/lines instead of overscrolling on every fraction of a notch.
- Mouse events in terminal padding now clamp to the nearest grid edge instead of reporting past the last cell to mouse-tracking applications.
- Terminal copy failures no longer clear the existing clipboard contents, and the scrollback indicator no longer risks a panic during very small transient GTK allocations.
- Terminal resize no longer aborts inside libghostty when GTK briefly reports a one-row allocation after wrapped output.
- Terminal resize no longer aborts inside libghostty when maximizing a window with wrapped scrollback; the vendored Ghostty build now uses a temporary cursor-preservation pin and bounded wrap-count walks during column reflow.
- Terminal mouse selection, Ctrl+click link detection, and mouse-tracking coordinates now account for the terminal widget's CSS padding, so highlighted/copied text lines up with the visible character grid.
- Agent terminal panes now keep plain clicks working in mouse-tracking TUIs while treating a real left-button drag as local ForkTTY text selection, so Claude Code/OpenCode-style panes can select text without holding Shift.
- Antigravity `PreToolUse` hooks now return an explicit `{"decision":"approve"}` response, and generated wrapper fallback scripts do the same, so ForkTTY status hooks no longer make `agy` deny every tool call when the hook response is parsed strictly.
- Antigravity agent resume metadata now uses the hook payload's `workspacePaths` instead of the generated wrapper script cwd (`~/.gemini/config`), so `agent-health` reports the real project directory after `agy` publishes a new hook event.
- Session restore path repair now uses the pane tree, not stale persisted surface metadata, to choose the owning workspace directory for browser/SSH surfaces whose saved cwd no longer exists.
- Shell-trampoline detection for `notification_command` now catches `env -u VAR sh -c ...`, `env --unset=VAR sh -c ...`, and `env -S "sh -c ..."` wrappers instead of only plain `env sh -c ...`.
- Closed terminal panes no longer stay alive through GTK controller/search/context-menu reference cycles, so their PTY child and UI timers can be dropped.
- Worktree merge failures now restore the checkout before returning an error, including failed fast-forward ref updates and failures after `repo.merge()`. If the merge commit was already created, a recovered finalization error no longer reports the merge as failed.
- Large PTY writes now retry `poll()` interrupted by signals instead of treating `EINTR` as a fatal partial-paste error.
- Config recovery no longer quarantines a valid config file on transient I/O errors such as permission/read failures.

## [0.2.0-alpha.11] - 2026-06-11

### Added
- MCP tools now declare spec tool annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) so clients can surface risk before invoking: list/status tools are read-only, `worktree_remove` is flagged destructive, `status_set`/`surface_focus` are idempotent, and every tool is closed-world (local instance only).
- New MCP tool `workspace_create` (working_dir + optional name): agents can open a workspace on a repository themselves, which is the precondition the worktree tools enforce — precondition error, `workspace_create`, retry, without leaving MCP.

### Changed
- Every socket CLI subcommand now answers `--help` with its accepted options (generated from the same allow-list validation uses), and `create-workspace` accepts `--cwd` as an alias for `--working-dir`, matching the worktree commands' spelling.
- The worktree open-workspace boundary rejection now names the open workspace roots and the `forktty create-workspace --working-dir <repo>` remedy instead of a bare "cwd must be inside the git repository of an open workspace"; the MCP worktree tool descriptions state the precondition up front and SPEC.md documents the boundary as deliberate.

### Fixed
- Worktree socket operations no longer run their git work on the socket server's async runtime: `worktree.create`/`remove` execute the repo's setup/teardown hook (up to 30s) and every worktree method walks the repository on disk, which pinned tokio workers and could starve other socket connections (agent hook status updates timing out) while worktrees were being created or removed in parallel. The git2 work, the hooks, the open-workspace boundary validation, and the config read now run on the blocking pool.
- Creating a notification through the socket (`notification.create` from the CLI, MCP server, or agent hooks) no longer kills the connection without a response when desktop notifications are enabled: the desktop notifier blocks on its own async runtime, which panics inside the socket server's runtime ("Cannot start a runtime from within a runtime"); dispatch now runs on a dedicated thread.
- MCP list tools (`workspace_list`, `surface_list`, `worktree_list`) no longer fail with strict MCP clients (including Claude Code): the socket returns bare JSON arrays for list methods and the server passed them straight through as `structuredContent`, which the MCP spec requires to be an object. Non-object results are now wrapped as `{"result": ...}`.

## [0.2.0-alpha.10] - 2026-06-11

### Fixed
- The pty master and slave are now opened with `O_CLOEXEC` atomically (`posix_openpt`) instead of setting the flag after `openpty()`: a process forked on another thread in that window (worktree hooks, notification commands) inherited the descriptors and kept the pty alive past its session. Slave duplicates for the child's stdio use `F_DUPFD_CLOEXEC` for the same reason.
- Release binaries no longer inherit the CPU feature set of the CI build machine: libghostty's zig build now targets the generic x86-64 baseline (`-Dcpu=baseline`, via a vendored one-line patch in `vendor/libghostty-rs`). The first alpha.10 cut was built on an AVX-512 runner and its statically linked `memset` crashed every non-AVX-512 machine with SIGILL at startup; the same lottery silently applied to every release since alpha.7.

### Added
- `forktty mcp` now runs a local stdio MCP server exposing ForkTTY workspaces, surfaces, worktrees, notifications, and status metadata as typed tools; `forktty mcp setup/remove` registers the server for Codex, Claude Code, Gemini CLI, and Antigravity while preserving foreign MCP servers (Codex `config.toml` is edited in place, keeping comments and formatting). Claude Code session-start hooks now include ForkTTY workspace/branch context and a short MCP/CLI capability cheat sheet.
- Agent hooks for Antigravity CLI (`agy`, Google's Gemini CLI successor): `forktty hooks setup antigravity` installs a ForkTTY-owned `"forktty"` group in `~/.gemini/config/hooks.json` for the verified `PreInvocation`/`PreToolUse`/`PostToolUse` events, plus generated wrapper scripts (Antigravity executes a hook command as one bare executable path, without arguments or a shell). Sessions are correlated via `conversationId`, hook responses use the strict-`protojson`-safe `{}`, and `hooks doctor antigravity` reports the launcher state from the generated scripts. Gemini CLI hooks remain supported.
- `forktty hooks doctor codex` now reports `trustCheck`: Codex requires per-hook trust approval (recorded under `[hooks.state]` in its `config.toml`) before running installed hooks, so the doctor lists events with no approval record yet and points at `/hooks` inside Codex.

### Changed
- `forktty hooks setup claude` now installs lifecycle hooks by default and omits the blocking per-tool `PreToolUse`/`PostToolUse`/`PostToolUseFailure`/`PostToolBatch` hooks; use `forktty hooks setup --full claude` to restore the previous full profile. Existing installs keep working, and re-running setup migrates Claude hooks to the lifecycle default unless `--full` is passed.
- Chromium bookmark import now deserializes only the bookmark fields ForkTTY uses, avoiding large extra allocations for ignored browser metadata.

### Security
- Pasted text is now encoded with libghostty's paste encoder, which neutralizes control bytes in the payload: clipboard content containing `\x1b[201~` can no longer terminate the bracketed-paste wrapper early and inject the remainder as typed input (including command execution in a shell).
- A socket client that sends a request and then stops reading the response is now disconnected after a write timeout instead of holding one of the 64 connection slots forever; enough stuck clients used to deny the socket to agent hooks.

### Fixed
- Agent hook status updates that lose `FORKTTY_WORKSPACE_ID`/`FORKTTY_SURFACE_ID` after session start (for example through tmux, ssh, or containers) can now be attributed back to the originating pane by `hook_session_id` when the socket server has seen that session with explicit targets.
- "Merge Worktree" now works when invoked from inside a linked worktree (it used to always fail with "Cannot resolve admin directory"), and fast-forward merges update the main checkout's files instead of only moving the branch ref.
- Large pastes (bigger than the kernel pty buffer, ~12KB) are no longer silently truncated; the terminal now waits for the child to drain its input, with a 10s safety timeout. Automatic VT query replies sent over the same path can no longer be cut off mid-sequence either.
- Splitting panes beyond the persistable depth (6 nested splits) is refused instead of silently breaking every subsequent session autosave and losing the layout on restart.
- Ctrl with non-letter keys (Space, `[`, `\`, `]`, `^`, `_`, `?`) now sends the standard C0 control codes instead of nothing.
- In maximize mode, focus changes that don't alter the pane tree (command palette "Focus Next Pane", socket `surface.focus`) now switch the visible pane instead of leaving a stale one on screen.
- The Close Pane confirmation now closes the pane it was opened for, even if a socket client switched the active workspace while the dialog was open.
- Piped CLI output (`forktty list --json | head -1` and similar) no longer panics with exit 101 and a panic.log entry when the consumer closes the pipe early.
- "Reset Terminal" no longer reverts the pane to libghostty's default colors; the configured theme is re-applied and stale paste/focus mode state is cleared.
- A configured shell or notification command that is temporarily missing from disk no longer quarantines the whole config and resets every setting to defaults; the value is normalized (default shell / cleared command) on load instead.
- The socket server keeps serving through transient `accept()` failures such as file-descriptor exhaustion (EMFILE/ENFILE) instead of shutting down for the rest of the app's lifetime.
- Two concurrent closes of the last workspace (or a close racing a worktree removal) no longer leave a duplicate replacement workspace.
- Replacing a pane or workspace context menu no longer risks a re-entrant `RefCell` crash from the popover's synchronous `closed` signal.
- Saving a setting from the Settings dialog no longer reverts config changes made outside the dialog while it was open (e.g. the F9 sidebar toggle); each change is rebased onto the config on disk.
- `forktty doctor --socket/--verbose/--debug` now explains that `doctor` runs locally and that the socket doctor takes global flags first (`forktty --json doctor`); the CLI help documents the reachable spelling.
- Hook event ordering now uses CLOCK_BOOTTIME instead of the wall clock, so hook status updates are no longer silently dropped after the system clock steps backwards; orders from different clock sources are no longer compared against each other.
- `forktty hooks` with a missing or typoed subcommand now exits with a helpful error instead of printing hook continue-JSON and exiting 0.
- `worktree-status` rejects combining a positional path with `--path`/`--cwd` instead of silently ignoring the positional, matching its sibling commands.
- `forktty --json doctor` and `forktty hooks setup` no longer hang forever when an inspected config path is a FIFO.
- Socket connections from the CLI and agent hooks are bounded by a connect timeout instead of hanging when the app's accept backlog is full.
- A poisoned model lock no longer makes the socket event stream broadcast false removal events for every workspace and surface.
- Workspace cwd validation no longer runs git repository discovery while holding the model lock shared with the UI thread, and the startup socket probe now has an overall deadline instead of only per-read timeouts.
- Shell-trampoline detection for the notification command now catches clustered flags (`bash -lc`), separated flags (`bash -x -c`), and `env`-wrapped shells (`env bash -c`).
- A child process flooding its terminal can no longer hold the UI in a single unbounded read; terminal reads are capped at 1MiB per pump tick.

## [0.2.0-alpha.9] - 2026-06-11

### Changed
- The Settings dialog was reorganized and rebuilt with standard GNOME preference rows: the terminal palette now lives on the Terminal page next to font and scrollback, the Alerts page is named Notifications, enum options (palette, window mode, sidebar position, worktree layout, font family) use combo rows with explicit value mapping instead of free-form combo boxes, font size and scrollback use spin rows with the validated bounds, the font picker is searchable, and an invalid shell or notification command now marks the row in red in addition to the error toast.
- Dropped the vendored `libghostty-vt-sys` copy: upstream libghostty-rs now makes the zig optimize mode follow the Cargo profile (uzaaft/libghostty-rs#55), so both libghostty crates are pinned to upstream master via `[patch.crates-io]`, with `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe` pinned in `.cargo/config.toml` so debug and test builds keep the optimized VT parser. The lines-to-bytes scrollback conversion stays on our side (upstream C API issue, uzaaft/libghostty-rs#56).

### Added
- Double-click selects the word under the pointer (ghostty's default word boundaries, so paths like `/tmp/a.txt` select whole) and triple-click selects the visual row; both publish to the PRIMARY selection like a finished drag and work with Shift inside mouse-tracking apps.
- Dragging a selection past the top or bottom edge of a pane now autoscrolls the viewport (faster the further the pointer goes) and keeps extending the selection; the selection is re-anchored under the scroll instead of being dropped.
- Panics now also append message, location, and backtrace to `$XDG_STATE_HOME/forktty/panic.log` before the process dies, so field crashes (which abort inside GTK signal trampolines) no longer require coredump symbolization to diagnose.
- The session state is now guarded by a lock file: a second running instance (e.g. a deb-installed and an AppImage forktty that DBus could not deduplicate) refuses to start instead of silently fighting the first one's session autosave.

### Fixed
- An empty or malformed OSC 99 sequence no longer leaves a debug-formatted "Terminal metadata" status entry in the sidebar; it is ignored.
- Terminal pixel-size reports now keep the measured cell dimensions after pane-only cell resizes instead of reverting to the 10x20 startup fallback.
- Terminal OSC 110/111/104 color resets now restore ForkTTY's configured theme defaults instead of falling back to libghostty's built-in black/background palette.
- Dead-key and compose-key input now has the GTK input-method handoff documented at the terminal key fallback, clarifying why committed text is not duplicated by fallback key encoding.
- The search bar's match counter no longer shows a stale "current/total" after new terminal output removes the match highlight; it resets to 0/0 until the next step.
- Two processes quarantining the same corrupt profile/bookmark store at the same time can no longer overwrite each other's backup file.
- The sidebar toggle and the periodic PR lookup no longer read the config file on the GTK main thread, and the 2s session autosave no longer builds a debug dump of the whole session to detect changes.
- An `events.subscribe` client that stops reading is now disconnected after 10s instead of holding one of the 64 socket connection slots forever; enough stuck subscribers used to silently deny the socket to agent hooks.
- The event stream now reports a surface whose owning workspace changes (session-restore repair) by re-asserting it as removed + added; subscribers' per-workspace surface lists used to go silently stale.
- `forktty events` flushes every event line, so piped consumers (`| jq`, `| while read`) see events as they happen instead of in 8KB bursts; when the server drops events because the consumer lags, a warning now also lands on stderr.

### CI
- CI now verifies on every push that the binary carries the RUNPATH it needs to find the bundled libghostty, and the release smoke test runs the packaged binaries exactly the way the alpha.7 field failures did: the deb tree and the extracted AppImage's inner binary without any `LD_LIBRARY_PATH`, plus the `FORKTTY_APPIMAGE` exports in AppRun.

## [0.2.0-alpha.8] - 2026-06-10

### Added
- Added scrollback search: Ctrl+Shift+F (also in the command palette) opens a floating per-pane search bar with case-insensitive matching over the full scrollback, wrapping next/previous navigation, a match counter, and highlight-on-jump that feeds the copy shortcut.

### Changed
- The focused pane's cursor now blinks at the conventional cadence and snaps visible on keystrokes; unfocused panes keep a steady hollow cursor.
- Pane headers now slide in and out over 180ms when a workspace transitions between single-pane and split layouts.
- Split dividers tint with the accent color while being dragged, and the worktree dialog's mode selector uses the accent for the selected mode.
- libghostty is now compiled optimized (ReleaseSafe): upstream's build script left zig in Debug mode, so terminal output parsing ran ~870x slower than it should have (a 64 KiB burst took ~1 second; `cat` of a large file crawled at ~65 KB/s).
- The `events.subscribe` stream now emits `workspace_renamed` when a workspace changes name; subscribers previously kept the stale name forever.

### Fixed
- Scrollback now actually retains the configured number of lines: the limit was being passed to ghostty as a byte budget, so 10k configured lines kept only a few dozen rows of history.
- Scrollback search no longer re-dumps the entire scrollback on every keystroke and every Enter; matches are cached until terminal content or the query changes, keeping search instant even at 100k lines.
- Ctrl+Shift+C with nothing selected now copies the visible screen instead of silently filling the clipboard with the entire scrollback history.
- Mouse clicks and selection highlights no longer drift from the painted text grid with fonts whose metrics round differently (input mapping and rendering now share one cell measurement).
- Launching forktty while it is already running now presents the existing window instead of building a second window, workspace model, and socket server (which used to steal the IPC socket from the running instance).
- The quake window re-derives its size from the current monitor on each show instead of keeping its launch-time geometry after dock/undock or resolution changes.
- The IPC socket is now bound via a private staging directory instead of flipping the process-wide umask, which could corrupt files created concurrently by other threads.
- Wheel scrolling a pane whose application does not track the mouse (a plain shell prompt) aborted the whole app: the scroll handler double-borrowed the terminal runtime and the panic could not unwind across the GTK signal trampoline. Tracking-aware apps (tmux, vim, htop) were unaffected, which made the crash look random.
- AppImage: `forktty` CLI calls from shells inside the app (agent hooks, `forktty ping`) failed with "error while loading shared libraries: libghostty-vt.so.0"; the binary now locates the bundled library itself (RUNPATH `$ORIGIN/../lib`). Agent hooks set up from inside the AppImage now reference the stable `.AppImage` path instead of the temporary `/tmp/.mount_*` mount, which broke on the next launch.

## [0.2.0-alpha.7] - 2026-06-09

### Added
- Added mouse text selection in terminal panes: left-drag selects when application mouse tracking is off, Shift+drag overrides tracking-aware apps (vim, htop), the selection is highlighted with the theme highlight color, and the extracted text feeds Ctrl+Shift+C and the primary clipboard (middle-click paste).
- Surfaced OSC 9 and OSC 99 terminal escape notifications as ForkTTY notifications.

### Changed
- Replaced the VTE terminal backend with libghostty-vt and a custom GTK renderer: ghostty-driven key encoding, cursor styles, wide-cell rendering, OSC 8 hyperlinks, text decorations, bracketed paste tracking, focus reporting, mouse routing, configurable scrollback, and theme-color seeding.
- Worktree operations (list/create/attach/merge/remove) now run their git work off the GTK main thread, so slow repositories no longer freeze the UI; the worktree dialog opens immediately and populates its worktree chooser asynchronously.

### Fixed
- Terminal children now acquire the pty as their controlling terminal (`TIOCSCTTY`), fixing `/dev/tty` consumers such as fzf, less, and ssh/sudo prompts under zsh.
- The pty master fd is now CLOEXEC, so spawned children (and any other subprocess) no longer inherit one extra descriptor per open terminal.
- The PTY pump timer stops after the child exits instead of polling a dead pty at 60Hz per closed pane, and closed panes now release their terminal runtime.
- Closing a pane now focuses the adjacent sibling instead of teleporting to the first pane, and closing a background pane no longer steals focus; a stale Close Pane confirmation can no longer close the wrong pane.
- The IPC socket server survives transient `accept()` errors instead of shutting down for the rest of the session, and `forktty events` bounds its subscribe handshake so a wedged server cannot hang the CLI.
- Fixed terminal theme seeding (fresh surfaces rendered ghostty's built-in colors), CSS provider accumulation, Shift+Tab/Alt+Backspace encoding, and a panic on non-ASCII hex color strings in configs.
- Hardened agent hooks: the OpenCode plugin caps payload recursion (deeply nested MCP tool responses could crash the host session) and `hooks setup` warns before replacing a non-map `hooks` config value.

### CI
- Restored CI after the runner image dropped the `zig` apt package: Zig is now installed via the pinned `setup-zig` action.

## [0.2.0-alpha.6] - 2026-05-30

### Added
- Added `events.subscribe` NDJSON streaming and `system.capabilities` discovery, with `forktty events` and `forktty capabilities` CLI entry points.
- Added an optional source-build browser-pane path behind the `browser` feature: WebKitGTK6 pane surfaces, socket/CLI open/navigate/snapshot/click/fill/eval/back/forward/reload verbs, GUI open/close controls, persistent per-profile WebKit sessions, and browser profile CRUD.
- Added per-profile browser history and bookmark stores plus `browser.history.*` / `browser.bookmark.*` socket verbs and `forktty browser history|bookmark` CLI mirrors; GTK address-bar/history integration remains follow-up work.
- Added browser import via the new `forktty-import` crate: `browser.import.discover`/`preview`/`run` socket verbs, `forktty browser import discover|preview|run` CLI, and a Settings "Import Browser Data" dialog that imports history and bookmarks from local Firefox/Chromium-family profiles (cookies are preview-only, not yet written) with rollback on failure.
- Added SSH remote workspaces: `SurfaceKind::Ssh` panes spawned as `ssh <host>`, the `workspace.create_ssh` socket method, a `forktty ssh` CLI, sidebar `ssh:<host>` hints, and respawn on session restore.
- Added per-pane tabs: `pane.new_tab`/`pane.select_tab` socket methods, `forktty new-tab`/`select-tab` CLI, and pane-chrome/command-palette tab controls.

### Changed
- Promoted the AppImage from an experimental smoke-test artifact to the primary portable Linux download while keeping host-runtime caveats for glibc, GSettings/GIO, fontconfig, desktop services, and GPU drivers.
- Packaged builds, CI, and release QA now use `--features browser`, so browser panes and browser import ship in the `.deb` and AppImage.
- Updated the RustCrypto stack (`aes`, `cbc`, `hmac`, `pbkdf2`, and `sha1`) together with the cookie decryption API changes.
- Renamed Linux desktop/AppStream metadata to the reverse-DNS `dev.forktty.forktty` desktop id and refreshed app/icon assets across installed sizes.
- Refined GTK topbar, settings, about, notifications, workspace/sidebar, tab, pane, and drag-and-drop visuals for a more consistent native dark UI.

### Fixed
- Fixed `forktty ssh <user@host>` routing so the documented CLI command reaches the socket handler instead of being rejected as an unknown argument.
- Fixed mixed/phantom drag highlights by using typed drag-and-drop payloads for tabs, panes, and workspaces, with clearer drop acceptance.
- Fixed pane navigation/swap desync handling so a missing focused surface no longer falls back to pane index 0.
- Hardened session restore and config persistence with XDG state-dir migration, atomic saves, directory fsync, quarantine of corrupt/oversized files, and allocation-free pane-surface lookup.
- Hardened socket CLI reads, import readers, browser profile import, worktree lifecycle rollback, and terminal AppImage hook launching against oversized input, unsafe paths, stale handles, and AppImage runtime leakage.
- Fixed release and packaging docs so desktop validation paths, feature flags, and packaged artifact expectations match CI and the build scripts.

### Security
- Strengthened local robustness by bounding socket responses, browser/import file reads, config/session loads, and stdin payloads, while preserving owner-only Unix socket behavior and argv-based command execution.

### Documentation
- Audited Markdown docs against the current Rust workspace, scripts, feature gates, socket methods, browser profile/storage behavior, packaging flow, and support links, and brought SPEC/ROADMAP/cmux-gap docs in line with the shipped SSH workspace, per-pane tab, and browser import surfaces.

## [0.2.0-alpha.5] - 2026-05-23

### Added
- Added native Rust socket CLI and hook installer/test/doctor support inside the `forktty` binary, replacing the legacy Node.js CLI and making AppImage hook flows independent of a source checkout.
- Hook handling now surfaces Codex/Claude `permission_mode`, Claude risk colors, session ids, and supported events for richer local automation.

### Changed
- The socket CLI and agent hook bridge now run natively inside the `forktty` binary. `forktty hooks setup` installs hook commands that call the stable `forktty` launcher directly, so AppImage users no longer need a source checkout or Node.js for hook installation/execution.
- Packaging and release checks now align `.deb`, AppImage, and `SHA256SUMS` asset names, pin AppImage smoke-test tooling/packages, and ship consistent desktop/AppStream runtime metadata.
- README, hooks, native GTK/VTE, release QA, and contributor documentation now describe the prebuilt artifact flow, `forktty doctor`, and the native hook diagnostics.

### Documentation
- Restructured README install instructions around prebuilt AppImage and `.deb` artifacts, with a dedicated "Build from source" section and a first-run / troubleshooting flow that points at `forktty doctor`.
- Documented `forktty hooks doctor <agent>` and `forktty hooks test <agent>` in the README and `hooks/README.md`.
- Clarified that the experimental AppImage bundles GTK4/libadwaita/VTE via the `ldd` graph but still depends on the host's glibc, GSettings/GIO data, fontconfig, and desktop session services.

### Fixed
- `surface.send_text` now waits for terminal readiness before writing text, preventing early sends from racing pane startup.
- Session persistence now keeps saves working when the state path is a broken symlink and repairs duplicate or leafless pane trees before they can poison restore state.
- Codex and Claude hook timeout values are interpreted as seconds, and `forktty hooks doctor` reports stale launcher paths.
- GTK polish/stability fixes tightened the alpha pill, status/sidebar labels, command-palette and popover accent treatment, pane titles, settings layout, and destructive confirmation target names.

## [0.2.0-alpha.4] - 2026-05-22

### Added
- Added experimental AppImage packaging via `scripts/build-appimage.sh`, producing a tagged AppImage artifact under `target/packaging/appimage/` alongside the existing Debian package.
- Unread counter badge on the notifications toolbar button so the queue depth is visible without opening the panel.
- Configurable VTE terminal theme presets via `appearance.terminal_theme`, with System, Catppuccin Mocha, Rose Pine, Tokyo Night, Dracula, and Gruvbox Dark choices exposed in Settings.

### Changed
- GitHub release packaging now builds and uploads both the `.deb` and the experimental AppImage, with a shared `SHA256SUMS` file covering both artifacts.
- The README download link now points directly to the alpha.4 AppImage as the default downloadable artifact, while keeping the Debian package documented.
- Refreshed the README screenshot and updated terminal environment documentation after the alpha.3 release.
- Consolidated the shell-trampoline, executable-file, and worktree-name validators into a single `forktty_core::command_safety` module so the socket layer, GTK shell, and notification dispatcher cannot drift apart on the same security rules.
- Socket dispatch errors now carry structured codes (`method_not_found`, `missing_param`, `not_found`, `payload_too_large`) instead of the catch-all `error` code, so clients can branch on outcome rather than parsing message text.
- `surface.send_text` now rejects payloads larger than 256 KiB with a `payload_too_large` response instead of blocking the dispatch task on a wedged VTE pipe.
- GTK shell visual-polish pass: tightened sidebar / pane header / topbar / status-bar contrast and hierarchy, libadwaita-native header separator, neutral "exited" badge, premium focus rings and inner shadows on form controls, minimal overlay scrollbars, an 8 px / 16 px dialog spatial grid, tactile button feedback, settings dialog label/subtitle wrapping, and softer needs-input emphasis so the active workspace and pane read as the primary anchors.
- Audited project documentation: SPEC now lists the socket error codes and the `surface.send_text` cap, the ROADMAP no longer interleaves implemented appearance work with backlog items, and the stale `.jules/bolt.md` note targeting the removed React sidebar was removed.

### Fixed
- Resolved terminal font discovery through GTK/Pango instead of spawning `fc-list`/`fc-match` by name, removing a PATH-hijack risk when ForkTTY is launched from an untrusted environment.
- `forktty close-workspace <name-with-dash>` no longer misroutes to a workspace id lookup; the CLI now tries the positional selector as an id first and falls back to the name, matching `focus`.
- Notification dispatch no longer silently swallows config-load errors; a broken `config.toml` now logs the underlying cause before falling back to defaults.
- Socket connection-loop I/O failures are now logged to stderr instead of being silently dropped, so socket-layer regressions are visible without attaching a debugger.
- Session restore now logs the reason it quarantined a session file (parse failure, validation failure, oversized, or not a regular file) instead of silently moving it aside, so a session that fails to come back up is debuggable from stderr.
- `forktty hooks setup` now writes the agent config files atomically (tmp + rename) instead of truncate-then-write, eliminating the corruption window on SIGKILL or power loss. A `--dry-run` flag prints the would-be result without touching disk, and malformed existing configs now report which agent and path failed instead of bubbling up a raw `SyntaxError`.
- VTE `child-exited` and `bell` signals no longer create notifications when the user has already closed the originating pane, and `child-exited` is now latched per-surface so a duplicate emission from VTE cannot generate two "Terminal exited" notifications. Session restore also re-runs the workspace invariant repair as a defensive pass, matching what `save_session` already does.
- GTK font picker no longer collapses families whose synthesized IDs would collide with another real family, so every installed font is selectable.
- Sidebar refresh no longer races a closing workspace context popover, which previously could leave the sidebar pointing at a stale workspace entry.
- Worktree context menu actions now target the workspace the menu was opened on instead of the currently focused workspace.
- Workspace-scoped notifications (no specific surface) once again raise workspace attention reliably and clear it on read.
- Closing a pane preserves the workspace pane-tree invariants when the closed pane was the focused leaf of a deeper split, preventing a stale focused-surface id after collapse.

## [0.2.0-alpha.3] - 2026-05-15

### Added
- Rebuilt Settings with native libadwaita preferences pages/groups and added terminal scrollback and audible-bell controls.

### Changed
- Rebalanced the built-in VTE color palettes with a softer terminal background and full ANSI colors instead of relying on saturated VTE defaults.
- Aligned VTE child sessions with terminal conventions by advertising `COLORTERM=truecolor`, app identity variables, system cursor blink, hyperlink support, and non-bright bold text.
- Added standard terminal text actions for Select All and Reset/Clear to shortcuts, the command palette, and the terminal context menu.
- Reset/Clear now asks the child shell to redraw with `Ctrl+L` after clearing VTE state, so users return to a clean prompt instead of a blank pane.
- Softened the active-pane border so split-pane focus remains clear without drawing a heavy purple frame around the terminal.
- Moved the GTK polish design note into `docs/design/` and removed stale GTK/Tauri-era repository artifacts.
- Workspace-scoped notifications without a surface target now raise workspace attention until they are read or dismissed.
- Updated GTK/runtime helper dependencies (`gtk4`, `global-hotkey`, and `libloading`) after validating the GTK/VTE build and Debian package.

### Fixed
- Scoped global terminal clipboard shortcuts to the VTE widget that currently owns GTK focus, preventing stale-pane paste/copy when a dialog or search entry is focused.
- Avoided a GTK/Wayland crash when restoring sessions with three or more VTE panes by deferring terminal focus until widgets are rooted and cancelling stale pane-ratio tick callbacks after rebuilds.
- `Open Latest` in the notification panel now resolves the current latest openable notification at click time, so dismissing a notification cannot leave the button targeting a removed item.
- Cleared the persisted workspace attention badge on session restore so freshly restarted sessions no longer show stale unread state when no surfaces are unread and no notifications carry over.
- `Ctrl+Shift+W` and the close-pane button now succeed when the underlying terminal has already exited; the model surface is removed even if the backend reports it as `NotFound`, matching the socket close path.
- Rejected hand-edited session files that disagree about which workspace is active (multiple `active: true` flags, or a flag pointing to a workspace different from `active_workspace_id`) so loads quarantine corrupt state instead of silently picking one.
- Dropped the stale `version` field from `package.json` (Cargo workspace is the source of truth; the package was already `private: true`) to stop the two version strings drifting apart between releases.

## [0.2.0-alpha.2] - 2026-05-15

### Added
- Added a README screenshot of the GTK/VTE app running on Ubuntu.
- Added a release QA checklist for GTK/VTE runtime and Debian package smoke testing.
- Added an existing-worktree chooser for Merge and Remove in the worktree dialog.

### Changed
- Removed the Ubuntu Docker development wrapper from the main workflow; native dependency installation and CI remain the supported build paths.
- Updated README release links to point directly at the current prerelease.
- Opening the notification panel now marks notifications read while preserving history.

### Fixed
- Added GTK actions for terminal copy/paste so `Ctrl+Shift+C` and `Ctrl+Shift+V` target the focused VTE pane.
- Moved terminal context menus out of clipped pane widgets so right-click paste remains reachable in heavily split layouts.
- Added per-notification dismiss so users do not have to clear the entire notification list.
- Dismissing the last notification now collapses the panel to the empty state and disables the Clear All and Open Latest actions.
- Closing the last unread pane in a workspace now clears the workspace's attention badge instead of leaving it pinned to a removed surface.
- Retried transient text-file-busy hook spawns so freshly checked-out worktree hooks do not flake under CI load.

## [0.2.0-alpha.1] - 2026-05-14

### Architecture
- Replaced the old Tauri/React/WebKit runtime with the native GTK4/libadwaita/VTE implementation as the primary app.
- Removed the legacy frontend, Tauri backend, Vite/TypeScript build, and npm dependency tree from the main code path.
- Installed the native binary and Debian package as `forktty` instead of `forktty-gtk`.

### UI
- Added the native GTK shell with compact header, product wordmark, workspace sidebar, recursive split panes, global status bar, command palette, settings, notification panel, keyboard shortcut reference, and context menus.
- Added the refreshed ForkTTY app icon used by README, desktop integration, notifications, window chrome, and About dialog.
- Added workspace rename support from the workspace context menu and command palette.
- Added sidebar toggle persistence, theme selection, sidebar visibility setting, reset-to-defaults staging, destructive confirmations, and improved empty/error states.
- Polished pane chrome with single-pane header hiding, hover/focus-revealed pane actions, duplicate CWD suppression, active pane indicators, and terminal placeholder recovery actions.

### Terminal
- Moved terminal spawning to GTK/VTE realization to avoid Wayland/VTE startup crashes and duplicate shell spawns.
- Restored sessions now rebuild panes incrementally instead of spawning every VTE surface in the same main-loop turn.
- Clean terminal exits no longer create noisy warning notifications.
- Added safer quake mode fallback to a normal decorated window when layer-shell support is unavailable.

### Reliability
- Fixed `workspace.close` to close by the resolved workspace ID so surface cleanup and model mutation cannot diverge on ambiguous selectors.
- Limited VTE prompt fallback scanning to a bounded visible tail instead of copying the full terminal text on every contents-changed signal.
- Added immediate session saves after workspace and pane mutations.
- Added config-load and session-restore user-facing error notifications.

### Tooling
- Replaced Vitest/Vite frontend checks with Node built-in CLI tests.
- Updated CI, dependency review, security audit, desktop entry validation, and Debian packaging for the Rust GTK/VTE stack.
- Debian prerelease package versions now use Debian ordering (`0.2.0~alpha.1`) while Cargo and GitHub use SemVer (`0.2.0-alpha.1`).

### Known Limitations
- Linux only.
- The first alpha ships a `.deb` package. AppImage packaging is deferred until the native GTK/VTE bundle can be tested reliably.
- PTY processes and scrollback are not preserved across restart; restored sessions spawn fresh shells.
- Quake global shortcuts and layer-shell placement depend on desktop/compositor support.

## [0.1.2] - 2026-05-11

### Documentation
- Updated README, SPEC, ROADMAP, SECURITY, and PRIVACY to match current UI polish, session restore, config validation, notification, worktree, AppImage, and test coverage behavior
- Clarified that `notification_command` still supports static argv arguments after the required absolute executable path; a no-arguments policy remains a future hardening item

### UI Polish
- Refined WelcomeScreen, modal focus behavior, and empty/loading/error states across key frontend surfaces
- Added safer focus defaults for destructive modals

### Reliability & Security
- Session restore now validates persisted pane trees and quarantines corrupt or invalid session files instead of failing startup
- Restored sessions suppress spurious prompt notifications during startup
- Config loading for ForkTTY's TOML config is bounded to regular files up to 1 MiB
- Ghostty config and theme loading now ignores missing, non-regular, oversized, or unreadable files instead of reading them unbounded
- Shell and notification command configuration now validate executable paths more defensively
- AppImage packaging normalizes root desktop/icon symlinks, rejects unsafe icon values, and refuses absolute root symlinks
- Socket request reading now enforces the 1 MiB line limit without relying on `BufReader::lines()`

### Tests & Tooling
- Added frontend and Rust coverage for restore, notification, config, and packaging hardening paths
- Refreshed dependency and tooling versions where relevant

## [0.1.1] - 2026-04-23

### UI Polish
- Refined sidebar, pane chrome, command palette, branch picker, notifications, settings, menus, and find bar with a more consistent dark desktop visual language
- Split UI typography from terminal typography: proportional font for chrome, monospace for terminal content, shortcuts, and badges
- Added explicit inactive-pane dimming and more restrained focus/unread states
- Added extra breathing room around terminal surfaces without changing PTY behavior
- Replaced placeholder text controls with shared SVG iconography
- Added `prefers-contrast` and `prefers-reduced-motion` polish for dark-theme accessibility

### Interaction Fixes
- Help & Shortcuts menu now renders above the sidebar correctly instead of appearing behind other UI
- Workspace switching from the sidebar triggers earlier and feels more immediate
- Workspace name hover now shows the text cursor only over the actual name, not across the full row
- Workspace reordering now uses a dedicated drag handle instead of making the whole row draggable
- Reduced duplicate prompt notifications with stronger switch-time suppression and short-window deduplication
- Avoid repeated `Prompt waiting` notifications while a workspace is already unread

### Socket & Worktree Hardening
- Fixed socket-driven `worktree.create` prompts being written twice to the target PTY
- Fixed removal of the last worktree-backed workspace so the replacement workspace falls back to a valid repository root instead of a deleted directory
- Relaxed socket `cwd` validation to accept subdirectories and linked worktrees from the same open repository while preserving repo-boundary checks

## [0.1.0] - 2026-03-19

### Phase 1 — MVP Terminal
- Tauri v2 + React 19 + TypeScript scaffold
- portable-pty PTY management with Tauri Channel streaming
- xterm.js terminal with Canvas renderer (WebGL fallback disabled due to WebKitGTK bugs)
- Full TUI support (htop, vim, less all render correctly)
- Terminal resize via ResizeObserver + FitAddon

### Phase 2 — Multi-Pane Splits
- react-resizable-panels recursive split layout (horizontal/vertical)
- Zustand store tracking PaneTree structure and focus
- Keyboard: Ctrl+D (split right), Ctrl+Shift+D (split down), Alt+Arrow (navigate), Ctrl+W (close)

### Phase 3 — Sidebar + Workspaces
- Sidebar showing workspace list with metadata (branch, directory, status)
- Workspace creation (Ctrl+N), switching (Ctrl+1..9), closing (Ctrl+Shift+W)
- Git branch detection via git2

### Phase 4 — Git Worktree Integration
- git2 crate for native worktree create/merge/remove
- Setup/teardown hook support (.forktty/setup, .forktty/teardown)
- Worktree layout config (nested/sibling/outer-nested)
- Sidebar worktree status badges (clean/dirty/conflicts)

### Phase 5 — Notification System
- OSC 133 shell integration parsing in Rust backend
- Pattern matching for Claude Code prompt detection
- In-app blue dot + unread count on sidebar
- Desktop notifications via notify-rust (XDG/D-Bus)
- Notification panel (Ctrl+Shift+I), jump to unread (Ctrl+Shift+U)

### Phase 6 — Socket API
- Unix domain socket JSON-RPC server (tokio)
- 22 methods: system.ping, workspace.*, surface.*, notification.*, worktree.*, metadata.*
- Environment variables set in spawned shells (`FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, `FORKTTY_SOCKET_PATH`)

### Phase 7 — Theming + Config
- Ghostty config parser with theme file and palette support
- TOML config at ~/.config/forktty/config.toml
- Settings panel (Ctrl+,) for in-app config editing
- Catppuccin Mocha as default fallback theme
- Configurable sidebar position (left/right)

### Phase 8 — Polish + Release
- Session persistence (auto-save and restore on startup)
- Command palette (Ctrl+Shift+P) with keyboard navigation and inline filtering
- Find in terminal (Ctrl+F) via xterm.js SearchAddon
- Copy selection (Ctrl+Shift+C)
- ErrorToast component for user-visible error feedback
- Structured logging to ~/.local/share/forktty/logs/
- .deb and AppImage bundle targets
- License: AGPL-3.0

### Security Hardening
- Socket: owner-only permissions (0o600), XDG_RUNTIME_DIR default path, 1 MiB request size limit
- Notifications: argv splitting instead of sh -c (no command injection)
- Worktree: path traversal protection via canonicalize + git-workdir boundary check
- Worktree names: reject /, \, .., \0
- Shell path: must be absolute and point to an executable file
- CSP: strict Content Security Policy in tauri.conf.json
- Config: Ghostty theme path traversal guard
- Logging: newline injection sanitization

### Known Limitations
- `beforeunload` session save is fire-and-forget (async IPC may not complete)
- No idle detection (`idle_threshold_ms` config field reserved but not active)
- No dark/light mode toggle (dark theme only; CSS has a minimal system-preference fallback)
- No flow control / backpressure on PTY output
