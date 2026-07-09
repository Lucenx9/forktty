# Task Router Multi-Harness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ForkTTY choose the best visible agent strategy for a user task instead of requiring the user or current agent to manually decide between team, workflow, loop, MCP, hooks, worktrees, and harnesses.

**Architecture:** Add a deterministic, capability-grounded task strategy router above the existing ForkTTY primitives. Phase 1 is read-only planning through socket, CLI, and MCP; Phase 2 applies a confirmed strategy by creating visible workflow/team/loop/worktree state without hidden schedulers or destructive actions.

**Tech Stack:** Rust workspace, `forktty-core` domain types, `forktty-socket` JSON-RPC methods, `forktty-ui-gtk` socket CLI and MCP server, existing workflow/team/feed/worktree primitives, Markdown docs.

---

## Current Status

This section is the current source of truth for progress. The detailed
checkboxes below preserve the original implementation recipe and should not be
read as the live progress tracker after the Phase 1/first Phase 2 slice.

Phase 1 read-only routing and the first Phase 2 apply paths have been
implemented in the `task-router-readonly` worktree as of June 28, 2026. The
implemented slice includes the pure core planner, socket methods, CLI entry
points, MCP tools, agent policy updates, repo docs, public site docs, and the
Phase 2 apply design spec. Planner harness selection now follows
`team_provider_policy.provider_order`, and reviewer strategies always produce a
reviewer assignment even when only one routable harness is available. When callers
omit `repo_dirty`, `task.strategy.plan` now uses the selected surface/workspace
effective project cwd to infer simple git dirty/conflict state before choosing
worktree isolation. When callers omit the user-visible edit hint, the socket
runtime infers likely edit intent from the goal text while still respecting an
explicit false override. Planner responses now include ranked
`candidate_scores`, with per-factor point breakdowns, so agents can inspect
why the chosen strategy won and which alternatives were considered. Harness
assignments now include role-specific score/factor breakdowns, so reviewer
roles can prefer plan-mode-capable harnesses and worktree-isolated implementer
roles can prefer cwd/worktree-capable harnesses while still using configured
provider order as the tie-break. Multi-role planner assignments now also
respect `max_parallel_sessions`, so parallel research/experiment plans are not
selected when only a single-session harness can run the roles.
Planner responses now also include a selected `router_profile`, and callers can
optionally pass `router_profile`/CLI `--profile` for `balanced`, `fast`,
`conservative`, `parallel`, or `review_heavy`. When omitted, ForkTTY keeps the
balanced scorer unless goal wording or request hints clearly imply speed,
caution, parallel work, or review-heavy routing. Profiles reweight the same
ranked candidate scorer instead of creating separate routers or hidden
execution semantics.
Planner inputs now also accept per-harness routing signals. `cooldown` is a
soft assignment penalty that preserves a harness as a fallback; `locked_out`
is a hard task/mode exclusion. These signals are separate from provider
capability health/readiness and are exposed through socket, CLI
`--harness-signals-json`, and MCP `harness_signals`.
Planner inputs now also accept advisory `last_known_good` evidence through
socket, CLI `--last-known-good-json`, and MCP. A matching strategy or harness
gets a small score factor; readiness, cooldown, lockout, task fit, approvals,
and visibility rules still win. When callers omit `last_known_good`,
`task.strategy.plan` now best-effort reads completed `task_strategy` workflows
for the selected workspace and infers the most recent usable strategy/harness
evidence recorded by `task.strategy.apply`; explicit caller evidence still
wins over inferred history.
`task.strategy.apply` defaults to staging visible workflow/team/task/message
state. With `submit=true`, an approved team plan now launches visible worker
panes and dispatches role prompts through the team mailbox; worktree-layer
plans require `worktree_name` for an already-open ForkTTY worktree workspace
and are rejected before mutation if it is missing.
Missing start-run approvals can be published as deterministic, request-bound
Feed approvals with `request_approval` and later consumed with the returned
approved `approval_id` only for the same run id, goal, plan, target scope, and
submit mode; if explicit attestations cover part of that same approved request,
the returned `approval_id` can still satisfy the remaining covered approvals.
Apply also recomputes dirty-repo edit isolation from the selected
surface/workspace plus any explicit `cwd`, then recomputes required approvals
from the requested operation and effective plan shape, so dirty editing tasks
cannot bypass worktree isolation by submitting a
weaker plan, worktree-layer plans cannot omit `create_worktree`, and
multi-worker submit cannot omit `launch_parallel_workers` from
`plan.approvals` to bypass review. The `approved` array is a programmatic
caller attestation; Feed `request_approval`/`approval_id` is the human-decision
path. Locally invalid requests, such as team-layer plans without assignments,
non-team submit, or worktree-layer apply without `worktree_name`, are rejected
before creating Feed approvals. Submit retries now refuse to reuse a live
deterministic worker whose harness, role, task, cwd/worktree target, or status
does not match the current assignment.
Worktree creation, push, merge, destructive commands, and hidden background
scheduling remain intentionally unsupported in the router.

Current implementation files:

- `crates/forktty-core/src/task_strategy.rs`
- `crates/forktty-socket/src/task_strategy_params.rs`
- `crates/forktty-socket/src/task_strategy_runtime.rs`
- `crates/forktty-socket/src/tests/task_strategy.rs`
- `crates/forktty-ui-gtk/src/socket_cli/task.rs`
- `crates/forktty-ui-gtk/src/socket_cli/tests/task_strategy.rs`
- `docs/task-router-multi-harness-plan.md`

Verification already run for the current planner + staged apply + visible team
submit slice:

- `cargo fmt --all`
- `cargo fmt --all --check`
- `cargo test -p forktty-core task_strategy`
- `cargo test -p forktty-socket task_strategy`
- `cargo test -p forktty-socket protocol_dispatch`
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty`
- `cargo test -p forktty-ui-gtk mcp_server --no-default-features --features gtk-ghostty`
- `cargo test -p forktty-ui-gtk socket_cli --no-default-features --features gtk-ghostty`
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty`
- `cargo run -p xtask -- check`
- `FORKTTY_SKIP_GTK_WIDGET_TESTS=1 cargo test --workspace --all-targets --no-default-features --features gtk-ghostty`
- `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
- `cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser -- --test-threads=1`
- `cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings`
- `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty`
- `target/debug/forktty skills setup agents --dry-run --json`
- `target/debug/forktty skills setup claude --dry-run --json`
- `git diff --check`
- `cargo test -p forktty-socket task_strategy_apply_submit_ -- --nocapture`
- `cargo test -p forktty-socket task_strategy_apply_ -- --nocapture`
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-socket task_strategy -- --nocapture` after adding
  selected-cwd dirty inference
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty -- --nocapture`
  after adding `task-plan --surface-id` and MCP target forwarding
- `cargo clippy -p forktty-socket -p forktty-ui-gtk --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
- In `/home/simone/forktty-site`: `npm test`, `npm run build`, and
  `git diff --check`
- `cargo test -p forktty-socket task_strategy_plan_infers_user_visible_change_from_editing_goal -- --nocapture`
- `cargo test -p forktty-socket task_strategy_plan_explicit_user_visible_false_overrides_inference -- --nocapture`
- `cargo test -p forktty-core task_strategy -- --nocapture` after adding
  candidate strategy scoring
- `cargo test -p forktty-socket task_strategy_plan_returns_read_only_strategy -- --nocapture`
- `cargo test -p forktty-core profile -- --nocapture` and
  `cargo test -p forktty-core task_strategy -- --nocapture` after adding router
  profiles
- `cargo test -p forktty-socket router_profile -- --nocapture` after adding
  socket `router_profile` parsing
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty -- --nocapture`
  after adding CLI `--profile` and MCP `router_profile` forwarding
- `cargo test -p forktty-core cooldown -- --nocapture`
- `cargo test -p forktty-core lockout -- --nocapture`
- `cargo test -p forktty-socket harness_signals -- --nocapture`
- `cargo test -p forktty-ui-gtk task_plan_requests_task_strategy_plan --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-ui-gtk task_strategy_plan_tool_maps_to_socket_method --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-core last_known_good -- --nocapture`
- `cargo test -p forktty-socket last_known_good -- --nocapture`
- `cargo test -p forktty-core task_strategy -- --nocapture` after adding
  advisory LKGP scoring
- `cargo test -p forktty-socket task_strategy -- --nocapture` after adding
  socket `last_known_good` parsing
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty -- --nocapture`
  after adding CLI/MCP `last_known_good` forwarding
- `cargo test -p forktty-ui-gtk mcp_server --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-ui-gtk socket_cli --no-default-features --features gtk-ghostty -- --nocapture`
- In `/home/simone/forktty-site`: `npm test` and `npm run build` after
  documenting LKGP
- `target/debug/forktty skills setup agents --dry-run --json` and
  `target/debug/forktty skills setup claude --dry-run --json` after rebuilding
  the embedded skill source; current source checksum:
  `fnv1a64:582ddfd946304ec1`
