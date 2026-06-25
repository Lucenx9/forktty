use forktty_core::{
    WorkflowEvidenceInput, WorkflowLoopGateInput, WorkflowLoopStateInput, WorkflowPlanStepInput,
    WorkflowQuery, WorkflowReplayQuery, WorkflowUpsert,
};
use serde_json::Value;
use std::path::PathBuf;

use crate::{
    optional_limit_param, optional_non_blank_string_param, optional_surface_id_param,
    optional_u64_param, required_trimmed_string, workspace_selector_from_params, DispatchError,
    SocketAppState,
};

pub(crate) struct WorkflowListRequest {
    pub(crate) query: WorkflowQuery,
}

impl WorkflowListRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let (workspace_id, surface_id) = workflow_target_ids(state, params)?;
        Ok(Self {
            query: WorkflowQuery {
                workspace_id,
                surface_id,
                session_id: optional_non_blank_string_param(params, "session_id")?
                    .map(str::to_string),
                query: optional_non_blank_string_param(params, "query")?.map(str::to_string),
                limit: optional_limit_param(params, "limit")?,
            },
        })
    }
}

pub(crate) struct WorkflowGetRequest {
    pub(crate) workflow_id: String,
}

impl WorkflowGetRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workflow_id: required_workflow_id(params)?.to_string(),
        })
    }
}

pub(crate) struct WorkflowUpsertRequest {
    pub(crate) input: WorkflowUpsert,
}

impl WorkflowUpsertRequest {
    pub(crate) fn decode(state: &SocketAppState, params: &Value) -> Result<Self, DispatchError> {
        let (workspace_id, surface_id) = workflow_target_ids(state, params)?;
        Ok(Self {
            input: WorkflowUpsert {
                workflow_id: optional_workflow_id(params)?.map(str::to_string),
                workspace_id,
                surface_id,
                agent: optional_non_blank_string_param(params, "agent")?.map(str::to_string),
                session_id: optional_non_blank_string_param(params, "session_id")?
                    .map(str::to_string),
                mode: optional_non_blank_string_param(params, "mode")?.map(str::to_string),
                status: optional_non_blank_string_param(params, "status")?.map(str::to_string),
                goal: optional_non_blank_string_param(params, "goal")?.map(str::to_string),
                memory: optional_non_blank_string_param(params, "memory")?.map(str::to_string),
            },
        })
    }
}

pub(crate) struct WorkflowLoopSetRequest {
    pub(crate) workflow_id: String,
    pub(crate) input: WorkflowLoopStateInput,
}

impl WorkflowLoopSetRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workflow_id: required_workflow_id(params)?.to_string(),
            input: workflow_loop_state_input(params)?,
        })
    }
}

pub(crate) struct WorkflowPlanSetRequest {
    pub(crate) workflow_id: String,
    pub(crate) steps: Vec<WorkflowPlanStepInput>,
}

impl WorkflowPlanSetRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workflow_id: required_workflow_id(params)?.to_string(),
            steps: workflow_plan_steps(params)?,
        })
    }
}

pub(crate) struct WorkflowEvidenceAddRequest {
    pub(crate) workflow_id: String,
    pub(crate) evidence: WorkflowEvidenceInput,
}

impl WorkflowEvidenceAddRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            workflow_id: required_workflow_id(params)?.to_string(),
            evidence: workflow_evidence_input(params)?,
        })
    }
}

pub(crate) struct WorkflowReplayRequest {
    pub(crate) query: WorkflowReplayQuery,
}

impl WorkflowReplayRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        Ok(Self {
            query: WorkflowReplayQuery {
                workflow_id: optional_workflow_id(params)?.map(str::to_string),
                query: optional_non_blank_string_param(params, "query")?.map(str::to_string),
                since_seq: optional_u64_param(params, "since_seq")?,
                limit: optional_limit_param(params, "limit")?,
            },
        })
    }
}

