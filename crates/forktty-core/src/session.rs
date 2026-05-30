use crate::model::{PaneNode, SplitAxis, Surface, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

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

pub const SESSION_FORMAT_VERSION: u32 = 3;
const MAX_SESSION_SIZE_BYTES: u64 = 1_048_576;
const MAX_SESSION_SPLIT_DEPTH: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionData {
    #[serde(default = "default_session_version")]
    pub version: u32,
    pub workspaces: Vec<Workspace>,
    #[serde(default)]
    pub active_workspace_id: Option<String>,
    /// Persisted per-surface state (currently the surface `kind`/url) keyed by
    /// surface id. Empty on sessions written before browser panes existed, in
    /// which case restore falls back to `SurfaceKind::Terminal` for every leaf.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
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

// ── v2 → v3 migration types ──────────────────────────────────────────────────
// v2 serialized `PaneNode::Leaf` as `{"type":"leaf","surface_id":"..."}`.
// v3 uses `{"type":"leaf","tabs":[...],"active":N}`.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
enum V2PaneNode {
    #[serde(rename = "leaf")]
    Leaf { surface_id: String },
    #[serde(rename = "split")]
    Split {
        axis: SplitAxis,
        children: Vec<V2PaneNode>,
        sizes: Vec<f64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct V2Workspace {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub active: bool,
    pub working_dir: std::path::PathBuf,
    #[serde(default)]
    pub git_branch: String,
    #[serde(default)]
    pub worktree_dir: Option<std::path::PathBuf>,
    #[serde(default)]
    pub worktree_name: Option<String>,
    pub pane_tree: V2PaneNode,
    pub focused_surface_id: String,
    #[serde(default)]
    pub needs_attention: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct V2SessionData {
    pub version: u32,
    pub workspaces: Vec<V2Workspace>,
    #[serde(default)]
    pub active_workspace_id: Option<String>,
    #[serde(default)]
    pub surfaces: Vec<Surface>,
}

pub fn save_session(data: &SessionData) -> Result<(), SessionError> {
    save_session_to_path(&session_path()?, data)
}

pub fn load_session() -> Result<Option<SessionData>, SessionError> {
    let current_path = session_path()?;
    let mut fallback_paths = Vec::new();
    if let Ok(previous_current_path) = previous_data_session_path() {
        if previous_current_path != current_path {
            fallback_paths.push(previous_current_path);
        }
    }
    if let Ok(legacy_path) = legacy_session_path() {
        fallback_paths.push(legacy_path);
    }
    let fallback_refs = fallback_paths
        .iter()
        .map(PathBuf::as_path)
        .collect::<Vec<_>>();
    load_session_from_ordered_paths(&current_path, &fallback_refs)
}

#[cfg(test)]
fn load_session_from_paths(
    current_path: &Path,
    legacy_path: &Path,
) -> Result<Option<SessionData>, SessionError> {
    load_session_from_ordered_paths(current_path, &[legacy_path])
}

fn load_session_from_ordered_paths(
    current_path: &Path,
    fallback_paths: &[&Path],
) -> Result<Option<SessionData>, SessionError> {
    let had_current_session = fs::symlink_metadata(current_path).is_ok();
    if let Some(data) = load_session_from_path(current_path)? {
        return Ok(Some(data));
    }
    if had_current_session {
        return Ok(None);
    }
    for fallback_path in fallback_paths {
        let had_fallback_session = fs::symlink_metadata(fallback_path).is_ok();
        if let Some(data) = load_session_from_path(fallback_path)? {
            return Ok(Some(data));
        }
        if had_fallback_session {
            return Ok(None);
        }
    }
    Ok(None)
}

pub fn save_session_to_path(path: &Path, data: &SessionData) -> Result<(), SessionError> {
    validate_session_data(data)?;
    let write_path = session_write_path(path)?;
    if let Some(parent) = write_path.parent() {
        ensure_session_parent_dir(parent)?;
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
        sync_parent_dir(&write_path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
}

fn ensure_session_parent_dir(parent: &Path) -> Result<(), SessionError> {
    if parent.as_os_str().is_empty() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let mut missing = Vec::new();
        let mut cursor = Some(parent);
        while let Some(dir) = cursor {
            match fs::metadata(dir) {
                Ok(meta) if meta.is_dir() => break,
                Ok(_) => {
                    return Err(SessionError::InvalidData(format!(
                        "session directory path is not a directory: {}",
                        dir.display()
                    )));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(dir.to_path_buf());
                    cursor = dir.parent().filter(|path| !path.as_os_str().is_empty());
                }
                Err(err) => return Err(err.into()),
            }
        }

        for dir in missing.iter().rev() {
            match fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !fs::metadata(dir)?.is_dir() {
                        return Err(SessionError::InvalidData(format!(
                            "session directory path is not a directory: {}",
                            dir.display()
                        )));
                    }
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn sync_parent_dir(path: &Path) -> Result<(), SessionError> {
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
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
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1) as u32;

    if version == SESSION_FORMAT_VERSION {
        return Ok(serde_json::from_value(value)?);
    }

    if version == 2 {
        let v2: V2SessionData = serde_json::from_value(value)?;
        return migrate_v2_session(v2);
    }

    let legacy: LegacySessionData = serde_json::from_value(value)?;
    migrate_legacy_session(legacy)
}

fn migrate_v2_session(v2: V2SessionData) -> Result<SessionData, SessionError> {
    let workspaces = v2
        .workspaces
        .into_iter()
        .map(|ws| {
            let pane_tree = migrate_v2_pane_node(ws.pane_tree);
            Workspace {
                id: ws.id,
                name: ws.name,
                active: ws.active,
                working_dir: ws.working_dir,
                git_branch: ws.git_branch,
                worktree_dir: ws.worktree_dir,
                worktree_name: ws.worktree_name,
                pane_tree,
                focused_surface_id: ws.focused_surface_id,
                needs_attention: ws.needs_attention,
                listening_ports: Vec::new(),
                pr: None,
            }
        })
        .collect();
    Ok(SessionData {
        version: SESSION_FORMAT_VERSION,
        workspaces,
        active_workspace_id: v2.active_workspace_id,
        surfaces: v2.surfaces,
    })
}

fn migrate_v2_pane_node(node: V2PaneNode) -> PaneNode {
    match node {
        V2PaneNode::Leaf { surface_id } => PaneNode::single_leaf(surface_id),
        V2PaneNode::Split {
            axis,
            children,
            sizes,
        } => PaneNode::Split {
            axis,
            children: children.into_iter().map(migrate_v2_pane_node).collect(),
            sizes,
        },
    }
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
            listening_ports: Vec::new(),
            pr: None,
        });
    }

    let active_workspace_id = workspaces
        .get(legacy.active_workspace_index)
        .map(|workspace| workspace.id.clone());
    Ok(SessionData {
        version: SESSION_FORMAT_VERSION,
        workspaces,
        active_workspace_id,
        // Legacy sessions predate browser panes: every surface is a terminal.
        surfaces: Vec::new(),
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
            Ok(PaneNode::single_leaf(surface_id))
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
    if data.workspaces.is_empty() {
        return Err(SessionError::InvalidData(
            "session must contain at least one workspace".to_string(),
        ));
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
    let mut persisted_surface_ids = HashSet::new();
    for surface in &data.surfaces {
        if surface.id.trim().is_empty() {
            return Err(SessionError::InvalidData(
                "persisted surface id must not be empty".to_string(),
            ));
        }
        if !persisted_surface_ids.insert(surface.id.as_str()) {
            return Err(SessionError::InvalidData(format!(
                "duplicate persisted surface id: {}",
                surface.id
            )));
        }
        if !surface_ids.contains(&surface.id) {
            return Err(SessionError::InvalidData(format!(
                "persisted surface id is not present in pane tree: {}",
                surface.id
            )));
        }
    }
    Ok(())
}

fn validate_pane_tree(node: &PaneNode, split_depth: usize) -> Result<usize, SessionError> {
    match node {
        PaneNode::Leaf { tabs, .. } if tabs.is_empty() => Err(SessionError::InvalidData(
            "pane leaf must have at least one tab".to_string(),
        )),
        PaneNode::Leaf { tabs, active } => {
            if *active >= tabs.len() {
                return Err(SessionError::InvalidData(
                    "pane leaf active tab index is out of bounds".to_string(),
                ));
            }
            for tab in tabs {
                if tab.trim().is_empty() {
                    return Err(SessionError::InvalidData(
                        "pane leaf surface id must not be empty".to_string(),
                    ));
                }
            }
            Ok(1)
        }
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
        PaneNode::Leaf { tabs, .. } => {
            for tab in tabs {
                ids.push(tab.clone());
            }
        }
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

fn state_dir() -> Result<PathBuf, SessionError> {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .map(|d| d.join("forktty"))
        .ok_or(SessionError::NoDataDir)
}

fn session_path() -> Result<PathBuf, SessionError> {
    Ok(state_dir()?.join("session-v2.json"))
}

fn previous_data_session_path() -> Result<PathBuf, SessionError> {
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
    use crate::model::{MovePosition, SurfaceKind, WorkspaceModel, WorkspaceSelector};
    use crate::profile::ProfileId;

    fn assert_ratio(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-6,
            "expected ratio {expected}, got {actual}"
        );
    }

    #[test]
    fn validates_round_trip_session() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: model.list_workspaces(),
            active_workspace_id: Some(workspace.id),
            surfaces: Vec::new(),
        };
        validate_session_data(&data).unwrap();
    }

    #[test]
    fn rejects_session_without_workspaces() {
        let data = SessionData {
            version: SESSION_FORMAT_VERSION,
            workspaces: Vec::new(),
            active_workspace_id: None,
            surfaces: Vec::new(),
        };

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(message))
                if message == "session must contain at least one workspace"
        ));
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
            surfaces: Vec::new(),
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
    fn save_session_to_path_creates_missing_parent_directories_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("state").join("forktty");
        let path = parent.join("session-v2.json");
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let data = model.to_session_data();

        save_session_to_path(&path, &data).unwrap();

        assert_eq!(
            fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
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
            surfaces: Vec::new(),
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
            surfaces: Vec::new(),
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
    fn loads_previous_data_home_session_when_state_session_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state").join("session-v2.json");
        let previous_data_path = dir.path().join("data").join("session-v2.json");
        let legacy_path = dir.path().join("data").join("session.json");
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("persisted", "/tmp/persisted");
        let data = model.to_session_data();
        save_session_to_path(&previous_data_path, &data).unwrap();
        write_legacy_session_file(&legacy_path);

        let loaded =
            load_session_from_ordered_paths(&state_path, &[&previous_data_path, &legacy_path])
                .unwrap()
                .unwrap();

        assert_eq!(loaded.version, SESSION_FORMAT_VERSION);
        assert_eq!(
            loaded.active_workspace_id.as_deref(),
            Some(workspace.id.as_str())
        );
        assert_eq!(loaded.workspaces[0].name, "persisted");
    }

    #[test]
    fn skips_legacy_session_after_previous_data_session_is_quarantined() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state").join("session-v2.json");
        let previous_data_path = dir.path().join("data").join("session-v2.json");
        let legacy_path = dir.path().join("data").join("session.json");
        fs::create_dir_all(previous_data_path.parent().unwrap()).unwrap();
        fs::write(&previous_data_path, "{ broken").unwrap();
        write_legacy_session_file(&legacy_path);

        let loaded =
            load_session_from_ordered_paths(&state_path, &[&previous_data_path, &legacy_path])
                .unwrap();

        assert!(loaded.is_none());
        assert!(
            !previous_data_path.exists(),
            "corrupt previous data-home session was not moved"
        );
        assert!(
            legacy_path.exists(),
            "legacy session should be left untouched"
        );
        let siblings: Vec<_> = fs::read_dir(previous_data_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(
            siblings
                .iter()
                .any(|name| name.to_string_lossy().contains("session-v2.json.bad-")),
            "expected a quarantined migrated v2 session sibling, got {siblings:?}"
        );
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
        data.workspaces[1].pane_tree = PaneNode::single_leaf(first.focused_surface_id.clone());
        data.workspaces[1].focused_surface_id = first.focused_surface_id;
        data.active_workspace_id = Some(second.id);

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_duplicate_persisted_surface_ids() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model
            .open_browser(
                &workspace.id,
                "https://example.com",
                crate::profile::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        let mut data = model.to_session_data();
        assert_eq!(data.surfaces.len(), 1);
        data.surfaces.push(data.surfaces[0].clone());

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_persisted_surface_outside_pane_tree() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model
            .open_browser(
                &workspace.id,
                "https://example.com",
                crate::profile::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        let mut data = model.to_session_data();
        assert_eq!(data.surfaces.len(), 1);
        data.surfaces[0].id = "surface-missing".to_string();

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_with_blank_persisted_surface_id() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        model
            .open_browser(
                &workspace.id,
                "https://example.com",
                crate::profile::ProfileId::default(),
                SplitAxis::Horizontal,
            )
            .unwrap();
        let mut data = model.to_session_data();
        assert_eq!(data.surfaces.len(), 1);
        data.surfaces[0].id = " ".to_string();

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
            tabs: vec![" \n ".to_string()],
            active: 0,
        };
        data.workspaces[0].focused_surface_id = " \n ".to_string();

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }

    #[test]
    fn rejects_session_leaf_with_active_tab_out_of_range() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        data.workspaces[0].pane_tree = PaneNode::Leaf {
            tabs: vec![workspace.focused_surface_id.clone()],
            active: 1,
        };

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
                PaneNode::single_leaf(data.workspaces[0].focused_surface_id.clone()),
                PaneNode::single_leaf("extra-leaf".to_string()),
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

    // ── v3 session tests ──────────────────────────────────────────────────────

    #[test]
    fn round_trip_multi_tab_leaf() {
        // Build a workspace with a multi-tab leaf manually and verify that
        // serialising → deserialising through SessionData preserves all tabs.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v3.json");

        let mut model = WorkspaceModel::new();
        let ws = model.create_workspace("main", "/tmp");
        let first_id = ws.focused_surface_id.clone();
        // Add a second tab using the public API.
        let tab2 = model.add_tab(&first_id).expect("add_tab");
        let tab2_id = tab2.id.clone();

        let data = model.to_session_data();
        assert_eq!(data.version, SESSION_FORMAT_VERSION);
        // The leaf must serialise with both tab ids.
        let json = serde_json::to_string_pretty(&data).unwrap();
        assert!(
            json.contains("\"tabs\""),
            "expected tabs key in JSON: {json}"
        );
        // The pane leaf must not use the old "surface_id" field form.
        assert!(
            !json.contains("\"surface_id\""),
            "must not contain old bare surface_id key: {json}"
        );

        save_session_to_path(&path, &data).unwrap();
        let loaded = load_session_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.version, SESSION_FORMAT_VERSION);
        let leaf = &loaded.workspaces[0].pane_tree;
        let tabs = leaf.leaf_tabs().expect("leaf must have tabs");
        assert_eq!(tabs.len(), 2);
        assert!(tabs.contains(&ws.focused_surface_id));
        assert!(tabs.contains(&tab2_id));
    }

    #[test]
    fn round_trip_restores_complex_manual_workspace_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v3-complex.json");
        let mut model = WorkspaceModel::new();

        let alpha = model.create_workspace("alpha", "/tmp/forktty-alpha");
        let alpha_root = alpha.focused_surface_id.clone();
        let alpha_tab = model.add_tab(&alpha_root).expect("alpha tab");
        let alpha_split = model
            .split_surface(&alpha_tab.id, SplitAxis::Horizontal)
            .expect("alpha split");
        let browser = model
            .open_browser(
                &alpha.id,
                "https://example.com/manual-restore",
                ProfileId::default(),
                SplitAxis::Vertical,
            )
            .expect("browser pane");
        assert!(model.update_split_partition_ratio(
            &alpha.id,
            &[alpha_root.clone(), alpha_tab.id.clone()],
            &[alpha_split.id.clone(), browser.id.clone()],
            0.72,
        ));
        assert!(model.update_split_partition_ratio(
            &alpha.id,
            std::slice::from_ref(&alpha_split.id),
            std::slice::from_ref(&browser.id),
            0.34,
        ));

        let beta = model.create_workspace("beta", "/tmp/forktty-beta");
        let beta_root = beta.focused_surface_id.clone();
        let beta_tab = model.add_tab(&beta_root).expect("beta tab");
        let gamma = model.create_workspace("gamma", "/tmp/forktty-gamma");
        assert!(model.move_workspace(&gamma.id, &alpha.id, MovePosition::Before));
        model
            .select_workspace(WorkspaceSelector::Id(&alpha.id))
            .expect("select alpha");
        assert!(model.focus_surface(&browser.id));

        let data = model.to_session_data();
        validate_session_data(&data).unwrap();
        save_session_to_path(&path, &data).unwrap();
        let loaded = load_session_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded, data);

        let mut restored = WorkspaceModel::new();
        restored.restore_session(loaded);
        let restored_data = restored.to_session_data();
        assert_eq!(restored_data, data);

        let workspace_ids = restored_data
            .workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            workspace_ids,
            vec![gamma.id.as_str(), alpha.id.as_str(), beta.id.as_str()]
        );
        assert_eq!(
            restored_data.active_workspace_id.as_deref(),
            Some(alpha.id.as_str())
        );

        let alpha_restored = restored_data
            .workspaces
            .iter()
            .find(|workspace| workspace.id == alpha.id)
            .expect("alpha restored");
        assert!(alpha_restored.active);
        assert_eq!(alpha_restored.focused_surface_id, browser.id);

        let PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: root_children,
            sizes: root_sizes,
        } = &alpha_restored.pane_tree
        else {
            panic!("expected alpha root horizontal split");
        };
        assert_eq!(root_children.len(), 2);
        assert_ratio(root_sizes[0], 0.72);
        assert_ratio(root_sizes[1], 0.28);

        let PaneNode::Leaf { tabs, active } = &root_children[0] else {
            panic!("expected alpha first child to be tab leaf");
        };
        assert_eq!(tabs, &vec![alpha_root.clone(), alpha_tab.id.clone()]);
        assert_eq!(*active, 1);

        let PaneNode::Split {
            axis: SplitAxis::Vertical,
            children: browser_children,
            sizes: browser_sizes,
        } = &root_children[1]
        else {
            panic!("expected alpha second child to be vertical split");
        };
        assert_eq!(browser_children.len(), 2);
        assert_ratio(browser_sizes[0], 0.34);
        assert_ratio(browser_sizes[1], 0.66);

        let beta_restored = restored_data
            .workspaces
            .iter()
            .find(|workspace| workspace.id == beta.id)
            .expect("beta restored");
        let PaneNode::Leaf { tabs, active } = &beta_restored.pane_tree else {
            panic!("expected beta to remain a tab leaf");
        };
        assert_eq!(tabs, &vec![beta_root, beta_tab.id.clone()]);
        assert_eq!(*active, 1);
        assert_eq!(beta_restored.focused_surface_id, beta_tab.id);

        let restored_browser = restored
            .surface(&browser.id)
            .expect("restored browser surface");
        assert_eq!(
            restored_browser.kind,
            SurfaceKind::Browser {
                url: "https://example.com/manual-restore".to_string(),
                profile: ProfileId::default(),
            }
        );
    }

    #[test]
    fn loads_v2_session_with_surface_id_leaf_as_single_tab_leaf() {
        // A hand-crafted v2 JSON with the old `surface_id` leaf form should be
        // migrated to the v3 `tabs` form automatically.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2-old.json");
        fs::write(
            &path,
            r#"{
              "version": 2,
              "active_workspace_id": "workspace-1",
              "workspaces": [
                {
                  "id": "workspace-1",
                  "name": "w1",
                  "active": true,
                  "working_dir": "/tmp",
                  "git_branch": "",
                  "pane_tree": { "type": "leaf", "surface_id": "surface-1" },
                  "focused_surface_id": "surface-1",
                  "needs_attention": false
                }
              ]
            }"#,
        )
        .unwrap();

        let loaded = load_session_from_path(&path).unwrap().unwrap();

        assert_eq!(loaded.version, SESSION_FORMAT_VERSION);
        assert_eq!(loaded.active_workspace_id.as_deref(), Some("workspace-1"));
        let pane_tree = &loaded.workspaces[0].pane_tree;
        let tabs = pane_tree.leaf_tabs().expect("root must be a leaf");
        assert_eq!(tabs, &["surface-1"]);
        let active = pane_tree.leaf_active_id().expect("active tab must exist");
        assert_eq!(active, "surface-1");
    }

    #[test]
    fn loads_v2_session_with_split_pane_tree() {
        // A v2 session that already has a split pane tree should also migrate
        // correctly: each leaf becomes a single-tab leaf.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-v2-split.json");
        fs::write(
            &path,
            r#"{
              "version": 2,
              "active_workspace_id": "workspace-1",
              "workspaces": [
                {
                  "id": "workspace-1",
                  "name": "w1",
                  "active": true,
                  "working_dir": "/tmp",
                  "git_branch": "",
                  "pane_tree": {
                    "type": "split",
                    "axis": "horizontal",
                    "sizes": [0.5, 0.5],
                    "children": [
                      { "type": "leaf", "surface_id": "surface-1" },
                      { "type": "leaf", "surface_id": "surface-2" }
                    ]
                  },
                  "focused_surface_id": "surface-1",
                  "needs_attention": false
                }
              ]
            }"#,
        )
        .unwrap();

        let loaded = load_session_from_path(&path).unwrap().unwrap();

        assert_eq!(loaded.version, SESSION_FORMAT_VERSION);
        let PaneNode::Split { ref children, .. } = loaded.workspaces[0].pane_tree else {
            panic!("expected split root");
        };
        assert_eq!(children.len(), 2);
        for (child, expected_id) in children.iter().zip(["surface-1", "surface-2"]) {
            let tabs = child.leaf_tabs().expect("child must be leaf");
            assert_eq!(tabs, &[expected_id]);
        }
    }

    #[test]
    fn rejects_session_with_empty_tabs_leaf() {
        let mut model = WorkspaceModel::new();
        model.create_workspace("main", "/tmp");
        let mut data = model.to_session_data();
        // Forge an invalid leaf with empty tabs.
        data.workspaces[0].pane_tree = PaneNode::Leaf {
            tabs: Vec::new(),
            active: 0,
        };

        assert!(matches!(
            validate_session_data(&data),
            Err(SessionError::InvalidData(_))
        ));
    }
}
