use std::path::Path;

use forktty_core::{WorkflowError, WorkflowStoreData};

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
