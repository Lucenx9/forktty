use crate::{
    current_unix_epoch_ms, format_param_names, optional_bool_param,
    optional_non_blank_string_param, optional_string_array_param, optional_surface_id_param,
    path_resolver::{canonical_existing_dir, canonical_repo_common_dir},
    required_trimmed_string, store_access, surface_effective_project_cwd, system_runtime,
    task_strategy_params::{task_strategy_goal_param, task_strategy_plan_params},
    team_provider::{select_team_worker_provider, team_worker_launch_command_with_program},
    team_runtime,
    team_state::worker_surface_is_live,
    workflow_runtime, workspace_effective_project_cwd, workspace_selector_from_params,
    workspace_selector_params, DispatchError, SocketAppState, WorkspaceSelectorKind,
};
use forktty_core::{
    plan_task_strategy, protocol_limits, validate_worktree_name, worktree, FeedApprovalState,
    FeedEntry, FeedEntryType, HarnessAssignment, HarnessCapability, HarnessHealth, HarnessRegistry,
    HarnessRole, HarnessRoutingSignals, TaskClass, TaskRouterProfile, TaskStrategy,
    TaskStrategyApproval, TaskStrategyInput, TaskStrategyLastKnownGood, TaskStrategyPlan,
    TaskStrategyScoreFactor, WorkflowQuery, WorkflowState, WorkspaceSelector,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const DEFAULT_PROVIDER_MAX_PARALLEL_SESSIONS: u32 = 4;
const MAX_TASK_STRATEGY_REASON_BYTES: usize =
    protocol_limits::SOCKET_TASK_STRATEGY_REASON_MAX_BYTES;
const MAX_TASK_STRATEGY_ASSIGNMENTS: usize =
    protocol_limits::SOCKET_TASK_STRATEGY_ASSIGNMENT_MAX_COUNT;
const MAX_TASK_STRATEGY_PLAN_REASONS: usize = 32;
const MAX_TASK_STRATEGY_SAFETY_NOTES: usize = 16;
const MAX_TASK_STRATEGY_CANDIDATE_SCORES: usize = 8;
const MAX_TASK_STRATEGY_SCORE_FACTORS: usize = 32;
const MAX_TASK_STRATEGY_APPROVALS: usize = 4;

pub(crate) async fn plan(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let plan_params = task_strategy_plan_params(params)?;
    let capabilities = system_runtime::capabilities();
    let mut registry = harness_registry_from_capabilities(&capabilities);
    if let Some(harness_signals) = plan_params.harness_signals.as_ref() {
        apply_harness_routing_signals(&mut registry, harness_signals)?;
    }
    let explicit_mode = plan_params
        .explicit_strategy
        .as_deref()
        .map(task_strategy_from_str)
        .transpose()?;
    let router_profile = plan_params
        .router_profile
        .as_deref()
        .map(task_router_profile_from_str)
        .transpose()?;
    let task_class_hint = plan_params.task_class_hint.clone();
    let explicit_last_known_good = plan_params
        .last_known_good
        .as_ref()
        .map(last_known_good_from_value)
        .transpose()?;
    let target = task_strategy_target_context(state, params, true)?;
    let (last_known_good, inferred_last_known_good_reason) =
        if let Some(last_known_good) = explicit_last_known_good {
            (Some(last_known_good), None)
        } else {
            let inferred =
                infer_last_known_good_from_workflows(state, target.workspace_id.as_deref()).await;
            let reason = inferred.as_ref().and_then(|value| value.reason.clone());
            (inferred, reason)
        };
    let repo_dirty = match plan_params.repo_dirty {
        Some(repo_dirty) => repo_dirty,
        None => infer_repo_dirty(target.cwd.as_deref()),
    };
    let inferred_user_visible_change = plan_params.likely_user_visible_change.is_none()
        && (task_class_hint
            .as_ref()
            .is_some_and(task_class_likely_user_visible_change)
            || infer_likely_user_visible_change(&plan_params.goal));
    let likely_user_visible_change = plan_params
        .likely_user_visible_change
        .unwrap_or(inferred_user_visible_change);
    let mut plan = plan_task_strategy(TaskStrategyInput {
        goal: plan_params.goal,
        explicit_mode,
        router_profile,
        task_class_hint,
        last_known_good,
        repo_dirty,
        user_requested_parallelism: plan_params.user_requested_parallelism,
        user_requested_review: plan_params.user_requested_review,
        likely_user_visible_change,
        harness_registry: registry,
    })
    .map_err(DispatchError::InvalidParam)?;
    if inferred_user_visible_change {
        plan.reasons
            .push("inferred likely user-visible change from task wording".to_string());
    }
    if target.explicit_cwd {
        plan.reasons
            .push("used explicit cwd for repo context inference".to_string());
    }
    if let Some(reason) = inferred_last_known_good_reason {
        plan.reasons.push(format!(
            "inferred last-known-good routing evidence from {reason}"
        ));
    }

    serde_json::to_value(plan)
        .map_err(|err| DispatchError::Other(format!("serialize task strategy plan: {err}")))
}

fn task_class_likely_user_visible_change(task_class: &TaskClass) -> bool {
    matches!(
        task_class,
        TaskClass::FocusedBugfix
            | TaskClass::FeatureImplementation
            | TaskClass::ParallelExperiment
            | TaskClass::VerifyFixLoop
            | TaskClass::LongRunningTeamRun
    )
}

fn task_strategy_likely_user_visible_change(strategy: &TaskStrategy) -> bool {
    matches!(
        strategy,
        TaskStrategy::SoloWithVerifyLoop
            | TaskStrategy::ImplementerPlusReviewer
            | TaskStrategy::ParallelExperiment
            | TaskStrategy::TeamPipeline
    )
}

fn task_strategy_plan_is_read_only_review(plan: &TaskStrategyPlan) -> bool {
    matches!(plan.task_class, TaskClass::ReviewOnly)
        && matches!(plan.strategy, TaskStrategy::ReviewOnly)
        && plan
            .assignments
            .iter()
            .all(|assignment| matches!(assignment.role, HarnessRole::Reviewer))
}

fn task_strategy_supports_team_layer(strategy: &TaskStrategy) -> bool {
    matches!(
        strategy,
        TaskStrategy::ImplementerPlusReviewer
            | TaskStrategy::ParallelResearch
            | TaskStrategy::ParallelExperiment
            | TaskStrategy::TeamPipeline
    )
}

fn task_strategy_supports_loop_metadata_layer(strategy: &TaskStrategy) -> bool {
    matches!(
        strategy,
        TaskStrategy::SoloWithVerifyLoop | TaskStrategy::ImplementerPlusReviewer
    )
}

fn task_strategy_supports_assignment_role(strategy: &TaskStrategy, role: &HarnessRole) -> bool {
    match strategy {
        TaskStrategy::Solo | TaskStrategy::SoloTracked | TaskStrategy::SoloWithVerifyLoop => {
            matches!(role, HarnessRole::Implementer)
        }
        TaskStrategy::ImplementerPlusReviewer | TaskStrategy::TeamPipeline => {
            matches!(role, HarnessRole::Implementer | HarnessRole::Reviewer)
        }
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment => {
            matches!(role, HarnessRole::Researcher)
        }
        TaskStrategy::ReviewOnly => matches!(role, HarnessRole::Reviewer),
    }
}

fn task_strategy_assignment_count_bounds(strategy: &TaskStrategy) -> (usize, usize) {
    match strategy {
        TaskStrategy::Solo
        | TaskStrategy::SoloTracked
        | TaskStrategy::SoloWithVerifyLoop
        | TaskStrategy::ReviewOnly => (1, 1),
        TaskStrategy::ImplementerPlusReviewer | TaskStrategy::TeamPipeline => (2, 2),
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment => (2, 3),
    }
}

fn infer_likely_user_visible_change(goal: &str) -> bool {
    let lower = goal.to_lowercase();
    let edit_tokens = ["fix", "bug", "bugs", "add", "drop", "port"];
    let edit_prefixes = [
        "fixe",
        "fixi",
        "bugfix",
        "implement",
        "adde",
        "addi",
        "build",
        "change",
        "updat",
        "modif",
        "edit",
        "refactor",
        "remove",
        "delet",
        "replac",
        "rewrit",
        "migrat",
        "dropp",
        "convert",
        "porte",
        "porti",
        "patch",
        "renam",
        "writ",
        "creat",
    ];
    if contains_token(&lower, &edit_tokens) || contains_token_prefix(&lower, &edit_prefixes) {
        return true;
    }

    let visible_surface_terms = [
        "doc",
        "readme",
        "ui",
        "cli",
        "mcp",
        "socket",
        "hook",
        "skill",
        "site",
        "packaging",
        "release",
        "changelog",
        "spec",
    ];
    contains_token_prefix(&lower, &visible_surface_terms)
}

fn contains_token_prefix(value: &str, prefixes: &[&str]) -> bool {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| prefixes.iter().any(|prefix| token.starts_with(prefix)))
}

fn contains_token(value: &str, tokens: &[&str]) -> bool {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| tokens.contains(&token))
}

