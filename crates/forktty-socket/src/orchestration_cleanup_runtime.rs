//! Conservative cleanup for stale team/workflow orchestration records.

use crate::{
    optional_bool_param, optional_non_blank_string_param, store_access, workflow_runtime,
    DispatchError, SocketAppState,
};
use forktty_core::{
    team_now_ms, workflow_now_ms, TeamMessagesSupersede, TeamStoreData, TeamTaskUpsert, TeamUpsert,
    TeamWorkerUpsert, WorkflowPlanStepInput, WorkflowStoreData, WorkflowUpsert,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) async fn cleanup(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let apply = optional_bool_param(params, "apply")?.unwrap_or(false);
    let dry_run = optional_bool_param(params, "dry_run")?.unwrap_or(!apply);
    if apply && dry_run {
        return Err(DispatchError::InvalidParam(
            "orchestration.cleanup: cannot combine apply=true with dry_run=true".to_string(),
        ));
    }
    if !apply && !dry_run {
        return Err(DispatchError::InvalidParam(
            "orchestration.cleanup: apply=true is required when dry_run=false".to_string(),
        ));
    }
    let mutate = apply;
    let workspace_id = optional_non_blank_string_param(params, "workspace_id")?.map(str::to_string);
    let surface_liveness = surface_liveness(state)?;

    let team_actions = match store_access::optional_team_store_access(state) {
        Some(access) if mutate => {
            let surface_liveness = surface_liveness.clone();
            let workspace_id = workspace_id.clone();
            access
                .update(move |store| {
                    let actions =
                        plan_team_cleanup(store, &surface_liveness, workspace_id.as_deref());
                    apply_team_cleanup(store, &actions)?;
                    Ok(actions
                        .into_iter()
                        .map(|action| {
                            let applied = action.applies_mutation();
                            action.into_json(applied)
                        })
                        .collect::<Vec<_>>())
                })
                .await?
        }
        Some(access) => {
            let store = access.load().await?;
            plan_team_cleanup(&store, &surface_liveness, workspace_id.as_deref())
                .into_iter()
                .map(|action| action.into_json(false))
                .collect()
        }
        None => Vec::new(),
    };

    let workflow_actions = match store_access::optional_workflow_store_access(state) {
        Some(access) if mutate => {
            let workspace_id = workspace_id.clone();
            access
                .update(move |store| {
                    let actions = plan_workflow_cleanup(store, workspace_id.as_deref());
                    apply_workflow_cleanup(store, &actions)?;
                    Ok(actions
                        .into_iter()
                        .map(|action| {
                            let applied = action.applies_mutation();
                            action.into_json(applied)
                        })
                        .collect::<Vec<_>>())
                })
                .await
                .map_err(workflow_runtime::error)?
        }
        Some(access) => {
            let store = access.load().await.map_err(workflow_runtime::error)?;
            plan_workflow_cleanup(&store, workspace_id.as_deref())
                .into_iter()
                .map(|action| action.into_json(false))
                .collect()
        }
        None => Vec::new(),
    };

    Ok(json!({
        "dryRun": !mutate,
        "applied": mutate,
        "workspaceId": workspace_id,
        "teamActions": team_actions,
        "workflowActions": workflow_actions,
    }))
}

#[derive(Clone)]
struct SurfaceLiveness {
    model_surface_ids: BTreeSet<String>,
    live_surface_ids: BTreeSet<String>,
}

