use forktty_core::events::{self, ModelEvent, Snapshot};
use forktty_core::{
    config, dispatch_notification, validate_worktree_name, worktree, BrowserCmdError,
    BrowserCommand, BrowserOp, CmdResult, JsonRpcRequest, JsonRpcResponse, LogLevel,
    NotificationKind, SplitAxis, StatusHookMetadata, WorkspaceModel, WorkspaceSelector,
    MAX_BROWSER_SCRIPT_BYTES,
};
use forktty_terminal::{SharedTerminalBackend, SpawnRequest, TerminalError};
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
use tokio::sync::{broadcast, Semaphore};

const MAX_REQUEST_SIZE: usize = 1_048_576;
const MAX_SEND_TEXT_BYTES: usize = 262_144;
const MAX_METADATA_TEXT_BYTES: usize = 16_384;
const MAX_BROWSER_URL_BYTES: usize = 8_192;
const BROWSER_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_SOCKET_CONNECTIONS: usize = 64;
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
/// Buffered events per subscriber before a slow client gets a `Lagged` notice.
const EVENTS_CHANNEL_CAPACITY: usize = 256;
/// How often the background task snapshots the model and emits diffs.
const EVENTS_TICK: Duration = Duration::from_millis(250);

/// Methods advertised by `system.capabilities`. Every entry except
/// `events.subscribe` (handled at the connection level, not in [`dispatch`]) is
/// covered by a `dispatch` match arm; the `capabilities_lists_dispatchable`
/// test guards against an entry here that has no handler.
pub const METHODS: &[&str] = &[
    "browser.back",
    "browser.bookmark.add",
    "browser.bookmark.list",
    "browser.bookmark.remove",
    "browser.click",
    "browser.eval",
    "browser.fill",
    "browser.forward",
    "browser.history.clear",
    "browser.history.list",
    "browser.history.search",
    "browser.navigate",
    "browser.open",
    "browser.profile.create",
    "browser.profile.delete",
    "browser.profile.list",
    "browser.reload",
    "browser.snapshot",
    "events.subscribe",
    "metadata.clear_logs",
    "metadata.clear_progress",
    "metadata.clear_status",
    "metadata.list_logs",
    "metadata.list_progress",
    "metadata.list_status",
    "metadata.log",
    "metadata.set_progress",
    "metadata.set_status",
    "notification.clear",
    "notification.create",
    "notification.list",
    "surface.close",
    "surface.focus",
    "surface.list",
    "surface.send_text",
    "surface.split",
    "system.capabilities",
    "system.ping",
    "workspace.close",
    "workspace.create",
    "workspace.list",
    "workspace.select",
    "worktree.attach",
    "worktree.create",
    "worktree.list",
    "worktree.merge",
    "worktree.remove",
    "worktree.status",
];

#[derive(Error, Debug)]
pub enum SocketError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Lock poisoned")]
    LockPoisoned,
}

/// Structured error categories surfaced by [`dispatch`].
///
/// The variants map to stable string codes that clients can branch on
/// (`method_not_found`, `missing_param`, `not_found`, `payload_too_large`,
/// `not_ready`, `error`). Existing handlers that return ad-hoc `String` errors keep
/// working via the [`From<String>`] impl below; new sites should prefer the
/// structured variants so the response carries a useful `error.code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    MethodNotFound(String),
    MissingParam(&'static str),
    NotFound(String),
    PayloadTooLarge {
        field: &'static str,
        limit: usize,
        actual: usize,
    },
    Conflict(String),
    AlreadyExists(String),
    NotReady(String),
    InvalidParam(String),
    Other(String),
}

impl DispatchError {
    pub fn code(&self) -> &'static str {
        match self {
            DispatchError::MethodNotFound(_) => "method_not_found",
            DispatchError::MissingParam(_) => "missing_param",
            DispatchError::NotFound(_) => "not_found",
            DispatchError::PayloadTooLarge { .. } => "payload_too_large",
            DispatchError::Conflict(_) => "conflict",
            DispatchError::AlreadyExists(_) => "already_exists",
            DispatchError::NotReady(_) => "not_ready",
            DispatchError::InvalidParam(_) => "invalid_param",
            DispatchError::Other(_) => "error",
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DispatchError::MethodNotFound(method) => write!(f, "Unknown method: {method}"),
            DispatchError::MissingParam(name) => write!(f, "Missing {name}"),
            DispatchError::NotFound(kind) => {
                let label = match kind.as_str() {
                    "workspace" => "Workspace not found",
                    "surface" => "Surface not found",
                    message if message.starts_with("Not a git repository: ") => {
                        return f.write_str(message);
                    }
                    other => return write!(f, "{other} not found"),
                };
                f.write_str(label)
            }
            DispatchError::PayloadTooLarge {
                field,
                limit,
                actual,
            } => write!(
                f,
                "{field} payload is {actual} bytes, exceeds limit of {limit} bytes"
            ),
            DispatchError::Conflict(message) => f.write_str(message),
            DispatchError::AlreadyExists(message) => f.write_str(message),
            DispatchError::NotReady(message) => f.write_str(message),
            DispatchError::InvalidParam(message) => f.write_str(message),
            DispatchError::Other(message) => f.write_str(message),
        }
    }
}

impl From<forktty_core::WorktreeNameError> for DispatchError {
    fn from(err: forktty_core::WorktreeNameError) -> Self {
        DispatchError::InvalidParam(format!("Invalid worktree name: {err}"))
    }
}

impl From<forktty_core::worktree::WorktreeError> for DispatchError {
    fn from(err: forktty_core::worktree::WorktreeError) -> Self {
        use forktty_core::worktree::WorktreeError as W;
        match err {
            W::NotFound(name) => DispatchError::NotFound(format!("Worktree '{name}'")),
            W::BranchNotFound(name) => DispatchError::NotFound(format!("Branch '{name}'")),
            W::NotARepo(path) => DispatchError::NotFound(format!("Not a git repository: {path}")),
            W::AlreadyExists(name) => {
                DispatchError::AlreadyExists(format!("Worktree '{name}' already exists"))
            }
            W::InvalidName(inner) => DispatchError::InvalidParam(inner.to_string()),
            W::InvalidHookName(name) => {
                DispatchError::InvalidParam(format!("Invalid hook name: {name}"))
            }
            W::TargetDirty
            | W::WorktreeDirty(_)
            | W::SourceDirty(_)
            | W::MergeConflicts
            | W::UpToDate
            | W::HookOutsideWorktree
            | W::WorktreeMetadataMismatch { .. } => DispatchError::Conflict(err.to_string()),
            other => DispatchError::Other(other.to_string()),
        }
    }
}

impl From<TerminalError> for DispatchError {
    fn from(err: TerminalError) -> Self {
        match err {
            TerminalError::NotFound(_) => DispatchError::NotFound("surface".to_string()),
            TerminalError::NotReady(surface_id) => DispatchError::NotReady(format!(
                "Terminal surface is not ready to receive text: {surface_id}"
            )),
            other => DispatchError::Other(other.to_string()),
        }
    }
}

impl From<String> for DispatchError {
    fn from(message: String) -> Self {
        DispatchError::Other(message)
    }
}

impl From<&str> for DispatchError {
    fn from(message: &str) -> Self {
        DispatchError::Other(message.to_string())
    }
}

#[derive(Clone)]
pub struct SocketAppState {
    pub model: Arc<Mutex<WorkspaceModel>>,
    pub profile_store_lock: Arc<Mutex<()>>,
    pub terminal: SharedTerminalBackend,
    pub shell: String,
    pub socket_path: PathBuf,
    pub notification_dispatch: bool,
    /// Broadcast channel feeding `events.subscribe` connections. The background
    /// tick task in [`serve`] is the sole producer.
    pub events: broadcast::Sender<ModelEvent>,
    /// Sends scripting commands to the GTK WebView. `None` when no browser
    /// engine is wired (no `browser` feature, or headless), in which case the
    /// browser scripting verbs report unavailable.
    pub browser_cmd: Option<async_channel::Sender<BrowserCommand>>,
}

impl SocketAppState {
    pub fn new(
        model: Arc<Mutex<WorkspaceModel>>,
        terminal: SharedTerminalBackend,
        shell: impl Into<String>,
        socket_path: impl Into<PathBuf>,
    ) -> Self {
        let (events, _) = broadcast::channel(EVENTS_CHANNEL_CAPACITY);
        Self {
            model,
            profile_store_lock: Arc::new(Mutex::new(())),
            terminal,
            shell: shell.into(),
            socket_path: socket_path.into(),
            notification_dispatch: true,
            events,
            browser_cmd: None,
        }
    }

    pub fn with_notification_dispatch(mut self, enabled: bool) -> Self {
        self.notification_dispatch = enabled;
        self
    }

    pub fn with_browser_cmd(mut self, sender: async_channel::Sender<BrowserCommand>) -> Self {
        self.browser_cmd = Some(sender);
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
    match fs::symlink_metadata(socket_path) {
        Ok(metadata) => {
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
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
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
    spawn_event_tick(state.clone());
    let connection_limit = Arc::new(Semaphore::new(MAX_SOCKET_CONNECTIONS));
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        let Ok(permit) = connection_limit.clone().try_acquire_owned() else {
            tokio::spawn(async move {
                reject_over_capacity_connection(stream).await;
            });
            continue;
        };
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) = handle_connection(stream, state).await {
                // We can't return errors to a client whose connection has
                // already dropped, but the operator should still see the
                // underlying I/O or JSON failure on stderr.
                eprintln!("forktty socket connection ended with error: {err}");
            }
        });
    }
}

/// Background task: snapshot the model every [`EVENTS_TICK`], diff against the
/// previous snapshot, and broadcast each resulting event. Send errors (no
/// subscribers) are ignored. Ends when the server process exits.
fn spawn_event_tick(state: SocketAppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(EVENTS_TICK);
        let mut prev = current_snapshot(&state.model);
        loop {
            interval.tick().await;
            let next = current_snapshot(&state.model);
            for event in events::diff(&prev, &next) {
                let _ = state.events.send(event);
            }
            prev = next;
        }
    });
}

fn current_snapshot(model: &Arc<Mutex<WorkspaceModel>>) -> Snapshot {
    match model.lock() {
        Ok(model) => events::snapshot(&model),
        Err(_) => Snapshot::default(),
    }
}

async fn reject_over_capacity_connection(stream: tokio::net::UnixStream) {
    let (_, mut writer) = stream.into_split();
    let response = JsonRpcResponse::error(
        Value::Null,
        "server_busy",
        format!("Too many active socket connections (limit {MAX_SOCKET_CONNECTIONS})"),
    );
    if let Err(err) = write_response(&mut writer, &response).await {
        eprintln!("forktty socket busy response failed: {err}");
    }
}