fn workflow_target_ids(
    state: &SocketAppState,
    params: &Value,
) -> Result<(Option<String>, Option<String>), DispatchError> {
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let workspace_id = match workspace_selector_from_params(params) {
        Ok(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    let surface_workspace_id = match surface_id.as_deref() {
        Some(surface_id) => Some(
            model
                .surface(surface_id)
                .ok_or(DispatchError::NotFound("surface".to_string()))?
                .workspace_id
                .clone(),
        ),
        None => None,
    };
    if let (Some(workspace_id), Some(surface_workspace_id)) =
        (workspace_id.as_deref(), surface_workspace_id.as_deref())
    {
        if workspace_id != surface_workspace_id {
            return Err(DispatchError::InvalidParam(
                "surface_id does not belong to the selected workspace".to_string(),
            ));
        }
    }
    Ok((workspace_id.or(surface_workspace_id), surface_id))
}

fn workflow_plan_steps(params: &Value) -> Result<Vec<WorkflowPlanStepInput>, DispatchError> {
    let Some(value) = params.get("steps").or_else(|| params.get("plan")) else {
        return Err(DispatchError::MissingParam("steps"));
    };
    let Some(items) = value.as_array() else {
        return Err(DispatchError::InvalidParam(
            "Invalid parameter steps: expected array".to_string(),
        ));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let Some(object) = item.as_object() else {
                return Err(DispatchError::InvalidParam(format!(
                    "Invalid parameter steps[{index}]: expected object"
                )));
            };
            Ok(WorkflowPlanStepInput {
                id: required_object_string(object, "id", "steps")?.to_string(),
                title: required_object_string(object, "title", "steps")?.to_string(),
                status: required_object_string(object, "status", "steps")?.to_string(),
                detail: optional_object_string(object, "detail", "steps")?.map(str::to_string),
            })
        })
        .collect()
}

fn workflow_evidence_input(params: &Value) -> Result<WorkflowEvidenceInput, DispatchError> {
    let evidence_id = optional_non_blank_string_param(params, "evidence_id")?.map(str::to_string);
    let kind = required_trimmed_string(params, "kind")?.to_string();
    let title = required_trimmed_string(params, "title")?.to_string();
    let text = optional_non_empty_raw_string_param(params, "text")?.map(str::to_string);
    let path = optional_non_blank_string_param(params, "path")?.map(PathBuf::from);
    if text.is_none() && path.is_none() {
        return Err(DispatchError::MissingParam("text"));
    }
    Ok(WorkflowEvidenceInput {
        id: evidence_id,
        kind,
        title,
        text,
        path,
    })
}

fn workflow_loop_state_input(params: &Value) -> Result<WorkflowLoopStateInput, DispatchError> {
    Ok(WorkflowLoopStateInput {
        recipe: optional_non_blank_string_param(params, "recipe")?.map(str::to_string),
        stage: optional_non_blank_string_param(params, "stage")?.map(str::to_string),
        iteration: optional_u32_param(params, "iteration")?,
        max_iterations: optional_u32_param(params, "max_iterations")?,
        stop_reason: optional_non_blank_string_param(params, "stop_reason")?.map(str::to_string),
        gates: workflow_loop_gates(params)?,
    })
}

fn workflow_loop_gates(
    params: &Value,
) -> Result<Option<Vec<WorkflowLoopGateInput>>, DispatchError> {
    let Some(value) = params.get("gates").or_else(|| params.get("loop_gates")) else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(DispatchError::InvalidParam(
            "Invalid parameter gates: expected array".to_string(),
        ));
    };
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let Some(object) = item.as_object() else {
                return Err(DispatchError::InvalidParam(format!(
                    "Invalid parameter gates[{index}]: expected object"
                )));
            };
            Ok(WorkflowLoopGateInput {
                id: required_object_string(object, "id", "gates")?.to_string(),
                kind: required_object_string(object, "kind", "gates")?.to_string(),
                label: required_object_string(object, "label", "gates")?.to_string(),
                status: required_object_string(object, "status", "gates")?.to_string(),
                summary: optional_object_string(object, "summary", "gates")?.map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, DispatchError>>()
        .map(Some)
}

fn optional_u32_param(params: &Value, key: &'static str) -> Result<Option<u32>, DispatchError> {
    let Some(value) = optional_u64_param(params, key)? else {
        return Ok(None);
    };
    u32::try_from(value).map(Some).map_err(|_| {
        DispatchError::InvalidParam(format!(
            "Invalid parameter {key}: expected integer <= {}",
            u32::MAX
        ))
    })
}

fn required_workflow_id(params: &Value) -> Result<&str, DispatchError> {
    optional_workflow_id(params)?.ok_or(DispatchError::MissingParam("workflow_id"))
}

fn optional_non_empty_raw_string_param<'a>(
    params: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, DispatchError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match value.as_str() {
        Some(value) if !value.trim().is_empty() => Ok(Some(value)),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

fn optional_workflow_id(params: &Value) -> Result<Option<&str>, DispatchError> {
    optional_non_blank_string_param(params, "workflow_id")
}

fn required_object_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    parent: &str,
) -> Result<&'a str, DispatchError> {
    optional_object_string(object, key, parent)?.ok_or_else(|| {
        DispatchError::InvalidParam(format!("Invalid parameter {parent}: missing {key}"))
    })
}

fn optional_object_string<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
    parent: &str,
) -> Result<Option<&'a str>, DispatchError> {
    let Some(value) = object.get(key) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match value.as_str().map(str::trim) {
        Some(value) if !value.is_empty() => Ok(Some(value)),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {parent}.{key}: must not be empty"
        ))),
        None => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {parent}.{key}: expected string"
        ))),
    }
}
