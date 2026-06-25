use std::path::{Path, PathBuf};

use forktty_core::{TeamError, TeamStoreData, WorkflowError, WorkflowStoreData};

use crate::{DispatchError, SocketAppState};

pub(crate) struct TeamStoreAccess {
    path: PathBuf,
}

impl TeamStoreAccess {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) async fn load(&self) -> Result<TeamStoreData, TeamError> {
        let path = self.path.clone();
        join_team_store_task(tokio::task::spawn_blocking(move || {
            forktty_core::load_teams_from_path(&path)
        }))
        .await
    }

    pub(crate) async fn update<F, T>(&self, update: F) -> Result<T, TeamError>
    where
        F: FnOnce(&mut TeamStoreData) -> Result<T, TeamError> + Send + 'static,
        T: Send + 'static,
    {
        let path = self.path.clone();
        join_team_store_task(tokio::task::spawn_blocking(move || {
            forktty_core::update_teams_at_path(&path, update)
        }))
        .await
    }
}

async fn join_team_store_task<T>(
    task: tokio::task::JoinHandle<Result<T, TeamError>>,
) -> Result<T, TeamError> {
    task.await
        .map_err(|err| TeamError::Invalid(format!("team store task failed: {err}")))?
}

pub(crate) fn optional_team_store_access(state: &SocketAppState) -> Option<TeamStoreAccess> {
    state
        .team_store_path
        .as_ref()
        .cloned()
        .map(TeamStoreAccess::new)
}

pub(crate) fn team_store_access(state: &SocketAppState) -> Result<TeamStoreAccess, DispatchError> {
    optional_team_store_access(state)
        .ok_or_else(|| DispatchError::Other("Team store is unavailable".to_string()))
}

pub(crate) struct WorkflowStoreAccess {
    path: PathBuf,
}

impl WorkflowStoreAccess {
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) async fn load(&self) -> Result<WorkflowStoreData, WorkflowError> {
        let path = self.path.clone();
        join_workflow_store_task(tokio::task::spawn_blocking(move || {
            forktty_core::load_workflows_from_path(&path)
        }))
        .await
    }

    pub(crate) async fn update<F, T>(&self, update: F) -> Result<T, WorkflowError>
    where
        F: FnOnce(&mut WorkflowStoreData) -> Result<T, WorkflowError> + Send + 'static,
        T: Send + 'static,
    {
        let path = self.path.clone();
        join_workflow_store_task(tokio::task::spawn_blocking(move || {
            forktty_core::update_workflows_at_path(&path, update)
        }))
        .await
    }
}

async fn join_workflow_store_task<T>(
    task: tokio::task::JoinHandle<Result<T, WorkflowError>>,
) -> Result<T, WorkflowError> {
    task.await
        .map_err(|err| WorkflowError::InvalidData(format!("workflow store task failed: {err}")))?
}

pub(crate) fn optional_workflow_store_access(
    state: &SocketAppState,
) -> Option<WorkflowStoreAccess> {
    state
        .workflow_store_path
        .as_ref()
        .cloned()
        .map(WorkflowStoreAccess::new)
}

pub(crate) fn workflow_store_access(
    state: &SocketAppState,
) -> Result<WorkflowStoreAccess, DispatchError> {
    optional_workflow_store_access(state).ok_or_else(|| {
        DispatchError::PreconditionFailed(
            "Workflow store path is unavailable on this system".to_string(),
        )
    })
}
