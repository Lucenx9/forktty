use crate::agent_params::{
    AgentHibernateRequest, AgentReclaimPlanRequest, AgentReclaimRequest, AgentResumeRequest,
    AgentWorkspaceRequest,
};
use crate::{
    agent_health_rows, agent_reclaim_plan, agent_session_rows, current_unix_epoch_ms,
    effective_agent_resume_cwd, hibernate_agent_surface, rollback_surface_creation, DispatchError,
    SocketAppState,
};
use forktty_terminal::SpawnRequest;
use serde_json::{json, Value};

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

pub(crate) fn resume(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let request = AgentResumeRequest::decode(params)?;
    let source_surface_id = request.source_surface_id;
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