- `cargo clippy -p forktty-core -p forktty-socket -p forktty-ui-gtk --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
- `target/debug/forktty skills setup agents --dry-run --json` and
  `target/debug/forktty skills setup claude --dry-run --json` after rebuilding
  the embedded skill source; current source checksum:
  `fnv1a64:2cdc2d85d67c7ebb`
- `cargo test -p forktty-socket task_strategy_plan_infers_last_known_good_from_completed_workflow -- --nocapture`
- `cargo test -p forktty-socket task_strategy_plan_ -- --nocapture`
- `cargo test -p forktty-core task_strategy -- --nocapture`
- `cargo test -p forktty-socket task_strategy -- --nocapture`
- `cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-ui-gtk mcp_server --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo test -p forktty-ui-gtk socket_cli --no-default-features --features gtk-ghostty -- --nocapture`
- `cargo clippy -p forktty-core -p forktty-socket -p forktty-ui-gtk --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
- `cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty`
- `cargo run -p xtask -- check`
- In `/home/simone/forktty-site`: `npm test`, `npm run build`, and
  `git diff --check` after documenting inferred LKGP
- `target/debug/forktty skills setup agents --dry-run --json` and
  `target/debug/forktty skills setup claude --dry-run --json` after rebuilding
  the embedded skill source; current source checksum:
  `fnv1a64:c8efb243753bdafc`
- `cargo fmt --all --check`
- `git diff --check`
- `FORKTTY_SKIP_GTK_WIDGET_TESTS=1 cargo test --workspace --all-targets --no-default-features --features gtk-ghostty`
  passed with 873 tests passed, 1 ignored, 0 failed.
- `cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings`
- `cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser -- --test-threads=1`
  passed with 894 tests passed, 2 ignored, 0 failed.
- `cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings`
- `cargo build -p forktty-ui-gtk --no-default-features --features browser`

A local full workspace test without `FORKTTY_SKIP_GTK_WIDGET_TESTS=1` hit the
known headless GTK widget-test SIGSEGV described in `AGENTS.md`; the CI-parity
rerun above passed.

Remaining before PR/merge: decide whether to split the read-only planner,
staged apply, and visible submit support into separate commits. Future owner
review is still needed before adding worktree creation, push/merge/destructive
approvals, or dedicated approval UX to apply.

Claude reviewed the first draft in team mode on June 27, 2026. The blocking feedback has been folded into this revision: use the real `provider_capabilities` shape, use `DispatchError::InvalidParam`, register CLI boolean flags correctly, use the existing MCP test helpers, and keep Phase 1 honest about client-provided risk hints versus future context auto-detection.

Research repos were cloned for product comparison under `/tmp/forktty-product-research-current`:

- `cmux`: strong decision Feed and hook-to-socket attention model.
- `vibe-kanban`: workflow-first task/workspace/review lifecycle.
- `claude-squad`: simple session/worktree mental model.
- `workmux`: one task maps naturally to one worktree/window; agent delegation via skills.
- `AgentHub`: closest planner pattern; advisory plans grounded in real installed harness capabilities.
- `batty`: state-driven orchestrator and intervention log; more autonomous than ForkTTY should ship first.
- `vibetunnel`: useful future follow-mode idea for worktrees.
- `open-vibe-island`: status/permission surface and hook normalization.
- `emdash`: desktop parallel-agent worktree lifecycle and review/PR flow.
- `OmniRoute`: useful router/product pattern for zero-config `auto/*`
  selection, weighted factor scoring, mode packs, last-known-good stickiness,
  and health/cooldown/lockout separation. ForkTTY should adapt the idea as
  explainable harness/mode scoring, not copy OmniRoute's model-provider
  fallback strategies wholesale.

OmniRoute detail check, June 28, 2026:

- Cloned/updated repo:
  `/tmp/forktty-product-research-current/OmniRoute` at
  `bc9d184 chore(ci): Trivy advisory scan ignores unfixed CVEs (Security-tab noise) (#5234)`.
- Relevant files inspected:
  `docs/routing/AUTO-COMBO.md`,
  `docs/architecture/RESILIENCE_GUIDE.md`,
  `open-sse/services/autoCombo/scoring.ts`,
  `open-sse/services/autoCombo/modePacks.ts`,
  `open-sse/services/autoCombo/autoPrefix.ts`,
  `open-sse/services/autoCombo/virtualFactory.ts`, and
  `src/lib/usage/comboScoringInspector.ts`.
- Useful product pattern: the user can say `auto`, `auto/coding`,
  `auto/fast`, `auto/cheap`, etc.; the product builds a candidate pool at
  request time and returns the best route without making the user choose a
  routing primitive manually.
- Useful engineering pattern: candidate selection is a weighted, explainable
  score over normalized factors, with mode packs that bias the same scorer
  toward fast, cheap, reliable, quality-first, or offline/headroom behavior.
- Useful observability pattern: the scoring inspector reports factor sources
  and notes, so users and agents can see why one route beat another instead of
  treating routing as magic.
- Useful resilience pattern: OmniRoute keeps provider circuit breaker,
  connection cooldown, and model lockout as separate scopes. For ForkTTY the
  equivalent should be separate harness/runtime health, worker/session
  cooldown, and task/mode lockout signals; do not collapse them into one
  generic "provider failed" flag.
- Useful stickiness pattern: LKGP (last known good path) biases toward the
  previous successful provider without making it permanent. For ForkTTY this
  maps to a future "last successful harness/mode for this repo/task class"
  factor, bounded by current health and capabilities.
- Not directly portable: OmniRoute routes stateless model/API calls; ForkTTY
  coordinates visible terminal panes, user approvals, dirty worktrees, hooks,
  MCP, and human-observable workflow state. ForkTTY must keep explicit
  approvals, visible workers, no hidden scheduler, and local security
  boundaries even if the scoring model becomes more automatic.

ForkTTY design consequence from the OmniRoute check:

- Keep `task.strategy.plan` as the `auto` entry point for agents and humans.
- Extend the already-added strategy `candidate_scores` with harness assignment
  scoring next: score each routable harness per role using readiness, provider
  order, plan-mode support, MCP/hooks/resume support, worktree-cwd support, and
  max parallel sessions.
- Mode-pack style policy is now represented as named router profiles, not
  separate routers: `balanced`, `fast`, `conservative`, `parallel`, and
  `review_heavy` reweight the same candidate strategy factors.
- Basic scoring now includes separate readiness, cooldown, and task/mode
  lockout signals for harness assignment.
- LKGP is now represented as explicit advisory `last_known_good` input that
  adds small explainable strategy/harness score factors without hard override.
  When omitted, ForkTTY now infers LKGP from completed task-strategy workflow
  history for the selected workspace.

External best-practice anchors:

- Anthropic effective agents: keep systems simple, transparent, and tool interfaces well documented.
- Anthropic multi-agent research: use multi-agent only when parallelism/context/tool complexity justifies cost; many coding tasks are less parallelizable than research.
- OpenAI Agents SDK orchestration: combine code-driven orchestration with LLM decisions when appropriate.
- OpenAI HITL and tracing: approvals should pause/resume a run and traces should capture tools, handoffs, and guardrails.
- LangChain multi-agent docs: distinguish router, supervisor/subagents, skills, and handoffs; context engineering is central.
- Google ADK patterns: specialization and explicit workflows are more reliable than a single overloaded agent.

## Product Decisions

- Default product behavior is **automatic routing, supervised execution**.
- ForkTTY should eventually auto-detect context, classify the task, select a strategy, and explain why.
- Phase 1 is intentionally narrower: it grounds harness selection in
  `system.capabilities`, maps explicit client hints to
  strategy/layers/approvals, infers simple dirty git state from the selected
  surface/workspace cwd, infers likely edit intent from goal wording, returns
  ranked candidate strategy scores plus role-specific harness assignment scores
  with factor breakdowns, selects or infers a router profile, accepts explicit
  last-known-good and per-harness cooldown/lockout signals from callers with
  runtime evidence, infers last-known-good strategy/harness evidence from
  completed task-strategy workflows when explicit evidence is omitted, and does
  not yet infer every risk signal itself.
- ForkTTY should ask once before starting real execution: `Start run`.
- After approval, ForkTTY can create visible panes, workflows, tasks, loop metadata, and run allowed verification commands.
- ForkTTY must ask again only when risk increases: push, merge, delete, destructive commands, secrets, global installs, extra paid/parallel workers, or out-of-scope file edits.
- Multi-agent is not the default. Use it only when there are independent subtasks, large context/research, useful review, alternative implementations, high task value, or explicit user request.
- Multi-harness means roles, not a flat list of processes: `implementer`, `reviewer`, `researcher`, `verifier`, `synthesizer`.
- Workflows and loops remain coordination metadata. Do not add a hidden background scheduler.
- Team workers must be launched visibly in terminal panes. Existing task-first ordering matters: create/update team task, launch worker, then dispatch prompt.

## Strategy Vocabulary

Use these strategy ids in code, JSON, CLI text, docs, and tests:

