use forktty_core::{
    config, dispatch_notification, worktree, JsonRpcRequest, JsonRpcResponse, LogLevel,
    NotificationKind, SplitAxis, WorkspaceModel, WorkspaceSelector,
};
use forktty_terminal::{SharedTerminalBackend, SpawnRequest};
use serde_json::{json, Value};
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

const MAX_REQUEST_SIZE: usize = 1_048_576;
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(250);

#[derive(Error, Debug)]
pub enum SocketError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Lock poisoned")]
    LockPoisoned,
}

#[derive(Clone)]
pub struct SocketAppState {
    pub model: Arc<Mutex<WorkspaceModel>>,
    pub terminal: SharedTerminalBackend,
    pub shell: String,
    pub socket_path: PathBuf,
    pub notification_dispatch: bool,
}

impl SocketAppState {
    pub fn new(
        model: Arc<Mutex<WorkspaceModel>>,
        terminal: SharedTerminalBackend,
        shell: impl Into<String>,
        socket_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            model,
            terminal,
            shell: shell.into(),
            socket_path: socket_path.into(),
            notification_dispatch: true,
        }
    }

    pub fn with_notification_dispatch(mut self, enabled: bool) -> Self {
        self.notification_dispatch = enabled;
        self
    }
}

pub fn default_socket_path() -> PathBuf {
    default_socket_dir().join("forktty.sock")
}

pub fn bind_socket_listener(
    socket_path: impl AsRef<Path>,
    enforce_private_parent: bool,
) -> io::Result<StdUnixListener> {
    let socket_path = socket_path.as_ref();
    prepare_socket_parent(socket_path, enforce_private_parent)?;
    if socket_path.exists() {
        let metadata = fs::symlink_metadata(socket_path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!(
                    "refusing to replace non-socket path at {}",
                    socket_path.display()
                ),
            ));
        }
        match inspect_existing_socket(socket_path) {
            ExistingSocketOccupant::ForkTTY => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!(
                        "another ForkTTY instance is already using {}",
                        socket_path.display()
                    ),
                ));
            }
            ExistingSocketOccupant::Other => {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("socket path {} is already in use", socket_path.display()),
                ));
            }
            ExistingSocketOccupant::Stale => fs::remove_file(socket_path)?,
        }
    }
    let listener = StdUnixListener::bind(socket_path)?;
    listener.set_nonblocking(true)?;
    if let Err(err) = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(socket_path);
        return Err(err);
    }
    Ok(listener)
}

pub async fn serve(listener: StdUnixListener, state: SocketAppState) -> Result<(), SocketError> {
    let listener = UnixListener::from_std(listener)?;
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            let _ = handle_connection(stream, state).await;
        });
    }
}