fn infer_repo_dirty(cwd: Option<&Path>) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    let status = worktree::status(&cwd.to_string_lossy()).ok();
    matches!(status.as_deref(), Some("dirty") | Some("conflicts"))
}

struct TaskStrategyTargetContext {
    cwd: Option<PathBuf>,
    workspace_id: Option<String>,
    explicit_cwd: bool,
}

fn optional_task_strategy_surface_alias_param(
    params: &Value,
) -> Result<Option<String>, DispatchError> {
    let surface_id = optional_surface_id_param(params)?;
    let leader_surface_id = optional_non_blank_string_param(params, "leader_surface_id")?;
    match (surface_id, leader_surface_id) {
        (Some(surface_id), Some(leader_surface_id)) if surface_id != leader_surface_id => {
            Err(DispatchError::InvalidParam(
                "surface_id and leader_surface_id must match when both are provided".to_string(),
            ))
        }
        (Some(surface_id), _) => Ok(Some(surface_id.to_string())),
        (_, Some(leader_surface_id)) => Ok(Some(leader_surface_id.to_string())),
        (None, None) => Ok(None),
    }
}

struct TaskStrategyWorkspaceSelectorParams {
    workspace_id: Option<String>,
    workspace_name: Option<String>,
    worktree_name: Option<String>,
}

fn task_strategy_workspace_selector_params(
    params: &Value,
) -> Result<TaskStrategyWorkspaceSelectorParams, DispatchError> {
    let selectors = workspace_selector_params(params)?;
    if selectors.is_empty() {
        return Ok(TaskStrategyWorkspaceSelectorParams {
            workspace_id: None,
            workspace_name: None,
            worktree_name: None,
        });
    }
    if selectors.len() > 1 {
        return Err(format!(
            "Ambiguous workspace selector: cannot combine {}",
            format_param_names(selectors.iter().map(|selector| selector.key))
        )
        .into());
    }
    let selector = &selectors[0];
    match selector.kind {
        WorkspaceSelectorKind::Id => Ok(TaskStrategyWorkspaceSelectorParams {
            workspace_id: Some(selector.value.to_string()),
            workspace_name: None,
            worktree_name: None,
        }),
        WorkspaceSelectorKind::Name => Ok(TaskStrategyWorkspaceSelectorParams {
            workspace_id: None,
            workspace_name: Some(selector.value.to_string()),
            worktree_name: None,
        }),
        WorkspaceSelectorKind::WorktreeName => Ok(TaskStrategyWorkspaceSelectorParams {
            workspace_id: None,
            workspace_name: None,
            worktree_name: Some(
                validate_worktree_name(selector.value)
                    .map_err(DispatchError::from)?
                    .to_string(),
            ),
        }),
    }
}

fn task_strategy_target_context(
    state: &SocketAppState,
    params: &Value,
    allow_explicit_cwd: bool,
) -> Result<TaskStrategyTargetContext, DispatchError> {
    let explicit_cwd = if allow_explicit_cwd {
        explicit_task_strategy_cwd(params)?
    } else {
        None
    };
    if let Some(cwd) = explicit_cwd.as_deref() {
        validate_explicit_task_strategy_cwd(state, cwd)?;
    }
    let has_explicit_cwd = explicit_cwd.is_some();
    let requested_surface_id = optional_task_strategy_surface_alias_param(params)?;
    let workspace_selector = match workspace_selector_from_params(params) {
        Ok(selector) => Some(selector),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;

    if let Some(surface_id) = requested_surface_id.as_deref() {
        let surface = model
            .surface(surface_id)
            .ok_or_else(|| DispatchError::NotFound("surface".to_string()))?;
        if let Some(selector) = workspace_selector {
            let workspace_id = model
                .workspace_id_for(selector)
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
            if workspace_id != surface.workspace_id {
                return Err(DispatchError::NotFound("surface".to_string()));
            }
        }
        return Ok(TaskStrategyTargetContext {
            cwd: Some(explicit_cwd.unwrap_or_else(|| surface_effective_project_cwd(surface))),
            workspace_id: Some(surface.workspace_id.clone()),
            explicit_cwd: has_explicit_cwd,
        });
    }

    let workspace_id = if let Some(selector) = workspace_selector {
        Some(
            model
                .workspace_id_for(selector)
                .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?,
        )
    } else {
        model.active_workspace_id()
    };
    let Some(workspace_id) = workspace_id else {
        return Ok(TaskStrategyTargetContext {
            cwd: explicit_cwd,
            workspace_id: None,
            explicit_cwd: has_explicit_cwd,
        });
    };
    let Some(workspace) = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
    else {
        return Ok(TaskStrategyTargetContext {
            cwd: explicit_cwd,
            workspace_id: Some(workspace_id),
            explicit_cwd: has_explicit_cwd,
        });
    };
    let surfaces = model.list_surfaces(Some(&workspace_id));
    Ok(TaskStrategyTargetContext {
        cwd: Some(
            explicit_cwd.unwrap_or_else(|| workspace_effective_project_cwd(&workspace, &surfaces)),
        ),
        workspace_id: Some(workspace_id),
        explicit_cwd: has_explicit_cwd,
    })
}

fn explicit_task_strategy_cwd(params: &Value) -> Result<Option<PathBuf>, DispatchError> {
    let Some(cwd) = optional_non_blank_string_param(params, "cwd")? else {
        return Ok(None);
    };
    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err(DispatchError::InvalidParam(
            "Invalid parameter cwd: expected an absolute path".to_string(),
        ));
    }
    canonical_existing_dir(&path, "cwd")
        .map(Some)
        .map_err(DispatchError::from)
}

fn validate_explicit_task_strategy_cwd(
    state: &SocketAppState,
    cwd: &Path,
) -> Result<(), DispatchError> {
    let canonical_cwd = canonical_existing_dir(cwd, "cwd").map_err(DispatchError::from)?;
    let candidate_repo = canonical_repo_common_dir(&canonical_cwd).map_err(|err| {
        DispatchError::PreconditionFailed(format!(
            "cwd is not inside an open ForkTTY workspace git repository: {err}"
        ))
    })?;
    let roots = open_task_strategy_repo_context_dirs(state)?;
    let allowed = roots
        .iter()
        .filter_map(|root| canonical_repo_common_dir(root).ok())
        .any(|open_repo| open_repo == candidate_repo);
    if allowed {
        Ok(())
    } else {
        Err(DispatchError::PreconditionFailed(
            "cwd is not inside the git repository of any open ForkTTY workspace or surface"
                .to_string(),
        ))
    }
}

fn open_task_strategy_repo_context_dirs(
    state: &SocketAppState,
) -> Result<Vec<PathBuf>, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
    let mut dirs = Vec::new();
    for workspace in model.list_workspaces() {
        dirs.push(workspace.working_dir.clone());
        let surfaces = model.list_surfaces(Some(&workspace.id));
        dirs.push(workspace_effective_project_cwd(&workspace, &surfaces));
        for surface in surfaces {
            dirs.push(surface.cwd.clone());
            dirs.push(surface_effective_project_cwd(&surface));
        }
    }
    Ok(dirs)
}

async fn infer_last_known_good_from_workflows(
    state: &SocketAppState,
    workspace_id: Option<&str>,
) -> Option<TaskStrategyLastKnownGood> {
    let access = store_access::optional_workflow_store_access(state)?;
    let store = access.load().await.ok()?;
    let workflows = store.list(&WorkflowQuery {
        workspace_id: workspace_id.map(str::to_string),
        limit: Some(200),
        ..WorkflowQuery::default()
    });

    workflows
        .into_iter()
        .filter(is_last_known_good_workflow)
        .find_map(last_known_good_from_workflow)
}

fn is_last_known_good_workflow(workflow: &WorkflowState) -> bool {
    workflow.mode == "task_strategy"
        && matches!(workflow.status.as_str(), "done" | "complete" | "completed")
}

fn last_known_good_from_workflow(workflow: WorkflowState) -> Option<TaskStrategyLastKnownGood> {
    let strategy = workflow
        .memory
        .as_deref()
        .and_then(task_strategy_from_workflow_memory);
    let harness_id = harness_id_from_workflow_plan(&workflow);
    if strategy.is_none() && harness_id.is_none() {
        return None;
    }

    Some(TaskStrategyLastKnownGood {
        strategy,
        harness_id,
        reason: Some(format!(
            "last successful task_strategy workflow {}",
            workflow.id
        )),
    })
}

fn task_strategy_from_workflow_memory(memory: &str) -> Option<TaskStrategy> {
    let (_, after_marker) = memory.split_once("Task strategy:")?;
    let strategy = after_marker
        .trim()
        .split(['.', '\n'])
        .next()
        .unwrap_or_default()
        .trim();
    task_strategy_from_history_value(strategy)
}

fn task_strategy_from_history_value(value: &str) -> Option<TaskStrategy> {
    task_strategy_from_str(value).ok().or(match value {
        "Solo" => Some(TaskStrategy::Solo),
        "SoloTracked" => Some(TaskStrategy::SoloTracked),
        "SoloWithVerifyLoop" => Some(TaskStrategy::SoloWithVerifyLoop),
        "ImplementerPlusReviewer" => Some(TaskStrategy::ImplementerPlusReviewer),
        "ParallelResearch" => Some(TaskStrategy::ParallelResearch),
        "ParallelExperiment" => Some(TaskStrategy::ParallelExperiment),
        "TeamPipeline" => Some(TaskStrategy::TeamPipeline),
        "ReviewOnly" => Some(TaskStrategy::ReviewOnly),
        _ => None,
    })
}

