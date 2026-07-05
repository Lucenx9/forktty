use crate::agent_params::{
    AgentHibernateRequest, AgentReclaimPlanRequest, AgentReclaimRequest, AgentResumeRequest,
    AgentWorkspaceRequest,
};
use crate::{
    agent_kind_from_status_key, close_terminal_surface_if_present, current_unix_epoch_ms,
    env_var_os, rollback_surface_creation, surface_effective_project_cwd, DispatchError,
    SocketAppState,
};
use forktty_core::{
    agent_resume_command_with_cwd_and_permission_mode, codex_session_cwd, normalize_agent_status,
    AgentKind, AgentResumeError, AgentSession, AgentSessionLifecycle, AgentStatus, StatusEntry,
    SurfaceKind, WorkspaceModel,
};
use forktty_terminal::{spawn::resolve_child_program, SpawnRequest};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(crate) fn health(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = AgentWorkspaceRequest::decode(&model, params)?;
    Ok(json!(agent_health_rows(
        &model,
        request.workspace_id.as_deref(),
        current_unix_epoch_ms(),
    )))
}

pub(crate) fn list(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = AgentWorkspaceRequest::decode(&model, params)?;
    Ok(json!(agent_session_rows(
        &model,
        request.workspace_id.as_deref(),
        current_unix_epoch_ms(),
    )))
}

pub(crate) fn reclaim_plan(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let request = AgentReclaimPlanRequest::decode(&model, params)?;
    Ok(agent_reclaim_plan(
        &model,
        request.workspace_id.as_deref(),
        request.min_idle_ms,
    ))
}

pub(crate) fn hibernate(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = AgentHibernateRequest::decode(params)?;
    hibernate_agent_surface(state, &request.surface_id, request.min_idle_ms)
}

pub(crate) fn reclaim(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let (plan, request) = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let request = AgentReclaimRequest::decode(&model, params)?;
        (
            agent_reclaim_plan(&model, request.workspace_id.as_deref(), request.min_idle_ms),
            request,
        )
    };

    let candidate_ids = plan
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("surface_id").and_then(Value::as_str))
        .take(request.limit)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let mut hibernated = Vec::new();
    let mut failed = Vec::new();
    for surface_id in candidate_ids {
        match hibernate_agent_surface(state, &surface_id, request.min_idle_ms) {
            Ok(row) => hibernated.push(row),
            Err(err) => failed.push(json!({
                "surface_id": surface_id,
                "code": err.code(),
                "error": err.to_string(),
            })),
        }
    }

    Ok(json!({
        "policy": plan.get("policy").cloned().unwrap_or(Value::Null),
        "hibernated": hibernated,
        "protected": plan.get("protected").cloned().unwrap_or_else(|| json!([])),
        "failed": failed,
    }))
}

pub(crate) async fn resume(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = AgentResumeRequest::decode(params)?;
    let source_surface_id = request.source_surface_id;
    let _surface_set_guard = state.coordinator.surface_set.lock().await;
    let (surface, agent, session_id, program, args, resume_cwd) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
        let source = model
            .surface(&source_surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?
            .clone();
        let agent_session = source.agent_session.ok_or_else(|| {
            DispatchError::PreconditionFailed(
                "Surface has no persisted agent session to resume".to_string(),
            )
        })?;
        let resume_cwd = effective_agent_resume_cwd(&agent_session);
        let command = forktty_core::agent_resume_command_with_cwd_and_permission_mode(
            agent_session.agent,
            &agent_session.session_id,
            resume_cwd.as_deref(),
            agent_session.permission_mode.as_deref(),
        )
        .map_err(|err| DispatchError::PreconditionFailed(err.to_string()))?;
        let new_surface = model
            .add_tab(&source_surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        model.set_surface_agent_session(
            &new_surface.id,
            agent_session.agent,
            agent_session.session_id.clone(),
        );
        if let Some(resume_cwd) = resume_cwd.clone() {
            model.set_surface_agent_session_resume_cwd(&new_surface.id, resume_cwd);
        }
        if let Some(permission_mode) = agent_session.permission_mode.as_deref() {
            model.set_surface_agent_session_permission_mode(&new_surface.id, permission_mode);
        }
        let surface = model
            .surface(&new_surface.id)
            .cloned()
            .unwrap_or(new_surface);
        (
            surface,
            agent_session.agent,
            agent_session.session_id,
            command.program,
            command.args,
            resume_cwd,
        )
    };

    let mut request =
        SpawnRequest::for_surface(&surface, program.clone(), state.socket_path.clone());
    if let Some(resume_cwd) = resume_cwd {
        request.cwd = resume_cwd;
    }
    let request = request.with_args(args.clone());
    if let Err(err) = state.terminal.spawn(request) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }

    let argv = std::iter::once(program.clone())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
    Ok(json!({
        "surface": surface,
        "agent": agent,
        "session_id": session_id,
        "argv": argv,
    }))
}