pub async fn dispatch(
    state: &SocketAppState,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    match method {
        "system.ping" => Ok(json!("pong")),
        "workspace.list" => {
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            Ok(json!(model.list_workspaces()))
        }
        "workspace.create" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("workspace");
            let cwd = resolve_workspace_cwd_param(&params)?;
            let workspace = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.create_workspace(name, cwd)
            };
            state
                .terminal
                .spawn(SpawnRequest {
                    surface_id: workspace.focused_surface_id.clone(),
                    workspace_id: workspace.id.clone(),
                    shell: state.shell.clone(),
                    cwd: workspace.working_dir.clone(),
                    socket_path: state.socket_path.clone(),
                    extra_env: Vec::new(),
                })
                .map_err(|err| err.to_string())?;
            Ok(json!(workspace))
        }
        "workspace.select" => {
            let selector = workspace_selector_from_params(&params)?;
            let workspace = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .select_workspace(selector)
                    .ok_or_else(|| "Workspace not found".to_string())?
            };
            Ok(json!(workspace))
        }
        "workspace.close" => {
            let selector = workspace_selector_from_params(&params)?;
            let (workspace, surface_ids) = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let workspace_id = match selector {
                    WorkspaceSelector::Id(id) => id.to_string(),
                    WorkspaceSelector::Name(name) => model
                        .list_workspaces()
                        .into_iter()
                        .find(|workspace| workspace.name == name)
                        .map(|workspace| workspace.id)
                        .ok_or_else(|| "Workspace not found".to_string())?,
                    WorkspaceSelector::WorktreeName(name) => model
                        .list_workspaces()
                        .into_iter()
                        .find(|workspace| workspace.worktree_name.as_deref() == Some(name))
                        .map(|workspace| workspace.id)
                        .ok_or_else(|| "Workspace not found".to_string())?,
                };
                let surface_ids = model
                    .list_surfaces(Some(&workspace_id))
                    .into_iter()
                    .map(|surface| surface.id)
                    .collect::<Vec<_>>();
                let workspace = model
                    .close_workspace(WorkspaceSelector::Id(&workspace_id))
                    .ok_or_else(|| "Workspace not found".to_string())?;
                if model.list_workspaces().is_empty() {
                    model.create_workspace(
                        "main",
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
                    );
                }
                (workspace, surface_ids)
            };
            for surface_id in surface_ids {
                let _ = state.terminal.close(&surface_id);
            }
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!(workspace))
        }
        "worktree.list" => {
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"])?;
            let worktrees = worktree::list(&cwd).map_err(|err| err.to_string())?;
            Ok(json!(worktrees))
        }
        "worktree.status" => {
            let path = resolve_open_repo_cwd_param(state, &params, &["path", "cwd"])?;
            let status = worktree::status(&path).map_err(|err| err.to_string())?;
            Ok(json!({"status": status}))
        }
        "worktree.create" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing name".to_string())?;
            let name = validate_worktree_name_param(name)?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"])?;
            let layout = worktree_layout();
            let info = worktree::create(&cwd, name, &layout).map_err(|err| err.to_string())?;
            let workspace = open_worktree_workspace(state, &info).await?;
            Ok(json!({
                "id": workspace.id,
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
            }))
        }
        "worktree.attach" => {
            let name = params
                .get("name")
                .or_else(|| params.get("branch"))
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing name".to_string())?;
            let name = validate_worktree_name_param(name)?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"])?;
            let layout = worktree_layout();
            let info = worktree::attach(&cwd, name, &layout).map_err(|err| err.to_string())?;
            let workspace = open_worktree_workspace(state, &info).await?;
            Ok(json!({
                "id": workspace.id,
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
            }))
        }
        "worktree.remove" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing name".to_string())?;
            let name = validate_worktree_name_param(name)?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"])?;
            let fallback_path =
                worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
            let mut workspace_worktree_name = name.to_string();
            if let Ok(existing) = worktree::list(&cwd) {
                if let Some(info) = existing
                    .iter()
                    .find(|info| info.worktree_name == name || info.branch == name)
                {
                    workspace_worktree_name = info.worktree_name.clone();
                }
            }
            worktree::remove(&cwd, name, true).map_err(|err| err.to_string())?;
            let surface_ids = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let workspace_id = model
                    .list_workspaces()
                    .into_iter()
                    .find(|workspace| {
                        workspace.worktree_name.as_deref() == Some(workspace_worktree_name.as_str())
                    })
                    .map(|workspace| workspace.id);
                let surface_ids = workspace_id
                    .as_deref()
                    .map(|workspace_id| {
                        model
                            .list_surfaces(Some(workspace_id))
                            .into_iter()
                            .map(|surface| surface.id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let _ = model.close_workspace(WorkspaceSelector::WorktreeName(
                    workspace_worktree_name.as_str(),
                ));
                if model.list_workspaces().is_empty() {
                    model.create_workspace("main", fallback_path);
                }
                surface_ids
            };
            for surface_id in surface_ids {
                let _ = state.terminal.close(&surface_id);
            }
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!({"removed": name}))
        }
        "worktree.merge" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing name".to_string())?;
            let name = validate_worktree_name_param(name)?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"])?;
            let result = worktree::merge(&cwd, name).map_err(|err| err.to_string())?;
            Ok(json!(result))
        }
        "surface.list" => {
            let workspace_id = params.get("workspace_id").and_then(Value::as_str);
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            Ok(json!(model.list_surfaces(workspace_id)))
        }
        "surface.send_text" => {
            let surface_id = params
                .get("surface_id")
                .or_else(|| params.get("surfaceId"))
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing surface_id".to_string())?;
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing text".to_string())?;
            state
                .terminal
                .send_text(surface_id, text)
                .map_err(|err| err.to_string())?;
            Ok(json!({"sent": true}))
        }
        "surface.split" => {
            let surface_id = params
                .get("surface_id")
                .or_else(|| params.get("surfaceId"))
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing surface_id".to_string())?;
            let axis = match params.get("axis").and_then(Value::as_str) {
                Some("vertical") => SplitAxis::Vertical,
                _ => SplitAxis::Horizontal,
            };
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .split_surface(surface_id, axis)
                    .ok_or_else(|| "Surface not found".to_string())?
            };
            state
                .terminal
                .spawn(SpawnRequest {
                    surface_id: surface.id.clone(),
                    workspace_id: surface.workspace_id.clone(),
                    shell: state.shell.clone(),
                    cwd: surface.cwd.clone(),
                    socket_path: state.socket_path.clone(),
                    extra_env: Vec::new(),
                })
                .map_err(|err| err.to_string())?;
            Ok(json!(surface))
        }
        "surface.focus" => {
            let surface_id = params
                .get("surface_id")
                .or_else(|| params.get("surfaceId"))
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing surface_id".to_string())?;
            let focused = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.focus_surface(surface_id)
            };
            if focused {
                Ok(json!({"focused": true}))
            } else {
                Err("Surface not found".to_string())
            }
        }
        "surface.close" => {
            let surface_id = params
                .get("surface_id")
                .or_else(|| params.get("surfaceId"))
                .and_then(Value::as_str)
                .ok_or_else(|| "Missing surface_id".to_string())?;
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .close_surface(surface_id)
                    .ok_or_else(|| "Surface not found".to_string())?
            };
            state
                .terminal
                .close(surface_id)
                .map_err(|err| err.to_string())?;
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!(surface))
        }
        "notification.create" => {
            let title = params
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("ForkTTY");
            let body = params.get("body").and_then(Value::as_str).unwrap_or("");
            let kind = match params.get("kind").and_then(Value::as_str) {
                Some("prompt") => NotificationKind::Prompt,
                Some("error") => NotificationKind::Error,
                Some("custom") => NotificationKind::Custom,
                _ => NotificationKind::Info,
            };
            let workspace_id = params
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(String::from);
            let surface_id = params
                .get("surface_id")
                .or_else(|| params.get("surfaceId"))
                .and_then(Value::as_str)
                .map(String::from);
            let item = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.create_notification(title, body, kind, workspace_id, surface_id)
            };
            if state.notification_dispatch {
                dispatch_notification_with_loaded_config(&item);
            }
            Ok(json!(item))
        }
        "notification.list" => {
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            Ok(json!(model.list_notifications()))
        }
        "notification.clear" => {
            let mut model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            model.clear_notifications();
            Ok(json!({"cleared": true}))
        }
        "metadata.set_status" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let key = required_string(&params, "key")?;
            let label = required_string(&params, "label")?;
            let value = required_string(&params, "value")?;
            let color = params
                .get("color")
                .and_then(Value::as_str)
                .filter(|color| !color.trim().is_empty())
                .map(String::from);
            let status = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .set_status(&workspace_id, key, label, value, color)
                    .ok_or_else(|| "Workspace not found".to_string())?
            };
            Ok(json!(status))
        }
        "metadata.list_status" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let statuses = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.list_status(&workspace_id)
            };
            Ok(json!(statuses))
        }
        "metadata.clear_status" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let key = params
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| !key.trim().is_empty());
            let cleared = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.clear_status(&workspace_id, key)
            };
            if cleared {
                Ok(json!({"cleared": true}))
            } else {
                Err("Workspace not found".to_string())
            }
        }
        "metadata.set_progress" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let key = required_string(&params, "key")?;
            let label = required_string(&params, "label")?;
            let value = required_f64(&params, "value")?;
            let total = optional_f64(&params, "total")?;
            if total.is_some_and(|total| total <= 0.0) {
                return Err("Invalid parameter total: expected positive number".to_string());
            }
            let progress = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .set_progress(&workspace_id, key, label, value, total)
                    .ok_or_else(|| "Workspace not found".to_string())?
            };
            Ok(json!(progress))
        }
        "metadata.list_progress" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let progress = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.list_progress(&workspace_id)
            };
            Ok(json!(progress))
        }
        "metadata.clear_progress" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let key = params
                .get("key")
                .and_then(Value::as_str)
                .filter(|key| !key.trim().is_empty());
            let cleared = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.clear_progress(&workspace_id, key)
            };
            if cleared {
                Ok(json!({"cleared": true}))
            } else {
                Err("Workspace not found".to_string())
            }
        }
        "metadata.log" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let level = match params
                .get("level")
                .and_then(Value::as_str)
                .unwrap_or("info")
            {
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                "info" | "" => LogLevel::Info,
                _ => {
                    return Err("Invalid parameter level: expected info, warn, or error".to_string())
                }
            };
            let message = required_string(&params, "message")?;
            let log = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .append_log(&workspace_id, level, message)
                    .ok_or_else(|| "Workspace not found".to_string())?
            };
            Ok(json!(log))
        }
        "metadata.list_logs" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let logs = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.list_logs(&workspace_id)
            };
            Ok(json!(logs))
        }
        "metadata.clear_logs" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let cleared = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.clear_logs(&workspace_id)
            };
            if cleared {
                Ok(json!({"cleared": true}))
            } else {
                Err("Workspace not found".to_string())
            }
        }
        _ => Err(format!("Unknown method: {method}")),
    }
}