fn harness_id_from_workflow_plan(workflow: &WorkflowState) -> Option<String> {
    workflow
        .plan
        .iter()
        .filter_map(|step| step.detail.as_deref())
        .flat_map(str::lines)
        .find_map(|line| line.trim().strip_prefix("Harness:"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| validate_task_strategy_id("workflow.plan.harness_id", value).is_ok())
        .map(str::to_string)
}

pub(crate) async fn apply(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let mut request = TaskStrategyApplyRequest::decode(params)?;
    request.resolve_apply_target(state)?;
    request.enforce_server_detected_worktree_isolation(state, params)?;
    request.validate_before_mutation(state)?;
    let required_approvals = request.required_approvals();
    let missing_approvals = request.missing_approvals_from(&required_approvals);
    if !missing_approvals.is_empty() {
        if request.feed_approval_satisfies(state, &required_approvals, &missing_approvals)? {
        } else if request.request_approval {
            return request.record_approval_request(state, &missing_approvals);
        } else {
            return Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply missing approval(s): {}",
                missing_approvals.join(", ")
            )));
        }
    }
    if request.submit && request.plan.layers.team {
        request.validate_submit_assignments_launchable()?;
    }

    let run_status = if request.submit { "running" } else { "staged" };
    let team_status = if request.submit { "active" } else { "staged" };
    let mut actions = Vec::new();
    request.dismiss_superseded_approval_requests(state, &required_approvals, &mut actions)?;
    if request.plan.layers.workflow {
        let workflow = workflow_runtime::upsert(
            state,
            &request.workflow_upsert_params(run_status, "task_strategy"),
        )
        .await?;
        actions.push(json!({
            "method": "workflow.upsert",
            "status": "applied",
            "workflow_id": workflow["id"],
        }));

        let planned = workflow_runtime::plan_set(state, &request.workflow_plan_params()).await?;
        actions.push(json!({
            "method": "workflow.plan.set",
            "status": "applied",
            "workflow_id": planned["id"],
            "steps": planned["plan"].as_array().map(Vec::len).unwrap_or_default(),
        }));

        if request.plan.layers.loop_metadata {
            let looped =
                workflow_runtime::loop_set(state, &request.workflow_loop_params(run_status))
                    .await?;
            actions.push(json!({
                "method": "workflow.loop.set",
                "status": "applied",
                "workflow_id": looped["id"],
                "stage": looped["loop_stage"],
            }));
        }
    }

    if request.plan.layers.team {
        let team = team_runtime::upsert(state, &request.team_upsert_params(team_status)).await?;
        actions.push(json!({
            "method": "team.upsert",
            "status": "applied",
            "team_id": team["id"],
        }));

        for (index, assignment) in request.plan.assignments.iter().enumerate() {
            let task_id = request.assignment_task_id(index, assignment);
            let task = team_runtime::task_upsert(
                state,
                &request.team_task_params(index, assignment, &task_id),
            )
            .await?;
            actions.push(json!({
                "method": "team.task.upsert",
                "status": "applied",
                "team_id": request.team_id,
                "task_id": task["id"],
            }));

            let worker_id = request.assignment_worker_id(index, assignment);
            let mut worker_launched_for_assignment = false;
            if request.submit {
                if request
                    .team_worker_has_compatible_live_surface(
                        state, assignment, &task_id, &worker_id,
                    )
                    .await?
                {
                    actions.push(json!({
                        "method": "team.worker.launch",
                        "status": "already_exists",
                        "team_id": request.team_id,
                        "worker_id": worker_id,
                    }));
                } else {
                    let launched = team_runtime::worker_launch(
                        state,
                        &request.team_worker_launch_params(assignment, &task_id, &worker_id),
                    )
                    .await?;
                    actions.push(json!({
                        "method": "team.worker.launch",
                        "status": "applied",
                        "team_id": request.team_id,
                        "worker_id": launched["worker"]["id"],
                        "surface_id": launched["surface"]["id"],
                        "selected_agent": launched["selection"]["selected_agent"],
                    }));
                    worker_launched_for_assignment = true;
                }

                let assigned = team_runtime::task_upsert(
                    state,
                    &request.team_task_assign_params(&task_id, &worker_id),
                )
                .await?;
                actions.push(json!({
                    "method": "team.task.upsert",
                    "status": "applied",
                    "team_id": request.team_id,
                    "task_id": assigned["id"],
                    "assigned_worker_id": worker_id,
                }));
            }

            let base_message_id = format!("{task_id}-msg-1");
            let expected_body = assignment_prompt(
                &request.goal,
                &request.plan.strategy,
                index,
                assignment,
                request.assignment_target_context(),
            );
            let message_id = request
                .assignment_message_id(
                    state,
                    &task_id,
                    &base_message_id,
                    request.submit.then_some(worker_id.as_str()),
                    &expected_body,
                    worker_launched_for_assignment,
                )
                .await?;
            if request.team_message_exists(state, &message_id).await? {
                actions.push(json!({
                    "method": "team.message.send",
                    "status": "already_exists",
                    "team_id": request.team_id,
                    "message_id": message_id,
                }));
            } else {
                let message = team_runtime::message_send(
                    state,
                    &request.team_message_params(
                        index,
                        assignment,
                        &task_id,
                        &message_id,
                        request.submit.then_some(worker_id.as_str()),
                    ),
                )
                .await?;
                actions.push(json!({
                    "method": "team.message.send",
                    "status": "applied",
                    "team_id": request.team_id,
                    "message_id": message["id"],
                }));
            }
            let superseded_message_ids = request
                .supersede_stale_assignment_messages(state, &task_id, &message_id)
                .await?;
            if !superseded_message_ids.is_empty() {
                actions.push(json!({
                    "method": "team.message.supersede",
                    "status": "applied",
                    "team_id": request.team_id,
                    "message_ids": superseded_message_ids,
                }));
            }

            if request.submit {
                if !worker_launched_for_assignment
                    && request.team_message_delivered(state, &message_id).await?
                {
                    actions.push(json!({
                        "method": "team.message.dispatch",
                        "status": "already_dispatched",
                        "team_id": request.team_id,
                        "message_id": message_id,
                        "worker_id": worker_id,
                    }));
                } else {
                    let dispatched = team_runtime::message_dispatch(
                        state,
                        &json!({
                            "team_id": request.team_id,
                            "message_id": message_id,
                            "worker_id": worker_id,
                            "submit": true,
                        }),
                    )
                    .await?;
                    actions.push(json!({
                        "method": "team.message.dispatch",
                        "status": "applied",
                        "team_id": request.team_id,
                        "message_id": dispatched["message"]["id"],
                        "worker_id": dispatched["worker_id"],
                        "surface_id": dispatched["surface_id"],
                        "submitted": dispatched["submitted"],
                    }));
                }
            }
        }
    }

    Ok(json!({
        "run_id": request.run_id,
        "status": run_status,
        "workflow_id": if request.plan.layers.workflow { Value::String(request.workflow_id) } else { Value::Null },
        "team_id": if request.plan.layers.team { Value::String(request.team_id) } else { Value::Null },
        "actions": actions,
        "blocked_approvals": [],
        "monitoring": {
            "workflow": if request.plan.layers.workflow { Value::String("workflow.get".to_string()) } else { Value::Null },
            "team": if request.plan.layers.team { Value::String("team.summary".to_string()) } else { Value::Null },
        }
    }))
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
        _ => Err(DispatchError::InvalidParam(
            "unsupported task strategy".to_string(),
        )),
    }
}

fn task_router_profile_from_str(value: &str) -> Result<TaskRouterProfile, DispatchError> {
    match value {
        "balanced" => Ok(TaskRouterProfile::Balanced),
        "fast" => Ok(TaskRouterProfile::Fast),
        "conservative" => Ok(TaskRouterProfile::Conservative),
        "parallel" => Ok(TaskRouterProfile::Parallel),
        "review_heavy" => Ok(TaskRouterProfile::ReviewHeavy),
        _ => Err(DispatchError::InvalidParam(
            "unsupported router profile".to_string(),
        )),
    }
}

fn last_known_good_from_value(value: &Value) -> Result<TaskStrategyLastKnownGood, DispatchError> {
    let Some(object) = value.as_object() else {
        return Err(DispatchError::InvalidParam(
            "last_known_good must be an object".to_string(),
        ));
    };
    let strategy = optional_object_string(object, "strategy", "last_known_good")?
        .as_deref()
        .map(task_strategy_from_str)
        .transpose()?;
    let harness_id = optional_object_string(object, "harness_id", "last_known_good")?;
    if let Some(harness_id) = harness_id.as_deref() {
        validate_task_strategy_id("last_known_good.harness_id", harness_id)?;
    }
    let reason = optional_bounded_object_string(
        object,
        "reason",
        "last_known_good",
        "last_known_good.reason",
        MAX_TASK_STRATEGY_REASON_BYTES,
    )?;
    Ok(TaskStrategyLastKnownGood {
        strategy,
        harness_id,
        reason,
    })
}

