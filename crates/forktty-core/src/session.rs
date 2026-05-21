use crate::model::{PaneNode, SplitAxis, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
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
    let current_path = session_path()?;
    if let Some(data) = load_session_from_path(&current_path)? {
        return Ok(Some(data));
    }
    load_session_from_path(&legacy_session_path()?)
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
    let result = (|| -> Result<(), SessionError> {
        let mut tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        tmp_file.write_all(json.as_bytes())?;
        tmp_file.sync_all()?;
        fs::rename(&tmp_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

pub fn load_session_from_path(path: &Path) -> Result<Option<SessionData>, SessionError> {
    if !path.exists() {
        return Ok(None);
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        log_quarantine_reason(path, "session path is not a regular file");
        quarantine_corrupt_session(path)?;
        return Ok(None);
    }
    if metadata.len() > MAX_SESSION_SIZE_BYTES {
        log_quarantine_reason(
            path,
            &format!(
                "session file is {} bytes, exceeds limit of {} bytes",
                metadata.len(),
                MAX_SESSION_SIZE_BYTES
            ),
        );
        quarantine_corrupt_session(path)?;
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    match parse_session_content(&content) {
        Ok(data) => match validate_session_data(&data) {
            Ok(()) => Ok(Some(data)),
            Err(err) => {
                log_quarantine_reason(path, &format!("validation failed: {err}"));
                quarantine_corrupt_session(path)?;
                Ok(None)
            }
        },
        Err(err) => {
            log_quarantine_reason(path, &format!("parse failed: {err}"));
            quarantine_corrupt_session(path)?;
            Ok(None)
        }
    }
}

fn log_quarantine_reason(path: &Path, reason: &str) {
    // Operators need to know *why* a session disappeared on startup — silent
    // quarantine masks broken migrations and on-disk corruption.
    eprintln!("Quarantining session at {}: {reason}", path.display());
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
    let mut workspace_ids = HashSet::new();
    let mut surface_ids = HashSet::new();
    let active_flag_workspaces: Vec<&str> = data
        .workspaces
        .iter()
        .filter(|workspace| workspace.active)
        .map(|workspace| workspace.id.as_str())
        .collect();
    if active_flag_workspaces.len() > 1 {
        return Err(SessionError::InvalidData(
            "multiple workspaces are marked active".to_string(),
        ));
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
        if active_flag_workspaces
            .first()
            .is_some_and(|id| *id != active_id.as_str())
        {
            return Err(SessionError::InvalidData(
                "active workspace flag disagrees with active_workspace_id".to_string(),
            ));
        }
    }
    for workspace in &data.workspaces {
        if workspace.id.trim().is_empty() {
            return Err(SessionError::InvalidData(
                "workspace id must not be empty".to_string(),
            ));
        }
        if !workspace_ids.insert(workspace.id.as_str()) {
            return Err(SessionError::InvalidData(format!(
                "duplicate workspace id: {}",
                workspace.id
            )));
        }
        let leaf_count = validate_pane_tree(&workspace.pane_tree, 0)?;
        if leaf_count == 0 {
            return Err(SessionError::InvalidData(
                "workspace pane tree has no leaves".to_string(),
            ));
        }
        let mut workspace_leaf_ids = Vec::new();
        collect_pane_surface_ids(&workspace.pane_tree, &mut workspace_leaf_ids);
        if !workspace_leaf_ids.contains(&workspace.focused_surface_id) {
            return Err(SessionError::InvalidData(format!(
                "workspace {} focused surface id is not present",
                workspace.id
            )));
        }
        for surface_id in workspace_leaf_ids {
            if !surface_ids.insert(surface_id.clone()) {
                return Err(SessionError::InvalidData(format!(
                    "duplicate pane surface id: {surface_id}"
                )));
            }
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

fn collect_pane_surface_ids(node: &PaneNode, ids: &mut Vec<String>) {
    match node {
        PaneNode::Leaf { surface_id } => ids.push(surface_id.clone()),
        PaneNode::Split { children, .. } => {
            for child in children {
                collect_pane_surface_ids(child, ids);
            }
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

fn legacy_session_path() -> Result<PathBuf, SessionError> {
    Ok(data_dir()?.join("session.json"))
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
    fn save_session_to_path_removes_temp_file_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::create_dir(&path).unwrap();
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let data = model.to_session_data();

        let result = save_session_to_path(&path, &data);

        assert!(matches!(result, Err(SessionError::Io(_))));
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .all(|name| !name.to_string_lossy().contains(".tmp-")),
            "unexpected temp session file sibling: {siblings:?}"
        );
    }

    #[test]
    fn rejects_session_with_duplicate_workspace_ids() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp/main");
        model.create_workspace("other", "/tmp/other");
        let mut data = model.to_session_data();
        data.workspaces[1].id = data.workspaces[0].id.clone();

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_multiple_active_workspaces() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp/main");
        model.create_workspace("other", "/tmp/other");
        let mut data = model.to_session_data();
        // Forge an inconsistent session where both workspaces report `active`.
        for workspace in &mut data.workspaces {
            workspace.active = true;
        }

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));

        data.active_workspace_id = None;
        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_when_active_flag_disagrees_with_active_workspace_id() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("main", "/tmp/main");
        model.create_workspace("other", "/tmp/other");
        let mut data = model.to_session_data();
        // Active_workspace_id points to `other` (set by the second create), but
        // we forge the active flag onto the first workspace.
        for workspace in &mut data.workspaces {
            workspace.active = workspace.id == first.id;
        }

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_missing_focused_surface() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        data.workspaces[0].focused_surface_id = "missing-surface".to_string();

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_duplicate_surface_ids() {
        let mut model = WorkspaceModel::new();
        let first = model.create_workspace("main", "/tmp/main");
        let second = model.create_workspace("other", "/tmp/other");
        let mut data = model.to_session_data();
        data.workspaces[1].pane_tree = PaneNode::Leaf {
            surface_id: first.focused_surface_id.clone(),
        };
        data.workspaces[1].focused_surface_id = first.focused_surface_id;
        data.active_workspace_id = Some(second.id);

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
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

    #[test]
    fn quarantines_corrupted_session_file_instead_of_returning_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        fs::write(&path, "{ this is not valid JSON ::: ").unwrap();

        let loaded = load_session_from_path(&path).unwrap();

        assert!(loaded.is_none());
        assert!(!path.exists(), "corrupt file should be renamed aside");
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .any(|name| name.to_string_lossy().contains(".bad-")),
            "expected a .bad-* quarantine sibling, got {siblings:?}"
        );
    }

    #[test]
    fn quarantines_oversized_session_file_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let oversized = "x".repeat((MAX_SESSION_SIZE_BYTES + 1) as usize);
        fs::write(&path, oversized).unwrap();

        let loaded = load_session_from_path(&path).unwrap();
        assert!(loaded.is_none());
        assert!(!path.exists());
    }

    #[test]
    fn rejects_session_pane_tree_with_non_finite_sizes() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        data.workspaces[0].pane_tree = PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: vec![
                PaneNode::Leaf {
                    surface_id: data.workspaces[0].focused_surface_id.clone(),
                },
                PaneNode::Leaf {
                    surface_id: "extra-leaf".to_string(),
                },
            ],
            sizes: vec![f64::NAN, 0.5],
        };

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_legacy_session_with_out_of_bounds_focused_leaf_index() {
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
                  "working_dir": "/tmp",
                  "git_branch": "feature",
                  "worktree_dir": "",
                  "worktree_name": "",
                  "focused_leaf_index": 7,
                  "pane_tree": { "type": "leaf" }
                }
              ]
            }"#,
        )
        .unwrap();

        // Out-of-bounds focused_leaf_index is an InvalidData failure during
        // migration; load_session_from_path should quarantine, not panic.
        let loaded = load_session_from_path(&path).unwrap();
        assert!(loaded.is_none());
        assert!(!path.exists());
    }
}