pub async fn dispatch(
    state: &SocketAppState,
    method: &str,
    params: Value,
) -> Result<Value, DispatchError> {
    match method {
        "system.ping" => Ok(json!("pong")),
        "system.capabilities" => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "methods": METHODS,
        })),
        "workspace.list" => {
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            Ok(json!(model.list_workspaces()))
        }
        "workspace.create" => {
            let name = workspace_create_name_from_params(&params)?;
            let cwd = resolve_workspace_cwd_param(&params)?;
            let (workspace, previous_active_id) = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let previous_active_id = model.active_workspace_id();
                (model.create_workspace(name, cwd), previous_active_id)
            };
            if let Err(err) = spawn_workspace_terminal(state, &workspace) {
                rollback_workspace_creation(state, &workspace.id, previous_active_id)?;
                return Err(err.into());
            }
            Ok(json!(workspace))
        }
        "workspace.select" => {
            let selector = workspace_selector_from_params(&params)?;
            let (workspace, previous_active_id) = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let previous_active_id = model.active_workspace_id();
                (
                    model
                        .select_workspace(selector)
                        .ok_or(DispatchError::NotFound("workspace".to_string()))?,
                    previous_active_id,
                )
            };
            if let Err(err) = ensure_terminal_for_active_workspace(state).await {
                let mut err = err;
                if previous_active_id.as_deref() != Some(workspace.id.as_str()) {
                    if let Some(previous_active_id) = previous_active_id.as_deref() {
                        let restored = {
                            let mut model = state
                                .model
                                .lock()
                                .map_err(|_| "Lock poisoned".to_string())?;
                            model.select_workspace(WorkspaceSelector::Id(previous_active_id))
                        };
                        if restored.is_none() {
                            err = format!(
                                "{err}; failed to restore previous workspace {previous_active_id}"
                            );
                        }
                    }
                }
                return Err(err.into());
            }
            Ok(json!(workspace))
        }
        "workspace.close" => {
            let selector = workspace_selector_from_params(&params)?;
            let (workspace_id, workspace, surface_ids, is_last_workspace) = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let workspace_id = model
                    .workspace_id_for(selector)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?;
                let surface_ids = model
                    .list_surfaces(Some(&workspace_id))
                    .into_iter()
                    .map(|surface| surface.id)
                    .collect::<Vec<_>>();
                let workspace = model
                    .list_workspaces()
                    .into_iter()
                    .find(|workspace| workspace.id == workspace_id)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?;
                let is_last_workspace = model.list_workspaces().len() == 1;
                (workspace_id, workspace, surface_ids, is_last_workspace)
            };
            if is_last_workspace {
                let (replacement, previous_active_id) = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    let previous_active_id = model.active_workspace_id();
                    (
                        model.create_workspace("main", workspace.working_dir.clone()),
                        previous_active_id,
                    )
                };
                if let Err(err) = spawn_workspace_terminal(state, &replacement) {
                    rollback_workspace_creation(state, &replacement.id, previous_active_id)?;
                    return Err(err.into());
                }
                if let Err(err) = close_terminal_surfaces_if_present(state, &surface_ids) {
                    let mut err = err;
                    if let Err(cleanup_err) =
                        forget_terminal_surface_if_present(state, &replacement.focused_surface_id)
                    {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    return Err(err.into());
                }
                {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    model
                        .close_workspace(WorkspaceSelector::Id(&workspace_id))
                        .ok_or(DispatchError::NotFound("workspace".to_string()))?;
                }
                return Ok(json!(workspace));
            }
            close_terminal_surfaces_if_present(state, &surface_ids)?;
            {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .close_workspace(WorkspaceSelector::Id(&workspace_id))
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?;
            }
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!(workspace))
        }
        "worktree.list" => {
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd")?;
            let worktrees = worktree::list(&cwd).map_err(DispatchError::from)?;
            Ok(json!(worktrees))
        }
        "worktree.status" => {
            let path =
                resolve_open_repo_cwd_param(state, &params, &["path", "cwd"], "path or cwd")?;
            let status = worktree::status(&path).map_err(DispatchError::from)?;
            Ok(json!({"status": status}))
        }
        "worktree.create" => {
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd")?;
            let layout = worktree_layout();
            let info = worktree::create(&cwd, name, &layout).map_err(DispatchError::from)?;
            let workspace = match open_worktree_workspace(state, &info).await {
                Ok(workspace) => workspace,
                Err(err) => {
                    return Err(
                        rollback_created_worktree_after_spawn_failure(&cwd, &info, err).into(),
                    );
                }
            };
            Ok(json!({
                "id": workspace.id,
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
            }))
        }
        "worktree.attach" => {
            let name = worktree_name_from_params(&params, &["name", "branch"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd")?;
            let layout = worktree_layout();
            let info = worktree::attach(&cwd, name, &layout).map_err(DispatchError::from)?;
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
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd")?;
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
            worktree::remove(&cwd, name, false).map_err(DispatchError::from)?;
            let (workspace, surface_ids, is_last_workspace) = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let workspace = model.list_workspaces().into_iter().find(|workspace| {
                    workspace.worktree_name.as_deref() == Some(workspace_worktree_name.as_str())
                });
                let surface_ids = workspace
                    .as_ref()
                    .map(|workspace| {
                        model
                            .list_surfaces(Some(&workspace.id))
                            .into_iter()
                            .map(|surface| surface.id)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let is_last_workspace = workspace.is_some() && model.list_workspaces().len() == 1;
                (workspace, surface_ids, is_last_workspace)
            };
            if is_last_workspace {
                let workspace = workspace
                    .as_ref()
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?;
                let (replacement, previous_active_id) = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    let previous_active_id = model.active_workspace_id();
                    (
                        model.create_workspace("main", fallback_path.clone()),
                        previous_active_id,
                    )
                };
                if let Err(err) = spawn_workspace_terminal(state, &replacement) {
                    let mut err = err;
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    return Err(err.into());
                }
                if let Err(err) = close_terminal_surfaces_if_present(state, &surface_ids) {
                    let mut err = err;
                    if let Err(cleanup_err) =
                        forget_terminal_surface_if_present(state, &replacement.focused_surface_id)
                    {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    return Err(err.into());
                }
                {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
                }
                return Ok(json!({"removed": name}));
            }
            close_terminal_surfaces_if_present(state, &surface_ids)?;
            {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                if let Some(workspace) = workspace {
                    let _ = model.close_workspace(WorkspaceSelector::Id(&workspace.id));
                }
                if model.list_workspaces().is_empty() {
                    model.create_workspace("main", fallback_path);
                }
            }
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!({"removed": name}))
        }
        "worktree.merge" => {
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd")?;
            let result = worktree::merge(&cwd, name).map_err(DispatchError::from)?;
            Ok(json!(result))
        }
        "surface.list" => {
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let workspace_id = match workspace_selector_from_params(&params) {
                Ok(selector) => Some(
                    model
                        .workspace_id_for(selector)
                        .ok_or(DispatchError::NotFound("workspace".to_string()))?,
                ),
                Err(DispatchError::MissingParam(_)) => None,
                Err(err) => return Err(err),
            };
            Ok(json!(model.list_surfaces(workspace_id.as_deref())))
        }
        "surface.send_text" => {
            let surface_id = required_surface_id(&params)?;
            let text = required_string_param(&params, "text")?;
            if text.is_empty() {
                return Err("Invalid parameter text: must not be empty".into());
            }
            if text.len() > MAX_SEND_TEXT_BYTES {
                return Err(DispatchError::PayloadTooLarge {
                    field: "text",
                    limit: MAX_SEND_TEXT_BYTES,
                    actual: text.len(),
                });
            }
            ensure_model_surface_exists(state, surface_id)?;
            state
                .terminal
                .send_text(surface_id, text)
                .map_err(DispatchError::from)?;
            Ok(json!({"sent": true}))
        }
        "surface.split" => {
            let surface_id = required_surface_id(&params)?;
            let axis = split_axis_from_params(&params)?;
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .split_surface(surface_id, axis)
                    .ok_or(DispatchError::NotFound("surface".to_string()))?
            };
            if let Err(err) = spawn_surface_terminal(state, &surface) {
                rollback_surface_creation(state, &surface.id)?;
                return Err(err.into());
            }
            Ok(json!(surface))
        }
        "browser.open" => {
            let workspace_id = required_string_param(&params, "workspace_id")?.to_string();
            let url = required_browser_url(&params)?;
            let axis = split_axis_from_params(&params)?;
            let surface = {
                let _profile_store_guard = state
                    .profile_store_lock
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let profile = match params.get("profile") {
                    Some(value) => {
                        let profile_name = value.as_str().ok_or_else(|| {
                            DispatchError::InvalidParam(
                                "Invalid parameter profile: expected string".to_string(),
                            )
                        })?;
                        profiles_store()?
                            .resolve(profile_name)
                            .ok_or(DispatchError::NotFound("profile".to_string()))?
                    }
                    None => forktty_core::ProfileId::default(),
                };
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .open_browser(&workspace_id, &url, profile, axis)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
            };
            Ok(json!(surface))
        }
        "browser.navigate" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let url = required_browser_url(&params)?;
            let updated = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.set_surface_url(&surface_id, &url)
            };
            if updated {
                Ok(json!({"navigated": true}))
            } else {
                Err(DispatchError::NotFound("surface".to_string()))
            }
        }
        "browser.snapshot" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Snapshot).await
        }
        "browser.click" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let reference = required_string_param(&params, "ref")?.to_string();
            if reference.is_empty() {
                return Err("Invalid parameter ref: must not be empty".into());
            }
            dispatch_browser_cmd(state, surface_id, BrowserOp::Click { reference }).await
        }
        "browser.fill" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let reference = required_string_param(&params, "ref")?.to_string();
            if reference.is_empty() {
                return Err("Invalid parameter ref: must not be empty".into());
            }
            let value = required_string_param(&params, "value")?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Fill { reference, value }).await
        }
        "browser.eval" => {
            let surface_id = required_surface_id(&params)?.to_string();
            let script = required_string_param(&params, "script")?.to_string();
            if script.is_empty() {
                return Err("Invalid parameter script: must not be empty".into());
            }
            if script.len() > MAX_BROWSER_SCRIPT_BYTES {
                return Err(DispatchError::PayloadTooLarge {
                    field: "script",
                    limit: MAX_BROWSER_SCRIPT_BYTES,
                    actual: script.len(),
                });
            }
            dispatch_browser_cmd(state, surface_id, BrowserOp::Eval { script }).await
        }
        "browser.back" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Back).await
        }
        "browser.forward" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Forward).await
        }
        "browser.reload" => {
            let surface_id = required_surface_id(&params)?.to_string();
            dispatch_browser_cmd(state, surface_id, BrowserOp::Reload).await
        }
        "browser.profile.list" => {
            let _profile_store_guard = state
                .profile_store_lock
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let store = profiles_store()?;
            let out: Vec<_> = store
                .list()
                .iter()
                .map(|p| {
                    json!({
                        "id": p.id.to_string(),
                        "display_name": p.display_name,
                        "is_default": p.is_default,
                    })
                })
                .collect();
            Ok(json!(out))
        }
        "browser.profile.create" => {
            let display_name = required_string_param(&params, "display_name")?.to_string();
            if display_name.trim().is_empty() {
                return Err(DispatchError::InvalidParam(
                    "display_name must not be empty".to_string(),
                ));
            }
            let _profile_store_guard = state
                .profile_store_lock
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let mut store = profiles_store()?;
            let meta = store
                .create(&display_name)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "id": meta.id.to_string(), "display_name": meta.display_name }))
        }
        "browser.profile.delete" => {
            let id_str = required_string_param(&params, "id")?.to_string();
            let id: forktty_core::ProfileId = id_str
                .parse()
                .map_err(|_| DispatchError::NotFound("profile".to_string()))?;
            let _profile_store_guard = state
                .profile_store_lock
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let in_use = model.list_surfaces(None).iter().any(|s| {
                    matches!(
                        &s.kind,
                        forktty_core::SurfaceKind::Browser { profile, .. } if *profile == id
                    )
                });
                if in_use {
                    return Err(DispatchError::Conflict(
                        "profile in use by an open browser pane".to_string(),
                    ));
                }
            }
            let mut store = profiles_store()?;
            // on-disk data dir cleanup deferred to the GUI profile manager (P4)
            store.delete(&id).map_err(|e| match e {
                forktty_core::ProfileError::NotFound => {
                    DispatchError::NotFound("profile".to_string())
                }
                forktty_core::ProfileError::CannotDeleteDefault => {
                    DispatchError::Conflict("the default profile cannot be deleted".to_string())
                }
                other => DispatchError::from(other.to_string()),
            })?;
            Ok(json!({ "deleted": true }))
        }
        "browser.history.list" => {
            let profile = resolve_profile_param(&params)?;
            let limit = history_limit_from_params(&params);
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let rows = store
                .list(limit)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(rows))
        }
        "browser.history.search" => {
            let query = required_string_param(&params, "query")?.to_string();
            let profile = resolve_profile_param(&params)?;
            let limit = history_limit_from_params(&params);
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let rows = store
                .search(&query, limit)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(rows))
        }
        "browser.history.clear" => {
            let profile = resolve_profile_param(&params)?;
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            store
                .clear()
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "cleared": true }))
        }
        "browser.bookmark.add" => {
            let url = required_string_param(&params, "url")?.to_string();
            if url.trim().is_empty() {
                return Err(DispatchError::InvalidParam(
                    "url must not be empty".to_string(),
                ));
            }
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let profile = resolve_profile_param(&params)?;
            let mut store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            store
                .add(&url, &title)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "added": true }))
        }
        "browser.bookmark.list" => {
            let profile = resolve_profile_param(&params)?;
            let store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(store.list()))
        }
        "browser.bookmark.remove" => {
            let url = required_string_param(&params, "url")?.to_string();
            let profile = resolve_profile_param(&params)?;
            let mut store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let removed = store
                .remove(&url)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "removed": removed }))
        }
        "surface.focus" => {
            let surface_id = required_surface_id(&params)?;
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
                Err(DispatchError::NotFound("surface".to_string()))
            }
        }
        "surface.close" => {
            let surface_id = required_surface_id(&params)?;
            let root_replacement = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                if model.surface(surface_id).is_none() {
                    return Err(DispatchError::NotFound("surface".to_string()));
                }
                model.prepare_root_surface_replacement(surface_id)
            };
            if let Some(replacement) = root_replacement {
                if let Err(err) = spawn_surface_terminal(state, &replacement) {
                    return Err(err.into());
                }
                if let Err(err) = close_terminal_surface_if_present(state, surface_id) {
                    let mut err = err;
                    if let Err(cleanup_err) =
                        forget_terminal_surface_if_present(state, &replacement.id)
                    {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    return Err(err.into());
                }
                let (surface, replacement_in_model) = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    let surface = model
                        .close_surface_with_replacement(surface_id, Some(replacement.clone()))
                        .ok_or(DispatchError::NotFound("surface".to_string()));
                    let replacement_in_model = model.surface(&replacement.id).is_some();
                    (surface, replacement_in_model)
                };
                if surface.is_err() || !replacement_in_model {
                    forget_terminal_surface_if_present(state, &replacement.id)?;
                }
                let surface = surface?;
                return Ok(json!(surface));
            }
            close_terminal_surface_if_present(state, surface_id)?;
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .close_surface(surface_id)
                    .ok_or(DispatchError::NotFound("surface".to_string()))?
            };
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!(surface))
        }
        "notification.create" => {
            let title = notification_title_from_params(&params)?;
            let body = notification_body_from_params(&params)?;
            ensure_max_text_size("title", title)?;
            ensure_max_text_size("body", body)?;
            let kind = notification_kind_from_params(&params)?;
            let (workspace_id, surface_id) = resolve_notification_target(state, &params)?;
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
            let key = required_trimmed_string(&params, "key")?;
            let label = required_trimmed_string(&params, "label")?;
            ensure_max_text_size("key", key)?;
            ensure_max_text_size("label", label)?;
            let value = required_trimmed_string(&params, "value")?;
            ensure_max_text_size("value", value)?;
            let color = status_color_from_params(&params)?;
            let hook = optional_hook_status_metadata(&params)?;
            let status = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .set_status_with_hook_metadata(&workspace_id, key, label, value, color, hook)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
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
            let key = optional_non_blank_string_param(&params, "key")?;
            let hook = optional_hook_status_metadata(&params)?;
            let cleared = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.clear_status_with_hook_metadata(&workspace_id, key, hook)
            };
            if cleared {
                Ok(json!({"cleared": true}))
            } else {
                Err(DispatchError::NotFound("workspace".to_string()))
            }
        }
        "metadata.set_progress" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let key = required_trimmed_string(&params, "key")?;
            let label = required_trimmed_string(&params, "label")?;
            ensure_max_text_size("key", key)?;
            ensure_max_text_size("label", label)?;
            let value = required_f64(&params, "value")?;
            let total = optional_f64(&params, "total")?;
            if total.is_some_and(|total| total <= 0.0) {
                return Err("Invalid parameter total: expected positive number"
                    .to_string()
                    .into());
            }
            let progress = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .set_progress(&workspace_id, key, label, value, total)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
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
            let key = optional_non_blank_string_param(&params, "key")?;
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
                Err(DispatchError::NotFound("workspace".to_string()))
            }
        }
        "metadata.log" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let level = log_level_from_params(&params)?;
            let message = required_string(&params, "message")?;
            ensure_max_text_size("message", message)?;
            let log = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .append_log(&workspace_id, level, message)
                    .ok_or(DispatchError::NotFound("workspace".to_string()))?
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
                Err(DispatchError::NotFound("workspace".to_string()))
            }
        }
        _ => Err(DispatchError::MethodNotFound(method.to_string())),
    }
}