- `solo`
- `solo_tracked`
- `solo_with_verify_loop`
- `implementer_plus_reviewer`
- `parallel_research`
- `parallel_experiment`
- `team_pipeline`
- `review_only`

Use these task class ids:

- `tiny_answer`
- `repo_inspection`
- `focused_bugfix`
- `feature_implementation`
- `review_only`
- `parallel_research`
- `parallel_experiment`
- `verify_fix_loop`
- `long_running_team_run`

Phase 1's deterministic classifier may emit only a subset of this vocabulary. Any id that remains explicit/future-only must be documented in tests so agents do not assume every public id is automatically inferred from free text.

Use these harness role ids:

- `implementer`
- `reviewer`
- `researcher`
- `verifier`
- `synthesizer`

## Non-Goals For The First Implementation

- No auto-merge.
- No auto-push.
- No hidden scheduler.
- No LLM-only opaque router.
- No provider/model ranking based on reputation.
- No destructive action without explicit approval.
- No GTK UI changes in Phase 1 unless needed to keep existing tests compiling.
- No forktty-site update only while the change remains internal/SPEC-only. If README-facing behavior is added, update `forktty-site` in the same task or record the exact pending site update.

## File Map

Create:

- `crates/forktty-core/src/task_strategy.rs` - pure strategy types and deterministic planning rules.
- `crates/forktty-socket/src/task_strategy_params.rs` - JSON-RPC parameter parsing for `task.strategy.plan`.
- `crates/forktty-socket/src/task_strategy_runtime.rs` - read-only socket runtime that combines params, `system.capabilities`, and `context.snapshot`.
- `crates/forktty-ui-gtk/src/socket_cli/task.rs` - CLI handlers and concise human formatting for task strategy planning.
- `crates/forktty-socket/src/tests/task_strategy.rs` - socket regression tests for the method and capability registry.

Modify:

- `crates/forktty-core/src/lib.rs` - export task strategy types/functions.
- `crates/forktty-socket/src/lib.rs` - add modules.
- `crates/forktty-socket/src/methods.rs` - expose `task.strategy.plan`.
- `crates/forktty-socket/src/dispatcher.rs` - dispatch `task.strategy.plan`.
- `crates/forktty-socket/src/tests/protocol_dispatch.rs` - keep adversarial dispatch coverage green.
- `crates/forktty-ui-gtk/src/socket_cli.rs` - add `task` module imports for tests.
- `crates/forktty-ui-gtk/src/socket_cli/router.rs` - route `task-plan`, `task:plan`, and `task.strategy.plan`.
- `crates/forktty-ui-gtk/src/socket_cli/help.rs` - add help text and completion command.
- `crates/forktty-ui-gtk/src/socket_cli/tests/status_workflow.rs` or new `tests/task_strategy.rs` - CLI parser/format tests.
- `crates/forktty-ui-gtk/src/cli.rs` - recognize direct socket-style aliases if needed.
- `crates/forktty-ui-gtk/src/mcp_server/tool_specs.rs` - add read-only `task_strategy_plan`.
- `crates/forktty-ui-gtk/src/mcp_server/tool_calls.rs` - validate MCP args and map to socket method.
- `crates/forktty-ui-gtk/src/mcp_server.rs` - extend MCP registry tests.
- `crates/forktty-ui-gtk/src/agent_guide.rs` - tell agents to call `task_strategy_plan` before choosing team/loop/worktree for non-trivial work.
- `.agents/skills/forktty-agent-orchestration/SKILL.md` - same policy for managed agents.
- `SPEC.md` - document `task.strategy.plan`.
- `README.md` - add short user-facing mention once CLI/MCP works.
- `CHANGELOG.md` - add entry under `## [Unreleased]`.

Phase 2 likely creates:

- `crates/forktty-socket/src/task_strategy_apply.rs` or extends `task_strategy_runtime.rs` with `apply`.
- `crates/forktty-ui-gtk/src/socket_cli/task.rs` with `task-start`.
- MCP tool `task_strategy_apply` only after confirmation semantics are pinned.

---

### Task 1: Core Strategy Model And Planner

**Files:**
- Create: `crates/forktty-core/src/task_strategy.rs`
- Modify: `crates/forktty-core/src/lib.rs`
- Test: unit tests inside `crates/forktty-core/src/task_strategy.rs`

- [ ] **Step 1: Write failing core tests**

