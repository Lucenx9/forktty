use crate::{
    current_unix_epoch_ms, optional_bool_param, optional_non_blank_string_param,
    optional_string_array_param, optional_surface_id_param, required_trimmed_string, store_access,
    surface_effective_project_cwd, system_runtime, task_strategy_params::task_strategy_plan_params,
    team_runtime, workflow_runtime, workspace_effective_project_cwd,
    workspace_selector_from_params, DispatchError, SocketAppState,
};
use forktty_core::{
    plan_task_strategy, validate_worktree_name, worktree, FeedApprovalState, FeedEntry,
    FeedEntryType, HarnessAssignment, HarnessCapability, HarnessHealth, HarnessRegistry,
    HarnessRole, HarnessRoutingSignals, TaskRouterProfile, TaskStrategy, TaskStrategyApproval,
    TaskStrategyInput, TaskStrategyLastKnownGood, TaskStrategyPlan, WorkflowQuery, WorkflowState,
    WorkspaceSelector,
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

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
    let explicit_last_known_good = plan_params
        .last_known_good
        .as_ref()
        .map(last_known_good_from_value)
        .transpose()?;
    let target = task_strategy_target_context(state, params)?;
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
        && infer_likely_user_visible_change(&plan_params.goal);
    let likely_user_visible_change = plan_params
        .likely_user_visible_change
        .unwrap_or(inferred_user_visible_change);
    let mut plan = plan_task_strategy(TaskStrategyInput {
        goal: plan_params.goal,
        explicit_mode,
        router_profile,
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
    if let Some(reason) = inferred_last_known_good_reason {
        plan.reasons.push(format!(
            "inferred last-known-good routing evidence from {reason}"
        ));
    }

    serde_json::to_value(plan)
        .map_err(|err| DispatchError::Other(format!("serialize task strategy plan: {err}")))
}

fn infer_likely_user_visible_change(goal: &str) -> bool {
    let lower = goal.to_lowercase();
    let edit_prefixes = [
        "fix",
        "bug",
        "implement",
        "add",
        "build",
        "change",
        "updat",
        "modif",
        "edit",
        "refactor",
        "remove",
        "renam",
        "writ",
        "creat",
    ];
    if contains_token_prefix(&lower, &edit_prefixes) {
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
}

fn task_strategy_target_context(
    state: &SocketAppState,
    params: &Value,
) -> Result<TaskStrategyTargetContext, DispatchError> {
    let requested_surface_id = optional_surface_id_param(params)?
        .or(optional_non_blank_string_param(
            params,
            "leader_surface_id",
        )?)
        .map(str::to_string);
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
            cwd: Some(surface_effective_project_cwd(surface)),
            workspace_id: Some(surface.workspace_id.clone()),
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
            cwd: None,
            workspace_id: None,
        });
    };
    let Some(workspace) = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
    else {
        return Ok(TaskStrategyTargetContext {
            cwd: None,
            workspace_id: Some(workspace_id),
        });
    };
    let surfaces = model.list_surfaces(Some(&workspace_id));
    Ok(TaskStrategyTargetContext {
        cwd: Some(workspace_effective_project_cwd(&workspace, &surfaces)),
        workspace_id: Some(workspace_id),
    })
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
        .map(str::to_string)
}