fn ensure_max_text_size(field: &'static str, value: &str) -> Result<(), DispatchError> {
    if value.len() > MAX_METADATA_TEXT_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field,
            limit: MAX_METADATA_TEXT_BYTES,
            actual: value.len(),
        });
    }
    Ok(())
}

fn dispatch_notification_with_loaded_config(notification: &forktty_core::NotificationItem) {
    let config = match config::load_config() {
        Ok(config) => config,
        Err(err) => {
            // Surface the underlying cause so a misconfigured custom command or
            // a corrupted config.toml is debuggable rather than silently
            // turning into "default behavior with no custom command".
            eprintln!("Falling back to default notification settings: {err}");
            forktty_core::AppConfig::default()
        }
    };
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
    let (workspace, previous_active_id) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let previous_active_id = model.active_workspace_id();
        (
            model.create_worktree_workspace(
                &info.branch,
                PathBuf::from(&info.path),
                &info.branch,
                &info.worktree_name,
            ),
            previous_active_id,
        )
    };
    if let Err(err) = spawn_workspace_terminal(state, &workspace) {
        let mut err = err;
        if let Err(rollback_err) =
            rollback_workspace_creation(state, &workspace.id, previous_active_id)
        {
            err = format!("{err}; workspace rollback failed: {rollback_err}");
        }
        return Err(err);
    }
    Ok(workspace)
}

fn rollback_created_worktree_after_spawn_failure(
    cwd: &str,
    info: &worktree::WorktreeInfo,
    spawn_error: String,
) -> String {
    match worktree::remove(cwd, &info.worktree_name, true) {
        Ok(()) => spawn_error,
        Err(rollback_error) => format!(
            "{spawn_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
}

fn spawn_workspace_terminal(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), String> {
    state
        .terminal
        .spawn(SpawnRequest::for_workspace(
            workspace,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
        .map_err(|err| err.to_string())
}

fn spawn_surface_terminal(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Result<(), String> {
    state
        .terminal
        .spawn(SpawnRequest::for_surface(
            surface,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
        .map_err(|err| err.to_string())
}

fn close_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn close_terminal_surfaces_if_present(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), String> {
    for surface_id in surface_ids {
        close_terminal_surface_if_present(state, surface_id)?;
    }
    Ok(())
}

fn forget_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.forget_surface(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn rollback_workspace_creation(
    state: &SocketAppState,
    workspace_id: &str,
    previous_active_id: Option<String>,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_workspace(WorkspaceSelector::Id(workspace_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(())
}

fn rollback_surface_creation(state: &SocketAppState, surface_id: &str) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_surface(surface_id);
    Ok(())
}

async fn ensure_terminal_for_active_workspace(state: &SocketAppState) -> Result<(), String> {
    let workspace = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.active_workspace()
    };
    let Some(workspace) = workspace else {
        return Ok(());
    };
    if state
        .terminal
        .surfaces()
        .map_err(|err| err.to_string())?
        .iter()
        .any(|surface| surface.surface_id == workspace.focused_surface_id)
    {
        return Ok(());
    }
    state
        .terminal
        .spawn(SpawnRequest::for_workspace(
            &workspace,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
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
    missing_param: &'static str,
) -> Result<String, DispatchError> {
    let cwd = resolve_required_existing_dir_param(params, keys, missing_param)?;
    validate_socket_cwd_against_open_workspaces(state, &cwd).map_err(DispatchError::from)?;
    Ok(cwd.to_string_lossy().to_string())
}

fn resolve_required_existing_dir_param(
    params: &Value,
    keys: &[&str],
    missing_param: &'static str,
) -> Result<PathBuf, DispatchError> {
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        let raw = value
            .as_str()
            .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
        if raw.trim().is_empty() {
            return Err(format!("Invalid parameter {key}: path must not be empty").into());
        }
        return canonical_existing_dir(Path::new(raw), key).map_err(DispatchError::from);
    }
    Err(DispatchError::MissingParam(missing_param))
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

fn worktree_name_from_params<'a>(
    params: &'a Value,
    keys: &[&str],
    missing_label: &'static str,
) -> Result<&'a str, DispatchError> {
    let mut found = Vec::new();
    for key in keys {
        let Some(value) = params.get(*key) else {
            continue;
        };
        let Some(name) = value.as_str() else {
            return Err(format!("Invalid parameter {key}: expected string").into());
        };
        let name = validate_worktree_name(name).map_err(DispatchError::from)?;
        found.push((*key, name));
    }
    if found.is_empty() {
        return Err(DispatchError::MissingParam(missing_label));
    }
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous worktree selector: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        )
        .into());
    }
    Ok(found[0].1)
}

fn worktree_layout() -> String {
    config::load_config()
        .ok()
        .map(|config| config.general.worktree_layout)
        .filter(|layout| !layout.is_empty())
        .unwrap_or_else(|| "nested".to_string())
}

fn required_string<'a>(params: &'a Value, key: &'static str) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    match value.as_str() {
        Some(value) if !value.trim().is_empty() => Ok(value),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

fn required_trimmed_string<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    match value.as_str().map(str::trim) {
        Some(value) if !value.is_empty() => Ok(value),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

fn required_surface_id(params: &Value) -> Result<&str, DispatchError> {
    let surface_ids = surface_id_params(params)?;
    if surface_ids.is_empty() {
        return Err(DispatchError::MissingParam("surface_id"));
    }
    if surface_ids.len() > 1 {
        return Err(format!(
            "Ambiguous surface selector: cannot combine {}",
            format_param_names(surface_ids.iter().map(|param| param.key))
        )
        .into());
    }
    Ok(surface_ids[0].value)
}

fn required_string_param<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<&'a str, DispatchError> {
    let Some(value) = params.get(key) else {
        return Err(DispatchError::MissingParam(key));
    };
    value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected string").into())
}

fn workspace_create_name_from_params(params: &Value) -> Result<&str, DispatchError> {
    let Some(value) = params.get("name") else {
        return Ok("workspace");
    };
    match value.as_str().map(str::trim) {
        Some(name) if !name.is_empty() => Ok(name),
        Some(_) => Err("Invalid parameter name: must not be empty".into()),
        None => Err("Invalid parameter name: expected string".into()),
    }
}

fn split_axis_from_params(params: &Value) -> Result<SplitAxis, DispatchError> {
    let Some(axis) = params.get("axis") else {
        return Ok(SplitAxis::Horizontal);
    };
    match axis.as_str() {
        Some("horizontal") => Ok(SplitAxis::Horizontal),
        Some("vertical") => Ok(SplitAxis::Vertical),
        Some(_) => Err("Invalid parameter axis: expected horizontal or vertical".into()),
        None => Err("Invalid parameter axis: expected string".into()),
    }
}

/// Validate the surface is a browser, send `op` to the GTK side, and await the
/// reply within [`BROWSER_CMD_TIMEOUT`]. Maps [`CmdResult`] to a JSON value or a
/// [`DispatchError`].
async fn dispatch_browser_cmd(
    state: &SocketAppState,
    surface_id: String,
    op: BrowserOp,
) -> Result<Value, DispatchError> {
    {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        match model.surface(&surface_id) {
            None => return Err(DispatchError::NotFound("surface".to_string())),
            Some(surface) => {
                if !matches!(surface.kind, forktty_core::SurfaceKind::Browser { .. }) {
                    return Err(DispatchError::NotFound("browser surface".to_string()));
                }
            }
        }
    }
    let Some(sender) = state.browser_cmd.clone() else {
        return Err("browser automation unavailable".into());
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    sender
        .send(BrowserCommand {
            surface_id,
            op,
            reply: reply_tx,
        })
        .await
        .map_err(|_| DispatchError::from("browser automation unavailable"))?;
    let result = tokio::time::timeout(BROWSER_CMD_TIMEOUT, reply_rx)
        .await
        .map_err(|_| DispatchError::from("browser command timed out"))?
        .map_err(|_| DispatchError::Other("browser reply dropped".to_string()))?;
    match result {
        CmdResult::Ok => Ok(json!({"ok": true})),
        CmdResult::Json(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|e| DispatchError::Other(format!("invalid browser result json: {e}"))),
        CmdResult::Err(err) => Err(browser_cmd_error_to_dispatch(err)),
    }
}

fn browser_cmd_error_to_dispatch(err: BrowserCmdError) -> DispatchError {
    match err {
        BrowserCmdError::SurfaceGone => DispatchError::NotFound("surface".to_string()),
        BrowserCmdError::NotABrowser => DispatchError::NotFound("browser surface".to_string()),
        BrowserCmdError::NoWebView => DispatchError::NotFound("web view".to_string()),
        BrowserCmdError::RefNotFound => DispatchError::NotFound("element ref".to_string()),
        BrowserCmdError::TooLarge => DispatchError::PayloadTooLarge {
            field: "result",
            limit: forktty_core::MAX_BROWSER_RESULT_BYTES,
            actual: forktty_core::MAX_BROWSER_RESULT_BYTES + 1,
        },
        BrowserCmdError::JsError(msg) => DispatchError::Other(msg),
        BrowserCmdError::Internal(msg) => DispatchError::Other(msg),
    }
}

fn required_browser_url(params: &Value) -> Result<String, DispatchError> {
    let raw = required_string_param(params, "url")?.trim();
    if raw.is_empty() {
        return Err("Invalid parameter url: must not be empty".into());
    }
    if raw.len() > MAX_BROWSER_URL_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "url",
            limit: MAX_BROWSER_URL_BYTES,
            actual: raw.len(),
        });
    }
    if forktty_core::has_uri_scheme(raw) {
        Ok(raw.to_string())
    } else {
        Ok(format!("https://{raw}"))
    }
}

fn profiles_store() -> Result<forktty_core::ProfileStore, DispatchError> {
    let path = dirs::data_local_dir()
        .map(|d| {
            d.join("forktty")
                .join("browser_profiles")
                .join("profiles.json")
        })
        .ok_or_else(|| DispatchError::from("no data dir for profiles".to_string()))?;
    forktty_core::ProfileStore::load(path).map_err(|e| DispatchError::from(e.to_string()))
}

/// Resolve an optional `profile` param (id or display name) to a `ProfileId`.
/// Absent or null → the Default profile. Present-but-unknown → NotFound.
/// Non-string → InvalidParam.
fn resolve_profile_param(params: &Value) -> Result<forktty_core::ProfileId, DispatchError> {
    match params.get("profile") {
        None => Ok(forktty_core::ProfileId::default()),
        Some(Value::Null) => Ok(forktty_core::ProfileId::default()),
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                DispatchError::InvalidParam(
                    "Invalid parameter profile: expected string".to_string(),
                )
            })?;
            profiles_store()?
                .resolve(name)
                .ok_or(DispatchError::NotFound("profile".to_string()))
        }
    }
}

fn history_limit_from_params(params: &Value) -> usize {
    params
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n.min(10_000) as usize)
        .unwrap_or(100)
}

fn notification_kind_from_params(params: &Value) -> Result<NotificationKind, DispatchError> {
    let Some(kind) = params.get("kind") else {
        return Ok(NotificationKind::Info);
    };
    match kind.as_str() {
        Some("info") => Ok(NotificationKind::Info),
        Some("prompt") => Ok(NotificationKind::Prompt),
        Some("error") => Ok(NotificationKind::Error),
        Some("custom") => Ok(NotificationKind::Custom),
        Some(_) => Err("Invalid parameter kind: expected info, prompt, error, or custom".into()),
        None => Err("Invalid parameter kind: expected string".into()),
    }
}

