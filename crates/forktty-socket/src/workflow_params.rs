use forktty_core::{
    WorkflowEvidenceInput, WorkflowLoopStateInput, WorkflowPlanStepInput, WorkflowQuery,
    WorkflowReplayQuery, WorkflowUpsert,
};
use serde_json::Value;

use crate::{
    optional_limit_param, optional_non_blank_string_param, optional_u64_param,
    optional_workflow_id, required_workflow_id, workflow_evidence_input, workflow_loop_state_input,
    workflow_plan_steps, workflow_target_ids, DispatchError, SocketAppState,
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