fn dispatch_notification_with_loaded_config(notification: &forktty_core::NotificationItem) {
    let config = config::load_config().unwrap_or_default();
    for error in dispatch_notification(&config, notification) {
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}

async fn open_worktree_workspace(
    state: &SocketAppState,
    info: &worktree::WorktreeInfo,
) -> Result<forktty_core::Workspace, String> {
    let workspace = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        )
    };
    state
        .terminal
        .spawn(SpawnRequest {
            surface_id: workspace.focused_surface_id.clone(),
            workspace_id: workspace.id.clone(),
            shell: state.shell.clone(),
            cwd: workspace.working_dir.clone(),
            socket_path: state.socket_path.clone(),
            extra_env: Vec::new(),
        })
        .map_err(|err| err.to_string())?;
    Ok(workspace)
}

async fn ensure_terminal_for_active_workspace(state: &SocketAppState) -> Result<(), String> {
    let workspace = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.active)
            .or_else(|| model.list_workspaces().into_iter().next())
    };
    let Some(workspace) = workspace else {
        return Ok(());
    };
    state
        .terminal
        .spawn(SpawnRequest {
            surface_id: workspace.focused_surface_id.clone(),
            workspace_id: workspace.id.clone(),
            shell: state.shell.clone(),
            cwd: workspace.working_dir.clone(),
            socket_path: state.socket_path.clone(),
            extra_env: Vec::new(),
        })
        .map_err(|err| err.to_string())
}