fn notification_title_from_params(params: &Value) -> Result<&str, DispatchError> {
    optional_non_blank_string_param(params, "title").map(|title| title.unwrap_or("ForkTTY"))
}

fn notification_body_from_params(params: &Value) -> Result<&str, DispatchError> {
    let Some(body) = params.get("body") else {
        return Ok("");
    };
    body.as_str()
        .ok_or_else(|| "Invalid parameter body: expected string".into())
}

fn log_level_from_params(params: &Value) -> Result<LogLevel, DispatchError> {
    let Some(level) = params.get("level") else {
        return Ok(LogLevel::Info);
    };
    match level.as_str() {
        Some("info") => Ok(LogLevel::Info),
        Some("warn") => Ok(LogLevel::Warn),
        Some("error") => Ok(LogLevel::Error),
        Some(_) => Err("Invalid parameter level: expected info, warn, or error".into()),
        None => Err("Invalid parameter level: expected string".into()),
    }
}

fn status_color_from_params(params: &Value) -> Result<Option<String>, DispatchError> {
    let Some(color) = params.get("color") else {
        return Ok(None);
    };
    let Some(color) = color.as_str().map(str::trim) else {
        return Err("Invalid parameter color: expected string".into());
    };
    if color.is_empty() {
        return Err("Invalid parameter color: must not be empty".into());
    }
    if is_supported_status_color(color) {
        Ok(Some(color.to_string()))
    } else {
        Err("Invalid parameter color: expected green, yellow, red, blue, muted, or #hex".into())
    }
}

fn is_supported_status_color(color: &str) -> bool {
    matches!(color, "green" | "yellow" | "red" | "blue" | "muted") || is_hex_status_color(color)
}

fn is_hex_status_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn optional_order_param(params: &Value) -> Result<Option<u128>, DispatchError> {
    let Some(order) = params.get("hook_event_order") else {
        return Ok(None);
    };
    if let Some(order) = order.as_u64() {
        return Ok(Some(u128::from(order)));
    }
    if let Some(order) = order.as_str().map(str::trim) {
        return order
            .parse::<u128>()
            .map(Some)
            .map_err(|_| "Invalid parameter hook_event_order: expected unsigned integer".into());
    }
    Err("Invalid parameter hook_event_order: expected unsigned integer".into())
}

fn optional_hook_status_metadata(
    params: &Value,
) -> Result<Option<StatusHookMetadata>, DispatchError> {
    let order = optional_order_param(params)?;
    let event = optional_non_blank_string_param(params, "hook_event_name")?
        .map(str::to_string)
        .unwrap_or_default();
    let clock = optional_non_blank_string_param(params, "hook_event_clock")?.map(str::to_string);
    let turn_id = optional_non_blank_string_param(params, "hook_turn_id")?.map(str::to_string);

    if event.is_empty() && order.is_none() && clock.is_none() && turn_id.is_none() {
        return Ok(None);
    }

    ensure_max_text_size("hook_event_name", &event)?;
    if let Some(clock) = &clock {
        ensure_max_text_size("hook_event_clock", clock)?;
    }
    if let Some(turn_id) = &turn_id {
        ensure_max_text_size("hook_turn_id", turn_id)?;
    }

    Ok(Some(StatusHookMetadata {
        event,
        order,
        clock,
        turn_id,
    }))
}

fn optional_non_blank_string_param<'a>(
    params: &'a Value,
    key: &str,
) -> Result<Option<&'a str>, DispatchError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    match value.as_str().map(str::trim) {
        Some(value) if !value.is_empty() => Ok(Some(value)),
        Some(_) => Err(format!("Invalid parameter {key}: must not be empty").into()),
        None => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

fn optional_surface_id_param(params: &Value) -> Result<Option<&str>, DispatchError> {
    let surface_ids = surface_id_params(params)?;
    if surface_ids.len() > 1 {
        return Err(format!(
            "Ambiguous surface selector: cannot combine {}",
            format_param_names(surface_ids.iter().map(|param| param.key))
        )
        .into());
    }
    Ok(surface_ids.first().map(|param| param.value))
}

struct SurfaceIdParam<'a> {
    key: &'static str,
    value: &'a str,
}

fn surface_id_params<'a>(params: &'a Value) -> Result<Vec<SurfaceIdParam<'a>>, DispatchError> {
    let mut surface_ids = Vec::new();
    for key in ["surface_id", "surfaceId"] {
        if let Some(value) = optional_non_blank_string_param(params, key)? {
            surface_ids.push(SurfaceIdParam { key, value });
        }
    }
    Ok(surface_ids)
}

fn ensure_model_surface_exists(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if model.surface(surface_id).is_some() {
        Ok(())
    } else {
        Err(DispatchError::NotFound("surface".to_string()))
    }
}

fn resolve_notification_target(
    state: &SocketAppState,
    params: &Value,
) -> Result<(Option<String>, Option<String>), DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let workspace_id = match workspace_selector_from_params(params) {
        Ok(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);

    if let Some(surface_id) = surface_id {
        let surface = model
            .surface(&surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        if workspace_id
            .as_deref()
            .is_some_and(|workspace_id| workspace_id != surface.workspace_id)
        {
            return Err(DispatchError::NotFound("surface".to_string()));
        }
        return Ok((Some(surface.workspace_id.clone()), Some(surface_id)));
    }

    Ok((workspace_id, None))
}

fn required_f64(params: &Value, key: &'static str) -> Result<f64, DispatchError> {
    optional_f64(params, key)?.ok_or(DispatchError::MissingParam(key))
}

fn optional_f64(params: &Value, key: &str) -> Result<Option<f64>, DispatchError> {
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .ok_or_else(|| format!("Invalid parameter {key}: expected finite number"))?;
    if !value.is_finite() {
        return Err(format!("Invalid parameter {key}: expected finite number").into());
    }
    Ok(Some(value))
}

fn resolve_workspace_id_for_metadata(
    state: &SocketAppState,
    params: &Value,
) -> Result<String, DispatchError> {
    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let model = state
        .model
        .lock()
        .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
    let workspace_id = match workspace_selector_from_params(params) {
        Ok(selector) => Some(
            model
                .workspace_id_for(selector)
                .ok_or(DispatchError::NotFound("workspace".to_string()))?,
        ),
        Err(DispatchError::MissingParam(_)) => None,
        Err(err) => return Err(err),
    };
    if let Some(surface_id) = surface_id {
        if let Some(surface) = model.surface(&surface_id) {
            if workspace_id
                .as_deref()
                .is_some_and(|workspace_id| workspace_id != surface.workspace_id)
            {
                return Err(DispatchError::NotFound("surface".to_string()));
            }
            return Ok(surface.workspace_id.clone());
        }
        if let Some(workspace_id) = workspace_id {
            return Ok(workspace_id);
        }
        return Err(DispatchError::NotFound("surface".to_string()));
    }
    if let Some(workspace_id) = workspace_id {
        return Ok(workspace_id);
    }
    model
        .active_workspace_id()
        .ok_or(DispatchError::NotFound("workspace".to_string()))
}

#[derive(Clone, Copy)]
enum WorkspaceSelectorKind {
    Id,
    Name,
    WorktreeName,
}

struct WorkspaceSelectorParam<'a> {
    key: &'static str,
    kind: WorkspaceSelectorKind,
    value: &'a str,
}

fn workspace_selector_from_params(params: &Value) -> Result<WorkspaceSelector<'_>, DispatchError> {
    let selectors = workspace_selector_params(params)?;
    if selectors.is_empty() {
        return Err(DispatchError::MissingParam("workspace selector"));
    }
    if selectors.len() > 1 {
        return Err(format!(
            "Ambiguous workspace selector: cannot combine {}",
            format_param_names(selectors.iter().map(|selector| selector.key))
        )
        .into());
    }
    let selector = &selectors[0];
    match selector.kind {
        WorkspaceSelectorKind::Id => Ok(WorkspaceSelector::Id(selector.value)),
        WorkspaceSelectorKind::Name => Ok(WorkspaceSelector::Name(selector.value)),
        WorkspaceSelectorKind::WorktreeName => Ok(WorkspaceSelector::WorktreeName(selector.value)),
    }
}

fn workspace_selector_params<'a>(
    params: &'a Value,
) -> Result<Vec<WorkspaceSelectorParam<'a>>, DispatchError> {
    let mut selectors = Vec::new();
    for (key, kind) in [
        ("id", WorkspaceSelectorKind::Id),
        ("workspace_id", WorkspaceSelectorKind::Id),
        ("workspaceId", WorkspaceSelectorKind::Id),
        ("name", WorkspaceSelectorKind::Name),
        ("workspace_name", WorkspaceSelectorKind::Name),
        ("workspaceName", WorkspaceSelectorKind::Name),
        ("worktreeName", WorkspaceSelectorKind::WorktreeName),
        ("worktree_name", WorkspaceSelectorKind::WorktreeName),
    ] {
        if let Some(value) = optional_non_blank_string_param(params, key)? {
            selectors.push(WorkspaceSelectorParam { key, kind, value });
        }
    }
    Ok(selectors)
}

fn format_param_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let names = names.collect::<Vec<_>>();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        _ => format!(
            "{}, and {}",
            names[..names.len() - 1].join(", "),
            names[names.len() - 1]
        ),
    }
}

pub fn bootstrap_default_workspace(state: &SocketAppState, cwd: PathBuf) -> Result<(), String> {
    let workspace = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        if let Some(existing) = model.active_workspace() {
            existing
        } else {
            model.create_workspace("main", cwd)
        }
    };
    state
        .terminal
        .spawn(SpawnRequest::for_workspace(
            &workspace,
            state.shell.clone(),
            state.socket_path.clone(),
        ))
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
            Some(Err(ReadLineError::Io(err))) => return Err(err.into()),
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
        if request.method == "events.subscribe" {
            // Takes over the connection: stream events until the peer drops.
            let replay = request
                .params
                .get("replay")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            return stream_events(&state, replay, &mut reader, &mut writer).await;
        }
        let id = request.id.clone();
        let response = match dispatch(&state, &request.method, request.params).await {
            Ok(result) => JsonRpcResponse::ok(id, result),
            Err(err) => JsonRpcResponse::error(id, err.code(), err.to_string()),
        };
        write_response(&mut writer, &response).await?;
    }
    Ok(())
}

