use crate::model::{PaneNode, SplitAxis, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
    load_session_from_paths(&session_path()?, &legacy_session_path()?)
}

fn load_session_from_paths(
    current_path: &Path,
    legacy_path: &Path,
) -> Result<Option<SessionData>, SessionError> {
    let had_current_session = fs::symlink_metadata(current_path).is_ok();
    if let Some(data) = load_session_from_path(current_path)? {
        return Ok(Some(data));
    }
    if had_current_session {
        return Ok(None);
    }
    load_session_from_path(legacy_path)
}

pub fn save_session_to_path(path: &Path, data: &SessionData) -> Result<(), SessionError> {
    validate_session_data(data)?;
    let write_path = session_write_path(path)?;
    if let Some(parent) = write_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(data)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp_path = write_path.with_extension(format!("json.tmp-{}-{nonce}", std::process::id()));
    let result = (|| -> Result<(), SessionError> {
        let mut tmp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        apply_session_permissions(&tmp_path)?;
        tmp_file.write_all(json.as_bytes())?;
        tmp_file.sync_all()?;
        fs::rename(&tmp_path, &write_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn session_write_path(path: &Path) -> Result<PathBuf, SessionError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => Ok(resolved),
            // Broken symlink: rename will replace the dangling link with the
            // freshly written file. The previous code surfaced this as a
            // confusing `canonicalize` IO error on every save.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
            Err(err) => Err(err.into()),
        },
        Ok(_) => Ok(path.to_path_buf()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(err) => Err(err.into()),
    }
}

fn apply_session_permissions(tmp_path: &Path) -> Result<(), SessionError> {
    #[cfg(unix)]
    {
        fs::set_permissions(tmp_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn load_session_from_path(path: &Path) -> Result<Option<SessionData>, SessionError> {
    let link_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if link_metadata.file_type().is_symlink() {
                log_quarantine_reason(path, "session path is a broken symlink");
                quarantine_corrupt_session(path)?;
            }
            return Ok(None);
        }
        Err(err) => return Err(err.into()),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        drop(file);
        log_quarantine_reason(path, "session path is not a regular file");
        quarantine_corrupt_session(path)?;
        return Ok(None);
    }
    if metadata.len() > MAX_SESSION_SIZE_BYTES {
        drop(file);
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
    let mut content = String::new();
    (&mut file)
        .take(MAX_SESSION_SIZE_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_SESSION_SIZE_BYTES {
        drop(file);
        log_quarantine_reason(path, "session file grew past size limit during read");
        quarantine_corrupt_session(path)?;
        return Ok(None);
    }
    drop(file);
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

fn sanitize_for_terminal(input: &str) -> String {
    input.chars().flat_map(char::escape_default).collect()
}

fn log_quarantine_reason(path: &Path, reason: &str) {
    // Operators need to know *why* a session disappeared on startup — silent
    // quarantine masks broken migrations and on-disk corruption.
    let safe_path = sanitize_for_terminal(&path.display().to_string());
    let safe_reason = sanitize_for_terminal(reason);
    eprintln!("Quarantining session at {safe_path}: {safe_reason}");
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
        if workspace.name.trim().is_empty() {
            return Err(SessionError::InvalidData(
                "workspace name must not be empty".to_string(),
            ));
        }
        if workspace.working_dir.as_os_str().is_empty() {
            return Err(SessionError::InvalidData(
                "workspace working directory must not be empty".to_string(),
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
        PaneNode::Leaf { surface_id } if surface_id.trim().is_empty() => Err(
            SessionError::InvalidData("pane leaf surface id must not be empty".to_string()),
        ),
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
    let timestamp = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
    quarantine_corrupt_session_with_timestamp(path, &timestamp).map(|_| ())
}

fn quarantine_corrupt_session_with_timestamp(
    path: &Path,
    timestamp: &str,
) -> Result<Option<PathBuf>, SessionError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }
    let quarantine_path = available_bad_session_path(path, timestamp);
    fs::rename(path, &quarantine_path)?;
    Ok(Some(quarantine_path))
}

fn available_bad_session_path(path: &Path, timestamp: &str) -> PathBuf {
    for suffix in std::iter::once(String::new()).chain((1u32..).map(|index| format!("-{index}"))) {
        let candidate = path.with_extension(format!("json.bad-{timestamp}{suffix}"));
        if matches!(
            fs::symlink_metadata(&candidate),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound
        ) {
            return candidate;
        }
    }
    unreachable!("unbounded quarantine path search should always return")
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(load_session_from_path(&path).unwrap(), Some(data));
    }

    #[cfg(unix)]
    #[test]
    fn save_and_load_session_through_symlink_updates_target_without_replacing_link() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let link_dir = dir.path().join("data");
        let managed_dir = dir.path().join("managed");
        fs::create_dir_all(&link_dir).unwrap();
        fs::create_dir_all(&managed_dir).unwrap();
        let path = link_dir.join("session-v2.json");
        let target = managed_dir.join("session-v2.json");
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: model.list_workspaces(),
            active_workspace_id: Some(workspace.id),
        };
        save_session_to_path(&target, &data).unwrap();
        symlink(&target, &path).unwrap();

        assert_eq!(load_session_from_path(&path).unwrap(), Some(data.clone()));
        save_session_to_path(&path, &data).unwrap();

        assert!(fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(
            fs::symlink_metadata(&target).is_ok(),
            "target file should remain present"
        );
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_session_from_path(&path).unwrap(), Some(data));
        let link_siblings: Vec<_> = fs::read_dir(&link_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(link_siblings, [std::ffi::OsString::from("session-v2.json")]);
        let managed_siblings: Vec<_> = fs::read_dir(&managed_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            managed_siblings
                .iter()
                .all(|name| !name.to_string_lossy().contains(".tmp-")),
            "unexpected temp session file sibling: {managed_siblings:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_session_to_path_replaces_broken_symlink_with_regular_file() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        symlink(dir.path().join("missing-target.json"), &path).unwrap();
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: model.list_workspaces(),
            active_workspace_id: Some(workspace.id),
        };

        save_session_to_path(&path, &data).expect("save through broken symlink should succeed");

        let stat = fs::symlink_metadata(&path).unwrap();
        assert!(
            stat.is_file(),
            "broken symlink should be replaced by a regular file"
        );
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
    fn loads_legacy_session_when_current_session_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let current_path = dir.path().join("session-v2.json");
        let legacy_path = dir.path().join("session.json");
        write_legacy_session_file(&legacy_path);

        let loaded = load_session_from_paths(&current_path, &legacy_path)
            .unwrap()
            .unwrap();

        assert_eq!(loaded.version, SESSION_FORMAT_VERSION);
        assert_eq!(loaded.active_workspace_id.as_deref(), Some("workspace-1"));
        assert_eq!(loaded.workspaces[0].name, "legacy");
        assert!(legacy_path.exists());
    }

    #[test]
    fn skips_legacy_session_after_current_session_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let current_path = dir.path().join("session-v2.json");
        let legacy_path = dir.path().join("session.json");
        fs::write(&current_path, "{ broken").unwrap();
        write_legacy_session_file(&legacy_path);

        let loaded = load_session_from_paths(&current_path, &legacy_path).unwrap();

        assert!(loaded.is_none());
        assert!(
            !current_path.exists(),
            "corrupt current session was not moved"
        );
        assert!(
            legacy_path.exists(),
            "legacy session should be left untouched"
        );
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .any(|name| name.to_string_lossy().contains("session-v2.json.bad-")),
            "expected a quarantined v2 session sibling, got {siblings:?}"
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
    fn rejects_session_with_blank_workspace_fields() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");

        let mut data = model.to_session_data();
        data.workspaces[0].name = " \t ".to_string();
        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));

        let mut data = model.to_session_data();
        data.workspaces[0].working_dir = PathBuf::new();
        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn quarantines_session_with_blank_workspace_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        data.workspaces[0].name.clear();
        fs::write(&path, serde_json::to_string_pretty(&data).unwrap()).unwrap();

        let loaded = load_session_from_path(&path).unwrap();

        assert!(loaded.is_none());
        assert!(!path.exists(), "invalid session should be renamed aside");
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .any(|name| name.to_string_lossy().contains("session-v2.json.bad-")),
            "expected a quarantined invalid session sibling, got {siblings:?}"
        );
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
    fn rejects_session_with_blank_pane_surface_id() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        data.workspaces[0].pane_tree = PaneNode::Leaf {
            surface_id: " \n ".to_string(),
        };
        data.workspaces[0].focused_surface_id = " \n ".to_string();

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

    #[cfg(unix)]
    #[test]
    fn quarantines_broken_session_symlink_instead_of_treating_it_as_missing() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        symlink(dir.path().join("missing-session-v2.json"), &path).unwrap();

        let loaded = load_session_from_path(&path).unwrap();

        assert!(loaded.is_none());
        assert!(
            fs::symlink_metadata(&path).is_err(),
            "broken session symlink should be renamed aside"
        );
        let siblings: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .any(|name| name.to_string_lossy().contains("session-v2.json.bad-")),
            "expected a quarantined session symlink sibling, got {siblings:?}"
        );
    }

    #[test]
    fn quarantine_does_not_overwrite_existing_quarantine_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2.json");
        let first_candidate = path.with_extension("json.bad-20260521010203");
        let second_candidate = path.with_extension("json.bad-20260521010203-1");
        fs::write(&path, "new bad session").unwrap();
        fs::write(&first_candidate, "previous bad session").unwrap();

        let quarantine_path = quarantine_corrupt_session_with_timestamp(&path, "20260521010203")
            .unwrap()
            .unwrap();

        assert_eq!(quarantine_path, second_candidate);
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(&first_candidate).unwrap(),
            "previous bad session"
        );
        assert_eq!(
            fs::read_to_string(&second_candidate).unwrap(),
            "new bad session"
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

    #[test]
    fn sanitize_for_terminal_escapes_control_sequences() {
        let raw = "dup-id-\u{001b}]52;c;payload\u{0007}\nnext";
        let sanitized = sanitize_for_terminal(raw);
        assert_eq!(sanitized, "dup-id-\\u{1b}]52;c;payload\\u{7}\\nnext");
        assert!(!sanitized.contains("\nnext"));
    }

    fn write_legacy_session_file(path: &Path) {
        fs::write(
            path,
            r#"{
              "version": 1,
              "active_workspace_index": 0,
              "workspaces": [
                {
                  "name": "legacy",
                  "working_dir": "/repo/legacy",
                  "git_branch": "main",
                  "worktree_dir": "",
                  "worktree_name": "",
                  "pane_tree": { "type": "leaf" }
                }
              ]
            }"#,
        )
        .unwrap();
    }
}