fn resolve_workspace_cwd_param(params: &Value) -> Result<PathBuf, String> {
    resolve_existing_dir_param(params, &["workingDir", "working_dir", "cwd"])
}

#[cfg(test)]
fn resolve_cwd_param(params: &Value) -> Result<String, String> {
    Ok(resolve_existing_dir_param(params, &["cwd"])?
        .to_string_lossy()
        .to_string())
}

fn resolve_open_repo_cwd_param(
    state: &SocketAppState,
    params: &Value,
    keys: &[&str],
) -> Result<String, String> {
    let cwd = resolve_existing_dir_param(params, keys)?;
    validate_socket_cwd_against_open_workspaces(state, &cwd)?;
    Ok(cwd.to_string_lossy().to_string())
}

fn resolve_existing_dir_param(params: &Value, keys: &[&str]) -> Result<PathBuf, String> {
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        let raw = value
            .as_str()
            .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
        if raw.trim().is_empty() {
            return Err(format!("Invalid parameter {key}: path must not be empty"));
        }
        return canonical_existing_dir(Path::new(raw), key);
    }
    Ok(fallback_cwd())
}

fn canonical_existing_dir(path: &Path, key: &str) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|err| format!("Invalid parameter {key}: cannot resolve path: {err}"))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|err| format!("Invalid parameter {key}: cannot read path metadata: {err}"))?;
    if !metadata.is_dir() {
        return Err(format!("Invalid parameter {key}: path must be a directory"));
    }
    Ok(canonical)
}