/// Hold the connection open and stream model events as NDJSON until the peer
/// disconnects (write error) or the broadcast channel closes.
///
/// Subscribes before snapshotting so changes that land during replay are
/// buffered rather than lost; this can duplicate an event across the
/// replay/live boundary, which clients tolerate because events are state
/// assertions, not deltas.
async fn stream_events(
    state: &SocketAppState,
    replay: bool,
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) -> Result<(), SocketError> {
    let mut receiver = state.events.subscribe();
    write_ndjson(writer, &json!({"event": "subscribed"})).await?;
    if replay {
        let snapshot = current_snapshot(&state.model);
        for event in events::diff(&Snapshot::default(), &snapshot) {
            write_ndjson(writer, &json!(event)).await?;
        }
    }
    loop {
        tokio::select! {
            // Watch the read half so an idle client's disconnect is noticed
            // immediately, releasing the connection permit instead of blocking
            // on recv() until the next broadcast.
            closed = peer_closed(reader) => {
                closed?;
                break;
            }
            received = receiver.recv() => match received {
                Ok(event) => write_ndjson(writer, &json!(event)).await?,
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    write_ndjson(writer, &lagged_notice(dropped)).await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

/// Resolve when the peer closes the connection (EOF) or the read errors.
/// Any bytes the client sends on a subscribed connection are discarded.
async fn peer_closed(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
) -> Result<(), SocketError> {
    loop {
        let consumed = {
            let buf = reader.fill_buf().await?;
            if buf.is_empty() {
                return Ok(()); // EOF: peer closed.
            }
            buf.len()
        };
        reader.consume(consumed);
    }
}

/// The NDJSON notice sent when a subscriber falls behind and the channel drops
/// `dropped` buffered events. The client should resync by reconnecting.
fn lagged_notice(dropped: u64) -> Value {
    json!({"event": "lagged", "dropped": dropped})
}

async fn write_ndjson(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
) -> Result<(), SocketError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
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

#[derive(Debug)]
enum ReadLineError {
    TooLarge,
    InvalidUtf8,
    Io(io::Error),
}

async fn read_limited_line(
    reader: &mut (impl AsyncBufRead + Unpin),
    max_size: usize,
) -> Option<Result<String, ReadLineError>> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let available = match reader.fill_buf().await {
            Ok(available) => available,
            Err(err) => return Some(Err(ReadLineError::Io(err))),
        };
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

// Cap how much the probe will buffer from a foreign socket while waiting for
// a newline. A genuine ForkTTY pong response is ~50 bytes; anything dramatically
// larger almost certainly comes from an unrelated peer that bound to our path,
// and we don't want to grow the response buffer indefinitely while the timeout
// drains.
const PROBE_RESPONSE_MAX_BYTES: usize = 4096;

fn probe_forktty_socket(mut stream: StdUnixStream) -> io::Result<bool> {
    use std::io::{Read, Write};
    stream.set_read_timeout(Some(SOCKET_PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(SOCKET_PROBE_TIMEOUT))?;
    stream.write_all(br#"{"id":"probe","method":"system.ping","params":{}}"#)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = Vec::with_capacity(256);
    let mut buf = [0u8; 256];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let chunk = &buf[..n];
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            response.extend_from_slice(&chunk[..pos]);
            break;
        }
        if response.len().saturating_add(n) > PROBE_RESPONSE_MAX_BYTES {
            return Ok(false);
        }
        response.extend_from_slice(chunk);
    }
    if response.is_empty() {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(&response)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(value.get("id").and_then(Value::as_str) == Some("probe")
        && value.get("ok").and_then(Value::as_bool) == Some(true)
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
    default_socket_dir_from_env(std::env::var("XDG_RUNTIME_DIR").ok().as_deref())
}

fn default_socket_dir_from_env(runtime_dir: Option<&str>) -> PathBuf {
    if let Some(runtime_dir) = runtime_dir.map(str::trim).filter(|value| !value.is_empty()) {
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
    use forktty_terminal::{
        HeadlessTerminalBackend, TerminalBackend, TerminalError, TerminalSurfaceState,
    };
    use git2::Repository;
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::{Arc, Barrier, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// RAII guard that sets an environment variable for the duration of a test
    /// and restores the previous value (or removes it) on drop, even on panic.
    ///
    /// Use together with `#[serial_test::serial]` so that tests touching
    /// process-global env vars do not race with each other.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: test-only; access serialized via #[serial_test::serial].
            unsafe { std::env::set_var(key, val) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

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

    #[derive(Debug, Default)]
    struct FailingSpawnBackend;

    impl TerminalBackend for FailingSpawnBackend {
        fn spawn(&self, _request: SpawnRequest) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("spawn failed".to_string()))
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("send failed".to_string()))
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("resize failed".to_string()))
        }

        fn close(&self, _surface_id: &str) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("close failed".to_string()))
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(Vec::new())
        }
    }

    #[derive(Debug, Default)]
    struct NotReadySendBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    }

    impl TerminalBackend for NotReadySendBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(
                    request.surface_id.clone(),
                    TerminalSurfaceState {
                        surface_id: request.surface_id,
                        workspace_id: request.workspace_id,
                        cwd: request.cwd,
                        shell: request.shell,
                        cols: 80,
                        rows: 24,
                    },
                );
            Ok(())
        }

        fn send_text(&self, surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Err(TerminalError::NotReady(surface_id.to_string()))
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
    }

    #[derive(Debug)]
    struct SpawnFailsCloseSucceedsBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    }

    impl SpawnFailsCloseSucceedsBackend {
        fn new(initial: TerminalSurfaceState) -> Self {
            let mut surfaces = BTreeMap::new();
            surfaces.insert(initial.surface_id.clone(), initial);
            Self {
                surfaces: Mutex::new(surfaces),
            }
        }
    }

    #[derive(Debug)]
    struct CloseMutatesModelBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
        model: Arc<Mutex<WorkspaceModel>>,
    }

    impl CloseMutatesModelBackend {
        fn new(initial: TerminalSurfaceState, model: Arc<Mutex<WorkspaceModel>>) -> Self {
            let mut surfaces = BTreeMap::new();
            surfaces.insert(initial.surface_id.clone(), initial);
            Self {
                surfaces: Mutex::new(surfaces),
                model,
            }
        }
    }

    impl TerminalBackend for CloseMutatesModelBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(
                    request.surface_id.clone(),
                    TerminalSurfaceState {
                        surface_id: request.surface_id,
                        workspace_id: request.workspace_id,
                        cwd: request.cwd,
                        shell: request.shell,
                        cols: 80,
                        rows: 24,
                    },
                );
            Ok(())
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            let mut model = self.model.lock().map_err(|_| TerminalError::LockPoisoned)?;
            let _ = model.close_surface(surface_id);
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
    }

    impl TerminalBackend for SpawnFailsCloseSucceedsBackend {
        fn spawn(&self, _request: SpawnRequest) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("spawn failed".to_string()))
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
    }

    #[derive(Debug, Default)]
    struct FailingCloseBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    }

    impl TerminalBackend for FailingCloseBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(
                    request.surface_id.clone(),
                    TerminalSurfaceState {
                        surface_id: request.surface_id,
                        workspace_id: request.workspace_id,
                        cwd: request.cwd,
                        shell: request.shell,
                        cols: 80,
                        rows: 24,
                    },
                );
            Ok(())
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, _surface_id: &str) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("close failed".to_string()))
        }

        fn forget_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            Ok(self
                .surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .values()
                .cloned()
                .collect())
        }
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

    fn probe_socket_with_response(response: &'static str) -> bool {
        use std::io::{BufRead as _, Write as _};

        let (client, server) = StdUnixStream::pair().unwrap();
        let server_thread = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(server);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            assert!(request.contains(r#""id":"probe""#));
            let mut server = reader.into_inner();
            server.write_all(response.as_bytes()).unwrap();
            server.write_all(b"\n").unwrap();
            server.flush().unwrap();
        });

        let result = probe_forktty_socket(client).unwrap();
        server_thread.join().unwrap();
        result
    }

    #[test]
    fn dispatch_error_from_worktree_error_assigns_stable_codes() {
        use forktty_core::worktree::WorktreeError as W;

        assert_eq!(
            DispatchError::from(W::NotFound("foo".into())).code(),
            "not_found"
        );
        assert_eq!(
            DispatchError::from(W::BranchNotFound("bar".into())).code(),
            "not_found"
        );
        assert_eq!(
            DispatchError::from(W::AlreadyExists("foo".into())).code(),
            "already_exists"
        );
        assert_eq!(DispatchError::from(W::TargetDirty).code(), "conflict");
        assert_eq!(
            DispatchError::from(W::WorktreeDirty("foo".into())).code(),
            "conflict"
        );
        assert_eq!(DispatchError::from(W::MergeConflicts).code(), "conflict");
        assert_eq!(
            DispatchError::from(W::HookOutsideWorktree).code(),
            "conflict"
        );
        assert_eq!(
            DispatchError::from(W::InvalidName(forktty_core::WorktreeNameError::Empty)).code(),
            "invalid_param"
        );
        assert_eq!(
            DispatchError::from(W::NotARepo("/tmp/repo".into())).code(),
            "not_found"
        );
        assert_eq!(DispatchError::from(W::BareRepo).code(), "error");
        assert_eq!(
            DispatchError::from(W::NotFound("foo".into())).to_string(),
            "Worktree 'foo' not found"
        );
        assert_eq!(
            DispatchError::from(W::BranchNotFound("bar".into())).to_string(),
            "Branch 'bar' not found"
        );
        assert_eq!(
            DispatchError::from(W::NotARepo("/tmp/repo".into())).to_string(),
            "Not a git repository: /tmp/repo"
        );
    }

    #[test]
    fn probe_accepts_matching_forktty_socket_response() {
        assert!(probe_socket_with_response(
            r#"{"id":"probe","ok":true,"result":"pong"}"#
        ));
    }

    #[test]
    fn probe_rejects_wrong_response_id_even_when_pong_matches() {
        assert!(!probe_socket_with_response(
            r#"{"id":"other","ok":true,"result":"pong"}"#
        ));
    }

    #[test]
    fn probe_rejects_oversized_response_without_newline() {
        use std::io::{BufRead as _, Write as _};

        let (client, server) = StdUnixStream::pair().unwrap();
        let server_thread = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(server);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut server = reader.into_inner();
            let payload = vec![b'x'; PROBE_RESPONSE_MAX_BYTES * 2];
            let _ = server.write_all(&payload);
            let _ = server.flush();
        });

        let result = probe_forktty_socket(client).unwrap();
        let _ = server_thread.join();
        assert!(!result);
    }

    #[test]
    fn bind_socket_listener_rejects_broken_socket_symlink() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");
        symlink(dir.path().join("missing.sock"), &socket_path).unwrap();

        let error = bind_socket_listener(&socket_path, false).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        assert!(error
            .to_string()
            .contains("refusing to replace non-socket path"));
        assert!(fs::symlink_metadata(&socket_path)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn default_socket_dir_trims_and_requires_absolute_runtime_dir() {
        assert_eq!(
            default_socket_dir_from_env(Some(" /run/user/1000 ")),
            PathBuf::from("/run/user/1000")
        );
        assert_eq!(
            default_socket_dir_from_env(Some("relative-runtime")),
            std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
        );
        assert_eq!(
            default_socket_dir_from_env(Some("  ")),
            std::env::temp_dir().join(format!("forktty-{}", effective_uid()))
        );
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
        assert_eq!(notification["workspace_id"], workspaces[0]["id"]);
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
    async fn send_text_returns_structured_not_ready_before_terminal_child_ready() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(NotReadySendBackend::default());
        let state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": "echo not-ready\n"}),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "not_ready");
        assert!(err.to_string().contains(surface_id));
    }

    #[tokio::test]
    async fn notification_create_rejects_stale_targets() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let split = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap();
        let stale_surface_id = split["id"].as_str().unwrap().to_string();
        {
            let mut model = state.model.lock().unwrap();
            model.close_surface(&stale_surface_id).unwrap();
        }

        let stale_workspace = dispatch(
            &state,
            "notification.create",
            json!({
                "workspace_id": "workspace-missing",
                "title": "Prompt",
                "body": "stale workspace"
            }),
        )
        .await
        .unwrap_err();
        let stale_surface = dispatch(
            &state,
            "notification.create",
            json!({
                "workspace_id": workspace_id,
                "surface_id": stale_surface_id,
                "title": "Prompt",
                "body": "stale surface"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(stale_workspace.code(), "not_found");
        assert_eq!(stale_surface.code(), "not_found");
        let notifications = dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap();
        assert!(notifications.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn notification_create_rejects_invalid_kind() {
        let (state, _backend) = test_state();

        for kind in [json!("promtp"), json!(""), json!(42)] {
            let error = dispatch(
                &state,
                "notification.create",
                json!({"title": "Prompt", "kind": kind}),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter kind"));
        }
        let notifications = dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap();
        assert!(notifications.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn notification_create_rejects_invalid_text_fields() {
        let (state, _backend) = test_state();

        for title in [json!(""), json!(" \n "), json!(42)] {
            let error = dispatch(&state, "notification.create", json!({"title": title}))
                .await
                .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter title"));
        }

        let error = dispatch(&state, "notification.create", json!({"body": 42}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "error");
        assert!(error.to_string().contains("Invalid parameter body"));

        let notifications = dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap();
        assert!(notifications.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn notification_create_rejects_invalid_surface_targets() {
        let (state, _backend) = test_state();

        for surface_id in [json!(""), json!(42)] {
            let error = dispatch(
                &state,
                "notification.create",
                json!({"title": "Prompt", "surface_id": surface_id}),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter surface_id"));
        }

        let notifications = dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap();
        assert!(notifications.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn notification_create_respects_workspace_selectors() {
        let (state, _backend) = test_state();
        let created = dispatch(
            &state,
            "workspace.create",
            json!({"name": "target", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        let notification = dispatch(
            &state,
            "notification.create",
            json!({
                "workspace_name": " target ",
                "title": "Targeted",
                "body": "by workspace name"
            }),
        )
        .await
        .unwrap();

        assert_eq!(notification["workspace_id"], created["id"]);
        assert!(notification["surface_id"].is_null());
    }

    #[tokio::test]
    async fn metadata_commands_reject_stale_workspace_targets() {
        let (state, _backend) = test_state();

        let error = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": "workspace-missing",
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.to_string(), "Workspace not found");
    }

    #[tokio::test]
    async fn metadata_commands_can_target_workspace_by_surface_id() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running"
            }),
        )
        .await
        .unwrap();

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses[0]["value"], "Running");

        let other_workspace = dispatch(
            &state,
            "workspace.create",
            json!({"name": "other", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let error = dispatch(
            &state,
            "metadata.log",
            json!({
                "workspace_id": other_workspace["id"],
                "surface_id": surface_id,
                "level": "info",
                "message": "mismatch"
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "not_found");
        assert_eq!(error.to_string(), "Surface not found");
    }

    #[tokio::test]
    async fn metadata_hooks_can_finish_cleanup_after_target_surface_closes() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_name": "prompt-submit",
                "hook_event_clock": "monotonic-ns",
                "hook_event_order": 100,
                "hook_turn_id": "prompt:one"
            }),
        )
        .await
        .unwrap();
        {
            let mut model = state.model.lock().unwrap();
            model.close_surface(surface_id).unwrap();
        }

        dispatch(
            &state,
            "metadata.log",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "level": "info",
                "message": "Codex session ended"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "hook_event_name": "session-end",
                "hook_event_clock": "monotonic-ns",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let logs = dispatch(
            &state,
            "metadata.list_logs",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
        assert_eq!(logs.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn ordered_metadata_status_ignores_stale_hook_events() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Ready",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses[0]["value"], "Ready");

        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "hook_event_order": "300"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_order": 250
            }),
        )
        .await
        .unwrap();
        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn metadata_hook_state_ignores_late_prompt_submit_for_same_turn() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_name": "prompt-submit",
                "hook_event_clock": "monotonic-ns",
                "hook_event_order": "100",
                "hook_turn_id": "prompt:one"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Ready",
                "hook_event_name": "stop",
                "hook_event_clock": "monotonic-ns",
                "hook_event_order": "200"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_event_name": "prompt-submit",
                "hook_event_clock": "monotonic-ns",
                "hook_event_order": "300",
                "hook_turn_id": "prompt:one"
            }),
        )
        .await
        .unwrap();

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses[0]["value"], "Ready");
    }

    #[tokio::test]
    async fn metadata_commands_reject_invalid_workspace_selectors() {
        let (state, _backend) = test_state();

        for workspace_id in [json!(""), json!(42)] {
            let error = dispatch(
                &state,
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": "agent:codex",
                    "label": "Codex",
                    "value": "Running"
                }),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter workspace_id"));
        }

        let statuses = dispatch(&state, "metadata.list_status", json!({}))
            .await
            .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn metadata_log_rejects_invalid_level() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for level in [json!("verbose"), json!(""), json!(42)] {
            let error = dispatch(
                &state,
                "metadata.log",
                json!({
                    "workspace_id": workspace_id,
                    "level": level,
                    "message": "waiting"
                }),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter level"));
        }

        let logs = dispatch(
            &state,
            "metadata.list_logs",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(logs.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn metadata_commands_reject_oversized_payload_fields() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let oversized = "x".repeat(MAX_METADATA_TEXT_BYTES + 1);

        for (method, params, expected_field) in [
            (
                "metadata.set_status",
                json!({"workspace_id": workspace_id, "key": oversized, "label": "Codex", "value": "Running"}),
                "key",
            ),
            (
                "metadata.set_progress",
                json!({"workspace_id": workspace_id, "key": "build", "label": oversized, "value": 1}),
                "label",
            ),
            (
                "metadata.log",
                json!({"workspace_id": workspace_id, "level": "info", "message": oversized}),
                "message",
            ),
            (
                "notification.create",
                json!({"workspace_id": workspace_id, "title": oversized, "body": "body"}),
                "title",
            ),
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "payload_too_large");
            assert!(error.to_string().contains(expected_field));
        }
    }

    #[tokio::test]
    async fn metadata_clear_rejects_invalid_keys_without_clearing_all() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": "build",
                "label": "Build",
                "value": 1
            }),
        )
        .await
        .unwrap();

        for key in [json!(""), json!(42)] {
            let status_error = dispatch(
                &state,
                "metadata.clear_status",
                json!({"workspace_id": workspace_id, "key": key.clone()}),
            )
            .await
            .unwrap_err();
            let progress_error = dispatch(
                &state,
                "metadata.clear_progress",
                json!({"workspace_id": workspace_id, "key": key}),
            )
            .await
            .unwrap_err();

            assert_eq!(status_error.code(), "error");
            assert_eq!(progress_error.code(), "error");
            assert!(status_error.to_string().contains("Invalid parameter key"));
            assert!(progress_error.to_string().contains("Invalid parameter key"));
        }

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let progress = dispatch(
            &state,
            "metadata.list_progress",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses.as_array().unwrap().len(), 1);
        assert_eq!(progress.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn metadata_set_trims_keys_before_storage() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        let status = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": " agent:codex ",
                "label": " Codex ",
                "value": " Running ",
                "color": " green "
            }),
        )
        .await
        .unwrap();
        let progress = dispatch(
            &state,
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": " build ",
                "label": " Build ",
                "value": 1
            }),
        )
        .await
        .unwrap();

        assert_eq!(status["key"], "agent:codex");
        assert_eq!(status["label"], "Codex");
        assert_eq!(status["value"], "Running");
        assert_eq!(status["color"], "green");
        assert_eq!(progress["key"], "build");
        assert_eq!(progress["label"], "Build");

        dispatch(
            &state,
            "metadata.clear_status",
            json!({"workspace_id": workspace_id, "key": "agent:codex"}),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.clear_progress",
            json!({"workspace_id": workspace_id, "key": "build"}),
        )
        .await
        .unwrap();

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let progress = dispatch(
            &state,
            "metadata.list_progress",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
        assert!(progress.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn metadata_commands_reject_invalid_required_fields() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for (method, params, message) in [
            (
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": 42,
                    "label": "Codex",
                    "value": "Running"
                }),
                "Invalid parameter key",
            ),
            (
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": "agent:codex",
                    "label": "",
                    "value": "Running"
                }),
                "Invalid parameter label",
            ),
            (
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": "agent:codex",
                    "label": "Codex",
                    "value": 42
                }),
                "Invalid parameter value",
            ),
            (
                "metadata.set_progress",
                json!({
                    "workspace_id": workspace_id,
                    "key": "",
                    "label": "Build",
                    "value": 1
                }),
                "Invalid parameter key",
            ),
            (
                "metadata.set_progress",
                json!({
                    "workspace_id": workspace_id,
                    "key": "build",
                    "label": 42,
                    "value": 1
                }),
                "Invalid parameter label",
            ),
            (
                "metadata.set_progress",
                json!({
                    "workspace_id": workspace_id,
                    "key": "build",
                    "label": "Build",
                    "value": "1"
                }),
                "Invalid parameter value",
            ),
            (
                "metadata.log",
                json!({
                    "workspace_id": workspace_id,
                    "message": ""
                }),
                "Invalid parameter message",
            ),
            (
                "metadata.log",
                json!({
                    "workspace_id": workspace_id,
                    "message": 42
                }),
                "Invalid parameter message",
            ),
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains(message));
        }

        let error = dispatch(
            &state,
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": "build",
                "label": "Build"
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "missing_param");
        assert!(error.to_string().contains("value"));

        assert!(dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
        assert!(dispatch(
            &state,
            "metadata.list_progress",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
        assert!(dispatch(
            &state,
            "metadata.list_logs",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    }

    #[tokio::test]
    async fn metadata_set_status_rejects_invalid_colors() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for color in [
            json!("purple"),
            json!(""),
            json!(42),
            json!("#"),
            json!("#12"),
            json!("#nothex"),
        ] {
            let error = dispatch(
                &state,
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": "agent:codex",
                    "label": "Codex",
                    "value": "Running",
                    "color": color
                }),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter color"));
        }

        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn metadata_set_status_accepts_hex_colors() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for color in ["#abc", "#abcd", "#a1B2c3", "#a1B2c3D4"] {
            let status = dispatch(
                &state,
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "key": format!("agent:codex:{color}"),
                    "label": "Codex",
                    "value": "Running",
                    "color": color
                }),
            )
            .await
            .unwrap();

            assert_eq!(status["color"], color);
        }
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
    async fn workspace_select_spawns_missing_terminal_for_selected_workspace() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        dispatch(
            &state,
            "workspace.create",
            json!({"name": "feature", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        backend.close(main_surface_id).unwrap();

        let selected = dispatch(&state, "workspace.select", json!({"name": "main"}))
            .await
            .unwrap();

        assert_eq!(selected["name"], "main");
        assert!(backend
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == main_surface_id));
    }

    #[tokio::test]
    async fn workspace_select_keeps_previous_workspace_when_spawn_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let (first, second) = {
            let mut model = model.lock().unwrap();
            let first = model.create_workspace("first", "/tmp");
            let second = model.create_workspace("second", "/tmp");
            (first, second)
        };
        let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
            surface_id: second.focused_surface_id.clone(),
            workspace_id: second.id.clone(),
            cwd: PathBuf::from("/tmp"),
            shell: "/bin/sh".to_string(),
            cols: 80,
            rows: 24,
        }));
        let state = SocketAppState::new(
            model.clone(),
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(&state, "workspace.select", json!({"id": first.id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("spawn failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 2);
        assert_eq!(workspaces[0]["active"], false);
        assert_eq!(workspaces[1]["id"], second.id);
        assert_eq!(workspaces[1]["active"], true);
        let backend_surfaces = backend.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, second.focused_surface_id);
    }

    #[tokio::test]
    async fn workspace_create_rejects_invalid_names() {
        let (state, _backend) = test_state();

        for name in [json!(""), json!(" \t "), json!(42)] {
            let error = dispatch(
                &state,
                "workspace.create",
                json!({"name": name, "workingDir": "/tmp"}),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter name"));
        }

        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["name"], "main");
    }

    #[tokio::test]
    async fn workspace_create_trims_valid_name() {
        let (state, _backend) = test_state();

        let created = dispatch(
            &state,
            "workspace.create",
            json!({"name": " feature\n", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        assert_eq!(created["name"], "feature");
        let selected = dispatch(&state, "workspace.select", json!({"name": "feature"}))
            .await
            .unwrap();
        assert_eq!(selected["id"], created["id"]);
    }

    #[tokio::test]
    async fn workspace_close_last_workspace_keeps_replacement_in_closed_cwd() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
        let (state, backend) = test_state();

        let initial = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let initial_id = initial[0]["id"].as_str().unwrap();
        let created = dispatch(
            &state,
            "workspace.create",
            json!({"name": "project", "workingDir": project_dir.path()}),
        )
        .await
        .unwrap();
        let project_id = created["id"].as_str().unwrap();
        let project_surface_id = created["focused_surface_id"].as_str().unwrap();
        dispatch(&state, "workspace.close", json!({"id": initial_id}))
            .await
            .unwrap();

        let closed = dispatch(&state, "workspace.close", json!({"id": project_id}))
            .await
            .unwrap();

        assert_eq!(closed["name"], "project");
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["name"], "main");
        assert_eq!(
            workspaces[0]["working_dir"].as_str().unwrap(),
            project_cwd.to_str().unwrap()
        );
        assert!(matches!(
            backend.sent_text(project_surface_id),
            Err(forktty_terminal::TerminalError::NotFound(_))
        ));
        let replacement_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        assert_eq!(
            backend
                .surfaces()
                .unwrap()
                .into_iter()
                .find(|surface| surface.surface_id == replacement_surface_id)
                .unwrap()
                .cwd,
            project_cwd
        );
    }

    #[tokio::test]
    async fn workspace_close_last_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let workspace = {
            let mut model = model.lock().unwrap();
            model.create_workspace("project", &project_cwd)
        };
        let surface_id = workspace.focused_surface_id.clone();
        let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
            surface_id: surface_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: project_cwd.clone(),
            shell: "/bin/sh".to_string(),
            cols: 80,
            rows: 24,
        }));
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(&state, "workspace.close", json!({"id": workspace.id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("spawn failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["id"], workspace.id);
        assert_eq!(workspaces[0]["name"], "project");
        assert_eq!(workspaces[0]["active"], true);
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace.id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
        assert_eq!(backend.surfaces().unwrap().len(), 1);
        assert_eq!(backend.surfaces().unwrap()[0].surface_id, surface_id);
    }

    #[tokio::test]
    async fn surface_split_rejects_invalid_axis() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        for axis in [json!("diagonal"), json!("")] {
            let error = dispatch(
                &state,
                "surface.split",
                json!({"surface_id": surface_id, "axis": axis}),
            )
            .await
            .unwrap_err();

            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains("Invalid parameter axis"));
        }

        let non_string_error = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": 42}),
        )
        .await
        .unwrap_err();
        assert_eq!(non_string_error.code(), "error");
        assert!(non_string_error
            .to_string()
            .contains("Invalid parameter axis"));

        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn surface_list_respects_workspace_selectors() {
        let (state, _backend) = test_state();
        let feature = dispatch(
            &state,
            "workspace.create",
            json!({"name": "feature", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        let all_surfaces = dispatch(&state, "surface.list", json!({})).await.unwrap();
        assert_eq!(all_surfaces.as_array().unwrap().len(), 2);

        let main_surfaces = dispatch(&state, "surface.list", json!({"workspace_name": " main\n"}))
            .await
            .unwrap();
        assert_eq!(main_surfaces.as_array().unwrap().len(), 1);
        assert_ne!(main_surfaces[0]["workspace_id"], feature["id"]);

        let missing = dispatch(&state, "surface.list", json!({"workspace_name": "missing"}))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "not_found");
    }

    #[tokio::test]
    async fn workspace_create_rolls_back_model_when_spawn_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let bootstrap_backend = Arc::new(HeadlessTerminalBackend::new());
        let bootstrap_state = SocketAppState::new(
            model.clone(),
            bootstrap_backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&bootstrap_state, PathBuf::from("/tmp")).unwrap();
        let state = SocketAppState::new(
            model,
            Arc::new(FailingSpawnBackend),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(
            &state,
            "workspace.create",
            json!({"name": "failed", "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("spawn failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["name"], "main");
        assert_eq!(workspaces[0]["active"], true);
    }

    #[tokio::test]
    async fn surface_split_rolls_back_model_when_spawn_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let bootstrap_backend = Arc::new(HeadlessTerminalBackend::new());
        let bootstrap_state = SocketAppState::new(
            model.clone(),
            bootstrap_backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&bootstrap_state, PathBuf::from("/tmp")).unwrap();
        let state = SocketAppState::new(
            model,
            Arc::new(FailingSpawnBackend),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let error = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("spawn failed"));
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn surface_close_keeps_model_when_backend_close_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailingCloseBackend::default());
        let state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("close failed"));
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspaces[0]["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
    }

    #[tokio::test]
    async fn surface_close_root_keeps_old_surface_when_replacement_spawn_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let workspace = {
            let mut model = model.lock().unwrap();
            model.create_workspace("project", &project_cwd)
        };
        let surface_id = workspace.focused_surface_id.clone();
        let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
            surface_id: surface_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: project_cwd,
            shell: "/bin/sh".to_string(),
            cols: 80,
            rows: 24,
        }));
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("spawn failed"));
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace.id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
        assert_eq!(backend.surfaces().unwrap().len(), 1);
        assert_eq!(backend.surfaces().unwrap()[0].surface_id, surface_id);
    }

    #[tokio::test]
    async fn surface_close_root_cleans_replacement_when_model_close_fails() {
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = fs::canonicalize(project_dir.path()).unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let workspace = {
            let mut model = model.lock().unwrap();
            model.create_workspace("project", &project_cwd)
        };
        let surface_id = workspace.focused_surface_id.clone();
        let backend = Arc::new(CloseMutatesModelBackend::new(
            TerminalSurfaceState {
                surface_id: surface_id.clone(),
                workspace_id: workspace.id.clone(),
                cwd: project_cwd,
                shell: "/bin/sh".to_string(),
                cols: 80,
                rows: 24,
            },
            model.clone(),
        ));
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(!error.is_empty());
        assert_eq!(backend.surfaces().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn workspace_close_keeps_model_when_backend_close_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailingCloseBackend::default());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let error = dispatch(&state, "workspace.close", json!({"id": workspace_id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("close failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["id"], workspace_id);
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
        assert_eq!(backend.surfaces().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn worktree_create_removes_created_worktree_when_spawn_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/spawn-rollback-{}", std::process::id());
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let bootstrap_backend = Arc::new(HeadlessTerminalBackend::new());
        let bootstrap_state = SocketAppState::new(
            model.clone(),
            bootstrap_backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&bootstrap_state, repo_dir.path().to_path_buf()).unwrap();
        let state = SocketAppState::new(
            model.clone(),
            Arc::new(FailingSpawnBackend),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(
            &state,
            "worktree.create",
            json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("spawn failed"));
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
        let repo = Repository::open(repo_dir.path()).unwrap();
        assert!(repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .is_err());
        let model = model.lock().unwrap();
        assert_eq!(model.list_workspaces().len(), 1);
        assert!(model
            .list_workspaces()
            .iter()
            .all(|workspace| workspace.git_branch != branch_name));
    }

    #[tokio::test]
    async fn surface_close_removes_model_surface_when_backend_already_missing() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        backend.close(surface_id).unwrap();

        dispatch(&state, "surface.close", json!({"surface_id": surface_id}))
            .await
            .unwrap();

        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_ne!(surfaces[0]["id"], surface_id);
        assert!(backend.sent_text(surface_id).is_err());
        assert!(backend
            .sent_text(surfaces[0]["id"].as_str().unwrap())
            .is_ok());
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

        let repo = Repository::open(repo_dir.path()).unwrap();
        assert!(repo
            .find_branch("topic/socket", git2::BranchType::Local)
            .is_ok());

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
    async fn worktree_remove_keeps_workspace_when_backend_close_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/socket-close-{}", std::process::id());
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailingCloseBackend::default());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();

        let created = dispatch(
            &state,
            "worktree.create",
            json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();
        let workspace_id = created["id"].as_str().unwrap();
        let surface_id = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap()[0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let error = dispatch(
            &state,
            "worktree.remove",
            json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("close failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert!(workspaces
            .as_array()
            .unwrap()
            .iter()
            .any(|workspace| workspace["id"] == workspace_id));
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
        assert!(backend
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id));
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn worktree_remove_last_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/socket-remove-spawn-{}", std::process::id());
        let info = worktree::create(
            repo_dir.path().to_str().unwrap(),
            &branch_name,
            &worktree_layout(),
        )
        .unwrap();
        let worktree_cwd = PathBuf::from(&info.path);
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let workspace = {
            let mut model = model.lock().unwrap();
            model.create_worktree_workspace(
                &info.branch,
                &worktree_cwd,
                &info.branch,
                &info.worktree_name,
            )
        };
        let surface_id = workspace.focused_surface_id.clone();
        let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
            surface_id: surface_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: worktree_cwd,
            shell: "/bin/sh".to_string(),
            cols: 80,
            rows: 24,
        }));
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let error = dispatch(
            &state,
            "worktree.remove",
            json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(error.contains("spawn failed"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["id"], workspace.id);
        assert_eq!(workspaces[0]["active"], true);
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace.id}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
        assert_eq!(surfaces[0]["id"], surface_id);
        assert_eq!(backend.surfaces().unwrap().len(), 1);
        assert_eq!(backend.surfaces().unwrap()[0].surface_id, surface_id);
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
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
        .unwrap_err()
        .to_string();

        assert!(error.contains("open workspace"));
        assert!(worktree::list(unopened_repo.path().to_str().unwrap())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn worktree_socket_rejects_invalid_name_params() {
        let (state, _backend) = test_state();

        for (method, params, code, message) in [
            (
                "worktree.create",
                json!({"name": 42}),
                "error",
                "Invalid parameter name: expected string",
            ),
            (
                "worktree.create",
                json!({"name": ""}),
                "invalid_param",
                "Invalid worktree name: must not be empty",
            ),
            (
                "worktree.attach",
                json!({"branch": 42}),
                "error",
                "Invalid parameter branch: expected string",
            ),
            (
                "worktree.attach",
                json!({"name": 42, "branch": "topic/socket"}),
                "error",
                "Invalid parameter name: expected string",
            ),
            (
                "worktree.attach",
                json!({"name": "topic/name", "branch": "topic/branch"}),
                "error",
                "Ambiguous worktree selector: cannot combine name and branch",
            ),
            (
                "worktree.remove",
                json!({"name": 42}),
                "error",
                "Invalid parameter name: expected string",
            ),
            (
                "worktree.merge",
                json!({"name": 42}),
                "error",
                "Invalid parameter name: expected string",
            ),
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), code, "method={method}");
            assert!(error.to_string().contains(message));
        }

        let error = dispatch(&state, "worktree.attach", json!({"branch": "topic/socket"}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "missing_param");
        assert!(error.to_string().contains("cwd"));
    }

    #[tokio::test]
    async fn worktree_socket_requires_explicit_repo_cwd() {
        let open_repo = make_temp_repo();
        let (state, _backend) = test_state();
        dispatch(
            &state,
            "workspace.create",
            json!({"name": "open", "workingDir": open_repo.path()}),
        )
        .await
        .unwrap();

        for (method, params, missing) in [
            ("worktree.list", json!({}), "cwd"),
            ("worktree.status", json!({}), "path or cwd"),
            ("worktree.create", json!({"name": "blocked"}), "cwd"),
            ("worktree.attach", json!({"name": "blocked"}), "cwd"),
            ("worktree.remove", json!({"name": "blocked"}), "cwd"),
            ("worktree.merge", json!({"name": "blocked"}), "cwd"),
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "missing_param");
            assert!(error.to_string().contains(missing));
        }

        assert!(worktree::list(open_repo.path().to_str().unwrap())
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn dispatch_returns_method_not_found_for_unknown_method() {
        let (state, _backend) = test_state();
        let err = dispatch(&state, "nonsense.bogus", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "method_not_found");
        assert!(err.to_string().contains("nonsense.bogus"));
    }

    #[tokio::test]
    async fn dispatch_returns_missing_param_for_workspace_command_without_selector() {
        let (state, _backend) = test_state();

        for method in ["workspace.select", "workspace.close"] {
            let err = dispatch(&state, method, json!({})).await.unwrap_err();
            assert_eq!(err.code(), "missing_param");
            assert!(err.to_string().contains("workspace selector"));
        }
    }

    #[tokio::test]
    async fn dispatch_rejects_invalid_workspace_command_selectors() {
        let (state, _backend) = test_state();

        for workspace_id in [json!("  "), json!(42)] {
            let err = dispatch(
                &state,
                "workspace.select",
                json!({"workspace_id": workspace_id}),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "error");
            assert!(err.to_string().contains("Invalid parameter workspace_id"));
        }

        let err = dispatch(
            &state,
            "workspace.select",
            json!({"workspace_id": "workspace-1", "workspace_name": "main"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "error");
        assert!(err.to_string().contains(
            "Ambiguous workspace selector: cannot combine workspace_id and workspace_name"
        ));
    }

    #[tokio::test]
    async fn dispatch_returns_missing_param_for_send_text_without_text() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "missing_param");
        assert!(err.to_string().contains("text"));

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": 42}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "error");
        assert!(err.to_string().contains("Invalid parameter text"));

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": ""}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "error");
        assert!(err.to_string().contains("Invalid parameter text"));
    }

    #[tokio::test]
    async fn dispatch_accepts_camel_case_surface_id_alias() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "surface.send_text",
            json!({"surfaceId": format!(" {surface_id}\n"), "text": "echo camel\n"}),
        )
        .await
        .unwrap();

        assert_eq!(backend.sent_text(surface_id).unwrap(), vec!["echo camel\n"]);
    }

    #[tokio::test]
    async fn surface_commands_reject_invalid_surface_id_params() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        for (method, params, message) in [
            (
                "surface.send_text",
                json!({"surface_id": "", "text": "echo bad\n"}),
                "Invalid parameter surface_id",
            ),
            (
                "surface.send_text",
                json!({"surface_id": 42, "surfaceId": surface_id, "text": "echo bad\n"}),
                "Invalid parameter surface_id",
            ),
            (
                "surface.send_text",
                json!({"surfaceId": 42, "text": "echo bad\n"}),
                "Invalid parameter surfaceId",
            ),
            (
                "surface.split",
                json!({"surface_id": "", "axis": "vertical"}),
                "Invalid parameter surface_id",
            ),
            (
                "surface.focus",
                json!({"surface_id": 42}),
                "Invalid parameter surface_id",
            ),
            (
                "surface.close",
                json!({"surface_id": ""}),
                "Invalid parameter surface_id",
            ),
        ] {
            let err = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(err.code(), "error");
            assert!(err.to_string().contains(message));
        }

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({
                "surface_id": surface_id,
                "surfaceId": surface_id,
                "text": "echo bad\n"
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "error");
        assert!(err
            .to_string()
            .contains("Ambiguous surface selector: cannot combine surface_id and surfaceId"));
    }

    #[tokio::test]
    async fn send_text_rejects_surface_removed_from_model_even_if_backend_still_has_it() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let split = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap();
        let stale_surface_id = split["id"].as_str().unwrap().to_string();
        {
            let mut model = state.model.lock().unwrap();
            model.close_surface(&stale_surface_id).unwrap();
        }

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": stale_surface_id, "text": "echo stale\n"}),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "not_found");
        assert!(backend.sent_text(&stale_surface_id).unwrap().is_empty());
    }

    #[tokio::test]
    async fn dispatch_returns_not_found_for_unknown_surface_focus() {
        let (state, _backend) = test_state();
        let err = dispatch(&state, "surface.focus", json!({"surface_id": "no-such"}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "not_found");
    }

    #[tokio::test]
    async fn dispatch_rejects_oversize_send_text_payload() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();
        let huge = "x".repeat(MAX_SEND_TEXT_BYTES + 1);
        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": huge}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "payload_too_large");
    }

    #[test]
    fn validates_worktree_name_params() {
        assert_eq!(validate_worktree_name(" feature/x ").unwrap(), "feature/x");
        let err = DispatchError::from(validate_worktree_name("../escape").unwrap_err());
        assert_eq!(err.code(), "invalid_param");
        assert!(validate_worktree_name("feature//empty").is_err());
        assert!(validate_worktree_name("feature\\windows").is_err());
        assert!(validate_worktree_name("").is_err());
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
        assert!(matches!(
            read_limited_line(&mut reader, 3).await,
            Some(Err(ReadLineError::TooLarge))
        ));
    }

    struct FailingAsyncBufRead;

    impl tokio::io::AsyncRead for FailingAsyncBufRead {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<io::Result<()>> {
            std::task::Poll::Ready(Err(io::Error::other("read failed")))
        }
    }

    impl AsyncBufRead for FailingAsyncBufRead {
        fn poll_fill_buf(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<io::Result<&[u8]>> {
            std::task::Poll::Ready(Err(io::Error::other("fill failed")))
        }

        fn consume(self: std::pin::Pin<&mut Self>, _amt: usize) {}
    }

    #[tokio::test]
    async fn limited_line_surfaces_io_errors() {
        let mut reader = FailingAsyncBufRead;
        let result = read_limited_line(&mut reader, 3).await;

        match result {
            Some(Err(ReadLineError::Io(err))) => {
                assert_eq!(err.kind(), io::ErrorKind::Other);
                assert_eq!(err.to_string(), "fill failed");
            }
            other => panic!("expected read IO error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn socket_connection_returns_structured_error_for_oversize_request() {
        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(handle_connection(server, state));
        let (read_half, mut write_half) = client.into_split();
        let oversize_request = vec![b'x'; MAX_REQUEST_SIZE + 1];

        write_half.write_all(&oversize_request).await.unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half.shutdown().await.unwrap();

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let response: JsonRpcResponse = serde_json::from_str(line.trim_end()).unwrap();

        assert!(!response.ok);
        assert_eq!(response.id, Value::Null);
        assert_eq!(response.error.unwrap().code, "request_too_large");
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn capabilities_lists_only_dispatchable_methods() {
        let (state, _backend) = test_state();
        let result = dispatch(&state, "system.capabilities", json!({}))
            .await
            .unwrap();
        let methods = result["methods"].as_array().unwrap();
        assert!(methods.iter().any(|m| m == "system.ping"));
        assert!(methods.iter().any(|m| m == "events.subscribe"));
        // Every advertised method except the connection-level events.subscribe
        // must resolve to a dispatch arm (not MethodNotFound).
        for method in METHODS {
            if *method == "events.subscribe" {
                continue;
            }
            if let Err(DispatchError::MethodNotFound(_)) = dispatch(&state, method, json!({})).await
            {
                panic!("advertised method {method} has no dispatch handler");
            }
        }
    }

    #[tokio::test]
    async fn events_subscribe_replays_then_streams_live_events() {
        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server_task = tokio::spawn(handle_connection(server, state.clone()));
        let (read_half, mut write_half) = client.into_split();
        write_half
            .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":true}}"#)
            .await
            .unwrap();
        write_half.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"subscribed\""), "first line is handshake");

        // Emit a distinctive live event the way the tick task would.
        state
            .events
            .send(ModelEvent::WorkspaceSelected {
                id: Some("LIVE".to_string()),
            })
            .unwrap();

        // Collect lines until both the replayed workspace and the live event appear.
        let mut saw_replay = false;
        let mut saw_live = false;
        for _ in 0..50 {
            let mut buf = String::new();
            let read = tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut buf))
                .await
                .expect("stream did not stall")
                .unwrap();
            assert!(read > 0, "stream closed unexpectedly");
            if buf.contains("workspace_added") {
                saw_replay = true;
            }
            if buf.contains("\"LIVE\"") {
                saw_live = true;
            }
            if saw_replay && saw_live {
                break;
            }
        }
        assert!(saw_replay, "replay emitted the bootstrapped workspace");
        assert!(saw_live, "live event reached the subscriber");

        // The server blocks on recv() until its next write fails, so abort it
        // rather than awaiting completion.
        drop(write_half);
        drop(reader);
        server_task.abort();
    }

    #[test]
    fn lagged_notice_reports_dropped_count() {
        assert_eq!(lagged_notice(7), json!({"event": "lagged", "dropped": 7}));
    }

    #[tokio::test]
    async fn events_subscribe_ends_when_idle_client_disconnects() {
        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server_task = tokio::spawn(handle_connection(server, state.clone()));
        let (read_half, mut write_half) = client.into_split();
        write_half
            .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":false}}"#)
            .await
            .unwrap();
        write_half.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("subscribed"));

        // Disconnect with no events ever broadcast: the server must notice the
        // closed socket and return, releasing its connection permit.
        drop(reader);
        drop(write_half);
        tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .expect("server did not exit on idle disconnect")
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn browser_open_creates_browser_surface_and_navigate_updates_url() {
        let (state, _backend) = test_state();
        let created = dispatch(&state, "workspace.create", json!({"name": "w"}))
            .await
            .unwrap();
        let ws_id = created["id"].as_str().unwrap().to_string();

        let opened = dispatch(
            &state,
            "browser.open",
            json!({"workspace_id": ws_id, "url": "example.com"}),
        )
        .await
        .unwrap();
        let surface_id = opened["id"].as_str().unwrap().to_string();
        // Bare domain gets https:// prepended. Kind now carries the profile id too.
        assert_eq!(opened["kind"]["type"], json!("browser"));
        assert_eq!(opened["kind"]["url"], json!("https://example.com"));

        let navigated = dispatch(
            &state,
            "browser.navigate",
            json!({"surface_id": surface_id, "url": "https://other.com"}),
        )
        .await
        .unwrap();
        assert_eq!(navigated["navigated"], json!(true));

        // navigate on a non-browser surface errors.
        let term = created["focused_surface_id"].as_str().unwrap();
        let err = dispatch(
            &state,
            "browser.navigate",
            json!({"surface_id": term, "url": "https://x.com"}),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DispatchError::NotFound(_)));
    }

    // --- SP2 browser scripting verbs ---------------------------------------

    fn state_with_browser_channel() -> (
        SocketAppState,
        async_channel::Receiver<forktty_core::BrowserCommand>,
    ) {
        let (state, _backend) = test_state();
        let (tx, rx) = async_channel::unbounded();
        (state.with_browser_cmd(tx), rx)
    }

    async fn open_browser_surface(state: &SocketAppState) -> String {
        let ws = dispatch(state, "workspace.create", json!({"name": "w"}))
            .await
            .unwrap();
        let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();
        let surface = dispatch(
            state,
            "browser.open",
            json!({"workspace_id": workspace_id, "url": "https://example.com"}),
        )
        .await
        .unwrap();
        surface.get("id").unwrap().as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn browser_snapshot_unavailable_without_channel() {
        let (state, _backend) = test_state();
        let sid = open_browser_surface(&state).await;
        let err = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("browser automation unavailable"));
    }

    #[tokio::test]
    async fn browser_snapshot_returns_stub_json() {
        let (state, rx) = state_with_browser_channel();
        let sid = open_browser_surface(&state).await;
        let responder = tokio::spawn(async move {
            let cmd = rx.recv().await.unwrap();
            assert_eq!(cmd.op, forktty_core::BrowserOp::Snapshot);
            cmd.reply
                .send(forktty_core::CmdResult::Json("{\"role\":\"root\"}".into()))
                .unwrap();
        });
        let result = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
            .await
            .unwrap();
        assert_eq!(result, json!({"role": "root"}));
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn browser_back_returns_ok() {
        let (state, rx) = state_with_browser_channel();
        let sid = open_browser_surface(&state).await;
        let responder = tokio::spawn(async move {
            let cmd = rx.recv().await.unwrap();
            assert_eq!(cmd.op, forktty_core::BrowserOp::Back);
            cmd.reply.send(forktty_core::CmdResult::Ok).unwrap();
        });
        let result = dispatch(&state, "browser.back", json!({"surface_id": sid}))
            .await
            .unwrap();
        assert_eq!(result, json!({"ok": true}));
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn browser_eval_rejects_oversize_script() {
        let (state, _rx) = state_with_browser_channel();
        let sid = open_browser_surface(&state).await;
        let big = "x".repeat(forktty_core::MAX_BROWSER_SCRIPT_BYTES + 1);
        let err = dispatch(
            &state,
            "browser.eval",
            json!({"surface_id": sid, "script": big}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "payload_too_large");
    }

    #[tokio::test]
    async fn browser_click_on_terminal_surface_is_not_found() {
        let (state, _rx) = state_with_browser_channel();
        let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
            .await
            .unwrap();
        let workspace_id = ws.get("id").unwrap().as_str().unwrap();
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let term_id = surfaces.as_array().unwrap()[0]
            .get("id")
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        let err = dispatch(
            &state,
            "browser.click",
            json!({"surface_id": term_id, "ref": "e1"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "not_found");
    }

    #[tokio::test]
    async fn browser_click_maps_ref_not_found_reply() {
        let (state, rx) = state_with_browser_channel();
        let sid = open_browser_surface(&state).await;
        let responder = tokio::spawn(async move {
            let cmd = rx.recv().await.unwrap();
            cmd.reply
                .send(forktty_core::CmdResult::Err(
                    forktty_core::BrowserCmdError::RefNotFound,
                ))
                .unwrap();
        });
        let err = dispatch(
            &state,
            "browser.click",
            json!({"surface_id": sid, "ref": "e1"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "not_found");
        responder.await.unwrap();
    }

    // --- SP3 P2 browser.profile verbs ----------------------------------------

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_profile_create_list_then_open_with_profile() {
        // Isolate profiles.json from the real user data dir.
        // XDG_DATA_HOME is process-global; serialize with capabilities test via
        // #[serial_test::serial] and restore on any exit path via EnvGuard.
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

        let (state, _backend) = test_state();

        // Create a workspace so we have a workspace_id for browser.open.
        let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
            .await
            .unwrap();
        let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();

        // browser.profile.create
        let created = dispatch(
            &state,
            "browser.profile.create",
            json!({ "display_name": "Work" }),
        )
        .await
        .unwrap();
        let new_id = created
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert_eq!(created["display_name"], json!("Work"));

        // browser.profile.list — should have Default (is_default=true) + Work
        let listed = dispatch(&state, "browser.profile.list", json!({}))
            .await
            .unwrap();
        let arr = listed.as_array().unwrap();
        assert!(
            arr.iter().any(|p| p["is_default"] == json!(true)),
            "list must contain the default profile"
        );
        assert!(
            arr.iter().any(|p| p["display_name"] == json!("Work")),
            "list must contain Work profile"
        );

        // browser.open with profile name — resolves "Work" to its id
        let opened = dispatch(
            &state,
            "browser.open",
            json!({
                "workspace_id": workspace_id,
                "url": "https://example.com",
                "profile": "Work"
            }),
        )
        .await
        .unwrap();
        assert!(opened.get("id").is_some(), "opened surface must have an id");
        // Surface kind should be a browser
        assert_eq!(opened["kind"]["type"], json!("browser"));

        // browser.profile.delete while a pane is open in that profile must be refused
        let del_err = dispatch(&state, "browser.profile.delete", json!({ "id": new_id }))
            .await
            .unwrap_err();
        assert!(
            del_err.to_string().contains("in use"),
            "expected in-use error, got: {del_err}"
        );

        // _env (EnvGuard) and dir (TempDir) are dropped here, restoring the
        // environment and removing temporary files on any exit path.
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_open_rejects_non_string_profile_param() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

        let (state, _backend) = test_state();
        let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
            .await
            .unwrap();
        let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();

        let err = dispatch(
            &state,
            "browser.open",
            json!({
                "workspace_id": workspace_id,
                "url": "https://example.com",
                "profile": 123
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert_eq!(
            err.to_string(),
            "Invalid parameter profile: expected string"
        );

        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": ws.get("id").unwrap().as_str().unwrap()}),
        )
        .await
        .unwrap();
        assert_eq!(surfaces.as_array().unwrap().len(), 1);
    }

    #[test]
    #[serial_test::serial]
    fn browser_profile_create_serializes_store_writes() {
        let dir = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

        let (state, _backend) = test_state();
        let task_count = 24;
        let barrier = Arc::new(Barrier::new(task_count));
        let mut handles = Vec::new();
        for index in 0..task_count {
            let state = state.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap();
                barrier.wait();
                runtime
                    .block_on(dispatch(
                        &state,
                        "browser.profile.create",
                        json!({ "display_name": format!("Profile {index}") }),
                    ))
                    .unwrap();
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let listed = runtime
            .block_on(dispatch(&state, "browser.profile.list", json!({})))
            .unwrap();
        let profiles = listed.as_array().unwrap();
        assert_eq!(profiles.len(), task_count + 1);
        for index in 0..task_count {
            assert!(
                profiles
                    .iter()
                    .any(|profile| profile["display_name"] == json!(format!("Profile {index}"))),
                "missing Profile {index}"
            );
        }
    }

    // --- SP3 P3 browser.history + browser.bookmark verbs ---------------------

    #[test]
    fn browser_history_limit_defaults_and_caps() {
        assert_eq!(history_limit_from_params(&json!({})), 100);
        assert_eq!(history_limit_from_params(&json!({"limit": 5})), 5);
        assert_eq!(
            history_limit_from_params(&json!({"limit": u64::MAX})),
            10_000
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_history_list_and_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let (state, _backend) = test_state();

        // history list on a fresh profile is empty
        let hist = dispatch(&state, "browser.history.list", json!({}))
            .await
            .unwrap();
        assert!(
            hist.as_array().unwrap().is_empty(),
            "fresh history must be empty"
        );

        // clear is a no-op success on empty history
        let cleared = dispatch(&state, "browser.history.clear", json!({}))
            .await
            .unwrap();
        assert_eq!(cleared["cleared"], json!(true));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_bookmark_add_list_remove_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let (state, _backend) = test_state();

        // add
        let added = dispatch(
            &state,
            "browser.bookmark.add",
            json!({"url": "https://a.test/", "title": "A"}),
        )
        .await
        .unwrap();
        assert_eq!(added["added"], json!(true));

        // list
        let listed = dispatch(&state, "browser.bookmark.list", json!({}))
            .await
            .unwrap();
        let arr = listed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["url"], json!("https://a.test/"));
        assert_eq!(arr[0]["title"], json!("A"));

        // remove
        let removed = dispatch(
            &state,
            "browser.bookmark.remove",
            json!({"url": "https://a.test/"}),
        )
        .await
        .unwrap();
        assert_eq!(removed["removed"], json!(true));

        // list is now empty
        let listed2 = dispatch(&state, "browser.bookmark.list", json!({}))
            .await
            .unwrap();
        assert!(listed2.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_history_search_requires_query() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let (state, _backend) = test_state();

        let err = dispatch(&state, "browser.history.search", json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "missing_param");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_bookmark_add_rejects_empty_url() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let (state, _backend) = test_state();

        let err = dispatch(&state, "browser.bookmark.add", json!({"url": "   "}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn browser_history_search_returns_results() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
        let (state, _backend) = test_state();

        // history is empty so search returns empty array (not an error)
        let results = dispatch(
            &state,
            "browser.history.search",
            json!({"query": "example"}),
        )
        .await
        .unwrap();
        assert!(results.as_array().unwrap().is_empty());
    }
}
