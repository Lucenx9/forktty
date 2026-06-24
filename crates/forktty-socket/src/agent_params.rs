use serde_json::Value;

use crate::{
    optional_u64_param, required_surface_id, workspace_selector_from_params, DispatchError,
    DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS,
};
use forktty_core::WorkspaceModel;

pub(crate) struct AgentWorkspaceRequest {
    pub(crate) workspace_id: Option<String>,
}

impl AgentWorkspaceRequest {
    pub(crate) fn decode(model: &WorkspaceModel, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: optional_workspace_id(model, params)?,
        })
    }
}

pub(crate) struct AgentHibernateRequest {
    pub(crate) surface_id: String,
    pub(crate) min_idle_ms: u64,
}

impl AgentHibernateRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            surface_id: required_surface_id(params)?.to_string(),
            min_idle_ms: optional_u64_param(params, "min_idle_ms")?.unwrap_or(0),
        })
    }
}

pub(crate) struct AgentReclaimPlanRequest {
    pub(crate) workspace_id: Option<String>,
    pub(crate) min_idle_ms: u64,
}

impl AgentReclaimPlanRequest {
    pub(crate) fn decode(model: &WorkspaceModel, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workspace_id: optional_workspace_id(model, params)?,
            min_idle_ms: optional_u64_param(params, "min_idle_ms")?
                .unwrap_or(DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS),
        })
    }
}

pub(crate) struct AgentReclaimRequest {
    pub(crate) workspace_id: Option<String>,
    pub(crate) min_idle_ms: u64,
    pub(crate) limit: usize,
}

impl AgentReclaimRequest {
    pub(crate) fn decode(model: &WorkspaceModel, params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            min_idle_ms: optional_u64_param(params, "min_idle_ms")?
                .unwrap_or(DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS),
            limit: optional_u64_param(params, "limit")?.unwrap_or(200).min(200) as usize,
            workspace_id: optional_workspace_id(model, params)?,
        })
    }
}

pub(crate) struct AgentResumeRequest {
    pub(crate) source_surface_id: String,
}

impl AgentResumeRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            source_surface_id: required_surface_id(params)?.to_string(),
        })
    }
}

fn optional_workspace_id(
    model: &WorkspaceModel,
    params: &Value,
) -> Result<Option<String>, DispatchError> {
    match workspace_selector_from_params(params) {
        Ok(selector) => model
            .workspace_id_for(selector)
            .map(Some)
            .ok_or(DispatchError::NotFound("workspace".to_string())),
        Err(DispatchError::MissingParam(_)) => Ok(None),
        Err(err) => Err(err),
    }
}
