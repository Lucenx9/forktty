use std::path::Path;

use forktty_core::{TeamError, TeamStoreData, WorkflowError, WorkflowStoreData};

use crate::{DispatchError, SocketAppState};

pub(crate) struct TeamStoreAccess<'a> {
    path: &'a Path,
}

impl<'a> TeamStoreAccess<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self { path }
    }

    pub(crate) fn path(&self) -> &'a Path {
        self.path
    }

    pub(crate) fn load(&self) -> Result<TeamStoreData, TeamError> {
        forktty_core::load_teams_from_path(self.path)
    }

    pub(crate) fn update<F, T>(&self, update: F) -> Result<T, TeamError>
    where
        F: FnOnce(&mut TeamStoreData) -> Result<T, TeamError>,
    {
        forktty_core::update_teams_at_path(self.path, update)
    }
}

pub(crate) fn optional_team_store_access(state: &SocketAppState) -> Option<TeamStoreAccess<'_>> {
    state.team_store_path.as_deref().map(TeamStoreAccess::new)
}

pub(crate) fn team_store_access(
    state: &SocketAppState,
) -> Result<TeamStoreAccess<'_>, DispatchError> {
    optional_team_store_access(state)
        .ok_or_else(|| DispatchError::Other("Team store is unavailable".to_string()))
}

pub(crate) struct WorkflowStoreAccess<'a> {
    path: &'a Path,
}

impl<'a> WorkflowStoreAccess<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self { path }
    }

    pub(crate) fn load(&self) -> Result<WorkflowStoreData, WorkflowError> {
        forktty_core::load_workflows_from_path(self.path)
    }

    pub(crate) fn update<F, T>(&self, update: F) -> Result<T, WorkflowError>
    where
        F: FnOnce(&mut WorkflowStoreData) -> Result<T, WorkflowError>,
    {
        forktty_core::update_workflows_at_path(self.path, update)
    }
}

pub(crate) fn optional_workflow_store_access(
    state: &SocketAppState,
) -> Option<WorkflowStoreAccess<'_>> {
    state
        .workflow_store_path
        .as_deref()
        .map(WorkflowStoreAccess::new)
}

pub(crate) fn workflow_store_access(
    state: &SocketAppState,
) -> Result<WorkflowStoreAccess<'_>, DispatchError> {
    optional_workflow_store_access(state).ok_or_else(|| {
        DispatchError::PreconditionFailed(
            "Workflow store path is unavailable on this system".to_string(),
        )
    })
}
