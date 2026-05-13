use crate::model::{PaneNode, Workspace};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Data directory not found")]
    NoDataDir,
    #[error("Unsupported session version: {0}")]
    UnsupportedVersion(u32),
    #[error("Invalid session data: {0}")]
    InvalidData(String),
}

pub const SESSION_FORMAT_VERSION: u32 = 2;
const MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;
const MAX_SESSION_SPLIT_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionData {
    #[serde(default = "default_session_version")]
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub active_workspace_id: Option<String>,
}

pub fn save_session(data: &SessionData) -> Result<(), SessionError> {
    save_session_to_path(&session_path()?, data)
}

pub fn load_session() -> Result<Option<SessionData>, SessionError> {
    load_session_from_path(&session_path()?)
}

pub fn save_session_to_path(path: &Path, data: &SessionData) -> Result<(), SessionError> {
    validate_session_data(data)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = path.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
    let mut tmp_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;
    tmp_file.write_all(json.as_bytes())?;
    tmp_file.sync_all()?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

pub fn load_session_from_path(path: &Path) -> Result<Option<SessionData>, SessionError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SESSION_SIZE_BYTES {
        quarantine_corrupt_session(path)?;
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    match serde_json::from_str(&content) {
        Ok(data) => match validate_session_data(&data) {
            Ok(()) => Ok(Some(data)),
            Err(_) => {
                quarantine_corrupt_session(path)?;
                Ok(None)
            }
        },
        Err(_) => {
            quarantine_corrupt_session(path)?;
            Ok(None)
        }
    }
}

pub fn validate_session_data(data: &SessionData) -> Result<(), SessionError> {
    if data.version != SESSION_FORMAT_VERSION {
        return Err(SessionError::UnsupportedVersion(data.version));
    }
    if let Some(active_id) = &data.active_workspace_id {
        if !data
            .workspaces
            .iter()
            .any(|workspace| &workspace.id == active_id)
        {
            return Err(SessionError::InvalidData(
                "active workspace id is not present".to_string(),
            ));
        }
    }
    for workspace in &data.workspaces {
        let leaf_count = validate_pane_tree(&workspace.pane_tree, 0)?;
        if leaf_count == 0 {
            return Err(SessionError::InvalidData(
                "workspace pane tree has no leaves".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_pane_tree(node: &PaneNode, split_depth: usize) -> Result<usize, SessionError> {
    match node {
        PaneNode::Leaf { surface_id } if surface_id.is_empty() => Err(SessionError::InvalidData(
            "pane leaf surface id must not be empty".to_string(),
        )),
        PaneNode::Leaf { .. } => Ok(1),
        PaneNode::Split {
            children, sizes, ..
        } => {
            if split_depth > MAX_SESSION_SPLIT_DEPTH {
                return Err(SessionError::InvalidData(format!(
                    "pane tree exceeds max split depth of {MAX_SESSION_SPLIT_DEPTH}"
                )));
            }
            if children.len() < 2 {
                return Err(SessionError::InvalidData(
                    "pane split must have at least 2 children".to_string(),
                ));
            }
            if sizes.len() != children.len() {
                return Err(SessionError::InvalidData(
                    "pane split sizes must match child count".to_string(),
                ));
            }
            if sizes.iter().any(|size| !size.is_finite() || *size <= 0.0) {
                return Err(SessionError::InvalidData(
                    "pane split sizes must be finite positive numbers".to_string(),
                ));
            }
            let mut leaf_count = 0;
            for child in children {
                leaf_count += validate_pane_tree(child, split_depth + 1)?;
            }
            Ok(leaf_count)
        }
    }
}

fn data_dir() -> Result<PathBuf, SessionError> {
    dirs::data_local_dir()
        .map(|d| d.join("forktty"))
        .ok_or(SessionError::NoDataDir)
}

fn session_path() -> Result<PathBuf, SessionError> {
    Ok(data_dir()?.join("session-v2.json"))
}

fn quarantine_corrupt_session(path: &Path) -> Result<(), SessionError> {
    if !path.exists() {
        return Ok(());
    }
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S");
    fs::rename(path, path.with_extension(format!("json.bad-{timestamp}")))?;
    Ok(())
}

fn default_session_version() -> u32 {
    SESSION_FORMAT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorkspaceModel;

    #[test]
    fn validates_round_trip_session() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: model.list_workspaces(),
            active_workspace_id: Some(workspace.id),
        };
        validate_session_data(&data).unwrap();
    }

    #[test]
    fn save_and_load_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: model.list_workspaces(),
            active_workspace_id: Some(workspace.id),
        };

        save_session_to_path(&path, &data).unwrap();
        assert_eq!(load_session_from_path(&path).unwrap(), Some(data));
    }
}
