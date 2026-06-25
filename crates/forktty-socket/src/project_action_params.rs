use serde_json::Value;

use crate::{
    ensure_max_text_size, path_resolver::resolve_open_repo_cwd_param, required_trimmed_string,
    DispatchError, SocketAppState,
};

pub(crate) struct ProjectActionListRequest {
    pub(crate) cwd: String,
}

impl ProjectActionListRequest {
    pub(crate) async fn decode(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        Ok(Self {
            cwd: resolve_open_repo_cwd_param(state, params, &["cwd"], "cwd").await?,
        })
    }
}

pub(crate) struct ProjectActionRunRequest {
    pub(crate) id: String,
    pub(crate) cwd: String,
}

impl ProjectActionRunRequest {
    pub(crate) async fn decode(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        let id = required_trimmed_string(params, "id")?;
        ensure_max_text_size("id", id)?;
        let cwd = resolve_open_repo_cwd_param(state, params, &["cwd"], "cwd").await?;
        Ok(Self {
            id: id.to_string(),
            cwd,
        })
    }
}