fn optional_object_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, DispatchError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.chars().any(char::is_control) {
                return Err(DispatchError::InvalidParam(format!(
                    "{path}.{key} must not contain control characters"
                )));
            }
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "{path}.{key} must be a string"
        ))),
    }
}

fn optional_bounded_object_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
    field: &'static str,
    limit: usize,
) -> Result<Option<String>, DispatchError> {
    let value = optional_object_string(object, key, path)?;
    if let Some(value) = value.as_deref() {
        if value.len() > limit {
            return Err(DispatchError::PayloadTooLarge {
                field,
                limit,
                actual: value.len(),
            });
        }
        if value.chars().any(char::is_control) {
            return Err(DispatchError::InvalidParam(format!(
                "Invalid parameter {field}: must not contain control characters"
            )));
        }
    }
    Ok(value)
}

pub(crate) fn harness_registry_from_capabilities(capabilities: &Value) -> HarnessRegistry {
    let mut harnesses = Vec::new();
    if let Some(providers) = capabilities["provider_capabilities"].as_object() {
        let mut seen = Vec::new();
        if let Some(order) = capabilities["team_provider_policy"]["provider_order"].as_array() {
            for id in order.iter().filter_map(Value::as_str) {
                if let Some(provider) = providers.get(id) {
                    harnesses.push(harness_capability_from_provider(id, provider));
                    seen.push(id.to_string());
                }
            }
        }
        for (id, provider) in providers {
            if seen.iter().any(|seen_id| seen_id == id) {
                continue;
            }
            harnesses.push(harness_capability_from_provider(id, provider));
        }
    }
    HarnessRegistry { harnesses }
}

fn harness_capability_from_provider(id: &str, provider: &Value) -> HarnessCapability {
    let launchable = provider["launchable"].as_bool().unwrap_or(false);
    let disabled = provider["disabled_by_config"].as_bool().unwrap_or(false);
    let available_on_path = provider["available_on_path"].as_bool().unwrap_or(false);
    let executable_present = !provider["executable"].is_null();
    let configured_command = !provider["configured_command"].is_null();
    let installed = available_on_path || executable_present || configured_command;

    let supports_team_launch =
        launchable && provider["team_worker_launch"].as_bool().unwrap_or(false);

    HarnessCapability {
        id: id.to_string(),
        installed,
        authenticated: launchable,
        supports_prompt_launch: supports_team_launch,
        supports_resume: provider["safe_resume"].as_bool().unwrap_or(false),
        supports_hooks: true,
        supports_mcp: true,
        supports_plan_mode: provider["plan_mode"]
            .as_bool()
            .or_else(|| provider["supports_plan_mode"].as_bool())
            .unwrap_or(false),
        supports_worktree_cwd: supports_team_launch,
        max_parallel_sessions: supports_team_launch
            .then_some(DEFAULT_PROVIDER_MAX_PARALLEL_SESSIONS),
        health: if disabled {
            HarnessHealth::Disabled
        } else if launchable {
            HarnessHealth::Ready
        } else {
            HarnessHealth::Missing
        },
        routing_signals: HarnessRoutingSignals::default(),
    }
}

pub(crate) fn apply_harness_routing_signals(
    registry: &mut HarnessRegistry,
    signals: &Value,
) -> Result<(), DispatchError> {
    let Some(signals) = signals.as_object() else {
        return Err(DispatchError::InvalidParam(
            "harness_signals must be an object".to_string(),
        ));
    };

    for (harness_id, value) in signals {
        validate_task_strategy_id("harness_signals key", harness_id)?;
        let path = format!("harness_signals.{harness_id}");
        let Some(signal) = value.as_object() else {
            return Err(DispatchError::InvalidParam(format!(
                "{path} must be an object"
            )));
        };
        let Some(harness) = registry
            .harnesses
            .iter_mut()
            .find(|harness| harness.id == *harness_id)
        else {
            return Err(DispatchError::InvalidParam(format!(
                "{path} references an unknown harness"
            )));
        };
        harness.routing_signals = HarnessRoutingSignals {
            cooldown: optional_signal_bool(signal, "cooldown", &path)?,
            cooldown_reason: optional_signal_reason(signal, "cooldown_reason", &path)?,
            locked_out: optional_signal_bool(signal, "locked_out", &path)?,
            lockout_reason: optional_signal_reason(signal, "lockout_reason", &path)?,
        };
    }

    Ok(())
}

fn optional_signal_bool(
    signal: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<bool, DispatchError> {
    match signal.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "{path}.{key} must be a boolean"
        ))),
    }
}

fn optional_signal_reason(
    signal: &serde_json::Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, DispatchError> {
    match signal.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.chars().any(char::is_control) {
                return Err(DispatchError::InvalidParam(format!(
                    "{path}.{key} must not contain control characters"
                )));
            }
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else if trimmed.len() > MAX_TASK_STRATEGY_REASON_BYTES {
                Err(DispatchError::PayloadTooLarge {
                    field: "harness_signals.reason",
                    limit: MAX_TASK_STRATEGY_REASON_BYTES,
                    actual: trimmed.len(),
                })
            } else if trimmed.chars().any(char::is_control) {
                Err(DispatchError::InvalidParam(format!(
                    "{path}.{key} must not contain control characters"
                )))
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "{path}.{key} must be a string"
        ))),
    }
}

fn validate_submitted_task_strategy_plan(plan: &TaskStrategyPlan) -> Result<(), DispatchError> {
    if plan.layers.team && !task_strategy_supports_team_layer(&plan.strategy) {
        return Err(DispatchError::InvalidParam(format!(
            "task.strategy.apply strategy {} does not support a team layer",
            task_strategy_wire_value(&plan.strategy)
        )));
    }
    if plan.layers.team && !plan.layers.workflow {
        return Err(DispatchError::InvalidParam(
            "task.strategy.apply team layer requires workflow layer".to_string(),
        ));
    }
    if plan.layers.loop_metadata && !task_strategy_supports_loop_metadata_layer(&plan.strategy) {
        return Err(DispatchError::InvalidParam(format!(
            "task.strategy.apply strategy {} does not support loop metadata layer",
            task_strategy_wire_value(&plan.strategy)
        )));
    }
    if plan.layers.loop_metadata && !plan.layers.workflow {
        return Err(DispatchError::InvalidParam(
            "task.strategy.apply loop metadata requires workflow layer".to_string(),
        ));
    }
    if plan.assignments.is_empty() {
        return Err(DispatchError::InvalidParam(
            "task.strategy.apply plan requires at least one assignment; team layer requires at least one team assignment"
                .to_string(),
        ));
    }
    if plan.assignments.len() > MAX_TASK_STRATEGY_ASSIGNMENTS {
        return Err(DispatchError::InvalidParam(format!(
            "task.strategy.apply plan has too many assignments (limit {MAX_TASK_STRATEGY_ASSIGNMENTS})"
        )));
    }
    let (min_assignments, max_assignments) = task_strategy_assignment_count_bounds(&plan.strategy);
    if plan.assignments.len() < min_assignments || plan.assignments.len() > max_assignments {
        let assignment_count = plan.assignments.len();
        let strategy = task_strategy_wire_value(&plan.strategy);
        let message = if min_assignments == max_assignments {
            let expected = if min_assignments == 1 {
                "one assignment".to_string()
            } else {
                format!("{min_assignments} assignments")
            };
            format!(
                "task.strategy.apply strategy {strategy} supports exactly {expected}; got {assignment_count}"
            )
        } else {
            format!(
                "task.strategy.apply strategy {strategy} supports between {min_assignments} and {max_assignments} assignments; got {assignment_count}"
            )
        };
        return Err(DispatchError::InvalidParam(message));
    }
    validate_submitted_task_strategy_item_count(
        "plan.reasons",
        plan.reasons.len(),
        MAX_TASK_STRATEGY_PLAN_REASONS,
    )?;
    validate_submitted_task_strategy_item_count(
        "plan.safety_notes",
        plan.safety_notes.len(),
        MAX_TASK_STRATEGY_SAFETY_NOTES,
    )?;
    validate_submitted_task_strategy_item_count(
        "plan.candidate_scores",
        plan.candidate_scores.len(),
        MAX_TASK_STRATEGY_CANDIDATE_SCORES,
    )?;
    validate_submitted_task_strategy_item_count(
        "plan.approvals",
        plan.approvals.len(),
        MAX_TASK_STRATEGY_APPROVALS,
    )?;
    validate_submitted_task_strategy_approvals(&plan.approvals)?;
    for reason in &plan.reasons {
        validate_submitted_task_strategy_text("plan.reasons", reason)?;
    }
    for safety_note in &plan.safety_notes {
        validate_submitted_task_strategy_text("plan.safety_notes", safety_note)?;
    }
    for assignment in &plan.assignments {
        if !task_strategy_supports_assignment_role(&plan.strategy, &assignment.role) {
            return Err(DispatchError::InvalidParam(format!(
                "task.strategy.apply strategy {} does not support role {}",
                task_strategy_wire_value(&plan.strategy),
                role_id(&assignment.role)
            )));
        }
        validate_task_strategy_id("plan.assignments.harness_id", &assignment.harness_id)?;
        validate_submitted_task_strategy_text("plan.assignments.reason", &assignment.reason)?;
        validate_submitted_score_factors("plan.assignments.factors", &assignment.factors)?;
    }
    for candidate in &plan.candidate_scores {
        validate_submitted_score_factors("plan.candidate_scores.factors", &candidate.factors)?;
    }
    Ok(())
}

