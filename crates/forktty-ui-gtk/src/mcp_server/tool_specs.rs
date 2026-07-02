//! MCP tool registry, JSON schemas, and client annotation metadata.

use serde_json::{json, Value};

pub(super) struct ToolSpec {
    pub(super) name: &'static str,
    pub(super) description: &'static str,
    pub(super) input_schema: Value,
    pub(super) annotations: Value,
}

// MCP tool annotations (2025-03-26 spec): UX hints for clients, not a
// security boundary. Most ForkTTY tools act on the local instance only, but
// terminal-input tools can drive shells that touch files, networks, or other
// external systems, so they must opt into open-world/destructive hints.
fn read_only_annotations() -> Value {
    json!({ "readOnlyHint": true, "openWorldHint": false })
}

fn mutating_annotations(destructive: bool, idempotent: bool) -> Value {
    mutating_annotations_with_open_world(destructive, idempotent, false)
}

fn mutating_annotations_with_open_world(
    destructive: bool,
    idempotent: bool,
    open_world: bool,
) -> Value {
    json!({
        "readOnlyHint": false,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": open_world,
    })
}

pub(super) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "workspace_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY workspaces, including active workspace, branch, worktree, and focused surface information. Start here before targeting panes.",
            input_schema: object_schema(&[], json!({})),
        },
        ToolSpec {
            name: "workspace_create",
            annotations: mutating_annotations(false, false),
            description: "Open a new ForkTTY workspace on a directory. Use this to satisfy the worktree tools' open-workspace precondition before worktree_create.",
            input_schema: object_schema(
                &["working_dir"],
                json!({
                    "working_dir": string_prop("Directory the workspace opens in (the repo root for worktree work)."),
                    "name": string_prop("Workspace name; defaults to \"workspace\"."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_list",
            annotations: read_only_annotations(),
            description: "List panes/surfaces in a workspace. Defaults to FORKTTY_WORKSPACE_ID when launched from a ForkTTY pane; omit targeting to inspect that pane's workspace.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "context_snapshot",
            annotations: read_only_annotations(),
            description: "Return a compact read-only situational snapshot for one ForkTTY workspace: workspace, pane tree, surfaces, status, agent health, compact workflow/loop/team/feed/remote summaries, and per-surface plus aggregate-bounded untrusted terminal tails.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "surface_id": string_prop("Surface id whose workspace should be inspected."),
                    "tail_lines": integer_prop("Terminal tail lines per terminal surface; 0 disables terminal text. Defaults to a compact bounded tail."),
                    "tail_max_bytes": integer_prop("Maximum UTF-8 bytes per terminal tail; socket also enforces aggregate surface and byte upper bounds."),
                    "include_team_details": boolean_prop("Include full team records with workers, tasks, and mailbox messages. Defaults to false; use team_summaries for compact monitoring."),
                    "include_workflow_details": boolean_prop("Include full workflow records with memory, plan steps, and evidence. Defaults to false; use workflow_summaries for compact monitoring."),
                    "include_feed_trace": boolean_prop("Include status/progress trace rows in the feed. Defaults to false; compact snapshots keep semantic notifications and approvals."),
                }),
            ),
        },
        ToolSpec {
            name: "task_strategy_plan",
            annotations: read_only_annotations(),
            description: "Ask ForkTTY to choose a read-only task strategy before selecting team, workflow, loop, worktree, hooks, MCP, or harnesses. The planner returns the selected router profile, ranked candidate strategy scores plus role-specific harness assignment scores with factor breakdowns, uses ForkTTY capabilities, configured team provider order as the harness tie-break, respects harness parallel-session capacity for multi-role plans, uses an explicit cwd from an open ForkTTY workspace/surface repo or selected workspace/surface cwd to infer simple repo context such as dirty state when repo_dirty is omitted, goal wording to infer likely user-visible edit intent and profile when omitted, and completed task-strategy workflow history for advisory last-known-good stickiness when explicit evidence is omitted. It includes explicit reviewer roles when a review strategy is selected. Use this for non-trivial tasks instead of guessing a mode.",
            input_schema: object_schema(
                &["goal"],
                json!({
                    "goal": string_prop("User task or desired outcome."),
                    "strategy": string_prop("Optional explicit strategy override: solo, solo_tracked, solo_with_verify_loop, implementer_plus_reviewer, parallel_research, parallel_experiment, team_pipeline, review_only."),
                    "router_profile": string_prop("Optional router profile: balanced, fast, conservative, parallel, review_heavy. When omitted, ForkTTY infers a profile from clear goal wording and request hints."),
                    "last_known_good": {
                        "type": "object",
                        "description": "Optional advisory last-known-good routing evidence. May include strategy, harness_id, and reason. When omitted, ForkTTY can infer evidence from completed task-strategy workflows in the selected workspace. Explicit evidence wins over inferred history. It adds a small explainable score bias and never overrides readiness, cooldown, lockout, approvals, or task fit.",
                        "additionalProperties": true
                    },
                    "harness_signals": {
                        "type": "object",
                        "description": "Optional per-harness routing signals keyed by harness id. Each value may include cooldown, cooldown_reason, locked_out, and lockout_reason. Cooldown is a soft penalty; locked_out excludes that harness from assignments.",
                        "additionalProperties": true
                    },
                    "workspace_id": string_prop("Workspace id whose focused surface/project cwd should inform planning. Defaults from the ForkTTY caller context when available."),
                    "workspace_name": string_prop("Workspace name whose focused surface/project cwd should inform planning."),
                    "worktree_name": string_prop("Worktree workspace name whose focused surface/project cwd should inform planning."),
                    "surface_id": string_prop("Surface id whose effective project cwd should inform planning."),
                    "cwd": string_prop("Absolute repo path inside a Git repository already represented by an open ForkTTY workspace, surface, or effective project cwd."),
                    "repo_dirty": boolean_prop("Whether the repository has uncommitted changes and editing should prefer worktree isolation. When omitted, ForkTTY tries to infer this from cwd or the selected surface/workspace cwd."),
                    "parallel": boolean_prop("True when the user explicitly requested parallelism, comparison, or independent approaches."),
                    "review": boolean_prop("True when the user requested or the task requires a separate reviewer role."),
                    "user_visible": boolean_prop("Optional override for whether the task is likely to change user-visible behavior, docs, CLI output, UI, packaging, or public docs. When omitted, ForkTTY infers this from the goal text."),
                }),
            ),
        },
        ToolSpec {
            name: "task_strategy_apply",
            annotations: mutating_annotations_with_open_world(true, false, true),
            description: "Apply a previously planned ForkTTY task strategy as visible workflow/team/task/message state. With submit omitted or false, this stages coordination state only. ForkTTY recomputes dirty-repo edit isolation from the selected surface/workspace plus any explicit cwd, then recomputes worktree approvals and multi-worker submit approvals from the requested operation and plan shape. Explicit cwd must be inside a Git repository already represented by an open ForkTTY workspace, surface, or effective project cwd. If required approvals are missing, request_approval can publish a pending Feed approval without starting work; a later call may pass the approved request-bound approval_id returned by that request, including remaining approvals that the same request covered when explicit attestations cover another part. With submit true and a supported team plan, ForkTTY launches visible worker panes and dispatches prompts through the team mailbox, but refuses to reuse a live deterministic worker whose harness, role, task, worktree, launch cwd, effective target cwd, or status no longer matches; worktree-layer plans require worktree_name for an already-open ForkTTY worktree workspace. This requires explicit approvals and never launches hidden background work.",
            input_schema: object_schema(
                &["run_id", "goal", "plan"],
                json!({
                    "run_id": string_prop("Stable run id used to derive deterministic workflow, team, task, and message ids."),
                    "goal": string_prop("User task or desired outcome."),
                    "plan": {
                        "type": "object",
                        "description": "The task.strategy.plan result object to apply.",
                        "additionalProperties": true
                    },
                    "approved": string_array_prop("Explicit approval ids granted for this apply call, for example start_run. Omit when using request_approval or an approved approval_id."),
                    "approval_id": string_prop("Deterministic request-bound task strategy Feed approval id that has already been approved. It can satisfy remaining approvals covered by the same approved request when explicit attestations cover another part."),
                    "request_approval": boolean_prop("When true and approvals are missing, publish a pending Feed approval instead of mutating workflow/team state."),
                    "workspace_id": string_prop("Workspace id for the staged workflow/team state."),
                    "workspace_name": string_prop("Workspace name for the staged workflow/team state."),
                    "worktree_name": string_prop("Already-open worktree workspace name for staged state or submit=true worktree-layer team runs."),
                    "cwd": string_prop("Absolute repo cwd inside a Git repository already represented by an open ForkTTY workspace, surface, or effective project cwd; used for visible worker launches and role prompts when no worktree_name is used."),
                    "leader_surface_id": string_prop("Visible leader surface id to bind the staged team."),
                    "surface_id": string_prop("Visible leader surface id; accepted as an alias for leader_surface_id."),
                    "workflow_id": string_prop("Optional explicit workflow id; defaults to run_id."),
                    "team_id": string_prop("Optional explicit team id; defaults to run_id."),
                    "submit": boolean_prop("When true, launch visible team workers and dispatch prompts for supported team plans. Worktree-layer plans require worktree_name naming an already-open ForkTTY worktree workspace. Defaults to false staging."),
                }),
            ),
        },
        ToolSpec {
            name: "identify",
            annotations: read_only_annotations(),
            description: "Return the canonical ForkTTY workspace/surface context for the caller or selected target, including effective_project_cwd, caller id validation, and current agent binding when present.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "surface_id": string_prop("Surface id to identify; defaults to caller surface when available, otherwise active workspace focus."),
                    "caller_workspace_id": string_prop("Caller workspace id for validation; defaults from FORKTTY_WORKSPACE_ID when available."),
                    "caller_surface_id": string_prop("Caller surface id for validation; defaults from FORKTTY_SURFACE_ID when available."),
                }),
            ),
        },
        ToolSpec {
            name: "topology_tree",
            annotations: read_only_annotations(),
            description: "Return a read-only ForkTTY workspace/pane/surface tree with surfaces nested under each workspace. Use before targeting a different pane.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "remote_list",
            annotations: read_only_annotations(),
            description: "List SSH remote workspaces/surfaces known to ForkTTY with connection state. This is read-only inventory; it does not open SSH or run remote commands.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "remote_status",
            annotations: read_only_annotations(),
            description: "Read one SSH remote surface status, or the focused SSH surface for a selected/default workspace. This does not reconnect or start a remote helper.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("SSH surface id to inspect."),
                    "workspace_id": string_prop("Workspace id whose focused SSH surface should be inspected."),
                    "workspace_name": string_prop("Workspace name whose focused SSH surface should be inspected."),
                    "worktree_name": string_prop("Worktree name whose focused SSH surface should be inspected."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY surfaces with persisted agent session ids, freshness fields, and lifecycle evidence. Use before planning manual resume or status/HUD work.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_health",
            annotations: read_only_annotations(),
            description: "Check whether persisted ForkTTY agent sessions have a safe provider resume command and provider executable on PATH, with lifecycle_evidence correlating persisted lifecycle freshness, the workspace/provider status row, permission mode, and readiness reason.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_reclaim_plan",
            annotations: read_only_annotations(),
            description: "Plan which persisted idle agent sessions are safe reclaim candidates without suspending or closing anything.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "min_idle_ms": integer_prop("Minimum idle age in milliseconds before a session can be a reclaim candidate."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_hibernate",
            annotations: mutating_annotations(false, false),
            description: "Hibernate one idle, resumable ForkTTY agent session by closing its terminal process and marking it suspended. Prefer agent_reclaim_plan first.",
            input_schema: object_schema(
                &["surface_id"],
                json!({
                    "surface_id": string_prop("Surface id with an idle persisted agent session."),
                    "min_idle_ms": integer_prop("Optional minimum idle age in milliseconds before hibernating."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_reclaim",
            annotations: mutating_annotations(false, false),
            description: "Apply the reclaim policy to idle, resumable ForkTTY agent sessions and hibernate matching candidates.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "min_idle_ms": integer_prop("Minimum idle age in milliseconds before a session can be reclaimed."),
                    "limit": integer_prop("Maximum number of candidate sessions to hibernate, capped by the socket."),
                }),
            ),
        },
        ToolSpec {
            name: "agent_resume",
            annotations: mutating_annotations(false, false),
            description: "Resume a persisted agent session from a source surface into a new tab using ForkTTY's argv-only provider command builder. Prefer agent_health first and continue only when the row is ready.",
            input_schema: object_schema(
                &["surface_id"],
                json!({
                    "surface_id": string_prop("Source surface id with a persisted agent session from agent_list or status_summary."),
                }),
            ),
        },
        ToolSpec {
            name: "team_list",
            annotations: read_only_annotations(),
            description: "List ForkTTY team orchestration records for the current or selected workspace. Use before updating workers, tasks, or mailbox messages.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to inspect."),
                    "workspace_name": string_prop("Workspace name to inspect."),
                    "worktree_name": string_prop("Worktree name to inspect."),
                    "status": string_prop("Optional team status filter."),
                    "query": string_prop("Optional substring query across team id, name, and goal."),
                    "limit": integer_prop("Maximum teams to return."),
                }),
            ),
        },
        ToolSpec {
            name: "team_get",
            annotations: read_only_annotations(),
            description: "Read one ForkTTY team orchestration record including workers, tasks, and mailbox messages.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id from team_list or team_upsert."),
                }),
            ),
        },
        ToolSpec {
            name: "team_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a ForkTTY team orchestration record. This stores leader metadata only; it does not open UI panes.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Stable team id."),
                    "workspace_id": string_prop("Workspace id to associate with the team; defaults from FORKTTY_WORKSPACE_ID only when no leader surface is available."),
                    "workspace_name": string_prop("Workspace name to associate with the team."),
                    "worktree_name": string_prop("Worktree name to associate with the team."),
                    "leader_surface_id": string_prop("Surface id for the leader pane; defaults from FORKTTY_SURFACE_ID when present."),
                    "name": string_prop("Human team name."),
                    "status": string_prop("Team status, for example active, paused, or done."),
                    "goal": string_prop("Team goal or brief."),
                }),
            ),
        },
        ToolSpec {
            name: "team_finish",
            annotations: mutating_annotations(true, false),
            description: "Finalize a ForkTTY team after verifying summary and worker health. Supports dry-run planning and optional cleanup of current-runtime launch-owned worker panes.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "dry_run": boolean_prop("Return the finish plan without mutating team state or closing panes."),
                    "close_workers": boolean_prop("Request shutdown and close current-runtime launch-owned worker panes before marking the team done."),
                    "force": boolean_prop("Proceed despite open tasks, pending messages, active workers, or cleanup errors after explicit review."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a worker record for a ForkTTY team. Surface/worktree fields are references; this does not spawn panes.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Stable worker id."),
                    "role": string_prop("Worker role."),
                    "agent": string_prop("Agent/provider name."),
                    "surface_id": string_prop("Surface id used by this worker."),
                    "worktree_name": string_prop("Worktree name assigned to this worker."),
                    "status": string_prop("Worker status, for example idle, running, busy, or blocked."),
                    "assigned_task_id": string_prop("Task id currently assigned to this worker."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_heartbeat",
            annotations: mutating_annotations(false, false),
            description: "Record a worker heartbeat/status update for a ForkTTY team.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "status": string_prop("Current worker status."),
                    "assigned_task_id": string_prop("Current assigned task id."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_launch",
            annotations: mutating_annotations(false, false),
            description: "Launch a provider worker in a new ForkTTY tab and attach it to a team worker record. If agent is omitted or auto, ForkTTY selects the first configured available provider from Settings. Supported agents are codex, claude, pi, opencode, and antigravity. Claude launches add documented permission-mode defaults unless args already include Claude permission controls; Pi review-role launches add read-only tool defaults unless args already include Pi tool controls.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "agent": string_prop("Provider to launch: auto, codex, claude, pi, opencode, or antigravity. Defaults to auto."),
                    "role": string_prop("Worker role."),
                    "assigned_task_id": string_prop("Task id currently assigned to this worker."),
                    "worktree_name": string_prop("Worktree name assigned to this worker."),
                    "cwd": string_prop("Absolute cwd for the launched worker pane when no worktree_name is used."),
                    "args": string_array_prop("Extra argv entries appended after the provider executable."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_health",
            annotations: read_only_annotations(),
            description: "Read per-worker team health including final_state, surface_present, surface_runtime_present, surface_ready, stale heartbeat, nudge, launch, and shutdown-request timestamps.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "stale_after_ms": integer_prop("Heartbeat age after which running workers are reported stale."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_nudge",
            annotations: mutating_annotations(false, false),
            description: "Send a nudge message to a team worker's attached terminal pane and record the nudge timestamp after delivery succeeds.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "text": string_prop("Optional exact text to send; defaults to a heartbeat request."),
                }),
            ),
        },
        ToolSpec {
            name: "team_worker_shutdown",
            annotations: mutating_annotations(true, false),
            description: "Request team worker shutdown with provider-aware submit behavior by default, including a short settle before Codex/Claude/Pi Enter, mark the worker shutdown_requested after the terminal accepts the input, and optionally close current-runtime launch-owned worker panes immediately.",
            input_schema: object_schema(
                &["team_id", "worker_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id."),
                    "text": string_prop("Optional exact shutdown request text."),
                    "submit": boolean_prop("Use provider-aware submit behavior for the shutdown terminal input, including a short settle before Codex/Claude/Pi Enter. Defaults to true; set false to stage text without Enter."),
                    "close_surface": boolean_prop("Immediately close the worker surface after shutdown text is accepted by the terminal. Defaults to false and only works for surfaces created by team_worker_launch in the current ForkTTY runtime."),
                }),
            ),
        },
        ToolSpec {
            name: "team_task_upsert",
            annotations: mutating_annotations(false, true),
            description: "Create or update a ForkTTY team task. Dependencies must form a DAG.",
            input_schema: object_schema(
                &["team_id", "task_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "task_id": string_prop("Stable task id."),
                    "title": string_prop("Task title."),
                    "status": string_prop("Task status, for example open, running, done, blocked, or cancelled."),
                    "detail": string_prop("Task detail or notes."),
                    "depends_on": string_array_prop("Task ids this task depends on."),
                    "assigned_worker_id": string_prop("Worker id assigned to this task."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_send",
            annotations: mutating_annotations(false, false),
            description: "Queue a mailbox message for a ForkTTY team worker or team-wide coordination.",
            input_schema: object_schema(
                &["team_id", "from", "body"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Optional stable message id; ForkTTY generates one when omitted."),
                    "from": string_prop("Sender id, usually leader or a worker id."),
                    "to_worker_id": string_prop("Target worker id. Omit for team-wide messages."),
                    "task_id": string_prop("Related task id."),
                    "body": string_prop("Message body. Whitespace is preserved."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_dispatch",
            annotations: mutating_annotations_with_open_world(true, false, true),
            description: "Send a queued, non-superseded ForkTTY team message to a worker terminal pane and acknowledge it only after the terminal accepts the dispatched input.",
            input_schema: object_schema(
                &["team_id", "message_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Message id."),
                    "worker_id": string_prop("Required when dispatching a team-wide message."),
                    "submit": boolean_prop("Use provider-aware submit behavior for the dispatched terminal input, including a short settle before Codex/Claude/Pi Enter and a brief initial prompt settle for freshly launched provider TUI workers. Defaults to false."),
                }),
            ),
        },
        ToolSpec {
            name: "team_message_ack",
            annotations: mutating_annotations(false, false),
            description: "Acknowledge a ForkTTY team mailbox message that has not been superseded.",
            input_schema: object_schema(
                &["team_id", "message_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "message_id": string_prop("Message id."),
                    "worker_id": string_prop("Optional worker id guard for worker-targeted acknowledgements."),
                }),
            ),
        },
        ToolSpec {
            name: "team_inbox",
            annotations: read_only_annotations(),
            description: "Read active pending mailbox messages for a ForkTTY team or worker. Superseded task-strategy prompts are hidden unless full history is requested.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                    "worker_id": string_prop("Worker id to filter messages."),
                    "include_delivered": boolean_prop("Include already acknowledged and superseded messages."),
                    "limit": integer_prop("Maximum messages to return."),
                }),
            ),
        },
        ToolSpec {
            name: "team_summary",
            annotations: read_only_annotations(),
            description: "Return aggregate counts for a ForkTTY team: workers, active workers, tasks, pending messages, and last event.",
            input_schema: object_schema(
                &["team_id"],
                json!({
                    "team_id": string_prop("Team id."),
                }),
            ),
        },
        ToolSpec {
            name: "team_events",
            annotations: read_only_annotations(),
            description: "List ForkTTY team orchestration events for polling or audit.",
            input_schema: object_schema(
                &[],
                json!({
                    "team_id": string_prop("Optional team id filter."),
                    "since_seq": integer_prop("Only return events with seq greater than this value."),
                    "limit": integer_prop("Maximum events to return."),
                }),
            ),
        },
        ToolSpec {
            name: "status_summary",
            annotations: read_only_annotations(),
            description: "Return a compact workspace summary with persisted agent sessions carrying source/age/lifecycle-evidence metadata and status/progress entries carrying source annotations for statusline/HUD integrations.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to summarize."),
                    "workspace_name": string_prop("Workspace name to summarize."),
                    "worktree_name": string_prop("Worktree name to summarize."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_list",
            annotations: read_only_annotations(),
            description: "List durable ForkTTY workflow states for goals, mode/session memory, plan steps, and evidence. Defaults to FORKTTY_WORKSPACE_ID when available.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Workspace id to filter."),
                    "workspace_name": string_prop("Workspace name to filter."),
                    "worktree_name": string_prop("Worktree name to filter."),
                    "surface_id": string_prop("Surface id to filter."),
                    "session_id": string_prop("Provider session id to filter."),
                    "query": string_prop("Case-insensitive search across workflow goal, memory, plan, and evidence."),
                    "limit": integer_prop("Maximum workflow rows to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_get",
            annotations: read_only_annotations(),
            description: "Read one durable ForkTTY workflow state by workflow id.",
            input_schema: object_schema(
                &["workflow_id"],
                json!({
                    "workflow_id": string_prop("Workflow id from workflow_list or workflow_upsert."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_upsert",
            annotations: mutating_annotations(false, false),
            description: "Create or update durable ForkTTY workflow state for a workspace, surface, provider session, mode, goal, and compaction-resistant memory.",
            input_schema: object_schema(
                &[],
                json!({
                    "workflow_id": string_prop("Optional explicit workflow id. If omitted, ForkTTY derives one from session_id, surface_id, or workspace_id."),
                    "workspace_id": string_prop("Workspace id to bind."),
                    "workspace_name": string_prop("Workspace name to bind."),
                    "worktree_name": string_prop("Worktree name to bind."),
                    "surface_id": string_prop("Surface id to bind."),
                    "agent": string_prop("Provider name such as codex or claude."),
                    "session_id": string_prop("Provider session id."),
                    "mode": string_prop("Workflow mode, for example review, plan, implementation, or default."),
                    "status": string_prop("Workflow status, for example running, blocked, done, or active."),
                    "goal": string_prop("Current workflow goal."),
                    "memory": string_prop("Durable project/session memory for compaction recovery."),
                }),
            ),
        },
        ToolSpec {
            name: "workflow_loop_set",
            annotations: mutating_annotations(false, false),
            description: "Update bounded closed-loop state for a workflow: recipe, stage, iteration budget, stop reason, and compact gate statuses. Advancing to a new iteration clears prior gates and stop reason unless replacements are supplied. This records loop progress only; it does not run commands, launch agents, schedule background work, push, merge, or approve actions.",
            input_schema: object_schema(
                &["workflow_id"],
                json!({
                    "workflow_id": string_prop("Workflow id to update."),
                    "recipe": string_prop("Loop recipe name, for example review-fix-verify."),
                    "stage": string_prop("Current loop stage, for example discover, plan, execute, verify, iterate, done, blocked, or needs_human."),
                    "iteration": integer_prop("Current loop iteration."),
                    "max_iterations": integer_prop("Maximum loop iterations before the caller should stop."),
                    "stop_reason": string_prop("Loop stop reason such as passed, failed, blocked, budget_exhausted, needs_human, or cancelled."),
                    "gates": {
                        "type": "array",
                        "description": "Array of {id,kind,label,status,summary?} compact gate status objects.",
                        "items": { "type": "object" },
                    },
                }),
            ),
        },
        ToolSpec {
            name: "workflow_plan_set",
            annotations: mutating_annotations(false, true),
            description: "Replace a workflow's durable plan steps. Each step object must include id, title, and status; detail is optional.",
            input_schema: object_schema(
                &["workflow_id", "steps"],
                json!({
                    "workflow_id": string_prop("Workflow id to update."),
                    "steps": {
                        "type": "array",
                        "description": "Array of {id,title,status,detail?} plan step objects.",
                        "items": { "type": "object" },
                    },
                }),
            ),
        },
        ToolSpec {
            name: "workflow_evidence_add",
            annotations: mutating_annotations(false, false),
            description: "Append a bounded evidence artifact to a workflow. Provide text or a path reference; ForkTTY stores metadata and optional text, not arbitrary unbounded files.",
            input_schema: {
                let mut schema = object_schema(
                    &["workflow_id", "kind", "title"],
                    json!({
                        "workflow_id": string_prop("Workflow id to update."),
                        "evidence_id": string_prop("Optional evidence id; ForkTTY derives the first available evidence-N id when omitted."),
                        "kind": string_prop("Evidence kind such as test, diff, note, log, or decision."),
                        "title": string_prop("Evidence title."),
                        "text": string_prop("Bounded evidence text."),
                        "path": string_prop("Optional path reference for an artifact already on disk."),
                    }),
                );
                schema["anyOf"] = json!([
                    { "required": ["text"] },
                    { "required": ["path"] },
                ]);
                schema
            },
        },
        ToolSpec {
            name: "workflow_replay",
            annotations: read_only_annotations(),
            description: "Replay durable ForkTTY workflow events for session search and recovery.",
            input_schema: object_schema(
                &[],
                json!({
                    "workflow_id": string_prop("Workflow id to filter."),
                    "query": string_prop("Case-insensitive event search."),
                    "since_seq": integer_prop("Only return events after this sequence number."),
                    "limit": integer_prop("Maximum event rows to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_split",
            annotations: mutating_annotations(false, false),
            description: "Split a ForkTTY surface to create another agent-ready pane. Defaults surface_id from FORKTTY_SURFACE_ID when present.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Surface id to split; defaults from FORKTTY_SURFACE_ID."),
                    "axis": enum_prop("Split direction.", &["horizontal", "vertical"]),
                }),
            ),
        },
        ToolSpec {
            name: "surface_send_text",
            annotations: mutating_annotations_with_open_world(true, false, true),
            description: "Send literal text to a ForkTTY terminal surface. Text may execute shell commands or interact with external systems; use this to drive a pane after inspecting surface_list; surface_id defaults from FORKTTY_SURFACE_ID.",
            input_schema: object_schema(
                &["text"],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "text": string_prop("Literal text to send to the terminal."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_read_text",
            annotations: read_only_annotations(),
            description: "Read text from a ForkTTY terminal surface. Defaults to visible screen text and FORKTTY_SURFACE_ID; use scope=all only when a bounded full scrollback read is needed.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "scope": enum_prop("Text range to read.", &["visible", "all"]),
                    "max_bytes": integer_prop("Maximum UTF-8 bytes to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_capture_tail",
            annotations: read_only_annotations(),
            description: "Capture the last N lines of a ForkTTY terminal surface, including scrollback. Defaults surface_id from FORKTTY_SURFACE_ID.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Target terminal surface id; defaults from FORKTTY_SURFACE_ID."),
                    "lines": integer_prop("Number of trailing lines to capture; defaults to 80."),
                    "max_bytes": integer_prop("Maximum UTF-8 bytes to return; socket enforces its upper bound."),
                }),
            ),
        },
        ToolSpec {
            name: "surface_focus",
            annotations: mutating_annotations(false, true),
            description: "Focus a ForkTTY surface in the UI. Defaults surface_id from FORKTTY_SURFACE_ID when present.",
            input_schema: object_schema(
                &[],
                json!({
                    "surface_id": string_prop("Surface id to focus; defaults from FORKTTY_SURFACE_ID."),
                }),
            ),
        },
        ToolSpec {
            name: "worktree_list",
            annotations: read_only_annotations(),
            description: "List git worktrees for a repository path so agents can see existing parallel work. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: object_schema(
                &["cwd"],
                json!({
                    "cwd": string_prop("Path inside the git repository."),
                }),
            ),
        },
        ToolSpec {
            name: "worktree_status",
            annotations: read_only_annotations(),
            description: "Report whether a worktree is clean, dirty, or otherwise blocked before attach/remove/merge operations. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: {
                let mut schema = object_schema(
                    &[],
                    json!({
                        "path": string_prop("Path to the worktree to inspect."),
                        "cwd": string_prop("Alternative path inside the worktree."),
                    }),
                );
                schema["oneOf"] = json!([
                    { "required": ["path"] },
                    { "required": ["cwd"] },
                ]);
                schema
            },
        },
        ToolSpec {
            name: "worktree_create",
            annotations: mutating_annotations(false, false),
            description: "Create an isolated git worktree + ForkTTY workspace for parallel agent work. Pass a branch/worktree name and repo cwd. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_attach",
            annotations: mutating_annotations(false, false),
            description: "Attach an existing branch/worktree to a ForkTTY workspace so another agent can work there. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_remove",
            annotations: mutating_annotations(true, false),
            description: "Remove a ForkTTY-managed worktree after checking worktree_status; closes its workspace when present. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "worktree_merge",
            annotations: mutating_annotations(false, false),
            description: "Merge a completed worktree branch back into the repository. Use worktree_status first to avoid merging dirty work. Requires an open ForkTTY workspace on the target repository (see workspace_list).",
            input_schema: worktree_named_schema(),
        },
        ToolSpec {
            name: "notification_create",
            annotations: mutating_annotations(false, false),
            description: "Create a ForkTTY notification for the user. Defaults workspace_id/surface_id from the ForkTTY pane environment when no target is supplied.",
            input_schema: object_schema(
                &[],
                json!({
                    "workspace_id": string_prop("Target workspace id."),
                    "workspace_name": string_prop("Target workspace name."),
                    "worktree_name": string_prop("Target worktree name."),
                    "surface_id": string_prop("Target surface id."),
                    "title": string_prop("Notification title; defaults to ForkTTY."),
                    "body": string_prop("Notification body."),
                    "kind": enum_prop("Notification kind.", &["info", "prompt", "error", "custom"]),
                }),
            ),
        },
        ToolSpec {
            name: "status_set",
            annotations: mutating_annotations(false, true),
            description: "Set ForkTTY sidebar status metadata for an agent or task. Defaults workspace_id/surface_id from the ForkTTY pane environment when no target is supplied.",
            input_schema: object_schema(
                &["key", "value"],
                json!({
                    "workspace_id": string_prop("Target workspace id."),
                    "workspace_name": string_prop("Target workspace name."),
                    "worktree_name": string_prop("Target worktree name."),
                    "surface_id": string_prop("Target surface id."),
                    "key": string_prop("Stable status key, for example agent:codex."),
                    "label": string_prop("Human label; defaults to key."),
                    "value": string_prop("Status value to display."),
                    "color": string_prop("green, yellow, red, blue, muted, or a CSS hex color."),
                }),
            ),
        },
    ]
}

fn worktree_named_schema() -> Value {
    object_schema(
        &["cwd", "name"],
        json!({
            "cwd": string_prop("Path inside the git repository."),
            "name": string_prop("Branch or worktree name."),
        }),
    )
}

fn object_schema(required: &[&str], properties: Value) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn string_prop(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description,
    })
}

fn integer_prop(description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": description,
    })
}

fn boolean_prop(description: &str) -> Value {
    json!({
        "type": "boolean",
        "description": description,
    })
}

fn string_array_prop(description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "string" },
        "description": description,
    })
}

fn enum_prop(description: &str, values: &[&str]) -> Value {
    json!({
        "type": "string",
        "description": description,
        "enum": values,
    })
}