pub(crate) fn agent_session_identify_row(agent_session: &AgentSession) -> Value {
    json!({
        "agent": agent_session.agent,
        "session_id": agent_session.session_id,
        "resume_cwd": agent_session.resume_cwd,
        "permission_mode": agent_session.permission_mode,
        "lifecycle": agent_session.lifecycle,
        "last_activity_ms": agent_session.last_activity_ms,
    })
}

pub(crate) fn agent_session_rows(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    observed_at_ms: u64,
) -> Vec<Value> {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.id.clone(), workspace))
        .collect::<HashMap<_, _>>();

    model
        .list_surfaces(workspace_id)
        .into_iter()
        .filter_map(|surface| {
            let effective_project_cwd = surface_effective_project_cwd(&surface);
            let agent_session = surface.agent_session?;
            let workspace_name = workspaces
                .get(&surface.workspace_id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_default();
            let age_ms = agent_session_age_ms(agent_session.last_activity_ms, observed_at_ms);
            let statuses = model.list_status(&surface.workspace_id);
            let status = agent_primary_status(&statuses, agent_session.agent);
            let lifecycle_evidence =
                agent_lifecycle_evidence(&agent_session, status, observed_at_ms, None);
            Some(json!({
                "workspace_id": surface.workspace_id,
                "workspace_name": workspace_name,
                "surface_id": surface.id,
                "title": surface.title,
                "cwd": surface.cwd,
                "effective_project_cwd": effective_project_cwd,
                "agent": agent_session.agent,
                "session_id": agent_session.session_id,
                "resume_cwd": agent_session.resume_cwd,
                "permission_mode": agent_session.permission_mode,
                "lifecycle": agent_session.lifecycle,
                "last_activity_ms": agent_session.last_activity_ms,
                "observed_at_ms": observed_at_ms,
                "age_ms": age_ms,
                "source": "persisted_agent_session",
                "lifecycle_evidence": lifecycle_evidence,
            }))
        })
        .collect()
}

pub(crate) fn agent_health_rows(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    observed_at_ms: u64,
) -> Vec<Value> {
    let path = env_var_os("PATH");
    agent_health_rows_with_path(model, workspace_id, path.as_deref(), observed_at_ms)
}

pub(crate) fn agent_health_rows_with_path(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    path: Option<&OsStr>,
    observed_at_ms: u64,
) -> Vec<Value> {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.id.clone(), workspace))
        .collect::<HashMap<_, _>>();

    model
        .list_surfaces(workspace_id)
        .into_iter()
        .filter_map(|surface| {
            let effective_project_cwd = surface_effective_project_cwd(&surface);
            let agent_session = surface.agent_session?;
            let workspace_name = workspaces
                .get(&surface.workspace_id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_default();
            let resume_cwd = effective_agent_resume_cwd(&agent_session);
            let (ready, reason, program, executable, argv) = agent_resume_readiness(
                agent_session.agent,
                &agent_session.session_id,
                resume_cwd.as_deref(),
                agent_session.permission_mode.as_deref(),
                path,
            );
            let age_ms = agent_session_age_ms(agent_session.last_activity_ms, observed_at_ms);
            let statuses = model.list_status(&surface.workspace_id);
            let status = agent_primary_status(&statuses, agent_session.agent);
            let lifecycle_evidence = agent_lifecycle_evidence(
                &agent_session,
                status,
                observed_at_ms,
                Some((ready, reason)),
            );
            Some(json!({
                "workspace_id": surface.workspace_id,
                "workspace_name": workspace_name,
                "surface_id": surface.id,
                "title": surface.title,
                "cwd": surface.cwd,
                "effective_project_cwd": effective_project_cwd,
                "agent": agent_session.agent,
                "session_id": agent_session.session_id,
                "resume_cwd": resume_cwd,
                "permission_mode": agent_session.permission_mode,
                "lifecycle": agent_session.lifecycle,
                "last_activity_ms": agent_session.last_activity_ms,
                "observed_at_ms": observed_at_ms,
                "age_ms": age_ms,
                "source": "persisted_agent_session",
                "ready": ready,
                "reason": reason,
                "program": program,
                "executable": executable,
                "argv": argv,
                "lifecycle_evidence": lifecycle_evidence,
            }))
        })
        .collect()
}