pub(crate) async fn apply(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let mut request = TaskStrategyApplyRequest::decode(params)?;
    request.resolve_apply_target(state)?;
    request.enforce_server_detected_worktree_isolation(state, params)?;
    request.validate_before_mutation(state)?;
    let missing_approvals = request.missing_approvals();
    if !missing_approvals.is_empty() {
        if request.feed_approval_satisfies(state, &missing_approvals)? {
        } else if request.request_approval {
            return request.record_approval_request(state, &missing_approvals);
        } else {
            return Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply missing approval(s): {}",
                missing_approvals.join(", ")
            )));
        }
    }

    let run_status = if request.submit { "running" } else { "staged" };
    let team_status = if request.submit { "active" } else { "staged" };
    let mut actions = Vec::new();
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
            if request.submit {
                if request.team_worker_exists(state, &worker_id).await? {
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

            let message_id = format!("{task_id}-msg-1");
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

            if request.submit {
                if request.team_message_delivered(state, &message_id).await? {
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
        other => Err(DispatchError::InvalidParam(format!(
            "unsupported task strategy: {other}"
        ))),
    }
}

fn task_router_profile_from_str(value: &str) -> Result<TaskRouterProfile, DispatchError> {
    match value {
        "balanced" => Ok(TaskRouterProfile::Balanced),
        "fast" => Ok(TaskRouterProfile::Fast),
        "conservative" => Ok(TaskRouterProfile::Conservative),
        "parallel" => Ok(TaskRouterProfile::Parallel),
        "review_heavy" => Ok(TaskRouterProfile::ReviewHeavy),
        other => Err(DispatchError::InvalidParam(format!(
            "unsupported router profile: {other}"
        ))),
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
    let reason = optional_object_string(object, "reason", "last_known_good")?;
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

    HarnessCapability {
        id: id.to_string(),
        installed,
        authenticated: launchable,
        supports_prompt_launch: launchable
            && provider["team_worker_launch"].as_bool().unwrap_or(false),
        supports_resume: provider["safe_resume"].as_bool().unwrap_or(false),
        supports_hooks: true,
        supports_mcp: true,
        supports_plan_mode: false,
        supports_worktree_cwd: launchable
            && provider["team_worker_launch"].as_bool().unwrap_or(false),
        max_parallel_sessions: None,
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

struct TaskStrategyApplyRequest {
    run_id: String,
    workflow_id: String,
    team_id: String,
    workspace_id: Option<String>,
    workspace_name: Option<String>,
    worktree_name: Option<String>,
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
        let goal = required_trimmed_string(params, "goal")?.to_string();
        let plan_value = params
            .get("plan")
            .ok_or(DispatchError::MissingParam("plan"))?
            .clone();
        let plan = serde_json::from_value::<TaskStrategyPlan>(plan_value)
            .map_err(|err| DispatchError::InvalidParam(format!("Invalid parameter plan: {err}")))?;
        let approved = optional_string_array_param(params, "approved")?.unwrap_or_default();
        let submit = optional_bool_param(params, "submit")?.unwrap_or(false);
        let approval_id =
            optional_non_blank_string_param(params, "approval_id")?.map(str::to_string);
        if let Some(approval_id) = approval_id.as_deref() {
            validate_task_strategy_id("approval_id", approval_id)?;
        }
        let request_approval = optional_bool_param(params, "request_approval")?.unwrap_or(false);
        let leader_surface_id = optional_non_blank_string_param(params, "leader_surface_id")?
            .or(optional_non_blank_string_param(params, "surface_id")?)
            .map(str::to_string);
        let workspace_id =
            optional_non_blank_string_param(params, "workspace_id")?.map(str::to_string);
        let workspace_name =
            optional_non_blank_string_param(params, "workspace_name")?.map(str::to_string);
        let worktree_name = optional_non_blank_string_param(params, "worktree_name")?
            .map(validate_worktree_name)
            .transpose()
            .map_err(DispatchError::from)?
            .map(str::to_string);
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
            leader_surface_id,
            goal,
            plan,
            approved,
            approval_id,
            request_approval,
            submit,
        })
    }

    fn missing_approvals(&self) -> Vec<String> {
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
        required
            .into_iter()
            .filter(|approval| !self.approved.iter().any(|value| value == approval))
            .map(str::to_string)
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
        let target = match task_strategy_target_context(state, params) {
            Ok(target) => target,
            Err(err) if self.worktree_name.is_some() && err.code() == "not_found" => {
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        if infer_repo_dirty(target.cwd.as_deref()) && infer_likely_user_visible_change(&self.goal) {
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
        missing_approvals: &[String],
    ) -> Result<bool, DispatchError> {
        let Some(approval_id) = self.approval_id.as_deref() else {
            return Ok(false);
        };
        let expected_id = self.approval_request_id(missing_approvals);
        if approval_id != expected_id {
            return Err(DispatchError::PreconditionFailed(format!(
                "task.strategy.apply approval_id {approval_id} does not match required approval request {expected_id}"
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
        if self.submit && self.plan.layers.worktree && self.worktree_name.is_none() {
            return Err(DispatchError::InvalidParam(
                "task.strategy.apply submit=true with worktree layer requires worktree_name for an already-open ForkTTY worktree workspace"
                    .to_string(),
            ));
        }
        if self.submit {
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
                        "task.strategy.apply submit=true requires an open ForkTTY worktree workspace named {worktree_name}"
                    )));
                }
            }
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

    fn add_worktree_launch_param(&self, params: &mut Value) {
        if let Some(worktree_name) = self.worktree_name.as_deref() {
            params["worktree_name"] = json!(worktree_name);
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
                    "detail": assignment_detail(&self.goal, assignment),
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
        _index: usize,
        assignment: &HarnessAssignment,
        task_id: &str,
    ) -> Value {
        json!({
            "team_id": self.team_id,
            "task_id": task_id,
            "title": assignment_title(assignment),
            "status": "open",
            "detail": assignment_detail(&self.goal, assignment),
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
        self.add_worktree_launch_param(&mut params);
        params
    }

    fn team_message_params(
        &self,
        _index: usize,
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
            "body": assignment_prompt(&self.goal, assignment),
        });
        if let Some(worker_id) = worker_id {
            params["to_worker_id"] = json!(worker_id);
        }
        params
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

    async fn team_worker_exists(
        &self,
        state: &SocketAppState,
        worker_id: &str,
    ) -> Result<bool, DispatchError> {
        let team = team_runtime::get(state, &json!({"team_id": self.team_id})).await?;
        Ok(team["workers"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|worker| worker["id"].as_str() == Some(worker_id)))
    }
}

fn assignment_title(assignment: &HarnessAssignment) -> String {
    format!(
        "{} via {}",
        role_label(&assignment.role),
        assignment.harness_id
    )
}

fn assignment_detail(goal: &str, assignment: &HarnessAssignment) -> String {
    format!(
        "Goal: {goal}\nRole: {}\nHarness: {}\nReason: {}\nScope: managed by task.strategy.apply; do not subdelegate.",
        role_id(&assignment.role),
        assignment.harness_id,
        assignment.reason
    )
}

fn assignment_prompt(goal: &str, assignment: &HarnessAssignment) -> String {
    format!(
        "ForkTTY task assignment.\nGoal: {goal}\nRole: {}\nHarness: {}\nReason: {}\nStay within this role, do not subdelegate, and report verification evidence before completion.",
        role_id(&assignment.role),
        assignment.harness_id,
        assignment.reason
    )
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