fn validate_submitted_task_strategy_approvals(
    approvals: &[TaskStrategyApproval],
) -> Result<(), DispatchError> {
    for (index, approval) in approvals.iter().enumerate() {
        if approvals
            .iter()
            .skip(index + 1)
            .any(|other| other == approval)
        {
            return Err(DispatchError::InvalidParam(
                "plan.approvals has duplicate items".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_submitted_score_factors(
    field: &'static str,
    factors: &[TaskStrategyScoreFactor],
) -> Result<(), DispatchError> {
    validate_submitted_task_strategy_item_count(
        field,
        factors.len(),
        MAX_TASK_STRATEGY_SCORE_FACTORS,
    )?;
    for factor in factors {
        validate_submitted_task_strategy_text(field, &factor.name)?;
        validate_submitted_task_strategy_text(field, &factor.reason)?;
    }
    Ok(())
}

fn validate_submitted_task_strategy_item_count(
    field: &'static str,
    len: usize,
    limit: usize,
) -> Result<(), DispatchError> {
    if len > limit {
        return Err(DispatchError::InvalidParam(format!(
            "{field} has too many items (limit {limit})"
        )));
    }
    Ok(())
}

fn validate_submitted_task_strategy_text(
    field: &'static str,
    value: &str,
) -> Result<(), DispatchError> {
    if value.len() > MAX_TASK_STRATEGY_REASON_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field,
            limit: MAX_TASK_STRATEGY_REASON_BYTES,
            actual: value.len(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {field}: must not contain control characters"
        )));
    }
    Ok(())
}

struct TaskStrategyApplyRequest {
    run_id: String,
    workflow_id: String,
    team_id: String,
    workspace_id: Option<String>,
    workspace_name: Option<String>,
    worktree_name: Option<String>,
    target_cwd: Option<PathBuf>,
    effective_target_cwd: Option<PathBuf>,
    leader_surface_id: Option<String>,
    goal: String,
    plan: TaskStrategyPlan,
    approved: Vec<String>,
    approval_id: Option<String>,
    request_approval: bool,
    submit: bool,
}

impl TaskStrategyApplyRequest {
    fn decode(params: &Value) -> Result<Self, DispatchError> {
        let run_id = required_trimmed_string(params, "run_id")?.to_string();
        validate_task_strategy_id("run_id", &run_id)?;
        let workflow_id = optional_non_blank_string_param(params, "workflow_id")?
            .unwrap_or(&run_id)
            .to_string();
        validate_task_strategy_id("workflow_id", &workflow_id)?;
        let team_id = optional_non_blank_string_param(params, "team_id")?
            .unwrap_or(&run_id)
            .to_string();
        validate_task_strategy_id("team_id", &team_id)?;
        let goal = task_strategy_goal_param(params)?;
        let plan_value = params
            .get("plan")
            .ok_or(DispatchError::MissingParam("plan"))?
            .clone();
        let plan = serde_json::from_value::<TaskStrategyPlan>(plan_value)
            .map_err(|_| DispatchError::InvalidParam("Invalid parameter plan".to_string()))?;
        validate_submitted_task_strategy_plan(&plan)?;
        let approved = optional_string_array_param(params, "approved")?.unwrap_or_default();
        let submit = optional_bool_param(params, "submit")?.unwrap_or(false);
        let approval_id =
            optional_non_blank_string_param(params, "approval_id")?.map(str::to_string);
        if let Some(approval_id) = approval_id.as_deref() {
            validate_task_strategy_id("approval_id", approval_id)?;
        }
        let request_approval = optional_bool_param(params, "request_approval")?.unwrap_or(false);
        let leader_surface_id = optional_task_strategy_surface_alias_param(params)?;
        let workspace_selector = task_strategy_workspace_selector_params(params)?;
        let workspace_id = workspace_selector.workspace_id;
        let workspace_name = workspace_selector.workspace_name;
        let worktree_name = workspace_selector.worktree_name;
        let explicit_cwd = explicit_task_strategy_cwd(params)?;
        if worktree_name.is_some() && explicit_cwd.is_some() {
            return Err(DispatchError::InvalidParam(
                "cannot combine worktree_name and cwd".to_string(),
            ));
        }
        let target_cwd = if worktree_name.is_none() {
            explicit_cwd
        } else {
            None
        };
        let selector_count = [&workspace_id, &workspace_name, &worktree_name]
            .into_iter()
            .filter(|value| value.is_some())
            .count();
        if selector_count > 1 {
            return Err(DispatchError::InvalidParam(
                "cannot combine workspace_id, workspace_name, and worktree_name".to_string(),
            ));
        }
        Ok(Self {
            run_id,
            workflow_id,
            team_id,
            workspace_id,
            workspace_name,
            worktree_name,
            target_cwd,
            effective_target_cwd: None,
            leader_surface_id,
            goal,
            plan,
            approved,
            approval_id,
            request_approval,
            submit,
        })
    }

    fn required_approvals(&self) -> Vec<String> {
        let mut required = vec!["start_run"];
        if self.plan.layers.worktree {
            required.push("create_worktree");
        }
        if self.submit && requires_parallel_workers_approval(self.plan.assignments.len()) {
            required.push("launch_parallel_workers");
        }
        for approval in self.plan.approvals.iter().map(approval_id) {
            if approval == "launch_parallel_workers" && !self.submit {
                continue;
            }
            if !required.contains(&approval) {
                required.push(approval);
            }
        }
        required.into_iter().map(str::to_string).collect()
    }

    fn missing_approvals_from(&self, required: &[String]) -> Vec<String> {
        required
            .iter()
            .filter(|approval| !self.approved.iter().any(|value| value == *approval))
            .cloned()
            .collect()
    }

    fn resolve_apply_target(&mut self, state: &SocketAppState) -> Result<(), DispatchError> {
        let Some(workspace_name) = self.workspace_name.take() else {
            return Ok(());
        };
        let model = state
            .model
            .lock()
            .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
        let workspace_id = model
            .workspace_id_for(WorkspaceSelector::Name(&workspace_name))
            .ok_or_else(|| DispatchError::NotFound("workspace".to_string()))?;
        self.workspace_id = Some(workspace_id);
        Ok(())
    }

    fn enforce_server_detected_worktree_isolation(
        &mut self,
        state: &SocketAppState,
        params: &Value,
    ) -> Result<(), DispatchError> {
        let explicit_target_dirty = self
            .target_cwd
            .as_deref()
            .map(|cwd| infer_repo_dirty(Some(cwd)))
            .unwrap_or(false);
        let selected_target_dirty = match task_strategy_target_context(state, params, false) {
            Ok(target) => {
                if self.target_cwd.is_none() {
                    self.effective_target_cwd = target.cwd.clone();
                }
                if self.target_cwd.is_some() {
                    false
                } else {
                    infer_repo_dirty(target.cwd.as_deref())
                }
            }
            Err(err) if self.worktree_name.is_some() && err.code() == "not_found" => false,
            Err(err) => return Err(err),
        };
        let read_only_review_plan = task_strategy_plan_is_read_only_review(&self.plan);
        let likely_user_visible_change =
            task_class_likely_user_visible_change(&self.plan.task_class)
                || task_strategy_likely_user_visible_change(&self.plan.strategy)
                || infer_likely_user_visible_change(&self.goal);
        if (explicit_target_dirty || selected_target_dirty)
            && likely_user_visible_change
            && !read_only_review_plan
        {
            if !self.plan.layers.worktree {
                self.plan.layers.worktree = true;
                self.plan.reasons.push(
                    "server detected dirty repo plus editing task and forced worktree isolation"
                        .to_string(),
                );
            }
            if !self
                .plan
                .approvals
                .contains(&TaskStrategyApproval::CreateWorktree)
            {
                self.plan
                    .approvals
                    .push(TaskStrategyApproval::CreateWorktree);
                self.plan
                    .approvals
                    .sort_by_key(task_strategy_approval_order);
            }
        }
        Ok(())
    }

    fn approval_request_id(&self, missing_approvals: &[String]) -> String {
        format!(
            "task-strategy:{}:approvals:{}",
            self.approval_scope_fingerprint(missing_approvals),
            missing_approvals.join(".")
        )
    }

    fn approval_scope_fingerprint(&self, missing_approvals: &[String]) -> String {
        let mut hash = 0xcbf29ce484222325u64;
        feed_hash(&mut hash, "run_id");
        feed_hash(&mut hash, &self.run_id);
        feed_hash(&mut hash, "workflow_id");
        feed_hash(&mut hash, &self.workflow_id);
        feed_hash(&mut hash, "team_id");
        feed_hash(&mut hash, &self.team_id);
        feed_hash(&mut hash, "workspace_id");
        feed_hash(&mut hash, self.workspace_id.as_deref().unwrap_or(""));
        feed_hash(&mut hash, "worktree_name");
        feed_hash(&mut hash, self.worktree_name.as_deref().unwrap_or(""));
        feed_hash(&mut hash, "cwd");
        feed_hash(
            &mut hash,
            &self
                .target_cwd
                .as_deref()
                .map(|cwd| cwd.to_string_lossy())
                .unwrap_or_default(),
        );
        feed_hash(&mut hash, "leader_surface_id");
        feed_hash(&mut hash, self.leader_surface_id.as_deref().unwrap_or(""));
        feed_hash(&mut hash, "goal");
        feed_hash(&mut hash, &self.goal);
        feed_hash(&mut hash, "plan");
        feed_hash(
            &mut hash,
            &serde_json::to_string(&self.plan).expect("TaskStrategyPlan serialization cannot fail"),
        );
        feed_hash(&mut hash, "missing_approvals");
        for approval in missing_approvals {
            feed_hash(&mut hash, approval);
        }
        feed_hash(&mut hash, "submit");
        feed_hash(&mut hash, if self.submit { "true" } else { "false" });
        format!("{hash:016x}")
    }

    fn feed_approval_satisfies(
        &self,
        state: &SocketAppState,
        required_approvals: &[String],
        missing_approvals: &[String],
    ) -> Result<bool, DispatchError> {
        let Some(approval_id) = self.approval_id.as_deref() else {
            return Ok(false);
        };
        let accepted_ids =
            self.approval_request_ids_covering_missing(required_approvals, missing_approvals);
        if !accepted_ids.iter().any(|id| id == approval_id) {
            let expected = accepted_ids
                .first()
                .cloned()
                .unwrap_or_else(|| self.approval_request_id(missing_approvals));
            return Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply approval_id {approval_id} does not match required approval request {expected}"
            )));
        }
        let store = state
            .feed_store
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let Some(store) = store.as_ref() else {
            return Err(DispatchError::NotReady(
                "Feed history is not available".to_string(),
            ));
        };
        let Some(entry) = store
            .list(None, usize::MAX)
            .into_iter()
            .find(|entry| entry.id == approval_id)
        else {
            return Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply approval request {approval_id} was not found"
            )));
        };
        match entry.approval_state {
            Some(FeedApprovalState::Approved) => Ok(true),
            Some(FeedApprovalState::Denied) => Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply approval request {approval_id} was denied"
            ))),
            Some(FeedApprovalState::Dismissed) | Some(FeedApprovalState::Stale) => {
                Err(DispatchError::PreconditionFailed(format!(
                    "task.strategy.apply approval request {approval_id} is no longer active"
                )))
            }
            Some(FeedApprovalState::Pending) | None => Err(DispatchError::PreconditionFailed(
                format!("task.strategy.apply approval request {approval_id} is not approved"),
            )),
        }
    }

    fn record_approval_request(
        &self,
        state: &SocketAppState,
        missing_approvals: &[String],
    ) -> Result<Value, DispatchError> {
        let approval_id = self.approval_request_id(missing_approvals);
        let mut store = state
            .feed_store
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let Some(store) = store.as_mut() else {
            return Err(DispatchError::NotReady(
                "Feed history is not available".to_string(),
            ));
        };
        if let Some(existing) = store
            .list(None, usize::MAX)
            .into_iter()
            .find(|entry| entry.id == approval_id)
        {
            return Ok(self.blocked_for_approval_result(
                missing_approvals,
                json!(existing),
                "already_exists",
            ));
        }

        let entry = FeedEntry {
            id: approval_id,
            entry_type: FeedEntryType::Approval,
            kind: Some("task_strategy".to_string()),
            read: false,
            key: Some(self.run_id.clone()),
            value: None,
            total: None,
            title: format!("Start ForkTTY task run {}", self.run_id),
            body: format!(
                "Goal: {}\nMissing approvals: {}\nStrategy: {:?}\nSubmit: {}",
                self.goal,
                missing_approvals.join(", "),
                self.plan.strategy,
                self.submit
            ),
            workspace_id: self.workspace_id.clone(),
            surface_id: self.leader_surface_id.clone(),
            created_at_ms: u128::from(current_unix_epoch_ms()),
            approval_state: Some(FeedApprovalState::Pending),
        };
        store
            .append(entry.clone())
            .map_err(|err| DispatchError::Other(err.to_string()))?;
        Ok(self.blocked_for_approval_result(missing_approvals, json!(entry), "applied"))
    }

    fn dismiss_superseded_approval_requests(
        &self,
        state: &SocketAppState,
        required_approvals: &[String],
        actions: &mut Vec<Value>,
    ) -> Result<(), DispatchError> {
        if required_approvals.is_empty() || (self.approval_id.is_some() && self.approved.is_empty())
        {
            return Ok(());
        }
        let approval_ids = self.approval_request_ids_for_subsets(required_approvals);
        let mut store = state
            .feed_store
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let Some(store) = store.as_mut() else {
            return Ok(());
        };
        let dismissed = store
            .mark_approvals(approval_ids, FeedApprovalState::Dismissed)
            .map_err(|err| DispatchError::Other(err.to_string()))?;
        for entry in dismissed {
            actions.push(json!({
                "method": "feed.approval.dismiss",
                "status": "applied",
                "approval_id": entry.id,
            }));
        }
        Ok(())
    }

    fn approval_request_ids_for_subsets(&self, approvals: &[String]) -> Vec<String> {
        let mut ids = Vec::new();
        let subset_count = 1usize << approvals.len();
        for mask in 1..subset_count {
            let subset = approvals
                .iter()
                .enumerate()
                .filter(|(index, _)| (mask & (1usize << index)) != 0)
                .map(|(_, approval)| approval.clone())
                .collect::<Vec<_>>();
            ids.push(self.approval_request_id(&subset));
        }
        ids
    }

    fn approval_request_ids_covering_missing(
        &self,
        required_approvals: &[String],
        missing_approvals: &[String],
    ) -> Vec<String> {
        let mut ids = Vec::new();
        let subset_count = 1usize << required_approvals.len();
        for mask in 1..subset_count {
            let subset = required_approvals
                .iter()
                .enumerate()
                .filter(|(index, _)| (mask & (1usize << index)) != 0)
                .map(|(_, approval)| approval.clone())
                .collect::<Vec<_>>();
            if missing_approvals
                .iter()
                .all(|approval| subset.iter().any(|candidate| candidate == approval))
            {
                ids.push(self.approval_request_id(&subset));
            }
        }
        ids
    }

    fn blocked_for_approval_result(
        &self,
        missing_approvals: &[String],
        approval_request: Value,
        action_status: &str,
    ) -> Value {
        json!({
            "run_id": self.run_id,
            "status": "blocked",
            "workflow_id": Value::Null,
            "team_id": Value::Null,
            "actions": [
                {
                    "method": "feed.approval.request",
                    "status": action_status,
                    "approval_id": approval_request["id"].clone(),
                }
            ],
            "blocked_approvals": missing_approvals,
            "approval_request": approval_request,
            "monitoring": {
                "feed": "feed.list",
                "workflow": Value::Null,
                "team": Value::Null,
            }
        })
    }

    fn validate_before_mutation(&self, state: &SocketAppState) -> Result<(), DispatchError> {
        if let Some(cwd) = self.target_cwd.as_deref() {
            validate_explicit_task_strategy_cwd(state, cwd)?;
        }
        if self.submit && !self.plan.layers.team {
            return Err(DispatchError::InvalidParam(
                "task.strategy.apply submit=true requires a supported team layer".to_string(),
            ));
        }
        if self.plan.layers.team && self.plan.assignments.is_empty() {
            return Err(DispatchError::InvalidParam(
                "task.strategy.apply team layer requires at least one team assignment".to_string(),
            ));
        }
        if self.plan.layers.worktree && self.worktree_name.is_none() {
            return Err(DispatchError::InvalidParam(
                "task.strategy.apply with worktree layer requires worktree_name for an already-open ForkTTY worktree workspace"
                    .to_string(),
            ));
        }
        if self.plan.layers.worktree {
            if let Some(worktree_name) = self.worktree_name.as_deref() {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                if model
                    .workspace_id_for(WorkspaceSelector::WorktreeName(worktree_name))
                    .is_none()
                {
                    return Err(DispatchError::PreconditionFailed(format!(
                        "task.strategy.apply with worktree layer requires an open ForkTTY worktree workspace named {worktree_name}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_submit_assignments_launchable(&self) -> Result<(), DispatchError> {
        for assignment in &self.plan.assignments {
            let selection = select_team_worker_provider(Some(assignment.harness_id.as_str()))?;
            let _ = team_worker_launch_command_with_program(
                &selection.selected_agent,
                &selection.program,
                Some(role_id(&assignment.role)),
                Vec::new(),
            )?;
        }
        Ok(())
    }

    fn workflow_upsert_params(&self, status: &str, mode: &str) -> Value {
        let mut params = json!({
            "workflow_id": self.workflow_id,
            "mode": mode,
            "status": status,
            "goal": self.goal,
            "memory": format!(
                "Task strategy: {}. Run id: {}.",
                task_strategy_wire_value(&self.plan.strategy),
                self.run_id
            ),
        });
        if let Some(worktree_name) = self.worktree_name.as_deref() {
            params["worktree_name"] = json!(worktree_name);
        } else if let Some(workspace_id) = self.workspace_id.as_deref() {
            params["workspace_id"] = json!(workspace_id);
        } else if let Some(workspace_name) = self.workspace_name.as_deref() {
            params["workspace_name"] = json!(workspace_name);
        }
        if self.worktree_name.is_none() {
            if let Some(surface_id) = self.leader_surface_id.as_deref() {
                params["surface_id"] = json!(surface_id);
            }
        }
        params
    }

    fn add_workspace_target_params(&self, params: &mut Value) {
        if let Some(worktree_name) = self.worktree_name.as_deref() {
            params["worktree_name"] = json!(worktree_name);
        } else if let Some(workspace_id) = self.workspace_id.as_deref() {
            params["workspace_id"] = json!(workspace_id);
        } else if let Some(workspace_name) = self.workspace_name.as_deref() {
            params["workspace_name"] = json!(workspace_name);
        }
    }

    fn add_leader_surface_param(&self, params: &mut Value) {
        if self.worktree_name.is_none() {
            if let Some(surface_id) = self.leader_surface_id.as_deref() {
                params["leader_surface_id"] = json!(surface_id);
            }
        }
    }

    fn add_launch_target_param(&self, params: &mut Value) {
        if let Some(worktree_name) = self.worktree_name.as_deref() {
            params["worktree_name"] = json!(worktree_name);
        } else if let Some(cwd) = self.target_cwd.as_deref() {
            params["cwd"] = json!(cwd);
        }
    }

    fn workflow_plan_params(&self) -> Value {
        let assignments = self
            .plan
            .assignments
            .iter()
            .enumerate()
            .map(|(index, assignment)| {
                let role = role_id(&assignment.role);
                json!({
                    "id": format!("{}-step-{role}-{}", self.run_id, index + 1),
                    "title": assignment_title(assignment),
                    "status": "pending",
                    "detail": assignment_detail(&self.goal, &self.plan.strategy, index, assignment, self.assignment_target_context()),
                })
            })
            .collect::<Vec<_>>();
        json!({
            "workflow_id": self.workflow_id,
            "steps": assignments,
        })
    }

    fn workflow_loop_params(&self, stage: &str) -> Value {
        json!({
            "workflow_id": self.workflow_id,
            "recipe": "task_strategy_verify_loop",
            "stage": stage,
            "iteration": 0,
            "max_iterations": 3,
            "gates": [
                {
                    "id": format!("{}-gate-verify", self.run_id),
                    "kind": "verification",
                    "label": "Run relevant verification",
                    "status": "pending",
                    "summary": "Verification must be recorded before the run is considered complete"
                }
            ]
        })
    }

    fn team_upsert_params(&self, status: &str) -> Value {
        let mut params = json!({
            "team_id": self.team_id,
            "name": format!("Task strategy {}", self.run_id),
            "status": status,
            "goal": self.goal,
        });
        self.add_workspace_target_params(&mut params);
        self.add_leader_surface_param(&mut params);
        params
    }

    fn assignment_task_id(&self, index: usize, assignment: &HarnessAssignment) -> String {
        format!(
            "{}-{}-{}",
            self.run_id,
            role_id(&assignment.role),
            index + 1
        )
    }

    fn assignment_worker_id(&self, index: usize, assignment: &HarnessAssignment) -> String {
        format!(
            "{}-{}-{}-worker",
            self.run_id,
            role_id(&assignment.role),
            index + 1
        )
    }

    fn team_task_params(
        &self,
        index: usize,
        assignment: &HarnessAssignment,
        task_id: &str,
    ) -> Value {
        json!({
            "team_id": self.team_id,
            "task_id": task_id,
            "title": assignment_title(assignment),
            "status": "open",
            "detail": assignment_detail(&self.goal, &self.plan.strategy, index, assignment, self.assignment_target_context()),
        })
    }

    fn team_task_assign_params(&self, task_id: &str, worker_id: &str) -> Value {
        json!({
            "team_id": self.team_id,
            "task_id": task_id,
            "assigned_worker_id": worker_id,
            "status": "running",
        })
    }

    fn team_worker_launch_params(
        &self,
        assignment: &HarnessAssignment,
        task_id: &str,
        worker_id: &str,
    ) -> Value {
        let mut params = json!({
            "team_id": self.team_id,
            "worker_id": worker_id,
            "agent": assignment.harness_id,
            "role": role_id(&assignment.role),
            "assigned_task_id": task_id,
        });
        self.add_launch_target_param(&mut params);
        params
    }

    fn team_message_params(
        &self,
        index: usize,
        assignment: &HarnessAssignment,
        task_id: &str,
        message_id: &str,
        worker_id: Option<&str>,
    ) -> Value {
        let mut params = json!({
            "team_id": self.team_id,
            "message_id": message_id,
            "from": "leader",
            "task_id": task_id,
            "body": assignment_prompt(&self.goal, &self.plan.strategy, index, assignment, self.assignment_target_context()),
        });
        if let Some(worker_id) = worker_id {
            params["to_worker_id"] = json!(worker_id);
        }
        params
    }

    fn assignment_target_context(&self) -> AssignmentTargetContext<'_> {
        AssignmentTargetContext {
            worktree_name: self.worktree_name.as_deref(),
            cwd: self.target_cwd.as_deref().or_else(|| {
                self.worktree_name
                    .as_ref()
                    .and(self.effective_target_cwd.as_deref())
            }),
        }
    }

    async fn team_message_exists(
        &self,
        state: &SocketAppState,
        message_id: &str,
    ) -> Result<bool, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        Ok(team["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|message| message["id"].as_str() == Some(message_id)))
    }

    async fn supersede_stale_assignment_messages(
        &self,
        state: &SocketAppState,
        task_id: &str,
        current_message_id: &str,
    ) -> Result<Vec<String>, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        let prefix = format!("{task_id}-msg-");
        let Some(messages) = team["messages"].as_array() else {
            return Ok(Vec::new());
        };
        let message_ids = messages
            .iter()
            .filter(|message| message["task_id"].as_str() == Some(task_id))
            .filter(|message| !message["delivered"].as_bool().unwrap_or(false))
            .filter(|message| !message["superseded"].as_bool().unwrap_or(false))
            .filter_map(|message| message["id"].as_str())
            .filter(|message_id| *message_id != current_message_id)
            .filter(|message_id| message_id.starts_with(&prefix))
            .map(str::to_string)
            .collect::<Vec<_>>();
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }
        let input = forktty_core::TeamMessagesSupersede {
            team_id: self.team_id.clone(),
            message_ids,
        };
        let messages = store_access::team_store_access(state)?
            .update(move |store| store.supersede_messages(input, forktty_core::team_now_ms()))
            .await
            .map_err(DispatchError::from)?;
        Ok(messages.into_iter().map(|message| message.id).collect())
    }

    async fn assignment_message_id(
        &self,
        state: &SocketAppState,
        task_id: &str,
        base_message_id: &str,
        worker_id: Option<&str>,
        expected_body: &str,
        worker_launched_for_assignment: bool,
    ) -> Result<String, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        let Some(messages) = team["messages"].as_array() else {
            return Ok(base_message_id.to_string());
        };
        let prefix = format!("{task_id}-msg-");
        let mut max_suffix = 0u64;
        let mut pending_id = None;
        let mut delivered_id = None;
        for message in messages {
            let Some(id) = message["id"].as_str() else {
                continue;
            };
            if let Some(suffix) = id
                .strip_prefix(&prefix)
                .and_then(|suffix| suffix.parse::<u64>().ok())
            {
                max_suffix = max_suffix.max(suffix);
            }
            if message["task_id"].as_str() != Some(task_id) {
                continue;
            }
            if !message_target_matches_worker(message, worker_id) {
                continue;
            }
            if message["body"].as_str() != Some(expected_body) {
                continue;
            }
            if message["delivered"].as_bool().unwrap_or(false) {
                delivered_id = Some(id.to_string());
            } else {
                pending_id = Some(id.to_string());
            }
        }
        if let Some(pending_id) = pending_id {
            return Ok(pending_id);
        }
        if let Some(delivered_id) = delivered_id {
            if !(self.submit && worker_launched_for_assignment) {
                return Ok(delivered_id);
            }
        }
        if max_suffix > 0 {
            let next_suffix = max_suffix.checked_add(1).ok_or_else(|| {
                DispatchError::Conflict(format!(
                    "task strategy message id suffix exhausted for task {task_id}"
                ))
            })?;
            return Ok(format!("{prefix}{next_suffix}"));
        }
        Ok(base_message_id.to_string())
    }

    async fn team_message_delivered(
        &self,
        state: &SocketAppState,
        message_id: &str,
    ) -> Result<bool, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        Ok(team["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|message| message["id"].as_str() == Some(message_id))
            .and_then(|message| message["delivered"].as_bool())
            .unwrap_or(false))
    }

    async fn team_worker_has_compatible_live_surface(
        &self,
        state: &SocketAppState,
        assignment: &HarnessAssignment,
        task_id: &str,
        worker_id: &str,
    ) -> Result<bool, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        let Some(worker) = team["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|worker| worker["id"].as_str() == Some(worker_id))
        else {
            return Ok(false);
        };
        let Some(surface_id) = worker["surface_id"].as_str() else {
            return Ok(false);
        };
        if !worker_surface_is_live(state, surface_id)? {
            return Ok(false);
        }

        let expected_role = role_id(&assignment.role);
        let expected_worktree = self.worktree_name.as_deref();
        let expected_cwd = self
            .target_cwd
            .as_deref()
            .or(self.effective_target_cwd.as_deref());
        let mut mismatches = Vec::new();
        if worker["agent"].as_str() != Some(assignment.harness_id.as_str()) {
            mismatches.push(format!(
                "agent expected {} got {}",
                assignment.harness_id,
                worker["agent"].as_str().unwrap_or("<none>")
            ));
        }
        if worker["role"].as_str() != Some(expected_role) {
            mismatches.push(format!(
                "role expected {expected_role} got {}",
                worker["role"].as_str().unwrap_or("<none>")
            ));
        }
        if worker["assigned_task_id"].as_str() != Some(task_id) {
            mismatches.push(format!(
                "assigned_task_id expected {task_id} got {}",
                worker["assigned_task_id"].as_str().unwrap_or("<none>")
            ));
        }
        if worker["worktree_name"].as_str() != expected_worktree {
            mismatches.push(format!(
                "worktree_name expected {} got {}",
                expected_worktree.unwrap_or("<none>"),
                worker["worktree_name"].as_str().unwrap_or("<none>")
            ));
        }
        if worker["cwd"].as_str().map(Path::new) != self.target_cwd.as_deref() {
            mismatches.push(format!(
                "launch cwd expected {} got {}",
                self.target_cwd
                    .as_deref()
                    .map(|cwd| cwd.display().to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                worker["cwd"].as_str().unwrap_or("<none>")
            ));
        }
        if let (Some(expected_cwd), Some(surface_id)) =
            (expected_cwd, worker["surface_id"].as_str())
        {
            let surfaces = state.terminal.surfaces().map_err(DispatchError::from)?;
            match surfaces
                .iter()
                .find(|surface| surface.surface_id == surface_id)
            {
                Some(surface) if surface.cwd == expected_cwd => {}
                Some(surface) => mismatches.push(format!(
                    "cwd expected {} got {}",
                    expected_cwd.display(),
                    surface.cwd.display()
                )),
                None => mismatches.push(format!(
                    "cwd expected {} got <unknown>",
                    expected_cwd.display()
                )),
            }
        }
        let worker_status = worker["status"].as_str().unwrap_or("<none>");
        if !matches!(worker_status, "running" | "busy" | "active" | "working") {
            mismatches.push(format!("status {worker_status} is not reusable"));
        }
        if !mismatches.is_empty() {
            return Err(DispatchError::Conflict(format!(
                "task.strategy.apply cannot reuse live worker {worker_id} for a different assignment: {}",
                mismatches.join("; ")
            )));
        }

        Ok(true)
    }
}

fn assignment_title(assignment: &HarnessAssignment) -> String {
    format!(
        "{} via {}",
        role_label(&assignment.role),
        assignment.harness_id
    )
}

#[derive(Clone, Copy)]
struct AssignmentTargetContext<'a> {
    worktree_name: Option<&'a str>,
    cwd: Option<&'a Path>,
}

fn assignment_detail(
    goal: &str,
    strategy: &TaskStrategy,
    index: usize,
    assignment: &HarnessAssignment,
    target: AssignmentTargetContext<'_>,
) -> String {
    let lane = assignment_lane_lines(strategy, index, assignment);
    let target = assignment_target_context_lines(target);
    format!(
        "Goal: {goal}\nRole: {}\nHarness: {}\nReason: {}{lane}{target}\nScope: managed by task.strategy.apply; do not subdelegate.",
        role_id(&assignment.role),
        assignment.harness_id,
        assignment.reason
    )
}

fn assignment_prompt(
    goal: &str,
    strategy: &TaskStrategy,
    index: usize,
    assignment: &HarnessAssignment,
    target: AssignmentTargetContext<'_>,
) -> String {
    let lane = assignment_lane_lines(strategy, index, assignment);
    let target = assignment_target_context_lines(target);
    format!(
        "ForkTTY task assignment.\nGoal: {goal}\nRole: {}\nHarness: {}\nReason: {}{lane}{target}\nThe leader already applied this task strategy and launched this worker pane; do not call task.strategy.apply, team.worker.launch, worktree_create, or launch nested workers unless the leader sends a separate explicit instruction.\nStay within this role, do not subdelegate, and report verification evidence before completion.",
        role_id(&assignment.role),
        assignment.harness_id,
        assignment.reason
    )
}

fn assignment_lane_lines(
    strategy: &TaskStrategy,
    index: usize,
    assignment: &HarnessAssignment,
) -> String {
    if !matches!(
        strategy,
        TaskStrategy::ParallelResearch | TaskStrategy::ParallelExperiment
    ) {
        return String::new();
    }
    let lane = match assignment.role {
        HarnessRole::Researcher => parallel_research_lane(index),
        HarnessRole::Synthesizer => {
            "synthesis of prior lanes; wait for available researcher evidence before concluding"
        }
        HarnessRole::Reviewer => "independent review of researcher findings and missed risks",
        HarnessRole::Verifier => "verification lane for reproduced behavior and test evidence",
        HarnessRole::Implementer => "prototype or implementation lane for the assigned approach",
    };
    format!("\nLane: {lane}")
}

fn parallel_research_lane(index: usize) -> &'static str {
    match index {
        0 => "correctness and regressions",
        1 => "tests, edge cases, and missing coverage",
        2 => "security, concurrency, and robustness",
        _ => "integration, documentation, and product fit",
    }
}

fn assignment_target_context_lines(target: AssignmentTargetContext<'_>) -> String {
    let mut lines = String::new();
    if let Some(worktree_name) = target.worktree_name {
        lines.push_str(&format!("\nWorktree: {worktree_name}"));
    }
    if let Some(cwd) = target.cwd {
        lines.push_str(&format!("\nRepository cwd: {}", cwd.display()));
    }
    lines
}

fn message_target_matches_worker(message: &Value, worker_id: Option<&str>) -> bool {
    match (message["to_worker_id"].as_str(), worker_id) {
        (Some(target), Some(worker_id)) => target == worker_id,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn role_label(role: &HarnessRole) -> &'static str {
    match role {
        HarnessRole::Implementer => "Implement",
        HarnessRole::Reviewer => "Review",
        HarnessRole::Researcher => "Research",
        HarnessRole::Verifier => "Verify",
        HarnessRole::Synthesizer => "Synthesize",
    }
}

fn role_id(role: &HarnessRole) -> &'static str {
    match role {
        HarnessRole::Implementer => "implementer",
        HarnessRole::Reviewer => "reviewer",
        HarnessRole::Researcher => "researcher",
        HarnessRole::Verifier => "verifier",
        HarnessRole::Synthesizer => "synthesizer",
    }
}

fn approval_id(approval: &TaskStrategyApproval) -> &'static str {
    match approval {
        TaskStrategyApproval::StartRun => "start_run",
        TaskStrategyApproval::CreateWorktree => "create_worktree",
        TaskStrategyApproval::LaunchParallelWorkers => "launch_parallel_workers",
        TaskStrategyApproval::IncreaseRisk => "increase_risk",
    }
}