fn validate_socket_cwd_against_open_workspaces(
    state: &SocketAppState,
    cwd: &Path,
) -> Result<(), String> {
    let candidate = canonical_repo_common_dir(cwd)?;
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let allowed = model
        .list_workspaces()
        .into_iter()
        .filter_map(|workspace| canonical_repo_common_dir(&workspace.working_dir).ok())
        .any(|open_repo| open_repo == candidate);
    if allowed {
        Ok(())
    } else {
        Err("cwd must be inside the git repository of an open workspace".to_string())
    }
}

fn canonical_repo_common_dir(path: &Path) -> Result<PathBuf, String> {
    let repo = git2::Repository::discover(path)
        .map_err(|_| format!("cwd must be inside a git repository: {}", path.display()))?;
    fs::canonicalize(repo.commondir())
        .map_err(|err| format!("Cannot resolve git common directory: {err}"))
}

fn fallback_cwd() -> PathBuf {
    std::env::current_dir()
        .ok()
        .and_then(|path| canonical_existing_dir(&path, "cwd").ok())
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn validate_worktree_name_param(name: &str) -> Result<&str, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Invalid worktree name: must not be empty".to_string());
    }
    if trimmed.len() > 255 {
        return Err("Invalid worktree name: must be 255 bytes or fewer".to_string());
    }
    if trimmed.contains('\0') || trimmed.contains('\\') {
        return Err("Invalid worktree name: contains unsupported characters".to_string());
    }
    if trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err("Invalid worktree name: contains an unsafe path segment".to_string());
    }
    Ok(trimmed)
}

fn worktree_layout() -> String {
    config::load_config()
        .ok()
        .map(|config| config.general.worktree_layout)
        .filter(|layout| !layout.is_empty())
        .unwrap_or_else(|| "nested".to_string())
}

fn required_string<'a>(params: &'a Value, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("Missing {key}"))
}

fn required_f64(params: &Value, key: &str) -> Result<f64, String> {
    optional_f64(params, key)?.ok_or_else(|| format!("Missing {key}"))
}

fn optional_f64(params: &Value, key: &str) -> Result<Option<f64>, String> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .ok_or_else(|| format!("Invalid parameter {key}: expected finite number"))?;
    if !value.is_finite() {
        return Err(format!("Invalid parameter {key}: expected finite number"));
    }
    Ok(Some(value))
}

fn resolve_workspace_id_for_metadata(
    state: &SocketAppState,
    params: &Value,
) -> Result<String, String> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if let Ok(selector) = workspace_selector_from_params(params) {
        return match selector {
            WorkspaceSelector::Id(id) => model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.id == id)
                .map(|workspace| workspace.id)
                .ok_or_else(|| "Workspace not found".to_string()),
            WorkspaceSelector::Name(name) => model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.name == name)
                .map(|workspace| workspace.id)
                .ok_or_else(|| "Workspace not found".to_string()),
            WorkspaceSelector::WorktreeName(name) => model
                .list_workspaces()
                .into_iter()
                .find(|workspace| workspace.worktree_name.as_deref() == Some(name))
                .map(|workspace| workspace.id)
                .ok_or_else(|| "Workspace not found".to_string()),
        };
    }
    model
        .active_workspace_id()
        .ok_or_else(|| "Workspace not found".to_string())
}

fn workspace_selector_from_params(params: &Value) -> Result<WorkspaceSelector<'_>, String> {
    if let Some(id) = params.get("id").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Id(id));
    }
    if let Some(id) = params.get("workspace_id").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Id(id));
    }
    if let Some(id) = params.get("workspaceId").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Id(id));
    }
    if let Some(name) = params.get("name").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Name(name));
    }
    if let Some(name) = params.get("workspace_name").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Name(name));
    }
    if let Some(name) = params.get("workspaceName").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Name(name));
    }
    if let Some(worktree_name) = params
        .get("worktreeName")
        .or_else(|| params.get("worktree_name"))
        .and_then(Value::as_str)
    {
        return Ok(WorkspaceSelector::WorktreeName(worktree_name));
    }
    Err("Missing workspace selector".to_string())
}

