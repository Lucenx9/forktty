use serde_json::Value;

use crate::{
    resolve_open_repo_cwd_param, worktree_name_from_params, DispatchError, SocketAppState,
};

pub(crate) struct WorktreeRepoRequest {
    pub(crate) cwd: String,
}

impl WorktreeRepoRequest {
    pub(crate) async fn decode_list(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        Ok(Self {
            cwd: resolve_open_repo_cwd_param(state, params, &["cwd"], "cwd").await?,
        })
    }
}

pub(crate) struct WorktreeStatusRequest {
    pub(crate) path: String,
}

impl WorktreeStatusRequest {
    pub(crate) async fn decode(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        Ok(Self {
            path: resolve_open_repo_cwd_param(state, params, &["path", "cwd"], "path or cwd")
                .await?,
        })
    }
}

pub(crate) struct WorktreeNamedRequest {
    pub(crate) name: String,
    pub(crate) cwd: String,
}

impl WorktreeNamedRequest {
    pub(crate) async fn decode_create_like(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        Self::decode(state, params, &["name"], "name", &["cwd"], "cwd").await
    }

    pub(crate) async fn decode_attach(
        state: &SocketAppState,
        params: &Value,
    ) -> Result<Self, DispatchError> {
        Self::decode(state, params, &["name", "branch"], "name", &["cwd"], "cwd").await
    }

    async fn decode(
        state: &SocketAppState,
        params: &Value,
        name_keys: &[&str],
        missing_name: &'static str,
        cwd_keys: &[&str],
        missing_cwd: &'static str,
    ) -> Result<Self, DispatchError> {
        let name = worktree_name_from_params(params, name_keys, missing_name)?;
        let cwd = resolve_open_repo_cwd_param(state, params, cwd_keys, missing_cwd).await?;
        Ok(Self {
            name: name.to_string(),
            cwd,
        })
    }
}