fn task_strategy_wire_value(strategy: &TaskStrategy) -> &'static str {
    match strategy {
        TaskStrategy::Solo => "solo",
        TaskStrategy::SoloTracked => "solo_tracked",
        TaskStrategy::SoloWithVerifyLoop => "solo_with_verify_loop",
        TaskStrategy::ImplementerPlusReviewer => "implementer_plus_reviewer",
        TaskStrategy::ParallelResearch => "parallel_research",
        TaskStrategy::ParallelExperiment => "parallel_experiment",
        TaskStrategy::TeamPipeline => "team_pipeline",
        TaskStrategy::ReviewOnly => "review_only",
    }
}

fn task_strategy_approval_order(approval: &TaskStrategyApproval) -> u8 {
    match approval {
        TaskStrategyApproval::StartRun => 0,
        TaskStrategyApproval::CreateWorktree => 1,
        TaskStrategyApproval::LaunchParallelWorkers => 2,
        TaskStrategyApproval::IncreaseRisk => 3,
    }
}

fn requires_parallel_workers_approval(assignment_count: usize) -> bool {
    assignment_count > 1
}

fn validate_task_strategy_id(field: &'static str, value: &str) -> Result<(), DispatchError> {
    if value.len() > 120
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {field}: expected an id containing only ASCII letters, digits, '-', '_', '.', or ':'"
        )));
    }
    Ok(())
}

fn feed_hash(hash: &mut u64, value: &str) {
    for byte in value.as_bytes() {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
    *hash ^= 0;
    *hash = hash.wrapping_mul(0x100000001b3);
}