fn surface_liveness(state: &SocketAppState) -> Result<SurfaceLiveness, DispatchError> {
    let terminal_surface_ids = state
        .terminal
        .surfaces()
        .map_err(DispatchError::from)?
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
    let model_surface_ids = model
        .list_surfaces(None)
        .into_iter()
        .map(|surface| surface.id)
        .collect::<BTreeSet<_>>();
    let live_surface_ids = model_surface_ids
        .intersection(&terminal_surface_ids)
        .cloned()
        .collect();
    Ok(SurfaceLiveness {
        model_surface_ids,
        live_surface_ids,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TeamCleanupAction {
    CloseWorker {
        team_id: String,
        worker_id: String,
        reason: &'static str,
    },
    CancelTask {
        team_id: String,
        task_id: String,
        reason: &'static str,
    },
    SupersedeMessages {
        team_id: String,
        message_ids: Vec<String>,
        reason: &'static str,
    },
    FinishTeam {
        team_id: String,
        reason: &'static str,
    },
    ManualReview {
        team_id: String,
        worker_id: String,
        surface_id: Option<String>,
        reason: &'static str,
    },
}

impl TeamCleanupAction {
    fn applies_mutation(&self) -> bool {
        !matches!(self, Self::ManualReview { .. })
    }

    fn into_json(self, applied: bool) -> Value {
        match self {
            Self::CloseWorker {
                team_id,
                worker_id,
                reason,
            } => json!({
                "kind": "team.worker.close_stale",
                "teamId": team_id,
                "workerId": worker_id,
                "reason": reason,
                "applied": applied,
            }),
            Self::CancelTask {
                team_id,
                task_id,
                reason,
            } => json!({
                "kind": "team.task.cancel_stale",
                "teamId": team_id,
                "taskId": task_id,
                "reason": reason,
                "applied": applied,
            }),
            Self::SupersedeMessages {
                team_id,
                message_ids,
                reason,
            } => json!({
                "kind": "team.messages.supersede_stale",
                "teamId": team_id,
                "messageIds": message_ids,
                "reason": reason,
                "applied": applied,
            }),
            Self::FinishTeam { team_id, reason } => json!({
                "kind": "team.finish_stale",
                "teamId": team_id,
                "reason": reason,
                "applied": applied,
            }),
            Self::ManualReview {
                team_id,
                worker_id,
                surface_id,
                reason,
            } => json!({
                "kind": "team.worker.manual_review",
                "teamId": team_id,
                "workerId": worker_id,
                "surfaceId": surface_id,
                "reason": reason,
                "applied": false,
            }),
        }
    }
}

fn plan_team_cleanup(
    store: &TeamStoreData,
    surface_liveness: &SurfaceLiveness,
    workspace_id: Option<&str>,
) -> Vec<TeamCleanupAction> {
    let mut actions = Vec::new();
    for team in store.teams.iter().filter(|team| {
        workspace_id.is_none_or(|workspace_id| team.workspace_id.as_deref() == Some(workspace_id))
    }) {
        let mut stale_workers = BTreeSet::new();
        let mut live_worker_blockers = false;
        for worker in &team.workers {
            if is_terminal_status(&worker.status) {
                continue;
            }
            match worker.surface_id.as_deref() {
                Some(surface_id) if surface_liveness.live_surface_ids.contains(surface_id) => {
                    live_worker_blockers = true;
                    actions.push(TeamCleanupAction::ManualReview {
                        team_id: team.id.clone(),
                        worker_id: worker.id.clone(),
                        surface_id: Some(surface_id.to_string()),
                        reason: "worker_surface_still_exists",
                    });
                }
                Some(surface_id) => {
                    stale_workers.insert(worker.id.clone());
                    let reason = if surface_liveness.model_surface_ids.contains(surface_id) {
                        "worker_surface_runtime_missing"
                    } else {
                        "worker_surface_missing"
                    };
                    actions.push(TeamCleanupAction::CloseWorker {
                        team_id: team.id.clone(),
                        worker_id: worker.id.clone(),
                        reason,
                    });
                }
                None => {
                    live_worker_blockers = true;
                    actions.push(TeamCleanupAction::ManualReview {
                        team_id: team.id.clone(),
                        worker_id: worker.id.clone(),
                        surface_id: None,
                        reason: "worker_surface_not_recorded",
                    });
                }
            }
        }

        let mut stale_tasks = BTreeSet::new();
        for task in &team.tasks {
            if is_terminal_status(&task.status) {
                continue;
            }
            if task
                .assigned_worker_id
                .as_ref()
                .is_some_and(|worker_id| stale_workers.contains(worker_id))
            {
                stale_tasks.insert(task.id.clone());
                actions.push(TeamCleanupAction::CancelTask {
                    team_id: team.id.clone(),
                    task_id: task.id.clone(),
                    reason: "assigned_worker_surface_missing",
                });
            }
        }

        let superseded_message_ids = team
            .messages
            .iter()
            .filter(|message| !message.delivered && !message.superseded)
            .filter(|message| {
                message
                    .to_worker_id
                    .as_ref()
                    .is_some_and(|worker_id| stale_workers.contains(worker_id))
                    || message
                        .task_id
                        .as_ref()
                        .is_some_and(|task_id| stale_tasks.contains(task_id))
            })
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        if !superseded_message_ids.is_empty() {
            actions.push(TeamCleanupAction::SupersedeMessages {
                team_id: team.id.clone(),
                message_ids: superseded_message_ids.clone(),
                reason: "message_target_surface_missing",
            });
        }

        let open_tasks_after_cleanup = team
            .tasks
            .iter()
            .any(|task| !is_terminal_status(&task.status) && !stale_tasks.contains(&task.id));
        let pending_messages_after_cleanup = team.messages.iter().any(|message| {
            !message.delivered
                && !message.superseded
                && !superseded_message_ids.iter().any(|id| id == &message.id)
        });
        if !is_terminal_status(&team.status)
            && !live_worker_blockers
            && !open_tasks_after_cleanup
            && !pending_messages_after_cleanup
            && (!stale_workers.is_empty() || !team.tasks.is_empty() || !team.messages.is_empty())
        {
            actions.push(TeamCleanupAction::FinishTeam {
                team_id: team.id.clone(),
                reason: "no_live_workers_open_tasks_or_pending_messages",
            });
        }
    }
    actions
}

fn apply_team_cleanup(
    store: &mut TeamStoreData,
    actions: &[TeamCleanupAction],
) -> Result<(), forktty_core::TeamError> {
    let now = team_now_ms();
    for action in actions {
        match action {
            TeamCleanupAction::CloseWorker {
                team_id, worker_id, ..
            } => {
                store.upsert_worker(
                    TeamWorkerUpsert {
                        team_id: team_id.clone(),
                        worker_id: worker_id.clone(),
                        role: None,
                        agent: None,
                        surface_id: None,
                        worktree_name: None,
                        report: None,
                        status: Some("closed".to_string()),
                        assigned_task_id: None,
                    },
                    now,
                )?;
            }
            TeamCleanupAction::CancelTask {
                team_id, task_id, ..
            } => {
                store.upsert_task(
                    TeamTaskUpsert {
                        team_id: team_id.clone(),
                        task_id: task_id.clone(),
                        title: None,
                        status: Some("cancelled".to_string()),
                        detail: None,
                        depends_on: None,
                        assigned_worker_id: None,
                    },
                    now,
                )?;
            }
            TeamCleanupAction::SupersedeMessages {
                team_id,
                message_ids,
                ..
            } => {
                store.supersede_messages(
                    TeamMessagesSupersede {
                        team_id: team_id.clone(),
                        message_ids: message_ids.clone(),
                    },
                    now,
                )?;
            }
            TeamCleanupAction::FinishTeam { team_id, .. } => {
                store.upsert_team(
                    TeamUpsert {
                        team_id: team_id.clone(),
                        workspace_id: None,
                        leader_surface_id: None,
                        name: None,
                        status: Some("done".to_string()),
                        goal: None,
                    },
                    now,
                )?;
            }
            TeamCleanupAction::ManualReview { .. } => {}
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WorkflowCleanupAction {
    FinishWorkflow {
        workflow_id: String,
        reason: &'static str,
    },
    ClosePlanSteps {
        workflow_id: String,
        status: String,
        step_ids: Vec<String>,
        reason: &'static str,
    },
}

impl WorkflowCleanupAction {
    fn applies_mutation(&self) -> bool {
        true
    }

    fn into_json(self, applied: bool) -> Value {
        match self {
            Self::FinishWorkflow {
                workflow_id,
                reason,
            } => json!({
                "kind": "workflow.finish_stale",
                "workflowId": workflow_id,
                "reason": reason,
                "applied": applied,
            }),
            Self::ClosePlanSteps {
                workflow_id,
                status,
                step_ids,
                reason,
            } => json!({
                "kind": "workflow.plan.close_stale_steps",
                "workflowId": workflow_id,
                "status": status,
                "stepIds": step_ids,
                "reason": reason,
                "applied": applied,
            }),
        }
    }
}

fn plan_workflow_cleanup(
    store: &WorkflowStoreData,
    workspace_id: Option<&str>,
) -> Vec<WorkflowCleanupAction> {
    let mut actions = Vec::new();
    for workflow in store.workflows.iter().filter(|workflow| {
        workspace_id
            .is_none_or(|workspace_id| workflow.workspace_id.as_deref() == Some(workspace_id))
    }) {
        let open_steps = workflow
            .plan
            .iter()
            .filter(|step| !is_terminal_status(&step.status))
            .map(|step| step.id.clone())
            .collect::<Vec<_>>();
        if is_terminal_status(&workflow.status) && !open_steps.is_empty() {
            let status = terminal_plan_status_for_workflow(&workflow.status).to_string();
            actions.push(WorkflowCleanupAction::ClosePlanSteps {
                workflow_id: workflow.id.clone(),
                status,
                step_ids: open_steps,
                reason: "terminal_workflow_has_open_plan_steps",
            });
        } else if !is_terminal_status(&workflow.status)
            && !workflow.plan.is_empty()
            && open_steps.is_empty()
        {
            actions.push(WorkflowCleanupAction::FinishWorkflow {
                workflow_id: workflow.id.clone(),
                reason: "all_plan_steps_terminal",
            });
        }
    }
    actions
}

fn apply_workflow_cleanup(
    store: &mut WorkflowStoreData,
    actions: &[WorkflowCleanupAction],
) -> Result<(), forktty_core::WorkflowError> {
    let now = workflow_now_ms();
    for action in actions {
        match action {
            WorkflowCleanupAction::FinishWorkflow { workflow_id, .. } => {
                store.upsert(
                    WorkflowUpsert {
                        workflow_id: Some(workflow_id.clone()),
                        status: Some("done".to_string()),
                        ..WorkflowUpsert::default()
                    },
                    now,
                )?;
            }
            WorkflowCleanupAction::ClosePlanSteps {
                workflow_id,
                status,
                step_ids,
                ..
            } => {
                let Some(workflow) = store.get(workflow_id) else {
                    continue;
                };
                let steps = workflow
                    .plan
                    .iter()
                    .map(|step| WorkflowPlanStepInput {
                        id: step.id.clone(),
                        title: step.title.clone(),
                        status: if step_ids.iter().any(|id| id == &step.id) {
                            status.clone()
                        } else {
                            step.status.clone()
                        },
                        detail: step.detail.clone(),
                    })
                    .collect::<Vec<_>>();
                store.set_plan(workflow_id, steps, now)?;
            }
        }
    }
    Ok(())
}

fn terminal_plan_status_for_workflow(status: &str) -> &'static str {
    match status.trim().to_ascii_lowercase().as_str() {
        "cancelled" | "canceled" => "cancelled",
        "failed" => "failed",
        _ => "done",
    }
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "done"
            | "closed"
            | "cancelled"
            | "canceled"
            | "failed"
            | "finished"
            | "skipped"
            | "superseded"
    )
}