pub fn bootstrap_default_workspace(state: &SocketAppState, cwd: PathBuf) -> Result<(), String> {
    let workspace = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if let Some(existing) = model.list_workspaces().into_iter().next() {
            existing
        } else {
            model.create_workspace("main", cwd)
        }
    };
    state
        .terminal
        .spawn(SpawnRequest {
            surface_id: workspace.focused_surface_id,
            workspace_id: workspace.id,
            shell: state.shell.clone(),
            cwd: workspace.working_dir,
            socket_path: state.socket_path.clone(),
            extra_env: Vec::new(),
        })
        .map_err(|err| err.to_string())
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
) -> Result<(), SocketError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let line = match read_limited_line(&mut reader, MAX_REQUEST_SIZE).await {
            None => break,
            Some(Err(ReadLineError::TooLarge)) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "request_too_large",
                    "Request exceeds 1 MiB",
                );
                write_response(&mut writer, &response).await?;
                break;
            }
            Some(Err(ReadLineError::InvalidUtf8)) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "parse_error",
                    "Request must be valid UTF-8 JSON",
                );
                write_response(&mut writer, &response).await?;
                break;
            }
            Some(Ok(line)) => line,
        };
        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = JsonRpcResponse::error(Value::Null, "parse_error", err.to_string());
                write_response(&mut writer, &response).await?;
                continue;
            }
        };
        let id = request.id.clone();
        let response = match dispatch(&state, &request.method, request.params).await {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(message) => JsonRpcResponse::error(id, "error", message),
        };
        write_response(&mut writer, &response).await?;
    }
    Ok(())
}

async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &JsonRpcResponse,
) -> Result<(), io::Error> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

#[derive(Debug, PartialEq, Eq)]
enum ReadLineError {
    TooLarge,
    InvalidUtf8,
}

async fn read_limited_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
) -> Option<Result<String, ReadLineError>> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let available = reader.fill_buf().await.ok()?;
        if available.is_empty() {
            return if buf.is_empty() {
                None
            } else {
                Some(String::from_utf8(buf).map_err(|_| ReadLineError::InvalidUtf8))
            };
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        }
        let len = available.len();
        if buf.len() + len > max_size {
            return Some(Err(ReadLineError::TooLarge));
        }
        buf.extend_from_slice(available);
        reader.consume(len);
    }
    if buf.len() > max_size {
        return Some(Err(ReadLineError::TooLarge));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    Some(String::from_utf8(buf).map_err(|_| ReadLineError::InvalidUtf8))
}

enum ExistingSocketOccupant {
    Stale,
    ForkTTY,
    Other,
}

fn inspect_existing_socket(path: &Path) -> ExistingSocketOccupant {
    match StdUnixStream::connect(path) {
        Ok(stream) => match probe_forktty_socket(stream) {
            Ok(true) => ExistingSocketOccupant::ForkTTY,
            Ok(false) | Err(_) => ExistingSocketOccupant::Other,
        },
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            ExistingSocketOccupant::Stale
        }
        Err(_) => ExistingSocketOccupant::Other,
    }
}

fn probe_forktty_socket(mut stream: StdUnixStream) -> io::Result<bool> {
    use std::io::{BufRead, BufReader as StdBufReader, Write};
    stream.set_read_timeout(Some(SOCKET_PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_PROBE_TIMEOUT))?;
    stream.write_all(br#"{"id":"probe","method":"system.ping","params":{}}"#)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut reader = StdBufReader::new(stream);
    let mut response = String::new();
    if reader.read_line(&mut response)? == 0 {
        return Ok(false);
    }
    let value: Value = serde_json::from_str(response.trim_end())?;
    Ok(value.get("ok").and_then(Value::as_bool) == Some(true)
        && value.get("result").and_then(Value::as_str) == Some("pong"))
}

fn prepare_socket_parent(socket_path: &Path, enforce_private_parent: bool) -> io::Result<()> {
    let parent = socket_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket path has no parent: {}", socket_path.display()),
        )
    })?;
    if !parent.exists() {
        let mut builder = DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(parent)?;
    }
    if enforce_private_parent {
        validate_private_socket_parent(parent)?;
    }
    Ok(())
}

