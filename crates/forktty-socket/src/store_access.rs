use std::path::Path;

use forktty_core::{TeamError, TeamStoreData, WorkflowError, WorkflowStoreData};

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
