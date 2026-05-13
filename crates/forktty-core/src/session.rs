use crate::model::{PaneNode, SplitAxis, Workspace};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacySessionData {
    #[serde(default = "default_legacy_session_version")]
    version: u32,
    workspaces: Vec<LegacyWorkspaceSnapshot>,
    active_workspace_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LegacyWorkspaceSnapshot {
    name: String,
    working_dir: String,
    git_branch: String,
    worktree_dir: String,
    worktree_name: String,
    pane_tree: LegacyPaneTreeSnapshot,
    #[serde(default)]
    focused_leaf_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum LegacyPaneTreeSnapshot {
    #[serde(rename = "leaf")]
    Leaf,
    #[serde(rename = "horizontal")]
    Horizontal {
        children: Vec<LegacyPaneTreeSnapshot>,
        sizes: Vec<f64>,
    },
    #[serde(rename = "vertical")]
    Vertical {
        children: Vec<LegacyPaneTreeSnapshot>,
        sizes: Vec<f64>,
    },
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
    match parse_session_content(&content) {
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

fn parse_session_content(content: &str) -> Result<SessionData, SessionError> {
    let value: serde_json::Value = serde_json::from_str(content)?;
    if value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1)
        == u64::from(SESSION_FORMAT_VERSION)
    {
        return Ok(serde_json::from_value(value)?);
    }

    let legacy: LegacySessionData = serde_json::from_value(value)?;
    migrate_legacy_session(legacy)
}

fn migrate_legacy_session(legacy: LegacySessionData) -> Result<SessionData, SessionError> {
    if legacy.version != 1 {
        return Err(SessionError::UnsupportedVersion(legacy.version));
    }
    if !legacy.workspaces.is_empty() && legacy.active_workspace_index >= legacy.workspaces.len() {
        return Err(SessionError::InvalidData(
            "legacy active workspace index is out of bounds".to_string(),
        ));
    }

    let mut next_surface_index = 1usize;
    let mut workspaces = Vec::with_capacity(legacy.workspaces.len());
    for (workspace_index, legacy_workspace) in legacy.workspaces.into_iter().enumerate() {
        let mut leaf_ids = Vec::new();
        let pane_tree = migrate_legacy_pane_tree(
            &legacy_workspace.pane_tree,
            &mut next_surface_index,
            &mut leaf_ids,
        )?;
        let focused_surface_id = leaf_ids
            .get(legacy_workspace.focused_leaf_index)
            .cloned()
            .ok_or_else(|| {
                SessionError::InvalidData("legacy focused leaf index is out of bounds".to_string())
            })?;
        workspaces.push(Workspace {
            id: format!("workspace-{}", workspace_index + 1),
            name: legacy_workspace.name,
            active: workspace_index == legacy.active_workspace_index,
            working_dir: PathBuf::from(&legacy_workspace.working_dir),
            git_branch: legacy_workspace.git_branch,
            worktree_dir: non_empty_path(legacy_workspace.worktree_dir),
            worktree_name: non_empty_string(legacy_workspace.worktree_name),
            pane_tree,
            focused_surface_id,
            needs_attention: false,
        });
    }

    let active_workspace_id = workspaces
        .get(legacy.active_workspace_index)
        .map(|workspace| workspace.id.clone());
    Ok(SessionData {
        version: SESSION_FORMAT_VERSION,
        workspaces,
        active_workspace_id,
    })
}

fn migrate_legacy_pane_tree(
    node: &LegacyPaneTreeSnapshot,
    next_surface_index: &mut usize,
    leaf_ids: &mut Vec<String>,
) -> Result<PaneNode, SessionError> {
    match node {
        LegacyPaneTreeSnapshot::Leaf => {
            let surface_id = format!("surface-{next_surface_index}");
            *next_surface_index += 1;
            leaf_ids.push(surface_id.clone());
            Ok(PaneNode::Leaf { surface_id })
        }
        LegacyPaneTreeSnapshot::Horizontal { children, sizes } => migrate_legacy_split(
            SplitAxis::Horizontal,
            children,
            sizes,
            next_surface_index,
            leaf_ids,
        ),
        LegacyPaneTreeSnapshot::Vertical { children, sizes } => migrate_legacy_split(
            SplitAxis::Vertical,
            children,
            sizes,
            next_surface_index,
            leaf_ids,
        ),
    }
}

fn migrate_legacy_split(
    axis: SplitAxis,
    children: &[LegacyPaneTreeSnapshot],
    sizes: &[f64],
    next_surface_index: &mut usize,
    leaf_ids: &mut Vec<String>,
) -> Result<PaneNode, SessionError> {
    if children.len() < 2 {
        return Err(SessionError::InvalidData(
            "legacy pane split must have at least 2 children".to_string(),
        ));
    }
    if children.len() != sizes.len() {
        return Err(SessionError::InvalidData(
            "legacy pane split sizes must match child count".to_string(),
        ));
    }
    let mut migrated_children = Vec::with_capacity(children.len());
    for child in children {
        migrated_children.push(migrate_legacy_pane_tree(
            child,
            next_surface_index,
            leaf_ids,
        )?);
    }
    Ok(PaneNode::Split {
        axis,
        children: migrated_children,
        sizes: sizes.to_vec(),
    })
}

fn non_empty_path(value: String) -> Option<PathBuf> {
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

fn non_empty_string(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
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

fn default_legacy_session_version() -> u32 {
    1
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

    #[test]
    fn loads_legacy_v1_session_with_pane_focus_and_worktree_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(
            &path,
            r#"{
              "version": 1,
              "active_workspace_index": 0,
              "workspaces": [
                {
                  "name": "feature",
                  "working_dir": "/repo/.worktrees/feature",
                  "git_branch": "feature",
                  "worktree_dir": "/repo/.worktrees/feature",
                  "worktree_name": "feature",
                  "focused_leaf_index": 1,
                  "pane_tree": {
                    "type": "horizontal",
                    "sizes": [0.4, 0.6],
                    "children": [
                      { "type": "leaf" },
                      { "type": "leaf" }
                    ]
                  }
                }
              ]
            }"#,
        )
        .unwrap();

        let migrated = load_session_from_path(&path).unwrap().unwrap();

        assert_eq!(migrated.version, SESSION_FORMAT_VERSION);
        assert_eq!(migrated.active_workspace_id.as_deref(), Some("workspace-1"));
        assert_eq!(migrated.workspaces[0].git_branch, "feature");
        assert_eq!(
            migrated.workspaces[0].worktree_name.as_deref(),
            Some("feature")
        );
        assert_eq!(migrated.workspaces[0].focused_surface_id, "surface-2");
        assert!(matches!(
            migrated.workspaces[0].pane_tree,
            PaneNode::Split {
                axis: SplitAxis::Horizontal,
                ..
            }
        ));
    }
}