fn agent_primary_status(statuses: &[StatusEntry], agent: AgentKind) -> Option<&StatusEntry> {
    statuses
        .iter()
        .find(|status| agent_kind_from_status_key(&status.key) == Some(agent))
}

fn agent_lifecycle_evidence(
    agent_session: &AgentSession,
    status: Option<&StatusEntry>,
    observed_at_ms: u64,
    readiness: Option<(bool, &str)>,
) -> Value {
    let age_ms = agent_session_age_ms(agent_session.last_activity_ms, observed_at_ms);
    let mut evidence = json!({
        "source": "persisted_agent_session",
        "lifecycle": agent_session.lifecycle,
        "last_activity_ms": agent_session.last_activity_ms,
        "observed_at_ms": observed_at_ms,
        "age_ms": age_ms,
    });
    if let Some(object) = evidence.as_object_mut() {
        object.insert(
            "status_key".to_string(),
            status
                .map(|status| Value::String(status.key.clone()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "status_value".to_string(),
            status
                .map(|status| Value::String(status.value.clone()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "status_source".to_string(),
            status
                .map(|_| Value::String("model".to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "status_scope".to_string(),
            status
                .map(|_| Value::String("workspace_provider".to_string()))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "permission_mode".to_string(),
            agent_session
                .permission_mode
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        if let Some((ready, reason)) = readiness {
            object.insert("ready".to_string(), Value::Bool(ready));
            object.insert(
                "readiness_reason".to_string(),
                Value::String(reason.to_string()),
            );
        }
    }
    evidence
}

fn agent_session_age_ms(last_activity_ms: u64, observed_at_ms: u64) -> Value {
    if last_activity_ms == 0 {
        Value::Null
    } else {
        json!(observed_at_ms.saturating_sub(last_activity_ms))
    }
}

fn agent_reclaim_plan(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    min_idle_ms: u64,
) -> Value {
    let path = env_var_os("PATH");
    agent_reclaim_plan_with_path(
        model,
        workspace_id,
        path.as_deref(),
        current_unix_epoch_ms(),
        min_idle_ms,
    )
}

pub(crate) fn agent_reclaim_plan_with_path(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    path: Option<&OsStr>,
    now_ms: u64,
    min_idle_ms: u64,
) -> Value {
    let mut candidates = Vec::new();
    let mut protected = Vec::new();
    for mut row in agent_health_rows_with_path(model, workspace_id, path, now_ms) {
        let last_activity_ms = row
            .get("last_activity_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let idle_ms = (last_activity_ms > 0).then(|| now_ms.saturating_sub(last_activity_ms));
        if let Some(object) = row.as_object_mut() {
            object.insert(
                "idle_ms".to_string(),
                idle_ms.map(Value::from).unwrap_or(Value::Null),
            );
        }
        if let Some(reason) = agent_reclaim_protect_reason(&row, idle_ms, min_idle_ms) {
            if let Some(object) = row.as_object_mut() {
                object.insert("protect_reason".to_string(), Value::String(reason));
            }
            protected.push(row);
        } else {
            candidates.push(row);
        }
    }

    candidates.sort_by(|a, b| {
        b.get("idle_ms")
            .and_then(Value::as_u64)
            .cmp(&a.get("idle_ms").and_then(Value::as_u64))
            .then_with(|| {
                a.get("surface_id")
                    .and_then(Value::as_str)
                    .cmp(&b.get("surface_id").and_then(Value::as_str))
            })
    });

    json!({
        "policy": {
            "now_ms": now_ms,
            "min_idle_ms": min_idle_ms,
        },
        "candidates": candidates,
        "protected": protected,
    })
}

fn agent_reclaim_protect_reason(
    row: &Value,
    idle_ms: Option<u64>,
    min_idle_ms: u64,
) -> Option<String> {
    match row
        .get("lifecycle")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "idle" => {}
        "needs_input" => return Some("needs_input".to_string()),
        "running" => return Some("running".to_string()),
        "suspended" => return Some("suspended".to_string()),
        "ended" => return Some("ended".to_string()),
        _ => return Some("unknown_lifecycle".to_string()),
    }
    let Some(idle_ms) = idle_ms else {
        return Some("unknown_activity".to_string());
    };
    if !row.get("ready").and_then(Value::as_bool).unwrap_or(false) {
        let reason = row
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("not_ready");
        return Some(format!("not_ready:{reason}"));
    }
    if idle_ms < min_idle_ms {
        return Some("recent_activity".to_string());
    }
    None
}

fn hibernate_agent_surface(
    state: &SocketAppState,
    surface_id: &str,
    min_idle_ms: u64,
) -> Result<Value, DispatchError> {
    let now_ms = current_unix_epoch_ms();
    let path = env_var_os("PATH");
    let (surface, status, agent, session_id, resume_cwd, permission_mode, argv, idle_ms, rollback) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let surface = model
            .surface(surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        if !matches!(surface.kind, SurfaceKind::Terminal) {
            return Err(DispatchError::PreconditionFailed(
                "Only terminal agent surfaces can be hibernated".to_string(),
            ));
        }
        let agent_session = surface.agent_session.as_ref().ok_or_else(|| {
            DispatchError::PreconditionFailed(
                "Surface has no persisted agent session to hibernate".to_string(),
            )
        })?;
        match agent_session.lifecycle {
            AgentSessionLifecycle::Idle => {}
            AgentSessionLifecycle::Suspended => {
                return Err(DispatchError::PreconditionFailed(
                    "Agent session is already suspended".to_string(),
                ));
            }
            lifecycle => {
                return Err(DispatchError::PreconditionFailed(format!(
                    "Only idle agent sessions can be hibernated (current lifecycle: {lifecycle:?})"
                )));
            }
        }
        if agent_session.last_activity_ms == 0 {
            return Err(DispatchError::PreconditionFailed(
                "Agent session activity is unknown".to_string(),
            ));
        }
        let idle_ms = now_ms.saturating_sub(agent_session.last_activity_ms);
        if idle_ms < min_idle_ms {
            return Err(DispatchError::PreconditionFailed(format!(
                "Agent session idle age {idle_ms}ms is below the reclaim policy {min_idle_ms}ms"
            )));
        }
        let resume_cwd = effective_agent_resume_cwd(agent_session);
        let (ready, reason, _program, _executable, argv) = agent_resume_readiness(
            agent_session.agent,
            &agent_session.session_id,
            resume_cwd.as_deref(),
            agent_session.permission_mode.as_deref(),
            path.as_deref(),
        );
        if !ready {
            return Err(DispatchError::PreconditionFailed(format!(
                "Agent session cannot be resumed safely: {reason}"
            )));
        }
        let previous_last_activity_ms = agent_session.last_activity_ms;
        let previous_unread = surface.unread;
        let workspace_id = surface.workspace_id.clone();
        let status_key = surface_status_key(surface_id);
        let previous_status = model
            .list_status(&workspace_id)
            .into_iter()
            .find(|status| status.key == status_key);
        let agent = agent_session.agent;
        let session_id = agent_session.session_id.clone();
        let permission_mode = agent_session.permission_mode.clone();
        let cleared_agent_metadata = model
            .set_surface_agent_session_lifecycle_with_cleared_metadata(
                surface_id,
                AgentSessionLifecycle::Suspended,
            )
            .ok_or(DispatchError::NotFound("agent session".to_string()))?;
        model.set_surface_agent_session_last_activity_ms(surface_id, now_ms);
        let _ = model.mark_surface_unread(surface_id, false);
        let status = model.set_status(
            &workspace_id,
            status_key.clone(),
            "Agent",
            "Suspended",
            Some("yellow".to_string()),
        );
        let surface = model
            .surface(surface_id)
            .cloned()
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        let rollback = (
            workspace_id,
            status_key,
            previous_last_activity_ms,
            previous_unread,
            previous_status,
            cleared_agent_metadata,
        );
        (
            surface,
            status.ok_or(DispatchError::NotFound("workspace".to_string()))?,
            agent,
            session_id,
            resume_cwd,
            permission_mode,
            argv,
            idle_ms,
            rollback,
        )
    };
    if let Err(err) = close_terminal_surface_if_present(state, surface_id) {
        let (
            workspace_id,
            status_key,
            previous_last_activity_ms,
            previous_unread,
            previous_status,
            cleared_agent_metadata,
        ) = rollback;
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.set_surface_agent_session_lifecycle(surface_id, AgentSessionLifecycle::Idle);
        model.set_surface_agent_session_last_activity_ms(surface_id, previous_last_activity_ms);
        let _ = model.mark_surface_unread(surface_id, previous_unread);
        let _ = model.restore_agent_metadata(&workspace_id, cleared_agent_metadata);
        if let Some(previous_status) = previous_status {
            let _ = model.set_status(
                &workspace_id,
                previous_status.key,
                previous_status.label,
                previous_status.value,
                previous_status.color,
            );
        } else {
            let _ = model.clear_status(&workspace_id, Some(&status_key));
        }
        return Err(DispatchError::Other(err));
    }

    Ok(json!({
        "surface": surface,
        "agent": agent,
        "session_id": session_id,
        "resume_cwd": resume_cwd,
        "permission_mode": permission_mode,
        "lifecycle": "suspended",
        "idle_ms": idle_ms,
        "argv": argv,
        "status": status,
    }))
}

pub(crate) fn surface_status_key(surface_id: &str) -> String {
    format!("surface:{surface_id}:status")
}

pub(crate) fn agent_session_lifecycle_from_hook(
    status_value: &str,
    hook_event_name: Option<&str>,
) -> AgentSessionLifecycle {
    let hook_event = hook_event_name
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match hook_event.as_str() {
        "session-end" => AgentSessionLifecycle::Ended,
        "stop" | "subagent-stop" | "teammate-idle" => AgentSessionLifecycle::Idle,
        "permission-request" | "elicitation" | "ask-user-question" => {
            AgentSessionLifecycle::NeedsInput
        }
        "session-start"
        | "prompt-submit"
        | "prompt-expansion"
        | "setup"
        | "pre-tool"
        | "post-tool"
        | "post-tool-failure"
        | "post-tool-batch"
        | "before-tool-selection"
        | "before-model"
        | "after-model"
        | "permission-denied"
        | "permission-replied"
        | "pre-compact"
        | "post-compact"
        | "config-change"
        | "instructions-loaded"
        | "cwd-changed"
        | "file-changed"
        | "worktree-create"
        | "worktree-remove"
        | "stop-failure" => AgentSessionLifecycle::Running,
        _ => agent_session_lifecycle_from_status(status_value),
    }
}

fn agent_session_lifecycle_from_status(status_value: &str) -> AgentSessionLifecycle {
    match normalize_agent_status(status_value) {
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Cancelled | AgentStatus::Failed => {
            AgentSessionLifecycle::Idle
        }
        AgentStatus::NeedsInput | AgentStatus::PermissionRequest => {
            AgentSessionLifecycle::NeedsInput
        }
        AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::TestsRunning => {
            AgentSessionLifecycle::Running
        }
        AgentStatus::Unknown => AgentSessionLifecycle::Unknown,
    }
}

pub(crate) fn effective_agent_resume_cwd(agent_session: &AgentSession) -> Option<PathBuf> {
    agent_session.resume_cwd.clone().or_else(|| {
        (agent_session.agent == AgentKind::Codex)
            .then(|| codex_session_cwd(&agent_session.session_id))
            .flatten()
    })
}

pub(crate) fn agent_resume_readiness(
    agent: AgentKind,
    session_id: &str,
    resume_cwd: Option<&Path>,
    permission_mode: Option<&str>,
    path: Option<&OsStr>,
) -> (bool, &'static str, Value, Value, Vec<String>) {
    let command = match agent_resume_command_with_cwd_and_permission_mode(
        agent,
        session_id,
        resume_cwd,
        permission_mode,
    ) {
        Ok(command) => command,
        Err(AgentResumeError::UnsupportedAgent(_)) => {
            return (
                false,
                "unsupported_agent",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
        Err(AgentResumeError::InvalidSessionId) => {
            return (
                false,
                "invalid_session_id",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
        Err(AgentResumeError::InvalidResumeCwd) => {
            return (
                false,
                "invalid_resume_cwd",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
    };
    let argv = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect::<Vec<_>>();
    let executable = resolve_child_program(&command.program, path);
    let executable_value = executable
        .as_ref()
        .map(|path| Value::String(path.to_string_lossy().into_owned()))
        .unwrap_or(Value::Null);
    let ready = executable.is_some();
    (
        ready,
        if ready { "ready" } else { "program_not_found" },
        Value::String(command.program),
        executable_value,
        argv,
    )
}
