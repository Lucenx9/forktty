use forktty_core::{
    JsonRpcRequest, JsonRpcResponse, NotificationKind, SplitAxis, WorkspaceModel, WorkspaceSelector,
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
        }
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
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))?;
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
            let cwd = params
                .get("workingDir")
                .or_else(|| params.get("working_dir"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
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
            let workspace = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .close_workspace(selector)
                    .ok_or_else(|| "Workspace not found".to_string())?
            };
            Ok(json!(workspace))
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
            Ok(json!(item))
        }
        "notification.list" => {
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            Ok(json!(model.list_notifications()))
        }
        _ => Err(format!("Unknown method: {method}")),
    }
}

fn workspace_selector_from_params(params: &Value) -> Result<WorkspaceSelector<'_>, String> {
    if let Some(id) = params.get("id").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Id(id));
    }
    if let Some(id) = params.get("workspace_id").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Id(id));
    }
    if let Some(name) = params.get("name").and_then(Value::as_str) {
        return Ok(WorkspaceSelector::Name(name));
    }
    if let Some(name) = params.get("workspace_name").and_then(Value::as_str) {
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
    use forktty_terminal::HeadlessTerminalBackend;
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
        );
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        (state, backend)
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
    }

    #[tokio::test]
    async fn dispatches_workspace_and_surface_parity_methods() {
        let (state, _backend) = test_state();
        let created = dispatch(
            &state,
            "workspace.create",
            json!({"name": "feature", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        assert_eq!(created["name"], "feature");

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
