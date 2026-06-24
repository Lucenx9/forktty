use forktty_core::{FeedApprovalState, WorkspaceModel};
use serde_json::Value;

use crate::{
    ensure_max_text_size, optional_u64_param, required_trimmed_string,
    workspace_selector_from_params, DispatchError,
};

pub(crate) struct FeedApprovalRespondRequest {
    pub(crate) id: String,
    pub(crate) decision: FeedApprovalState,
}

impl FeedApprovalRespondRequest {
    pub(crate) fn decode(params: &Value) -> Result<Self, DispatchError> {
        let id = required_trimmed_string(params, "id")?;
        ensure_max_text_size("id", id)?;
        Ok(Self {
            id: id.to_string(),
            decision: feed_approval_decision_from_params(params)?,
        })
    }
}

pub(crate) struct FeedListRequest {
    pub(crate) workspace_id: Option<String>,
    pub(crate) limit: usize,
}

impl FeedListRequest {
    pub(crate) fn decode(params: &Value, model: &WorkspaceModel) -> Result<Self, DispatchError> {
        let workspace_id = match workspace_selector_from_params(params) {
            Ok(selector) => Some(
                model
                    .workspace_id_for(selector)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?,
            ),
            Err(DispatchError::MissingParam(_)) => None,
            Err(err) => return Err(err),
        };
        let limit = optional_u64_param(params, "limit")?.unwrap_or(50).min(200) as usize;
        Ok(Self {
            workspace_id,
            limit,
        })
    }
}

fn feed_approval_decision_from_params(params: &Value) -> Result<FeedApprovalState, DispatchError> {
    match required_trimmed_string(params, "decision")? {
        "approve" | "approved" => Ok(FeedApprovalState::Approved),
        "deny" | "denied" => Ok(FeedApprovalState::Denied),
        other => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter decision: expected approve or deny, got {other}"
        ))),
    }
}