Add `crates/forktty-core/src/task_strategy.rs` with tests first. The tests should compile-fail before implementation because the types do not exist yet.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn caps() -> HarnessRegistry {
        HarnessRegistry {
            harnesses: vec![
                HarnessCapability {
                    id: "codex".to_string(),
                    installed: true,
                    authenticated: true,
                    supports_prompt_launch: true,
                    supports_resume: true,
                    supports_hooks: true,
                    supports_mcp: true,
                    supports_plan_mode: false,
                    supports_worktree_cwd: true,
                    max_parallel_sessions: Some(4),
                    health: HarnessHealth::Ready,
                },
                HarnessCapability {
                    id: "claude".to_string(),
                    installed: true,
                    authenticated: true,
                    supports_prompt_launch: true,
                    supports_resume: true,
                    supports_hooks: true,
                    supports_mcp: true,
                    supports_plan_mode: true,
                    supports_worktree_cwd: true,
                    max_parallel_sessions: Some(4),
                    health: HarnessHealth::Ready,
                },
            ],
        }
    }

    #[test]
    fn focused_bugfix_routes_to_verify_loop_without_team() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Fix the browser feature test failure and run the relevant tests".to_string(),
            explicit_mode: None,
            repo_dirty: false,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::FocusedBugfix);
        assert_eq!(plan.strategy, TaskStrategy::SoloWithVerifyLoop);
        assert!(plan.layers.workflow);
        assert!(plan.layers.loop_metadata);
        assert!(!plan.layers.team);
        assert_eq!(plan.assignments[0].role, HarnessRole::Implementer);
        assert_eq!(plan.approvals, vec![TaskStrategyApproval::StartRun]);
    }

    #[test]
    fn requested_parallel_research_uses_researchers_and_synthesizer() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Compare three approaches for a multi-harness router".to_string(),
            explicit_mode: None,
            repo_dirty: false,
            user_requested_parallelism: true,
            user_requested_review: false,
            likely_user_visible_change: false,
            harness_registry: caps(),
        })
        .unwrap();

        assert_eq!(plan.task_class, TaskClass::ParallelResearch);
        assert_eq!(plan.strategy, TaskStrategy::ParallelResearch);
        assert!(plan.layers.workflow);
        assert!(plan.layers.team);
        assert!(!plan.layers.worktree);
        assert!(plan.assignments.iter().any(|a| a.role == HarnessRole::Researcher));
        assert!(plan.assignments.iter().any(|a| a.role == HarnessRole::Synthesizer));
    }

    #[test]
    fn dirty_repo_requires_worktree_approval_for_editing_strategy() {
        let plan = plan_task_strategy(TaskStrategyInput {
            goal: "Implement the task router".to_string(),
            explicit_mode: None,
            repo_dirty: true,
            user_requested_parallelism: false,
            user_requested_review: false,
            likely_user_visible_change: true,
            harness_registry: caps(),
        })
        .unwrap();

        assert!(plan.layers.worktree);
        assert!(plan.approvals.contains(&TaskStrategyApproval::CreateWorktree));
        assert!(plan.reasons.iter().any(|reason| reason.contains("dirty repo")));
    }

    #[test]
    fn phase_one_classifier_reachable_classes_are_explicit() {
        let reachable = phase_one_classifier_reachable_classes();
        assert_eq!(
            reachable,
            &[
                TaskClass::RepoInspection,
                TaskClass::FocusedBugfix,
                TaskClass::FeatureImplementation,
                TaskClass::ReviewOnly,
                TaskClass::ParallelResearch,
            ]
        );
    }
}
```

- [ ] **Step 2: Run the failing core test**

Run:

```bash
cargo test -p forktty-core task_strategy --no-default-features --features gtk-ghostty
```

Expected: fail because `task_strategy` is not exported and the planner types/functions are missing.

- [ ] **Step 3: Implement the core model**

Implement in `crates/forktty-core/src/task_strategy.rs`:

```rust
//! Task strategy routing for visible multi-harness agent work.
//!
//! This module is pure domain logic. It does not inspect the filesystem, launch
//! agents, create worktrees, or mutate workflow/team state.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    TinyAnswer,
    RepoInspection,
    FocusedBugfix,
    FeatureImplementation,
    ReviewOnly,
    ParallelResearch,
    ParallelExperiment,
    VerifyFixLoop,
    LongRunningTeamRun,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStrategy {
    Solo,
    SoloTracked,
    SoloWithVerifyLoop,
    ImplementerPlusReviewer,
    ParallelResearch,
    ParallelExperiment,
    TeamPipeline,
    ReviewOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessRole {
    Implementer,
    Reviewer,
    Researcher,
    Verifier,
    Synthesizer,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessHealth {
    Ready,
    Missing,
    Unauthenticated,
    Disabled,
    Error,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessCapability {
    pub id: String,
    pub installed: bool,
    pub authenticated: bool,
    pub supports_prompt_launch: bool,
    pub supports_resume: bool,
    pub supports_hooks: bool,
    pub supports_mcp: bool,
    pub supports_plan_mode: bool,
    pub supports_worktree_cwd: bool,
    pub max_parallel_sessions: Option<u32>,
    pub health: HarnessHealth,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessRegistry {
    pub harnesses: Vec<HarnessCapability>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyLayers {
    pub workflow: bool,
    pub team: bool,
    pub loop_metadata: bool,
    pub worktree: bool,
    pub feed: bool,
    pub mcp: bool,
    pub hooks: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStrategyApproval {
    StartRun,
    CreateWorktree,
    LaunchParallelWorkers,
    IncreaseRisk,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HarnessAssignment {
    pub role: HarnessRole,
    pub harness_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyInput {
    pub goal: String,
    pub explicit_mode: Option<TaskStrategy>,
    pub repo_dirty: bool,
    pub user_requested_parallelism: bool,
    pub user_requested_review: bool,
    pub likely_user_visible_change: bool,
    pub harness_registry: HarnessRegistry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskStrategyPlan {
    pub task_class: TaskClass,
    pub strategy: TaskStrategy,
    pub layers: TaskStrategyLayers,
    pub assignments: Vec<HarnessAssignment>,
    pub approvals: Vec<TaskStrategyApproval>,
    pub reasons: Vec<String>,
    pub safety_notes: Vec<String>,
}

pub fn plan_task_strategy(input: TaskStrategyInput) -> Result<TaskStrategyPlan, String> {
    let goal = input.goal.trim();
    if goal.is_empty() {
        return Err("goal must not be empty".to_string());
    }

    let task_class = classify_task(goal, &input);
    let strategy = input
        .explicit_mode
        .clone()
        .unwrap_or_else(|| strategy_for_class(&task_class, &input));
    let mut layers = layers_for_strategy(&strategy);
    if input.repo_dirty && input.likely_user_visible_change {
        layers.worktree = true;
    }

    let assignments = assignments_for_strategy(&strategy, &input.harness_registry);
    if assignments.is_empty() {
        return Err("no routable harness can satisfy the selected strategy".to_string());
    }

    let mut approvals = vec![TaskStrategyApproval::StartRun];
    if layers.worktree {
        approvals.push(TaskStrategyApproval::CreateWorktree);
    }
    if assignments.len() > 1 {
        approvals.push(TaskStrategyApproval::LaunchParallelWorkers);
    }
    approvals.sort_by_key(approval_order);
    approvals.dedup();

    let mut reasons = vec![format!("classified task as {:?}", task_class)];
    if input.repo_dirty && input.likely_user_visible_change {
        reasons.push("dirty repo plus editing task requires worktree isolation".to_string());
    }
    if input.user_requested_parallelism {
        reasons.push("user requested parallelism or comparison".to_string());
    }
    if input.user_requested_review {
        reasons.push("user requested review coverage".to_string());
    }

    Ok(TaskStrategyPlan {
        task_class,
        strategy,
        layers,
        assignments,
        approvals,
        reasons,
        safety_notes: vec![
            "planning is read-only".to_string(),
            "applying this strategy must keep all agent work visible in ForkTTY panes".to_string(),
            "push, merge, destructive commands, and out-of-scope edits require a later approval".to_string(),
        ],
    })
}

fn approval_order(approval: &TaskStrategyApproval) -> u8 {
    match approval {
        TaskStrategyApproval::StartRun => 0,
        TaskStrategyApproval::CreateWorktree => 1,
        TaskStrategyApproval::LaunchParallelWorkers => 2,
        TaskStrategyApproval::IncreaseRisk => 3,
    }
}

fn classify_task(goal: &str, input: &TaskStrategyInput) -> TaskClass {
    let lower = goal.to_lowercase();
    if input.user_requested_parallelism
        || lower.contains("compare")
        || lower.contains("alternative")
        || lower.contains("approaches")
    {
        return TaskClass::ParallelResearch;
    }
    if lower.contains("review") || lower.contains("read-only") {
        return TaskClass::ReviewOnly;
    }
    if lower.contains("test") || lower.contains("verify") || lower.contains("fix") || lower.contains("bug") {
        return TaskClass::FocusedBugfix;
    }
    if lower.contains("implement") || lower.contains("add ") || lower.contains("build") {
        return TaskClass::FeatureImplementation;
    }
    TaskClass::RepoInspection
}

pub fn phase_one_classifier_reachable_classes() -> &'static [TaskClass] {
    &[
        TaskClass::RepoInspection,
        TaskClass::FocusedBugfix,
        TaskClass::FeatureImplementation,
        TaskClass::ReviewOnly,
        TaskClass::ParallelResearch,
    ]
}

fn strategy_for_class(task_class: &TaskClass, input: &TaskStrategyInput) -> TaskStrategy {
    match task_class {
        TaskClass::TinyAnswer => TaskStrategy::Solo,
        TaskClass::RepoInspection => TaskStrategy::SoloTracked,
        TaskClass::FocusedBugfix | TaskClass::VerifyFixLoop => TaskStrategy::SoloWithVerifyLoop,
        TaskClass::FeatureImplementation if input.user_requested_review => {
            TaskStrategy::ImplementerPlusReviewer
        }
        TaskClass::FeatureImplementation => TaskStrategy::SoloWithVerifyLoop,
        TaskClass::ReviewOnly => TaskStrategy::ReviewOnly,
        TaskClass::ParallelResearch => TaskStrategy::ParallelResearch,
        TaskClass::ParallelExperiment => TaskStrategy::ParallelExperiment,
        TaskClass::LongRunningTeamRun => TaskStrategy::TeamPipeline,
    }
}

fn layers_for_strategy(strategy: &TaskStrategy) -> TaskStrategyLayers {
    match strategy {
        TaskStrategy::Solo => TaskStrategyLayers {
            mcp: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::SoloTracked => TaskStrategyLayers {
            workflow: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        TaskStrategy::SoloWithVerifyLoop => TaskStrategyLayers {
            workflow: true,
            loop_metadata: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
        // Reviewer isolation defaults to a worktree even on a clean repo. If this
        // proves too heavy, change this policy and the approvals tests together.
        TaskStrategy::ImplementerPlusReviewer => TaskStrategyLayers {
            workflow: true,
            team: true,
            loop_metadata: true,
            worktree: true,
            feed: true,
            mcp: true,
            hooks: true,
        },
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment | TaskStrategy::TeamPipeline => {
            TaskStrategyLayers {
                workflow: true,
                team: true,
                feed: true,
                mcp: true,
                hooks: true,
                ..TaskStrategyLayers::default()
            }
        }
        TaskStrategy::ReviewOnly => TaskStrategyLayers {
            workflow: true,
            feed: true,
            mcp: true,
            hooks: true,
            ..TaskStrategyLayers::default()
        },
    }
}

fn assignments_for_strategy(
    strategy: &TaskStrategy,
    registry: &HarnessRegistry,
) -> Vec<HarnessAssignment> {
    let routable: Vec<&HarnessCapability> = registry
        .harnesses
        .iter()
        .filter(|harness| {
            harness.installed
                && harness.supports_prompt_launch
                && ((harness.authentication_known
                    && harness.authenticated
                    && harness.health == HarnessHealth::Ready)
                    || (!harness.authentication_known
                        && harness.health == HarnessHealth::Unknown))
        })
        .collect();
    let Some(primary) = routable.first() else {
        return Vec::new();
    };
    let mut assignments = vec![HarnessAssignment {
        role: match strategy {
            TaskStrategy::ReviewOnly => HarnessRole::Reviewer,
            TaskStrategy::ParallelResearch => HarnessRole::Researcher,
            _ => HarnessRole::Implementer,
        },
        harness_id: primary.id.clone(),
        reason: "first routable harness with prompt launch support".to_string(),
    }];
    if matches!(strategy, TaskStrategy::ImplementerPlusReviewer | TaskStrategy::TeamPipeline) {
        if let Some(second) = routable.iter().find(|harness| harness.id != primary.id) {
            assignments.push(HarnessAssignment {
                role: HarnessRole::Reviewer,
                harness_id: second.id.clone(),
                reason: "separate routable harness for review isolation".to_string(),
            });
        }
    }
    if matches!(strategy, TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment) {
        for harness in routable.iter().skip(1).take(2) {
            assignments.push(HarnessAssignment {
                role: HarnessRole::Researcher,
                harness_id: harness.id.clone(),
                reason: "additional routable harness for parallel context isolation".to_string(),
            });
        }
        assignments.push(HarnessAssignment {
            role: HarnessRole::Synthesizer,
            harness_id: primary.id.clone(),
            reason: "primary harness synthesizes worker results".to_string(),
        });
    }
    assignments
}
```

- [ ] **Step 4: Export the module**

Modify `crates/forktty-core/src/lib.rs`:

```rust
pub mod task_strategy;
```

Add exports near the other `pub use` blocks:

```rust
pub use task_strategy::{
    plan_task_strategy, HarnessAssignment, HarnessCapability, HarnessHealth, HarnessRegistry,
    HarnessRole, TaskClass, TaskStrategy, TaskStrategyApproval, TaskStrategyInput,
    TaskStrategyLayers, TaskStrategyPlan,
};
```

- [ ] **Step 5: Run core tests**

Run:

```bash
cargo fmt --all
cargo test -p forktty-core task_strategy --no-default-features --features gtk-ghostty
```

Expected: all `task_strategy` tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/forktty-core/src/lib.rs crates/forktty-core/src/task_strategy.rs
git commit -m "feat: add task strategy planner core"
```

---

### Task 2: Read-Only Socket Method

**Files:**
- Create: `crates/forktty-socket/src/task_strategy_params.rs`
- Create: `crates/forktty-socket/src/task_strategy_runtime.rs`
- Create: `crates/forktty-socket/src/tests/task_strategy.rs`
- Modify: `crates/forktty-socket/src/lib.rs`
- Modify: `crates/forktty-socket/src/methods.rs`
- Modify: `crates/forktty-socket/src/dispatcher.rs`
- Modify: `crates/forktty-socket/src/tests/protocol_dispatch.rs` only if adversarial params expose a real validation issue

- [ ] **Step 1: Write socket tests**

Create `crates/forktty-socket/src/tests/task_strategy.rs`:

```rust
//! Task strategy socket method regression tests.

use super::*;
use forktty_core::HarnessHealth;

#[tokio::test]
async fn task_strategy_plan_is_public_capability() {
    let capabilities = system_runtime::capabilities();
    let methods = capabilities["methods"].as_array().unwrap();
    assert!(methods.iter().any(|method| method == "task.strategy.plan"));
}

#[tokio::test]
async fn task_strategy_plan_returns_read_only_strategy() {
    let (state, _backend) = test_state();
    let result = dispatch(
        &state,
        "task.strategy.plan",
        json!({
            "goal": "Fix the failing workflow loop test and run verification",
            "repo_dirty": false,
            "likely_user_visible_change": true
        }),
    )
    .await
    .unwrap();

    assert_eq!(result["strategy"], "solo_with_verify_loop");
    assert_eq!(result["layers"]["workflow"], true);
    assert_eq!(result["layers"]["loop_metadata"], true);
    assert_eq!(result["layers"]["team"], false);
    assert!(result["approvals"].as_array().unwrap().contains(&json!("start_run")));
    assert!(result["safety_notes"].as_array().unwrap().iter().any(|note| {
        note.as_str().unwrap_or_default().contains("read-only")
    }));
}

#[test]
fn task_strategy_harness_registry_uses_real_provider_capability_shape() {
    let registry = crate::task_strategy_runtime::harness_registry_from_capabilities(&json!({
        "provider_capabilities": {
            "codex": {
                "team_worker_launch": true,
                "launchable": true,
                "safe_resume": true,
                "cwd_resume_flag": true,
                "available_on_path": true,
                "executable": "/usr/bin/codex",
                "disabled_by_config": false
            },
            "claude": {
                "team_worker_launch": true,
                "launchable": false,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": false,
                "executable": null,
                "disabled_by_config": false
            },
            "opencode": {
                "team_worker_launch": true,
                "launchable": false,
                "safe_resume": true,
                "cwd_resume_flag": false,
                "available_on_path": true,
                "executable": "/usr/bin/opencode",
                "disabled_by_config": true
            }
        }
    }));

    let codex = registry.harnesses.iter().find(|harness| harness.id == "codex").unwrap();
    assert!(codex.installed);
    assert!(!codex.authenticated);
    assert!(!codex.authentication_known);
    assert_eq!(codex.health, HarnessHealth::Unknown);
    assert!(codex.supports_resume);
    assert!(codex.supports_worktree_cwd);

    let claude = registry.harnesses.iter().find(|harness| harness.id == "claude").unwrap();
    assert!(!claude.installed);
    assert_eq!(claude.health, HarnessHealth::Missing);

    let opencode = registry.harnesses.iter().find(|harness| harness.id == "opencode").unwrap();
    assert!(opencode.installed);
    assert_eq!(opencode.health, HarnessHealth::Disabled);
}

#[tokio::test]
async fn task_strategy_plan_rejects_blank_goal() {
    let (state, _backend) = test_state();
    let err = dispatch(&state, "task.strategy.plan", json!({ "goal": " " }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert!(err.to_string().contains("goal"));
}
```

Register the test module in the existing socket test module list in `crates/forktty-socket/src/lib.rs`:

```rust
mod tests {
    // Add this inside the existing #[cfg(test)] test module near the other
    // socket regression modules; do not create a second outer tests module.
    mod task_strategy;
}
```

Use the existing local structure rather than duplicating an outer `mod tests` if the file already has one.

- [ ] **Step 2: Run the failing socket tests**

Run:

```bash
cargo test -p forktty-socket task_strategy --no-default-features --features gtk-ghostty
```

Expected: fail because `task.strategy.plan` is not registered.

- [ ] **Step 3: Add modules and method registry**

Modify `crates/forktty-socket/src/lib.rs` module list:

```rust
mod task_strategy_params;
mod task_strategy_runtime;
```

Modify `crates/forktty-socket/src/methods.rs` in `CORE_METHOD_SPECS`:

```rust
MethodSpec::public("task.strategy.plan"),
```

Place it near `system.capabilities` or before the `team.*` methods so capability output remains easy to scan.

- [ ] **Step 4: Parse task strategy params**

Create `crates/forktty-socket/src/task_strategy_params.rs`:

```rust
use crate::{optional_bool_param, optional_non_blank_string_param, required_trimmed_string, DispatchError};
use serde_json::Value;

pub(crate) struct TaskStrategyPlanParams {
    pub(crate) goal: String,
    pub(crate) explicit_strategy: Option<String>,
    pub(crate) repo_dirty: Option<bool>,
    pub(crate) user_requested_parallelism: bool,
    pub(crate) user_requested_review: bool,
    pub(crate) likely_user_visible_change: bool,
}

pub(crate) fn task_strategy_plan_params(
    params: &Value,
) -> Result<TaskStrategyPlanParams, DispatchError> {
    let goal = required_trimmed_string(params, "goal")?.to_string();
    let explicit_strategy = optional_non_blank_string_param(params, "strategy")?.map(str::to_string);
    let repo_dirty = optional_bool_param(params, "repo_dirty")?;
    let user_requested_parallelism =
        optional_bool_param(params, "user_requested_parallelism")?.unwrap_or(false);
    let user_requested_review =
        optional_bool_param(params, "user_requested_review")?.unwrap_or(false);
    let likely_user_visible_change =
        optional_bool_param(params, "likely_user_visible_change")?.unwrap_or(false);
    Ok(TaskStrategyPlanParams {
        goal,
        explicit_strategy,
        repo_dirty,
        user_requested_parallelism,
        user_requested_review,
        likely_user_visible_change,
    })
}
```

- [ ] **Step 5: Implement read-only runtime**

Create `crates/forktty-socket/src/task_strategy_runtime.rs`:

```rust
use crate::{
    system_runtime, task_strategy_params::task_strategy_plan_params, DispatchError, SocketAppState,
};
use forktty_core::{
    plan_task_strategy, HarnessCapability, HarnessHealth, HarnessRegistry, TaskStrategy,
    TaskStrategyInput,
};
use serde_json::Value;

pub(crate) async fn plan(_state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let params = task_strategy_plan_params(params)?;
    let capabilities = system_runtime::capabilities();
    let registry = harness_registry_from_capabilities(&capabilities);
    let explicit_mode = params
        .explicit_strategy
        .as_deref()
        .map(task_strategy_from_str)
        .transpose()?;
    let plan = plan_task_strategy(TaskStrategyInput {
        goal: params.goal,
        explicit_mode,
        repo_dirty: params.repo_dirty.unwrap_or(false),
        user_requested_parallelism: params.user_requested_parallelism,
        user_requested_review: params.user_requested_review,
        likely_user_visible_change: params.likely_user_visible_change,
        harness_registry: registry,
    })
    .map_err(DispatchError::InvalidParam)?;

    serde_json::to_value(plan)
        .map_err(|err| DispatchError::Other(format!("serialize task strategy plan: {err}")))
}

fn task_strategy_from_str(value: &str) -> Result<TaskStrategy, DispatchError> {
    match value {
        "solo" => Ok(TaskStrategy::Solo),
        "solo_tracked" => Ok(TaskStrategy::SoloTracked),
        "solo_with_verify_loop" => Ok(TaskStrategy::SoloWithVerifyLoop),
        "implementer_plus_reviewer" => Ok(TaskStrategy::ImplementerPlusReviewer),
        "parallel_research" => Ok(TaskStrategy::ParallelResearch),
        "parallel_experiment" => Ok(TaskStrategy::ParallelExperiment),
        "team_pipeline" => Ok(TaskStrategy::TeamPipeline),
        "review_only" => Ok(TaskStrategy::ReviewOnly),
        other => Err(DispatchError::InvalidParam(format!(
            "unsupported task strategy: {other}"
        ))),
    }
}

pub(crate) fn harness_registry_from_capabilities(capabilities: &Value) -> HarnessRegistry {
    let mut harnesses = Vec::new();
    if let Some(providers) = capabilities["provider_capabilities"].as_object() {
        for (id, provider) in providers {
            let launchable = provider["launchable"].as_bool().unwrap_or(false);
            let disabled = provider["disabled_by_config"].as_bool().unwrap_or(false);
            let available_on_path = provider["available_on_path"].as_bool().unwrap_or(false);
            let executable_present = !provider["executable"].is_null();
            let configured_command = !provider["configured_command"].is_null();
            let installed = available_on_path || executable_present || configured_command;
            harnesses.push(HarnessCapability {
                id: id.clone(),
                installed,
                authenticated: launchable,
                supports_prompt_launch: launchable
                    && provider["team_worker_launch"].as_bool().unwrap_or(false),
                supports_resume: provider["safe_resume"].as_bool().unwrap_or(false),
                supports_hooks: true,
                supports_mcp: true,
                // system.capabilities does not currently expose provider plan
                // mode. Keep false until provider_runtime exposes a real field;
                // do not infer it from a missing JSON key.
                supports_plan_mode: false,
                supports_worktree_cwd: provider["cwd_resume_flag"].as_bool().unwrap_or(false),
                max_parallel_sessions: None,
                health: if disabled {
                    HarnessHealth::Disabled
                } else if launchable {
                    HarnessHealth::Ready
                } else {
                    HarnessHealth::Missing
                },
            });
        }
    }
    HarnessRegistry { harnesses }
}
```

The shape above is pinned to `provider_runtime::capabilities`: `launchable`, `available_on_path`, `executable`, `configured_command`, `safe_resume`, `cwd_resume_flag`, `disabled_by_config`, and `team_worker_launch`. Do not use `team_worker_launch` as an installation signal; it is a capability flag and is true even when the executable is missing. If the router needs `supports_plan_mode`, extend `system.capabilities` deliberately and test the new field instead of reading a nonexistent `plan_mode` key.

- [ ] **Step 6: Dispatch the method**

Modify `crates/forktty-socket/src/dispatcher.rs` imports:

```rust
task_strategy_runtime,
```

Add match arm:

```rust
"task.strategy.plan" => task_strategy_runtime::plan(state, &params).await,
```

- [ ] **Step 7: Run socket verification**

Run:

```bash
cargo fmt --all
cargo test -p forktty-socket task_strategy --no-default-features --features gtk-ghostty
cargo test -p forktty-socket protocol_dispatch --no-default-features --features gtk-ghostty
```

Expected: task strategy tests pass; adversarial dispatch test does not panic.

- [ ] **Step 8: Commit**

```bash
git add crates/forktty-socket/src/lib.rs crates/forktty-socket/src/methods.rs crates/forktty-socket/src/dispatcher.rs crates/forktty-socket/src/task_strategy_params.rs crates/forktty-socket/src/task_strategy_runtime.rs crates/forktty-socket/src/tests/task_strategy.rs
git commit -m "feat: expose read-only task strategy socket plan"
```

---

### Task 3: CLI Entry Point

**Files:**
- Create: `crates/forktty-ui-gtk/src/socket_cli/task.rs`
- Modify: `crates/forktty-ui-gtk/src/socket_cli.rs`
- Modify: `crates/forktty-ui-gtk/src/socket_cli/router.rs`
- Modify: `crates/forktty-ui-gtk/src/socket_cli/help.rs`
- Modify: `crates/forktty-ui-gtk/src/cli.rs` if direct socket command recognition needs aliases
- Test: `crates/forktty-ui-gtk/src/socket_cli/tests/status_workflow.rs` or create `tests/task_strategy.rs`

- [ ] **Step 1: Write CLI formatting tests**

Add tests near existing socket CLI tests:

```rust
#[test]
fn task_strategy_plan_line_names_strategy_and_layers() {
    let line = format_task_strategy_plan_line(&json!({
        "strategy": "solo_with_verify_loop",
        "task_class": "focused_bugfix",
        "layers": {
            "workflow": true,
            "team": false,
            "loop_metadata": true,
            "worktree": false,
            "feed": true,
            "mcp": true,
            "hooks": true
        },
        "assignments": [
            {"role": "implementer", "harness_id": "codex", "reason": "ready"}
        ],
        "approvals": ["start_run"],
        "reasons": ["classified task as FocusedBugfix"]
    }));

    assert_eq!(
        line,
        "Strategy solo_with_verify_loop for focused_bugfix; layers workflow, loop, feed, mcp, hooks; implementer=codex; approvals start_run"
    );
}
```

- [ ] **Step 2: Run failing CLI test**

Run:

```bash
cargo test -p forktty-ui-gtk task_strategy_plan_line_names_strategy_and_layers --no-default-features --features gtk-ghostty
```

Expected: fail because the task CLI formatter does not exist.

- [ ] **Step 3: Add task CLI handler**

Create `crates/forktty-ui-gtk/src/socket_cli/task.rs`:

```rust
use super::{
    build_target_params, bool_option, non_blank_string_option, parse_flags, print_json,
    reject_unknown_options, send_socket_request, write_stdout_line, CliContext, CliResult,
};
use serde_json::{Map, Value};

pub(super) fn handle_task_plan(context: &CliContext, args: Vec<String>) -> CliResult<()> {
    let parsed = parse_flags(args, &["repo-dirty", "parallel", "review", "user-visible"]);
    reject_unknown_options(
        &parsed.options,
        &[
            "workspace-id",
            "workspace-name",
            "worktree-name",
            "surface-id",
            "strategy",
            "repo-dirty",
            "parallel",
            "review",
            "user-visible",
        ],
        "task-plan",
    )?;
    let goal = parsed.positionals.join(" ").trim().to_string();
    if goal.is_empty() {
        return Err(super::CliError::new("task-plan requires a goal"));
    }
    let mut params = build_target_params(&parsed.options, "task-plan")?;
    params.insert("goal".to_string(), Value::String(goal));
    insert_optional_flag(&parsed.options, &mut params, "repo-dirty", "repo_dirty")?;
    insert_optional_flag(
        &parsed.options,
        &mut params,
        "parallel",
        "user_requested_parallelism",
    )?;
    insert_optional_flag(&parsed.options, &mut params, "review", "user_requested_review")?;
    insert_optional_flag(
        &parsed.options,
        &mut params,
        "user-visible",
        "likely_user_visible_change",
    )?;
    if let Some(strategy) = non_blank_string_option(&parsed.options, "strategy", "--strategy")? {
        params.insert("strategy".to_string(), Value::String(strategy.trim().to_string()));
    }
    let result = send_socket_request(
        &context.socket_path,
        "task.strategy.plan",
        Value::Object(params),
    )?;
    if context.json {
        print_json(&result)
    } else {
        write_stdout_line(&format_task_strategy_plan_line(&result))
    }
}

fn insert_optional_flag(
    options: &std::collections::BTreeMap<String, super::FlagValue>,
    params: &mut Map<String, Value>,
    option: &str,
    param: &str,
) -> CliResult<()> {
    if let Some(value) = bool_option(options, option) {
        params.insert(param.to_string(), Value::Bool(value));
    }
    Ok(())
}

pub(super) fn format_task_strategy_plan_line(result: &Value) -> String {
    let strategy = result["strategy"].as_str().unwrap_or("unknown");
    let task_class = result["task_class"].as_str().unwrap_or("unknown");
    let layers = result["layers"]
        .as_object()
        .map(|layers| {
            [
                ("workflow", "workflow"),
                ("team", "team"),
                ("loop_metadata", "loop"),
                ("worktree", "worktree"),
                ("feed", "feed"),
                ("mcp", "mcp"),
                ("hooks", "hooks"),
            ]
            .into_iter()
            .filter_map(|(key, label)| layers.get(key).and_then(Value::as_bool).filter(|v| *v).map(|_| label))
            .collect::<Vec<_>>()
            .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string());
    let assignments = result["assignments"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(format!(
                        "{}={}",
                        item["role"].as_str()?,
                        item["harness_id"].as_str()?
                    ))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string());
    let approvals = result["approvals"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "none".to_string());
    format!(
        "Strategy {strategy} for {task_class}; layers {layers}; {assignments}; approvals {approvals}"
    )
}
```

Keep parser errors consistent with nearby CLI modules. `bool_option` returns `Option<bool>` and bare flags must be registered in the second `parse_flags` argument.

- [ ] **Step 4: Wire CLI module and router**

Modify `crates/forktty-ui-gtk/src/socket_cli.rs`:

```rust
mod task;
```

Add test-only imports:

```rust
#[cfg(test)]
use task::{format_task_strategy_plan_line, handle_task_plan};
```

Modify `crates/forktty-ui-gtk/src/socket_cli/router.rs` imports:

```rust
use super::task::handle_task_plan;
```

Add route:

```rust
"task-plan" | "task:plan" | "task.strategy.plan" => handle_task_plan(context, args),
```

- [ ] **Step 5: Update help and completions**

In `crates/forktty-ui-gtk/src/socket_cli/help.rs`, add:

```text
  forktty task-plan <goal> [workspace selectors] [--surface-id <id>] [--strategy <strategy>] [--repo-dirty] [--parallel] [--review] [--user-visible] [--json]
      Ask ForkTTY to choose a read-only strategy for a task before selecting team, loop, worktree, or harnesses.
```

Add `task-plan` to `COMPLETION_COMMANDS`.

- [ ] **Step 6: Run CLI verification**

Run:

```bash
cargo fmt --all
cargo test -p forktty-ui-gtk task_strategy --no-default-features --features gtk-ghostty
cargo test -p forktty-ui-gtk socket_cli --no-default-features --features gtk-ghostty
```

Expected: CLI task tests pass and existing socket CLI tests remain green.

- [ ] **Step 7: Commit**

```bash
git add crates/forktty-ui-gtk/src/socket_cli.rs crates/forktty-ui-gtk/src/socket_cli/task.rs crates/forktty-ui-gtk/src/socket_cli/router.rs crates/forktty-ui-gtk/src/socket_cli/help.rs crates/forktty-ui-gtk/src/socket_cli/tests
git commit -m "feat: add task strategy CLI plan"
```

---

### Task 4: MCP Tool And Agent Policy

**Files:**
- Modify: `crates/forktty-ui-gtk/src/mcp_server/tool_specs.rs`
- Modify: `crates/forktty-ui-gtk/src/mcp_server/tool_calls.rs`
- Modify: `crates/forktty-ui-gtk/src/mcp_server.rs`
- Modify: `crates/forktty-ui-gtk/src/agent_guide.rs`
- Modify: `.agents/skills/forktty-agent-orchestration/SKILL.md`

- [ ] **Step 1: Write MCP registry tests**

Extend tests in `crates/forktty-ui-gtk/src/mcp_server.rs`:

```rust
#[test]
fn task_strategy_plan_tool_is_read_only() {
    let names = tool_specs()
        .iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert!(names.contains(&"task_strategy_plan"));
    assert_eq!(annotation("task_strategy_plan")["readOnlyHint"], true);
    assert_eq!(annotation("task_strategy_plan")["openWorldHint"], false);
}

#[test]
fn task_strategy_plan_tool_maps_to_socket_method() {
    let (method, params) = build_socket_call_for_test(
        "task_strategy_plan",
        json!({
            "goal": "Fix the bug and verify tests",
            "repo_dirty": true,
            "user_visible": true
        }),
    )
    .unwrap();
    assert_eq!(method, "task.strategy.plan");
    assert_eq!(params["goal"], "Fix the bug and verify tests");
    assert_eq!(params["repo_dirty"], true);
    assert_eq!(params["likely_user_visible_change"], true);
}
```

Use the existing helper names in `mcp_server.rs`: `tool_specs()`, `annotation(...)`, and `build_socket_call_for_test(...)`.

- [ ] **Step 2: Run failing MCP tests**

Run:

```bash
cargo test -p forktty-ui-gtk task_strategy_plan_tool --no-default-features --features gtk-ghostty
```

Expected: fail because `task_strategy_plan` is not in the registry.

- [ ] **Step 3: Add MCP tool spec**

In `crates/forktty-ui-gtk/src/mcp_server/tool_specs.rs`, add:

```rust
ToolSpec {
    name: "task_strategy_plan",
    annotations: read_only_annotations(),
    description: "Ask ForkTTY to choose a read-only task strategy before selecting team, workflow, loop, worktree, hooks, MCP, or harnesses. Use this for non-trivial tasks instead of guessing a mode.",
    input_schema: object_schema(
        &["goal"],
        json!({
            "goal": string_prop("User task or desired outcome."),
            "strategy": string_prop("Optional explicit strategy override: solo, solo_tracked, solo_with_verify_loop, implementer_plus_reviewer, parallel_research, parallel_experiment, team_pipeline, review_only."),
            "repo_dirty": boolean_prop("Whether the repository has uncommitted changes and editing should prefer worktree isolation."),
            "parallel": boolean_prop("True when the user explicitly requested parallelism, comparison, or independent approaches."),
            "review": boolean_prop("True when the user requested or the task requires a separate reviewer role."),
            "user_visible": boolean_prop("True when the task is likely to change user-visible behavior, docs, CLI output, UI, packaging, or public docs."),
        }),
    ),
},
```

- [ ] **Step 4: Map MCP call to socket**

In `crates/forktty-ui-gtk/src/mcp_server/tool_calls.rs`, add match arm:

```rust
"task_strategy_plan" => {
    reject_unexpected(
        args,
        &["goal", "strategy", "repo_dirty", "parallel", "review", "user_visible"],
        name,
    )?;
    let mut params = Map::new();
    params.insert(
        "goal".to_string(),
        Value::String(required_non_empty_string(args, "goal")?),
    );
    insert_optional_non_blank_param(args, &mut params, "strategy")?;
    insert_optional_bool_param(args, &mut params, "repo_dirty")?;
    if let Some(value) = args.get("parallel").and_then(Value::as_bool) {
        params.insert("user_requested_parallelism".to_string(), Value::Bool(value));
    }
    if let Some(value) = args.get("review").and_then(Value::as_bool) {
        params.insert("user_requested_review".to_string(), Value::Bool(value));
    }
    if let Some(value) = args.get("user_visible").and_then(Value::as_bool) {
        params.insert("likely_user_visible_change".to_string(), Value::Bool(value));
    }
    SocketCall {
        method: "task.strategy.plan",
        params,
    }
}
```

Add result text:

```rust
"task_strategy_plan" => "Planned ForkTTY task strategy.".to_string(),
```

- [ ] **Step 5: Update agent policy**

In `crates/forktty-ui-gtk/src/agent_guide.rs`, add policy text near the read-only-first guidance:

```text
Before choosing team, workflow loop, worktree, or multi-harness execution for a non-trivial user task, call task_strategy_plan with the user's goal and current risk signals. Treat the returned strategy as the default operating plan unless the user explicitly overrides it. Do not launch team workers or create worktrees merely because those tools exist.
```

In `.agents/skills/forktty-agent-orchestration/SKILL.md`, add the same operational rule in the first section that tells agents how to inspect state.

- [ ] **Step 6: Run MCP and skill embedding checks**

Run:

```bash
cargo fmt --all
cargo test -p forktty-ui-gtk task_strategy_plan_tool --no-default-features --features gtk-ghostty
cargo test -p forktty-ui-gtk mcp_server --no-default-features --features gtk-ghostty
cargo run -p xtask -- check
```

Because the managed skill is embedded, after building the final binary for this branch also run:

```bash
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
target/debug/forktty skills setup agents --dry-run --json
target/debug/forktty skills setup claude --dry-run --json
```

Expected: tests pass, xtask passes, skill dry-runs report the updated embedded checksums from the built binary.

- [ ] **Step 7: Commit**

```bash
git add crates/forktty-ui-gtk/src/mcp_server/tool_specs.rs crates/forktty-ui-gtk/src/mcp_server/tool_calls.rs crates/forktty-ui-gtk/src/mcp_server.rs crates/forktty-ui-gtk/src/agent_guide.rs .agents/skills/forktty-agent-orchestration/SKILL.md
git commit -m "feat: expose task strategy planning to MCP agents"
```

---

### Task 5: Specs, README, Changelog

**Files:**
- Modify: `SPEC.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/agents.md` if it describes current MCP/agent guidance
- Modify: `/home/simone/forktty-site` docs/agent context if README-facing behavior is added, or record the exact site follow-up if the site checkout is unavailable

- [ ] **Step 1: Document socket contract in SPEC**

Add a `task.strategy.plan` entry near other socket method documentation:

```markdown
### `task.strategy.plan`

Read-only task strategy planner. The method accepts a user `goal` plus optional
risk hints and returns ForkTTY's recommended strategy before an agent chooses
team, workflow loop, worktree, MCP, hooks, or harnesses.

Request:

```json
{
  "goal": "Fix the failing workflow loop test and verify it",
  "strategy": "solo_with_verify_loop",
  "repo_dirty": false,
  "user_requested_parallelism": false,
  "user_requested_review": false,
  "likely_user_visible_change": true
}
```

Response:

```json
{
  "task_class": "focused_bugfix",
  "strategy": "solo_with_verify_loop",
  "layers": {
    "workflow": true,
    "team": false,
    "loop_metadata": true,
    "worktree": false,
    "feed": true,
    "mcp": true,
    "hooks": true
  },
  "assignments": [
    {
      "role": "implementer",
      "harness_id": "codex",
      "reason": "first routable harness with prompt launch support"
    }
  ],
  "approvals": ["start_run"],
  "reasons": ["classified task as FocusedBugfix"],
  "safety_notes": [
    "planning is read-only",
    "applying this strategy must keep all agent work visible in ForkTTY panes",
    "push, merge, destructive commands, and out-of-scope edits require a later approval"
  ]
}
```

The planner never launches processes, mutates workflow/team state, creates
worktrees, or schedules background work.
```

- [ ] **Step 2: Add README mention**

Add a short paragraph to the agent/MCP usage area:

```markdown
ForkTTY agents can ask the local task router for a strategy before choosing
manual modes. `forktty task-plan "fix this bug and verify it" --json` and the
MCP `task_strategy_plan` tool return whether the task should stay solo, use a
workflow loop, add a reviewer, create a team, or isolate work in a worktree.
The first planner is read-only; execution still happens in visible panes and
risky actions require approval.
```

- [ ] **Step 3: Add changelog entry**

Under `CHANGELOG.md` `## [Unreleased]` and the proper heading:

```markdown
### Added

- Added a read-only task strategy planner for agents and CLI users so ForkTTY
  can recommend when to use solo work, workflow loops, reviewers, teams,
  worktrees, MCP, and hooks before launching visible agent work.
```

- [ ] **Step 4: Sync forktty-site or record the exact follow-up**

If `README.md` or other public docs mention the task router, update the public site checkout in `/home/simone/forktty-site` in the same implementation pass. At minimum update the agent/MCP docs or public crawler context that describes how agents should choose team/loop/worktree modes.

Run in `/home/simone/forktty-site`:

```bash
npm test
npm run build
```

Expected: both pass. If the checkout is unavailable or the build is blocked by a local environment limitation, record the exact file/content update still needed and the exact command failure in the final implementation handoff.

- [ ] **Step 5: Run docs checks**

Run:

```bash
cargo run -p xtask -- check
git diff --check
```

Expected: xtask and whitespace checks pass.

- [ ] **Step 6: Commit**

```bash
git add SPEC.md README.md CHANGELOG.md docs/agents.md
git commit -m "docs: describe task strategy planner"
```

If `/home/simone/forktty-site` was updated, commit or report that checkout separately; do not try to add site files from the ForkTTY repo.

---

### Task 6: Phase 2 Confirmed Apply Design

**Files:**
- Modify: `docs/task-router-multi-harness-plan.md`
- The first `task.strategy.apply` implementation now supports staged apply,
  approved visible submit for team plans, and worktree-layer apply only when
  `worktree_name` points at an already-open ForkTTY worktree workspace. It also
  supports request-bound Feed-backed start-run approval requests via
  `request_approval` and the returned approved `approval_id`. Do not add
  automatic worktree creation, destructive
  execution, push/merge, or hidden scheduling until the next approval UX/storage
  decision is reviewed.

- [x] **Step 1: Write apply design spec**

Keep this design in this tracked handoff file with this contract:

```markdown
# Task Strategy Apply Design

`task.strategy.apply` starts from a previously returned `task.strategy.plan`
and applies only visible setup. With `submit` omitted or false, it stages
coordination state. With `submit=true`, it can start supported team plans by
launching visible workers and dispatching role prompts. Worktree-layer plans may
only apply when `worktree_name` names an already-open ForkTTY worktree
workspace. Team-layer plans require at least one assignment before any staged or
submitted mutation. When approvals are missing, `request_approval` records a
deterministic request-bound Feed approval and returns blocked without mutating
workflow/team state; a later apply can pass the returned approved `approval_id`
only for the same run id, goal, plan, target scope, and submit mode. Apply
derives `start_run`, `create_worktree`, and multi-worker submit
`launch_parallel_workers` requirements from the requested operation and plan
shape before trusting the plan's approval list:

- create or update a workflow
- write workflow plan steps
- set workflow loop metadata
- create or update a team
- create team tasks before launching workers
- launch visible worker panes
- dispatch initial worker prompts
- create notification/feed entries for approvals

It must not:

- push
- merge
- delete worktrees
- delete branches
- run destructive shell commands
- install global dependencies
- hide execution in a background scheduler

Required approvals:

- `start_run` before any apply call mutates state
- `create_worktree` before creating or attaching a worktree
- `launch_parallel_workers` before launching more than one worker
- later approval for any increased-risk action

Team apply ordering:

1. `team.upsert`
2. `team.task.upsert`
3. `team.worker.launch`
4. `team.task.upsert` with `assigned_worker_id`
5. `team.message.send`
6. `team.message.dispatch`

This ordering avoids known failures where assigning a nonexistent worker or
launching before task creation produces misleading team state.
```

- [x] **Step 2: Review apply design against existing primitives**

Check these files before implementation:

```bash
rg -n "team.task.upsert|team.worker.launch|team.message.dispatch|workflow.loop.set|worktree.create" crates/forktty-socket crates/forktty-ui-gtk/src
```

Expected: every apply action maps to an existing visible socket primitive; missing primitives are written as explicit follow-up requirements in the apply design spec.

Result: workflow, team, worker launch, message send/dispatch, and worktree
create/attach primitives exist. The apply design records one missing decision:
whether approvals should reuse notification/feed approval semantics or gain a
dedicated approval primitive.

- [ ] **Step 3: Commit**

```bash
git add docs/task-router-multi-harness-plan.md
git commit -m "docs: design confirmed task strategy apply"
```

---

### Task 7: Full Verification Before PR

**Files:**
- No new files unless earlier tasks found a documented gap.

- [x] **Step 1: Run formatting and repo consistency**

```bash
cargo fmt --all --check
cargo run -p xtask -- check
```

Expected: both commands pass.

- [x] **Step 2: Run core/socket/UI tests for gtk-ghostty**

```bash
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
```

Expected: all tests pass.

- [x] **Step 3: Run clippy for gtk-ghostty**

```bash
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
```

Expected: no clippy warnings.

- [x] **Step 4: Run browser feature parity checks if local GTK/WebKit deps exist**

```bash
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser -- --test-threads=1
cargo clippy -p forktty-ui-gtk --all-targets --no-default-features --features browser -- -D warnings
```

Expected: both pass, or record the exact missing local dependency that prevents running them.

- [x] **Step 5: Verify skill embedding from final binary**

```bash
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
target/debug/forktty skills setup agents --dry-run --json
target/debug/forktty skills setup claude --dry-run --json
```

Expected: dry-runs use the rebuilt binary and report embedded skill payloads matching the edited files.

- [x] **Step 6: Run whitespace check**

```bash
git diff --check
```

Expected: no whitespace errors.

- [x] **Step 7: Summarize exact verification**

Final implementation handoff must include:

```text
Implemented:
- task.strategy.plan core model
- socket method
- CLI task-plan
- MCP task_strategy_plan
- agent policy/docs

Verified:
- cargo fmt --all --check
- cargo run -p xtask -- check
- cargo test ...
- cargo clippy ...

Not run:
- <command> because <exact reason>
```

## Open Questions For Owner Review

- Should the CLI canonical command be `task-plan` or `task plan`? This plan uses `task-plan` because the current socket CLI router is flat.
- Should the read-only planner infer `repo_dirty` itself by calling git status, or should clients pass the hint? Implemented: callers may override with an explicit hint, otherwise the socket runtime infers dirty/conflict state from the selected surface/workspace effective project cwd while keeping the core planner pure.
- Should callers pass `likely_user_visible_change`/`user_visible`, or should ForkTTY infer it? Implemented: omitted values are inferred from goal wording, while explicit false remains an override for read-only or non-user-visible work.
- Should `system.capabilities` expose `supports_plan_mode` per provider, or should Phase 1 keep that field false until a later capability expansion?
- Should the public task-class vocabulary include future-only classes now, or should the first implementation publish only the classes the deterministic classifier can emit?
- Should Phase 2 apply create worktrees automatically after `start_run`, or should `create_worktree` always remain a second explicit approval? This plan keeps it as a separate approval.
- Should the first GTK surface be a small strategy chip in the existing Agent HUD, or a fuller Work Run panel? Defer until Phase 1 returns stable JSON.

## Agent Handoff Summary

Where we stopped: Phase 1 read-only routing, staged `task.strategy.apply`, and
approved visible submit for supported team plans are in the worktree. The
planner now has router profiles and separate per-harness routing signals:
cooldown is a soft score penalty, while task/mode lockout excludes assignment.
It also accepts advisory last-known-good strategy/harness evidence as a small
score bias; automatic LKGP extraction from completed workflow evidence remains
future work.
The apply
method requires explicit approvals, creates deterministic workflow/team/task/
message state, recomputes worktree and multi-worker submit approvals before
mutation, rejects team-layer plans with no assignments, launches visible
workers and dispatches prompts when `submit=true` is used on a supported team
plan, and allows worktree-layer apply only when `worktree_name` names an
already-open ForkTTY worktree workspace.
Missing start-run approvals can be requested through the Feed and consumed
later with the returned approved `approval_id` only for the same request.

Next best steps:

1. Review the diff and decide commit split: planner, staged apply, visible
   team submit, existing-open-worktree submit, Feed-backed start-run approval,
   and docs/site updates.
2. Review the next approval UX before adding automatic worktree creation,
   push/merge/destructive approvals, or richer run control to
   `task.strategy.apply`.
3. Add tests for any later launch/apply expansion before mutating panes or
   worktrees.