fn validate_private_socket_parent(path: &Path) -> io::Result<()> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket parent is not a directory: {}", path.display()),
        ));
    }
    if metadata.uid() != effective_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "socket parent {} is owned by uid {}, expected {}",
                path.display(),
                metadata.uid(),
                effective_uid()
            ),
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "socket parent {} must not be accessible by group/other (mode {:o})",
                path.display(),
                mode
            ),
        ));
    }
    Ok(())
}

fn default_socket_dir() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime_dir);
        if path.is_absolute() {
            return path;
        }
    }
    std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
}

fn effective_uid() -> u32 {
    unsafe { libc::geteuid() as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_terminal::{HeadlessTerminalBackend, TerminalBackend};
    use git2::Repository;
    use std::fs;
    use std::sync::Arc;
    use tokio::io::BufReader;

    fn test_state() -> (SocketAppState, Arc<HeadlessTerminalBackend>) {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        (state, backend)
    }

    fn make_temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        fs::write(dir.path().join("note.txt"), "base\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("note.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        drop(repo);
        dir
    }

    #[tokio::test]
    async fn dispatches_minimum_socket_methods_directly() {
        let (state, backend) = test_state();
        assert_eq!(
            dispatch(&state, "system.ping", json!({})).await.unwrap(),
            json!("pong")
        );

        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": "echo ok\n"}),
        )
        .await
        .unwrap();
        assert_eq!(backend.sent_text(surface_id).unwrap(), vec!["echo ok\n"]);

        let notification = dispatch(
            &state,
            "notification.create",
            json!({"title": "Prompt", "body": "Ready", "surface_id": surface_id}),
        )
        .await
        .unwrap();
        assert_eq!(notification["title"], "Prompt");
        dispatch(&state, "notification.clear", json!({}))
            .await
            .unwrap();
        assert!(dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty());

        let status = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspaces[0]["id"],
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "color": "blue"
            }),
        )
        .await
        .unwrap();
        assert_eq!(status["value"], "Running");

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(statuses.as_array().unwrap().len(), 1);

        dispatch(
            &state,
            "metadata.clear_status",
            json!({"workspace_id": workspaces[0]["id"], "key": "agent:codex"}),
        )
        .await
        .unwrap();
        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());

        let progress = dispatch(
            &state,
            "metadata.set_progress",
            json!({
                "workspace_id": workspaces[0]["id"],
                "key": "build",
                "label": "Build",
                "value": 12,
                "total": 10
            }),
        )
        .await
        .unwrap();
        assert_eq!(progress["value"], 10.0);
        let progress_entries = dispatch(
            &state,
            "metadata.list_progress",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(progress_entries.as_array().unwrap().len(), 1);

        let log = dispatch(
            &state,
            "metadata.log",
            json!({
                "workspace_id": workspaces[0]["id"],
                "level": "warn",
                "message": "waiting"
            }),
        )
        .await
        .unwrap();
        assert_eq!(log["level"], "warn");
        let logs = dispatch(
            &state,
            "metadata.list_logs",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(logs.as_array().unwrap().len(), 1);

        dispatch(
            &state,
            "metadata.clear_progress",
            json!({"workspace_id": workspaces[0]["id"], "key": "build"}),
        )
        .await
        .unwrap();
        let progress_entries = dispatch(
            &state,
            "metadata.list_progress",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert!(progress_entries.as_array().unwrap().is_empty());

        dispatch(
            &state,
            "metadata.clear_logs",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        let logs = dispatch(
            &state,
            "metadata.list_logs",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert!(logs.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatches_workspace_and_surface_parity_methods() {
        let (state, backend) = test_state();
        let created = dispatch(
            &state,
            "workspace.create",
            json!({"name": "feature", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        assert_eq!(created["name"], "feature");
        let feature_surface_id = created["focused_surface_id"].as_str().unwrap();

        let selected = dispatch(&state, "workspace.select", json!({"name": "main"}))
            .await
            .unwrap();
        assert_eq!(selected["name"], "main");

        let surface_id = selected["focused_surface_id"].as_str().unwrap();
        let split = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap();
        let split_id = split["id"].as_str().unwrap();
        dispatch(&state, "surface.focus", json!({"surface_id": split_id}))
            .await
            .unwrap();
        dispatch(&state, "surface.close", json!({"surface_id": split_id}))
            .await
            .unwrap();

        let closed = dispatch(&state, "workspace.close", json!({"name": "feature"}))
            .await
            .unwrap();
        assert_eq!(closed["name"], "feature");
        assert!(matches!(
            backend.sent_text(feature_surface_id),
            Err(forktty_terminal::TerminalError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn dispatches_worktree_lifecycle_methods_and_updates_workspace_model() {
        let repo_dir = make_temp_repo();
        let (state, backend) = test_state();
        dispatch(
            &state,
            "workspace.create",
            json!({"name": "repo", "workingDir": repo_dir.path()}),
        )
        .await
        .unwrap();

        let created = dispatch(
            &state,
            "worktree.create",
            json!({"name": "topic/socket", "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();

        assert_eq!(created["branch"], "topic/socket");
        assert_ne!(created["worktree_name"], "topic/socket");
        let workspace_id = created["id"].as_str().unwrap();
        let surface_id = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.workspace_id == workspace_id)
            .unwrap()
            .surface_id;
        assert!(backend
            .env(&surface_id)
            .unwrap()
            .contains(&("FORKTTY_WORKSPACE_ID".to_string(), workspace_id.to_string())));

        let listed = dispatch(&state, "worktree.list", json!({"cwd": repo_dir.path()}))
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);

        let status = dispatch(&state, "worktree.status", json!({"path": created["path"]}))
            .await
            .unwrap();
        assert_eq!(status["status"], "clean");

        dispatch(
            &state,
            "worktree.remove",
            json!({"name": "topic/socket", "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();

        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert!(!workspaces
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["git_branch"] == "topic/socket"));
        assert!(matches!(
            backend.sent_text(&surface_id),
            Err(forktty_terminal::TerminalError::NotFound(_))
        ));
    }

    #[tokio::test]
    async fn worktree_socket_rejects_unopened_repo_cwd() {
        let open_repo = make_temp_repo();
        let unopened_repo = make_temp_repo();
        let (state, _backend) = test_state();
        dispatch(
            &state,
            "workspace.create",
            json!({"name": "open", "workingDir": open_repo.path()}),
        )
        .await
        .unwrap();

        let error = dispatch(
            &state,
            "worktree.create",
            json!({"name": "blocked", "cwd": unopened_repo.path()}),
        )
        .await
        .unwrap_err();

        assert!(error.contains("open workspace"));
        assert!(worktree::list(unopened_repo.path().to_str().unwrap())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn validates_worktree_name_params() {
        assert_eq!(
            validate_worktree_name_param(" feature/x ").unwrap(),
            "feature/x"
        );
        assert!(validate_worktree_name_param("../escape").is_err());
        assert!(validate_worktree_name_param("feature//empty").is_err());
        assert!(validate_worktree_name_param("feature\\windows").is_err());
        assert!(validate_worktree_name_param("").is_err());
    }

    #[test]
    fn resolves_cwd_params_to_existing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve_workspace_cwd_param(&json!({"workingDir": dir.path()})).unwrap();
        assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());

        let missing = dir.path().join("missing");
        let error = resolve_cwd_param(&json!({"cwd": missing})).unwrap_err();
        assert!(error.contains("cannot resolve path"));
    }

    #[tokio::test]
    async fn limited_line_rejects_oversize() {
        let data = b"abcdef\n";
        let mut reader = BufReader::new(std::io::Cursor::new(data.to_vec()));
        assert_eq!(
            read_limited_line(&mut reader, 3).await,
            Some(Err(ReadLineError::TooLarge))
        );
    }
}
