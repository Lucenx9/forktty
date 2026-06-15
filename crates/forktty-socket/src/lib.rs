use forktty_core::events::{self, ModelEvent, Snapshot};
use forktty_core::{
    agent_resume_command_with_cwd_and_permission_mode, codex_session_cwd,
    command_safety::is_valid_ssh_host, config, dispatch_notification, normalize_agent_status,
    validate_worktree_name, worktree, AgentKind, AgentResumeError, AgentSession,
    AgentSessionLifecycle, AgentStatus, BrowserCmdError, BrowserCommand, BrowserOp, CmdResult,
    JsonRpcRequest, JsonRpcResponse, LogLevel, NotificationKind, SplitAxis, StatusHookMetadata,
    WorkspaceModel, WorkspaceSelector, MAX_BROWSER_URL_BYTES,
};
use forktty_terminal::{
    spawn::resolve_child_program, SharedTerminalBackend, SpawnRequest, TerminalError,
    TerminalTextCapture,
};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::{self, DirBuilder};
use std::io::{self, Seek, SeekFrom};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener as StdUnixListener, UnixStream as StdUnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::{broadcast, Semaphore};

const MAX_REQUEST_SIZE: usize = 1_048_576;
const MAX_SEND_TEXT_BYTES: usize = 262_144;
const MAX_TERMINAL_TEXT_BYTES: usize = 512 * 1024;
const DEFAULT_CAPTURE_TAIL_LINES: usize = 80;
const MAX_CAPTURE_TAIL_LINES: usize = 5_000;
const MAX_METADATA_TEXT_BYTES: usize = 16_384;
const BROWSER_CMD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_SOCKET_CONNECTIONS: usize = 64;
const SOCKET_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
/// Max time to wait for a client to deliver a complete request line before the
/// connection is dropped. Clients send a full `{json}\n` immediately, so this
/// only fires on idle or slow-loris connections that would otherwise hold one
/// of the [`MAX_SOCKET_CONNECTIONS`] permits indefinitely.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// Buffered events per subscriber before a slow client gets a `Lagged` notice.
const EVENTS_CHANNEL_CAPACITY: usize = 256;
/// How often the background task snapshots the model and emits diffs.
const EVENTS_TICK: Duration = Duration::from_millis(250);
/// Max time one event write to a subscriber may take. A subscriber that
/// stops reading fills the kernel socket buffer and would otherwise block
/// `stream_events` forever, holding one of the [`MAX_SOCKET_CONNECTIONS`]
/// permits; enough of those would deny the socket to agent hooks.
const EVENTS_WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Max time one response write may take. Responses can far exceed the kernel
/// socket buffer (`notification.list` or `metadata.list_logs` can reach tens
/// of MB), so a client that sends a request and then stops reading would
/// otherwise park `write_response` on a full buffer forever, holding one of
/// the [`MAX_SOCKET_CONNECTIONS`] permits. Generous so a legitimately large
/// response to a slow reader still gets through.
const RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const HOOK_SESSION_TARGET_CAPACITY: usize = 256;
/// Pending desktop notification dispatch jobs. Dispatch can block in
/// notify-rust/custom commands, so keep a small bounded queue rather than
/// spawning one OS thread per socket request.
const NOTIFICATION_DISPATCH_QUEUE_CAPACITY: usize = 64;
const DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS: u64 = 10 * 60 * 1_000;
/// Distinguishes concurrent [`bind_private_socket_path`] staging directories
/// within one process (tests bind many listeners in parallel).
static SOCKET_BIND_STAGING_SEQ: AtomicU64 = AtomicU64::new(0);
static NOTIFICATION_DISPATCHER: OnceLock<mpsc::SyncSender<forktty_core::NotificationItem>> =
    OnceLock::new();

/// Methods advertised by `system.capabilities`. Every entry except
/// `events.subscribe` (handled at the connection level, not in [`dispatch`]) is
/// covered by a `dispatch` match arm; the `capabilities_lists_dispatchable`
/// test guards against an entry here that has no handler.
#[cfg(feature = "browser")]
pub const METHODS: &[&str] = &[
    "browser.back",
    "browser.bookmark.add",
    "browser.bookmark.list",
    "browser.bookmark.remove",
    "browser.click",
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
    "agent.health",
    "agent.list",
    "agent.reclaim.plan",
    "agent.resume",
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
    "pane.new_tab",
    "pane.select_tab",
    "surface.close",
    "surface.focus",
    "surface.capture_tail",
    "surface.list",
    "surface.read_text",
    "surface.send_text",
    "surface.split",
    "status.summary",
    "system.capabilities",
    "system.ping",
    "topology.tree",
    "workspace.close",
    "workspace.create",
    "workspace.create_ssh",
    "workspace.list",
    "workspace.select",
    "worktree.attach",
    "worktree.create",
    "worktree.list",
    "worktree.merge",
    "worktree.remove",
    "worktree.status",
];

#[cfg(not(feature = "browser"))]
pub const METHODS: &[&str] = &[
    "events.subscribe",
    "agent.health",
    "agent.list",
    "agent.reclaim.plan",
    "agent.resume",
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
    "pane.new_tab",
    "pane.select_tab",
    "surface.close",
    "surface.focus",
    "surface.capture_tail",
    "surface.list",
    "surface.read_text",
    "surface.send_text",
    "surface.split",
    "status.summary",
    "system.capabilities",
    "system.ping",
    "topology.tree",
    "workspace.close",
    "workspace.create",
    "workspace.create_ssh",
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
    PreconditionFailed(String),
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
            DispatchError::PreconditionFailed(_) => "precondition_failed",
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
            DispatchError::PreconditionFailed(message) => f.write_str(message),
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
    hook_session_targets: Arc<Mutex<HookSessionTargets>>,
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
            hook_session_targets: Arc::new(Mutex::new(HookSessionTargets::default())),
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

#[derive(Clone, Debug, PartialEq, Eq)]
struct HookSessionTarget {
    workspace_id: String,
    surface_id: String,
}

#[derive(Default, Debug)]
struct HookSessionTargets {
    entries: HashMap<String, HookSessionTarget>,
    order: VecDeque<String>,
}

impl HookSessionTargets {
    fn learn(&mut self, session_id: String, target: HookSessionTarget) {
        if self.entries.contains_key(&session_id) {
            self.entries.insert(session_id.clone(), target);
            self.order.retain(|existing| existing != &session_id);
            self.order.push_back(session_id);
            return;
        }

        while self.entries.len() >= HOOK_SESSION_TARGET_CAPACITY {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }

        self.entries.insert(session_id.clone(), target);
        self.order.push_back(session_id);
    }

    fn get(&self, session_id: &str) -> Option<&HookSessionTarget> {
        self.entries.get(session_id)
    }

    fn remove_session(&mut self, session_id: &str) {
        if self.entries.remove(session_id).is_some() {
            self.order.retain(|existing| existing != session_id);
        }
    }

    fn remove_surface(&mut self, surface_id: &str) {
        let removed = self
            .entries
            .iter()
            .filter(|(_, target)| target.surface_id == surface_id)
            .map(|(session_id, _)| session_id.clone())
            .collect::<Vec<_>>();
        for session_id in removed {
            self.entries.remove(&session_id);
        }
        self.order
            .retain(|session_id| self.entries.contains_key(session_id));
    }
}

pub fn default_socket_path() -> PathBuf {
    default_socket_dir().join("forktty.sock")
}

/// Resolve the socket path from an optional override, falling back to the
/// default. An override is honored only when it trims to a non-empty absolute
/// path; anything else (relative, blank, unset) uses [`default_socket_path`].
pub fn socket_path_from_env(socket_env: Option<String>) -> PathBuf {
    socket_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && Path::new(value).is_absolute())
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path)
}

pub fn bind_socket_listener(
    socket_path: impl AsRef<Path>,
    enforce_private_parent: bool,
) -> io::Result<StdUnixListener> {
    let socket_path = socket_path.as_ref();
    prepare_socket_parent(socket_path, enforce_private_parent)?;
    // Removing a stale socket and re-binding is not atomic: a racing ForkTTY
    // start can recreate the path between `remove_file` and `bind`, surfacing a
    // bare `AddrInUse`. Re-inspect once on that error so the occupant is
    // reported accurately instead of as a confusing bind failure.
    let mut reclaimed_stale = false;
    let listener = loop {
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
        match bind_private_socket_path(socket_path) {
            Ok(listener) => break listener,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse && !reclaimed_stale => {
                reclaimed_stale = true;
                continue;
            }
            Err(err) => return Err(err),
        }
    };
    listener.set_nonblocking(true)?;
    if let Err(err) = fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600)) {
        let _ = fs::remove_file(socket_path);
        return Err(err);
    }
    Ok(listener)
}

/// Binds the socket without it ever being group/other-accessible at its public
/// path: the listener is created inside a chmod-0700 staging directory (whose
/// search bit shields the inode regardless of umask), chmod-0600, and only then
/// hard-linked into place. Linking fails with `AddrInUse` if the public path
/// appeared in the meantime, preserving the caller's stale-reclaim retry.
///
/// Deliberately NOT implemented with a process-wide `umask()` flip: the umask
/// is global, so flipping it poisons the creation mode of every file or
/// directory any other thread makes during the window (this broke concurrent
/// tests' `tempdir()` with mode-0600 directories).
fn bind_private_socket_path(socket_path: &Path) -> io::Result<StdUnixListener> {
    let parent = socket_path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path has no parent directory",
        )
    })?;
    let staging = parent.join(format!(
        ".forktty-bind-{}-{}",
        std::process::id(),
        SOCKET_BIND_STAGING_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    // A leftover directory can only come from a crashed process that had this
    // pid in a previous boot; the sequence number never repeats within one.
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging)?;
    let result = bind_in_staging_dir(&staging, socket_path);
    let _ = fs::remove_dir_all(&staging);
    result
}

fn bind_in_staging_dir(staging: &Path, socket_path: &Path) -> io::Result<StdUnixListener> {
    fs::set_permissions(staging, fs::Permissions::from_mode(0o700))?;
    let staged = staging.join("sock");
    let listener = StdUnixListener::bind(&staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600))?;
    fs::hard_link(&staged, socket_path).map_err(|err| {
        if err.kind() == io::ErrorKind::AlreadyExists {
            io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("socket path {} is already in use", socket_path.display()),
            )
        } else {
            err
        }
    })?;
    Ok(listener)
}

pub async fn serve(listener: StdUnixListener, state: SocketAppState) -> Result<(), SocketError> {
    let listener = UnixListener::from_std(listener)?;
    spawn_event_tick(state.clone());
    let connection_limit = Arc::new(Semaphore::new(MAX_SOCKET_CONNECTIONS));
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            // A client aborting mid-handshake (or a transient kernel hiccup)
            // must not take the whole IPC server down for the rest of the
            // process lifetime; only give up on genuinely fatal errors.
            Err(err) if is_transient_accept_error(&err) => {
                // Brief pause so accept() does not hot-spin while fds are
                // exhausted (EMFILE/ENFILE persists until something closes).
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(err) => return Err(err.into()),
        };
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

/// Whether an `accept()` failure is transient and the loop should keep
/// serving. Besides peer-side aborts, this covers process/system fd
/// exhaustion (EMFILE/ENFILE): those surface as `ErrorKind::Uncategorized`,
/// so they are matched by raw errno, and they clear once a connection (or
/// any other fd) closes — returning the error would kill the IPC server for
/// the rest of the process lifetime.
fn is_transient_accept_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
    ) || matches!(err.raw_os_error(), Some(libc::EMFILE) | Some(libc::ENFILE))
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
            // try_lock: if the GUI main thread holds the model lock, skip
            // this tick instead of parking a tokio worker on a std mutex;
            // the next tick (250ms) picks the changes up.
            let next = match state.model.try_lock() {
                Ok(model) => events::snapshot(&model),
                Err(std::sync::TryLockError::WouldBlock) => continue,
                // A poisoned lock must not be diffed as an empty snapshot:
                // that would broadcast a false removal of every workspace
                // and surface to all subscribers. Skip the tick instead.
                Err(std::sync::TryLockError::Poisoned(_)) => continue,
            };
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
        // Snapshot the poisoned data rather than pretending the model is
        // empty: replaying an empty world to a late subscriber is strictly
        // worse than a possibly mid-mutation (read-only) snapshot, and the
        // next healthy tick re-asserts the true state anyway.
        Err(poisoned) => events::snapshot(&poisoned.into_inner()),
    }
}

async fn reject_over_capacity_connection(stream: tokio::net::UnixStream) {
    let (_, mut writer) = stream.into_split();
    let response = JsonRpcResponse::error(
        Value::Null,
        "server_busy",
        format!("Too many active socket connections (limit {MAX_SOCKET_CONNECTIONS})"),
    );
    if let Err(err) = write_response(&mut writer, &response, RESPONSE_WRITE_TIMEOUT).await {
        eprintln!("forktty socket busy response failed: {err}");
    }
}

pub async fn dispatch(
    state: &SocketAppState,
    method: &str,
    params: Value,
) -> Result<Value, DispatchError> {
    #[cfg(not(feature = "browser"))]
    if method.starts_with("browser.") {
        return Err(DispatchError::MethodNotFound(method.to_string()));
    }

    let mut params = params;
    let _hook_session_end = prepare_hook_session_targets(state, method, &mut params)?;

    match method {
        "system.ping" => Ok(json!("pong")),
        "system.capabilities" => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "methods": METHODS,
        })),
        "agent.health" => {
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
            Ok(json!(agent_health_rows(&model, workspace_id.as_deref())))
        }
        "agent.list" => {
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
            Ok(json!(agent_session_rows(&model, workspace_id.as_deref())))
        }
        "agent.reclaim.plan" => {
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
            let min_idle_ms = optional_u64_param(&params, "min_idle_ms")?
                .unwrap_or(DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS);
            Ok(agent_reclaim_plan(
                &model,
                workspace_id.as_deref(),
                min_idle_ms,
            ))
        }
        "agent.resume" => resume_agent_session(state, &params),
        "status.summary" => {
            let workspace_id = resolve_workspace_id_for_metadata(state, &params)?;
            let model = state
                .model
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            status_summary(&model, &workspace_id)
                .ok_or(DispatchError::NotFound("workspace".to_string()))
        }
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
        "workspace.create_ssh" => {
            let host = required_ssh_host_param(&params)?;
            let name = workspace_create_name_from_params(&params)?;
            let cwd = resolve_workspace_cwd_param(&params)?;
            let (workspace, previous_active_id) = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let previous_active_id = model.active_workspace_id();
                (
                    model.create_ssh_workspace(name, cwd, host.to_string()),
                    previous_active_id,
                )
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
                    if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                        state,
                        &replacement.focused_surface_id,
                    ) {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    return Err(err.into());
                }
                let closed = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    model.close_workspace(WorkspaceSelector::Id(&workspace_id))
                };
                if closed.is_none() {
                    // A concurrent close won the race: the workspace is
                    // already gone, and if the winner spawned its own
                    // replacement ours is a duplicate that must be rolled
                    // back instead of leaving two "main" workspaces behind.
                    let rolled_back = rollback_replacement_if_redundant(
                        state,
                        &replacement.id,
                        previous_active_id,
                    )?;
                    if rolled_back {
                        if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                            state,
                            &replacement.focused_surface_id,
                        ) {
                            return Err(format!(
                                "Workspace not found; replacement cleanup failed: {cleanup_err}"
                            )
                            .into());
                        }
                    }
                    return Err(DispatchError::NotFound("workspace".to_string()));
                }
                evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
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
            evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!(workspace))
        }
        "worktree.list" => {
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd").await?;
            let worktrees = run_worktree_blocking(move || worktree::list(&cwd)).await?;
            Ok(json!(worktrees))
        }
        "worktree.status" => {
            let path = resolve_open_repo_cwd_param(state, &params, &["path", "cwd"], "path or cwd")
                .await?;
            let status = run_worktree_blocking(move || worktree::status(&path)).await?;
            Ok(json!({"status": status}))
        }
        "worktree.create" => {
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd").await?;
            let info = {
                let cwd = cwd.clone();
                let name = name.to_string();
                // worktree_layout() reads config.toml, so it joins the
                // blocking task too.
                run_worktree_blocking(move || {
                    let layout = worktree_layout();
                    worktree::create(&cwd, &name, &layout)
                })
                .await?
            };
            let workspace = match open_worktree_workspace(state, &info).await {
                Ok(workspace) => workspace,
                Err(err) => {
                    let message = match tokio::task::spawn_blocking(move || {
                        rollback_created_worktree_after_spawn_failure(&cwd, &info, err)
                    })
                    .await
                    {
                        Ok(message) => message,
                        Err(join_err) => format!("worktree rollback task failed: {join_err}"),
                    };
                    return Err(message.into());
                }
            };
            notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
            Ok(json!({
                "id": workspace.id,
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
                "setup_warning": info.setup_warning,
            }))
        }
        "worktree.attach" => {
            let name = worktree_name_from_params(&params, &["name", "branch"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd").await?;
            let info = {
                let name = name.to_string();
                run_worktree_blocking(move || {
                    let layout = worktree_layout();
                    worktree::attach(&cwd, &name, &layout)
                })
                .await?
            };
            let workspace = open_worktree_workspace(state, &info).await?;
            notify_worktree_setup_warning(state, &workspace.id, info.setup_warning.as_deref())?;
            Ok(json!({
                "id": workspace.id,
                "name": info.name,
                "path": info.path,
                "branch": info.branch,
                "worktree_name": info.worktree_name,
                "setup_warning": info.setup_warning,
            }))
        }
        "worktree.remove" => {
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd").await?;
            let (fallback_path, removal) = {
                let name = name.to_string();
                run_worktree_blocking(move || {
                    let fallback =
                        worktree::repository_root(&cwd).unwrap_or_else(|_| PathBuf::from(&cwd));
                    worktree::prepare_remove(&cwd, &name).map(|removal| (fallback, removal))
                })
                .await?
            };
            let workspace_worktree_name = removal.worktree_name().to_string();
            let (workspace, surfaces, is_last_workspace) = {
                let model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                let workspace = model.list_workspaces().into_iter().find(|workspace| {
                    workspace.worktree_name.as_deref() == Some(workspace_worktree_name.as_str())
                });
                let surfaces = workspace
                    .as_ref()
                    .map(|workspace| model.list_surfaces(Some(&workspace.id)))
                    .unwrap_or_default();
                let is_last_workspace = workspace.is_some() && model.list_workspaces().len() == 1;
                (workspace, surfaces, is_last_workspace)
            };
            let surface_ids = surfaces
                .iter()
                .map(|surface| surface.id.clone())
                .collect::<Vec<_>>();
            if workspace.is_none() {
                finish_removal_blocking(removal, false).await?;
                return Ok(json!({"removed": name}));
            }
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
                    if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                        state,
                        &replacement.focused_surface_id,
                    ) {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    return Err(err.into());
                }
                if let Err(err) = finish_removal_blocking(removal, false).await {
                    let mut err = err.to_string();
                    if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                        state,
                        &replacement.focused_surface_id,
                    ) {
                        err = format!("{err}; replacement cleanup failed: {cleanup_err}");
                    }
                    if let Err(rollback_err) =
                        rollback_workspace_creation(state, &replacement.id, previous_active_id)
                    {
                        err = format!("{err}; workspace rollback failed: {rollback_err}");
                    }
                    if let Err(respawn_err) = spawn_terminal_surfaces(state, &surfaces) {
                        err = format!("{err}; terminal restore failed: {respawn_err}");
                    }
                    return Err(err.into());
                }
                let closed = {
                    let mut model = state
                        .model
                        .lock()
                        .map_err(|_| "Lock poisoned".to_string())?;
                    model.close_workspace(WorkspaceSelector::Id(&workspace.id))
                };
                if closed.is_none() {
                    // A concurrent close already removed this workspace. The
                    // worktree itself is gone (removal committed above and
                    // cannot be undone), so the removal still succeeded; only
                    // roll back our now-redundant replacement instead of
                    // leaving a duplicate "main" workspace behind.
                    let rolled_back = rollback_replacement_if_redundant(
                        state,
                        &replacement.id,
                        previous_active_id,
                    )?;
                    if rolled_back {
                        if let Err(cleanup_err) = close_replacement_terminal_surface_if_present(
                            state,
                            &replacement.focused_surface_id,
                        ) {
                            return Err(format!(
                                "Worktree removed but replacement cleanup failed: {cleanup_err}"
                            )
                            .into());
                        }
                    }
                }
                evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
                return Ok(json!({"removed": name}));
            }
            close_terminal_surfaces_if_present(state, &surface_ids)?;
            if let Err(err) = finish_removal_blocking(removal, false).await {
                let mut err = err.to_string();
                if let Err(respawn_err) = spawn_terminal_surfaces(state, &surfaces) {
                    err = format!("{err}; terminal restore failed: {respawn_err}");
                }
                return Err(err.into());
            }
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
            evict_hook_session_targets_for_surfaces(state, &surface_ids)?;
            ensure_terminal_for_active_workspace(state).await?;
            Ok(json!({"removed": name}))
        }
        "worktree.merge" => {
            let name = worktree_name_from_params(&params, &["name"], "name")?;
            let cwd = resolve_open_repo_cwd_param(state, &params, &["cwd"], "cwd").await?;
            let result = {
                let name = name.to_string();
                run_worktree_blocking(move || worktree::merge(&cwd, &name)).await?
            };
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
        "surface.read_text" => {
            let surface_id = required_surface_id(&params)?;
            let capture = terminal_text_capture_from_params(&params)?;
            let max_bytes = terminal_text_max_bytes_from_params(&params)?;
            ensure_model_surface_exists(state, surface_id)?;
            let snapshot = state
                .terminal
                .read_text(surface_id, capture, max_bytes)
                .map_err(DispatchError::from)?;
            Ok(json!(snapshot))
        }
        "surface.capture_tail" => {
            let surface_id = required_surface_id(&params)?;
            let lines = terminal_tail_lines_from_params(&params)?;
            let max_bytes = terminal_text_max_bytes_from_params(&params)?;
            ensure_model_surface_exists(state, surface_id)?;
            let snapshot = state
                .terminal
                .read_text(surface_id, TerminalTextCapture::Tail { lines }, max_bytes)
                .map_err(DispatchError::from)?;
            Ok(json!(snapshot))
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
        "topology.tree" => {
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
            Ok(topology_tree(&model, workspace_id.as_deref()))
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
            let limit = history_limit_from_params(&params)?;
            let store = forktty_core::HistoryStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let rows = store
                .list(limit)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!(rows))
        }
        "browser.history.search" => {
            let query = required_string(&params, "query")?.to_string();
            let profile = resolve_profile_param(&params)?;
            let limit = history_limit_from_params(&params)?;
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
            let url = required_string_param(&params, "url")?.trim().to_string();
            if url.is_empty() {
                return Err(DispatchError::InvalidParam(
                    "url must not be empty".to_string(),
                ));
            }
            let title = optional_bookmark_title(&params)?;
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
            let url = required_string_param(&params, "url")?.trim().to_string();
            if url.is_empty() {
                return Err(DispatchError::InvalidParam(
                    "url must not be empty".to_string(),
                ));
            }
            let profile = resolve_profile_param(&params)?;
            let mut store = forktty_core::BookmarkStore::for_profile(profile)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            let removed = store
                .remove(&url)
                .map_err(|e| DispatchError::from(e.to_string()))?;
            Ok(json!({ "removed": removed }))
        }
        "browser.import.discover" => Ok(browser_import_discover_json()),
        "browser.import.preview" => browser_import_preview(&params).await,
        "browser.import.run" => browser_import_run(state, &params).await,
        "pane.new_tab" => {
            let surface_id = required_surface_id(&params)?;
            let surface = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model
                    .add_tab(surface_id)
                    .ok_or(DispatchError::NotFound("surface".to_string()))?
            };
            if let Err(err) = spawn_surface_terminal(state, &surface) {
                rollback_surface_creation(state, &surface.id)?;
                return Err(err.into());
            }
            Ok(json!(surface))
        }
        "pane.select_tab" => {
            let surface_id = required_surface_id(&params)?;
            let selected = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                model.select_tab(surface_id)
            };
            if selected {
                Ok(json!({"selected": true}))
            } else {
                Err(DispatchError::NotFound("surface".to_string()))
            }
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
                        close_replacement_terminal_surface_if_present(state, &replacement.id)
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
                    close_replacement_terminal_surface_if_present(state, &replacement.id)?;
                }
                let surface = surface?;
                evict_hook_session_targets_for_surface(state, surface_id)?;
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
            evict_hook_session_targets_for_surface(state, surface_id)?;
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
            let agent_session_lifecycle = agent_session_lifecycle_from_hook(
                value,
                hook.as_ref().map(|hook| hook.event.as_str()),
            );
            let hook_session_id = optional_non_blank_string_param(&params, "hook_session_id")?;
            if let Some(hook_session_id) = hook_session_id {
                ensure_max_text_size("hook_session_id", hook_session_id)?;
            }
            let hook_session_cwd = optional_hook_session_cwd(&params)?;
            let surface_id = optional_surface_id_param(&params)?;
            let agent = agent_kind_from_status_key(key);
            let last_activity_ms = current_unix_epoch_ms();
            let status = {
                let mut model = state
                    .model
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?;
                if let (Some(agent), Some(surface_id), Some(hook_session_id)) =
                    (agent, surface_id, hook_session_id)
                {
                    if model
                        .surface(surface_id)
                        .is_some_and(|surface| surface.workspace_id == workspace_id)
                    {
                        model.set_surface_agent_session(surface_id, agent, hook_session_id);
                        if let Some(hook_session_cwd) = hook_session_cwd {
                            model
                                .set_surface_agent_session_resume_cwd(surface_id, hook_session_cwd);
                        }
                        model.set_surface_agent_session_lifecycle(
                            surface_id,
                            agent_session_lifecycle,
                        );
                        model.set_surface_agent_session_last_activity_ms(
                            surface_id,
                            last_activity_ms,
                        );
                    }
                }
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

fn method_allowed_from_socket(method: &str) -> bool {
    !method.starts_with("browser.import.")
}

fn agent_session_rows(model: &WorkspaceModel, workspace_id: Option<&str>) -> Vec<Value> {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.id.clone(), workspace))
        .collect::<HashMap<_, _>>();

    model
        .list_surfaces(workspace_id)
        .into_iter()
        .filter_map(|surface| {
            let agent_session = surface.agent_session?;
            let workspace_name = workspaces
                .get(&surface.workspace_id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_default();
            Some(json!({
                "workspace_id": surface.workspace_id,
                "workspace_name": workspace_name,
                "surface_id": surface.id,
                "title": surface.title,
                "cwd": surface.cwd,
                "agent": agent_session.agent,
                "session_id": agent_session.session_id,
                "resume_cwd": agent_session.resume_cwd,
                "permission_mode": agent_session.permission_mode,
                "lifecycle": agent_session.lifecycle,
                "last_activity_ms": agent_session.last_activity_ms,
            }))
        })
        .collect()
}

fn agent_health_rows(model: &WorkspaceModel, workspace_id: Option<&str>) -> Vec<Value> {
    let path = std::env::var_os("PATH");
    agent_health_rows_with_path(model, workspace_id, path.as_deref())
}

fn agent_health_rows_with_path(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    path: Option<&OsStr>,
) -> Vec<Value> {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .map(|workspace| (workspace.id.clone(), workspace))
        .collect::<HashMap<_, _>>();

    model
        .list_surfaces(workspace_id)
        .into_iter()
        .filter_map(|surface| {
            let agent_session = surface.agent_session?;
            let workspace_name = workspaces
                .get(&surface.workspace_id)
                .map(|workspace| workspace.name.clone())
                .unwrap_or_default();
            let resume_cwd = effective_agent_resume_cwd(&agent_session);
            let (ready, reason, program, executable, argv) = agent_resume_readiness(
                agent_session.agent,
                &agent_session.session_id,
                resume_cwd.as_deref(),
                agent_session.permission_mode.as_deref(),
                path,
            );
            Some(json!({
                "workspace_id": surface.workspace_id,
                "workspace_name": workspace_name,
                "surface_id": surface.id,
                "title": surface.title,
                "cwd": surface.cwd,
                "agent": agent_session.agent,
                "session_id": agent_session.session_id,
                "resume_cwd": resume_cwd,
                "permission_mode": agent_session.permission_mode,
                "lifecycle": agent_session.lifecycle,
                "last_activity_ms": agent_session.last_activity_ms,
                "ready": ready,
                "reason": reason,
                "program": program,
                "executable": executable,
                "argv": argv,
            }))
        })
        .collect()
}

fn agent_reclaim_plan(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    min_idle_ms: u64,
) -> Value {
    let path = std::env::var_os("PATH");
    agent_reclaim_plan_with_path(
        model,
        workspace_id,
        path.as_deref(),
        current_unix_epoch_ms(),
        min_idle_ms,
    )
}

fn agent_reclaim_plan_with_path(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    path: Option<&OsStr>,
    now_ms: u64,
    min_idle_ms: u64,
) -> Value {
    let mut candidates = Vec::new();
    let mut protected = Vec::new();
    for mut row in agent_health_rows_with_path(model, workspace_id, path) {
        let last_activity_ms = row
            .get("last_activity_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let idle_ms = (last_activity_ms > 0).then(|| now_ms.saturating_sub(last_activity_ms));
        if let Some(object) = row.as_object_mut() {
            object.insert(
                "idle_ms".to_string(),
                idle_ms.map(Value::from).unwrap_or(Value::Null),
            );
        }
        if let Some(reason) = agent_reclaim_protect_reason(&row, idle_ms, min_idle_ms) {
            if let Some(object) = row.as_object_mut() {
                object.insert("protect_reason".to_string(), Value::String(reason));
            }
            protected.push(row);
        } else {
            candidates.push(row);
        }
    }

    candidates.sort_by(|a, b| {
        b.get("idle_ms")
            .and_then(Value::as_u64)
            .cmp(&a.get("idle_ms").and_then(Value::as_u64))
            .then_with(|| {
                a.get("surface_id")
                    .and_then(Value::as_str)
                    .cmp(&b.get("surface_id").and_then(Value::as_str))
            })
    });

    json!({
        "policy": {
            "now_ms": now_ms,
            "min_idle_ms": min_idle_ms,
        },
        "candidates": candidates,
        "protected": protected,
    })
}

fn agent_reclaim_protect_reason(
    row: &Value,
    idle_ms: Option<u64>,
    min_idle_ms: u64,
) -> Option<String> {
    match row
        .get("lifecycle")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
    {
        "idle" => {}
        "needs_input" => return Some("needs_input".to_string()),
        "running" => return Some("running".to_string()),
        "ended" => return Some("ended".to_string()),
        _ => return Some("unknown_lifecycle".to_string()),
    }
    let Some(idle_ms) = idle_ms else {
        return Some("unknown_activity".to_string());
    };
    if !row.get("ready").and_then(Value::as_bool).unwrap_or(false) {
        let reason = row
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("not_ready");
        return Some(format!("not_ready:{reason}"));
    }
    if idle_ms < min_idle_ms {
        return Some("recent_activity".to_string());
    }
    None
}

fn agent_session_lifecycle_from_hook(
    status_value: &str,
    hook_event_name: Option<&str>,
) -> AgentSessionLifecycle {
    let hook_event = hook_event_name
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match hook_event.as_str() {
        "session-end" => AgentSessionLifecycle::Ended,
        "stop" | "subagent-stop" | "teammate-idle" => AgentSessionLifecycle::Idle,
        "notification" | "permission-request" | "elicitation" | "ask-user-question" => {
            AgentSessionLifecycle::NeedsInput
        }
        "session-start"
        | "prompt-submit"
        | "prompt-expansion"
        | "setup"
        | "pre-tool"
        | "post-tool"
        | "post-tool-failure"
        | "post-tool-batch"
        | "before-tool-selection"
        | "before-model"
        | "after-model"
        | "permission-denied"
        | "permission-replied"
        | "pre-compact"
        | "post-compact"
        | "config-change"
        | "instructions-loaded"
        | "cwd-changed"
        | "file-changed"
        | "worktree-create"
        | "worktree-remove"
        | "stop-failure" => AgentSessionLifecycle::Running,
        _ => agent_session_lifecycle_from_status(status_value),
    }
}

fn agent_session_lifecycle_from_status(status_value: &str) -> AgentSessionLifecycle {
    match normalize_agent_status(status_value) {
        AgentStatus::Idle | AgentStatus::Done | AgentStatus::Cancelled | AgentStatus::Failed => {
            AgentSessionLifecycle::Idle
        }
        AgentStatus::NeedsInput | AgentStatus::PermissionRequest => {
            AgentSessionLifecycle::NeedsInput
        }
        AgentStatus::Running | AgentStatus::ToolRunning | AgentStatus::TestsRunning => {
            AgentSessionLifecycle::Running
        }
        AgentStatus::Unknown => AgentSessionLifecycle::Unknown,
    }
}

fn effective_agent_resume_cwd(agent_session: &AgentSession) -> Option<PathBuf> {
    agent_session.resume_cwd.clone().or_else(|| {
        (agent_session.agent == AgentKind::Codex)
            .then(|| codex_session_cwd(&agent_session.session_id))
            .flatten()
    })
}

fn agent_resume_readiness(
    agent: AgentKind,
    session_id: &str,
    resume_cwd: Option<&Path>,
    permission_mode: Option<&str>,
    path: Option<&OsStr>,
) -> (bool, &'static str, Value, Value, Vec<String>) {
    let command = match agent_resume_command_with_cwd_and_permission_mode(
        agent,
        session_id,
        resume_cwd,
        permission_mode,
    ) {
        Ok(command) => command,
        Err(AgentResumeError::UnsupportedAgent(_)) => {
            return (
                false,
                "unsupported_agent",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
        Err(AgentResumeError::InvalidSessionId) => {
            return (
                false,
                "invalid_session_id",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
        Err(AgentResumeError::InvalidResumeCwd) => {
            return (
                false,
                "invalid_resume_cwd",
                Value::Null,
                Value::Null,
                Vec::new(),
            );
        }
    };
    let argv = std::iter::once(command.program.clone())
        .chain(command.args.iter().cloned())
        .collect::<Vec<_>>();
    let executable = resolve_child_program(&command.program, path);
    let executable_value = executable
        .as_ref()
        .map(|path| Value::String(path.to_string_lossy().into_owned()))
        .unwrap_or(Value::Null);
    let ready = executable.is_some();
    (
        ready,
        if ready { "ready" } else { "program_not_found" },
        Value::String(command.program),
        executable_value,
        argv,
    )
}

fn resume_agent_session(state: &SocketAppState, params: &Value) -> Result<Value, DispatchError> {
    let source_surface_id = required_surface_id(params)?;
    let (surface, agent, session_id, program, args, resume_cwd) = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| DispatchError::Other("Lock poisoned".to_string()))?;
        let source = model
            .surface(source_surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?
            .clone();
        let agent_session = source.agent_session.ok_or_else(|| {
            DispatchError::PreconditionFailed(
                "Surface has no persisted agent session to resume".to_string(),
            )
        })?;
        let resume_cwd = effective_agent_resume_cwd(&agent_session);
        let command = agent_resume_command_with_cwd_and_permission_mode(
            agent_session.agent,
            &agent_session.session_id,
            resume_cwd.as_deref(),
            agent_session.permission_mode.as_deref(),
        )
        .map_err(|err| DispatchError::PreconditionFailed(err.to_string()))?;
        let new_surface = model
            .add_tab(source_surface_id)
            .ok_or(DispatchError::NotFound("surface".to_string()))?;
        model.set_surface_agent_session(
            &new_surface.id,
            agent_session.agent,
            agent_session.session_id.clone(),
        );
        if let Some(resume_cwd) = resume_cwd.clone() {
            model.set_surface_agent_session_resume_cwd(&new_surface.id, resume_cwd);
        }
        if let Some(permission_mode) = agent_session.permission_mode.as_deref() {
            model.set_surface_agent_session_permission_mode(&new_surface.id, permission_mode);
        }
        let surface = model
            .surface(&new_surface.id)
            .cloned()
            .unwrap_or(new_surface);
        (
            surface,
            agent_session.agent,
            agent_session.session_id,
            command.program,
            command.args,
            resume_cwd,
        )
    };

    let mut request =
        SpawnRequest::for_surface(&surface, program.clone(), state.socket_path.clone());
    if let Some(resume_cwd) = resume_cwd {
        request.cwd = resume_cwd;
    }
    let request = request.with_args(args.clone());
    if let Err(err) = state.terminal.spawn(request) {
        rollback_surface_creation(state, &surface.id)?;
        return Err(err.into());
    }

    let argv = std::iter::once(program.clone())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
    Ok(json!({
        "surface": surface,
        "agent": agent,
        "session_id": session_id,
        "argv": argv,
    }))
}

fn status_summary(model: &WorkspaceModel, workspace_id: &str) -> Option<Value> {
    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)?;
    let surface_count = model.list_surfaces(Some(workspace_id)).len();
    Some(json!({
        "workspace": {
            "id": workspace.id,
            "name": workspace.name,
            "working_dir": workspace.working_dir,
            "git_branch": workspace.git_branch,
            "worktree_name": workspace.worktree_name,
            "focused_surface_id": workspace.focused_surface_id,
            "surfaces": surface_count,
        },
        "agents": agent_session_rows(model, Some(workspace_id)),
        "status": model.list_status(workspace_id),
        "progress": model.list_progress(workspace_id),
    }))
}

fn topology_tree(model: &WorkspaceModel, workspace_id: Option<&str>) -> Value {
    let workspaces = model
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace_id.is_none_or(|id| workspace.id == id))
        .map(|workspace| {
            let surfaces = model.list_surfaces(Some(&workspace.id));
            json!({
                "id": workspace.id,
                "name": workspace.name,
                "active": workspace.active,
                "working_dir": workspace.working_dir,
                "git_branch": workspace.git_branch,
                "worktree_name": workspace.worktree_name,
                "focused_surface_id": workspace.focused_surface_id,
                "needs_attention": workspace.needs_attention,
                "pane_tree": workspace.pane_tree,
                "surface_count": surfaces.len(),
                "surfaces": surfaces,
            })
        })
        .collect::<Vec<_>>();
    json!({ "workspaces": workspaces })
}

fn current_unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
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

/// Surfaces a non-fatal worktree `setup` hook failure as a workspace-scoped
/// error notification so it is visible in the UI instead of only on stderr.
fn notify_worktree_setup_warning(
    state: &SocketAppState,
    workspace_id: &str,
    warning: Option<&str>,
) -> Result<(), DispatchError> {
    let Some(warning) = warning else {
        return Ok(());
    };
    let item = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.create_notification(
            "Worktree Setup Hook Failed",
            warning,
            NotificationKind::Error,
            Some(workspace_id.to_string()),
            None,
        )
    };
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&item);
    }
    Ok(())
}

fn dispatch_notification_with_loaded_config(notification: &forktty_core::NotificationItem) {
    let dispatcher = NOTIFICATION_DISPATCHER.get_or_init(spawn_notification_dispatcher);
    match dispatcher.try_send(notification.clone()) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            eprintln!(
                "Dropping desktop notification dispatch because the bounded dispatch queue is full"
            );
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {
            eprintln!("Dropping desktop notification dispatch because the dispatch worker stopped");
        }
    }
}

fn spawn_notification_dispatcher() -> mpsc::SyncSender<forktty_core::NotificationItem> {
    let (sender, receiver) = mpsc::sync_channel(NOTIFICATION_DISPATCH_QUEUE_CAPACITY);
    // notify_rust's show() blocks on its own async runtime, and doing that from
    // a tokio worker panics ("Cannot start a runtime from within a runtime"),
    // killing the connection task that carried the request. Use one dedicated
    // blocking worker with a bounded queue so repeated socket notifications
    // cannot create unbounded native threads.
    if let Err(err) = std::thread::Builder::new()
        .name("forktty-notification-dispatch".to_string())
        .spawn(move || {
            for notification in receiver {
                dispatch_notification_from_worker(&notification);
            }
        })
    {
        eprintln!("Failed to start desktop notification dispatch worker: {err}");
    }
    sender
}

fn dispatch_notification_from_worker(notification: &forktty_core::NotificationItem) {
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
    if !info.created {
        return spawn_error;
    }
    match worktree::remove(cwd, &info.worktree_name, true) {
        Ok(()) => spawn_error,
        Err(rollback_error) => format!(
            "{spawn_error}; created worktree '{}' remains because rollback failed: {rollback_error}",
            info.worktree_name
        ),
    }
}

/// Run blocking worktree/git work off the socket runtime: these operations
/// walk the repository on disk, and create/remove additionally run the
/// repo's setup/teardown hook for up to HOOK_TIMEOUT (30s), which would pin
/// a tokio worker and starve every other socket connection.
async fn run_worktree_blocking<T, F>(task: F) -> Result<T, DispatchError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, worktree::WorktreeError> + Send + 'static,
{
    match tokio::task::spawn_blocking(task).await {
        Ok(result) => result.map_err(DispatchError::from),
        Err(err) => Err(format!("Worktree task failed: {err}").into()),
    }
}

async fn finish_removal_blocking(
    removal: worktree::PreparedWorktreeRemoval,
    delete_branch: bool,
) -> Result<(), DispatchError> {
    run_worktree_blocking(move || removal.finish(delete_branch)).await
}

fn spawn_workspace_terminal(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), String> {
    let Some(request) = spawn_request_for_workspace(state, workspace)? else {
        return Ok(());
    };
    state.terminal.spawn(request).map_err(|err| err.to_string())
}

fn spawn_surface_terminal(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Result<(), String> {
    let Some(request) = spawn_request_for_socket_surface(state, surface) else {
        return Ok(());
    };
    state.terminal.spawn(request).map_err(|err| err.to_string())
}

/// Resolve the absolute path to the `ssh` binary, preferring known locations.
pub fn resolve_ssh_binary() -> String {
    for candidate in &["/usr/bin/ssh", "/bin/ssh"] {
        if forktty_core::command_safety::is_executable_file(std::path::Path::new(candidate)) {
            return candidate.to_string();
        }
    }
    "ssh".to_string()
}

fn spawn_request_for_workspace(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<Option<SpawnRequest>, String> {
    let surface = {
        let model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model
            .surface(&workspace.focused_surface_id)
            .cloned()
            .ok_or_else(|| "Surface not found".to_string())?
    };
    Ok(spawn_request_for_surface(
        SpawnRequest::for_workspace(workspace, state.shell.clone(), state.socket_path.clone()),
        &surface,
    ))
}

fn spawn_request_for_socket_surface(
    state: &SocketAppState,
    surface: &forktty_core::Surface,
) -> Option<SpawnRequest> {
    spawn_request_for_surface(
        SpawnRequest::for_surface(surface, state.shell.clone(), state.socket_path.clone()),
        surface,
    )
}

/// Adapt a base [`SpawnRequest`] for the complete persisted surface metadata.
///
/// This first applies the [`SurfaceKind`] rules (Browser never spawns a PTY,
/// Ssh launches `ssh <host>`, and Terminal keeps the normal shell). Restored
/// terminal surfaces that carry an agent session then resume through the
/// provider's safe argv-only resume command, e.g. `codex resume <SESSION_ID>`.
pub fn spawn_request_for_surface(
    request: SpawnRequest,
    surface: &forktty_core::Surface,
) -> Option<SpawnRequest> {
    let request = spawn_request_for_surface_kind(request, &surface.kind)?;
    if !matches!(surface.kind, forktty_core::SurfaceKind::Terminal) {
        return Some(request);
    }
    let Some(agent_session) = &surface.agent_session else {
        return Some(request);
    };
    let resume_cwd = effective_agent_resume_cwd(agent_session);
    let Ok(command) = agent_resume_command_with_cwd_and_permission_mode(
        agent_session.agent,
        &agent_session.session_id,
        resume_cwd.as_deref(),
        agent_session.permission_mode.as_deref(),
    ) else {
        return Some(request);
    };
    let mut request = request;
    request.shell = command.program;
    if let Some(resume_cwd) = resume_cwd {
        request.cwd = resume_cwd;
    }
    Some(request.with_args(command.args))
}

/// Adapt a base [`SpawnRequest`] for a surface's [`SurfaceKind`].
///
/// `Terminal` keeps the request as-is, `Ssh` rewrites the shell to the ssh
/// binary and passes the host as the sole argument, and `Browser` returns
/// `None` (browser panes never get a PTY backend).
///
/// The `Ssh` host is re-validated here, not only at the `workspace.create_ssh`
/// entry point. A persisted (or tampered) session file is a distinct trust
/// boundary: it is deserialized straight into `SurfaceKind::Ssh { host }` on
/// restore and respawned through this function. Re-checking the host before it
/// reaches the `ssh` argv stops a smuggled `-oProxyCommand=...` value from
/// being executed when a workspace is restored. An invalid host yields `None`
/// (the surface is not spawned) rather than a shell with injected options.
pub fn spawn_request_for_surface_kind(
    request: SpawnRequest,
    kind: &forktty_core::SurfaceKind,
) -> Option<SpawnRequest> {
    match kind {
        forktty_core::SurfaceKind::Terminal => Some(request),
        forktty_core::SurfaceKind::Ssh { host } => {
            if !is_valid_ssh_host(host) {
                return None;
            }
            let mut request = request;
            request.shell = resolve_ssh_binary();
            Some(request.with_args([host.clone()]))
        }
        forktty_core::SurfaceKind::Browser { .. } => None,
    }
}

/// Validate the `host` parameter for SSH verbs.
///
/// Returns `Err(DispatchError::InvalidParam)` if the host is missing or invalid.
fn required_ssh_host_param(params: &Value) -> Result<&str, DispatchError> {
    let Some(value) = params.get("host") else {
        return Err(DispatchError::MissingParam("host"));
    };
    let host = value.as_str().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter host: expected string".to_string())
    })?;
    if !is_valid_ssh_host(host) {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter host: {host:?} is not a valid SSH target"
        )));
    }
    Ok(host)
}

fn spawn_terminal_surfaces(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    for surface in surfaces {
        spawn_surface_terminal(state, surface)?;
    }
    Ok(())
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

fn forget_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.forget_surface(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn close_replacement_terminal_surface_if_present(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    match state.terminal.close(surface_id) {
        Ok(()) | Err(TerminalError::NotFound(_)) => Ok(()),
        Err(close_err) => {
            let close_err = close_err.to_string();
            if let Err(forget_err) = forget_terminal_surface_if_present(state, surface_id) {
                return Err(format!("{close_err}; forget failed: {forget_err}"));
            }
            Err(close_err)
        }
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

fn evict_hook_session_targets_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .remove_surface(surface_id);
    Ok(())
}

fn evict_hook_session_targets_for_surfaces(
    state: &SocketAppState,
    surface_ids: &[String],
) -> Result<(), DispatchError> {
    let mut targets = state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    for surface_id in surface_ids {
        targets.remove_surface(surface_id);
    }
    Ok(())
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

/// Roll back a replacement workspace spawned for a last-workspace close whose
/// commit lost a race (the target workspace was already closed by a
/// concurrent request, which spawned its own replacement). The replacement is
/// only removed while at least one other workspace exists: if the concurrent
/// close went through the non-last-workspace path instead, our replacement
/// may be the sole survivor and must be kept. The count check and the close
/// share one lock so the model can never end up empty. Returns whether the
/// replacement was removed (the caller must then forget its terminal
/// surface).
fn rollback_replacement_if_redundant(
    state: &SocketAppState,
    replacement_id: &str,
    previous_active_id: Option<String>,
) -> Result<bool, String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    if model.list_workspaces().len() <= 1 {
        return Ok(false);
    }
    let _ = model.close_workspace(WorkspaceSelector::Id(replacement_id));
    if let Some(previous_active_id) = previous_active_id {
        let _ = model.select_workspace(WorkspaceSelector::Id(&previous_active_id));
    }
    Ok(true)
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
    spawn_workspace_terminal(state, &workspace)
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

async fn resolve_open_repo_cwd_param(
    state: &SocketAppState,
    params: &Value,
    keys: &[&str],
    missing_param: &'static str,
) -> Result<String, DispatchError> {
    let cwd = resolve_required_existing_dir_param(params, keys, missing_param)?;
    // Copy the open-workspace roots out under the lock, then run the git2
    // discovery off the runtime: it walks the filesystem once per open
    // workspace plus once for the candidate.
    let working_dirs = open_workspace_working_dirs(state)?;
    let candidate = cwd.clone();
    match tokio::task::spawn_blocking(move || {
        validate_cwd_against_working_dirs(&working_dirs, &candidate)
    })
    .await
    {
        Ok(result) => result.map_err(DispatchError::PreconditionFailed)?,
        Err(err) => return Err(format!("Validation task failed: {err}").into()),
    }
    Ok(cwd.to_string_lossy().to_string())
}

fn open_workspace_working_dirs(state: &SocketAppState) -> Result<Vec<PathBuf>, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(model
        .list_workspaces()
        .into_iter()
        .map(|workspace| workspace.working_dir)
        .collect())
}

fn resolve_required_existing_dir_param(
    params: &Value,
    keys: &[&str],
    missing_param: &'static str,
) -> Result<PathBuf, DispatchError> {
    let found = keys
        .iter()
        .filter_map(|key| params.get(*key).map(|value| (*key, value)))
        .collect::<Vec<_>>();
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous path parameter: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        )
        .into());
    }
    let Some((key, value)) = found.first() else {
        return Err(DispatchError::MissingParam(missing_param));
    };
    let raw = value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
    if raw.trim().is_empty() {
        return Err(format!("Invalid parameter {key}: path must not be empty").into());
    }
    canonical_existing_dir(Path::new(raw), key).map_err(DispatchError::from)
}

fn resolve_existing_dir_param(params: &Value, keys: &[&str]) -> Result<PathBuf, String> {
    let found = keys
        .iter()
        .filter_map(|key| params.get(*key).map(|value| (*key, value)))
        .collect::<Vec<_>>();
    if found.len() > 1 {
        return Err(format!(
            "Ambiguous path parameter: cannot combine {}",
            format_param_names(found.iter().map(|(key, _)| *key))
        ));
    }
    let Some((key, value)) = found.first() else {
        return Ok(fallback_cwd());
    };
    let raw = value
        .as_str()
        .ok_or_else(|| format!("Invalid parameter {key}: expected path string"))?;
    if raw.trim().is_empty() {
        return Err(format!("Invalid parameter {key}: path must not be empty"));
    }
    canonical_existing_dir(Path::new(raw), key)
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

fn validate_cwd_against_working_dirs(working_dirs: &[PathBuf], cwd: &Path) -> Result<(), String> {
    let candidate = canonical_repo_common_dir(cwd)?;
    let allowed = working_dirs
        .iter()
        .filter_map(|working_dir| canonical_repo_common_dir(working_dir).ok())
        .any(|open_repo| open_repo == candidate);
    if allowed {
        Ok(())
    } else {
        let roots = working_dirs
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "cwd is not inside the git repository of any open workspace; \
             open a workspace on the repo first (`forktty create-workspace \
             --working-dir <repo>`). Open workspace roots: {roots}"
        ))
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
        Some(name) if !name.is_empty() => {
            ensure_max_text_size("name", name)?;
            Ok(name)
        }
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
        BrowserCmdError::ElementNotInteractable => {
            DispatchError::InvalidParam("element is not interactable".to_string())
        }
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
    let raw = required_string_param(params, "url")?;
    let Some(url) = forktty_core::normalize_browser_url(raw) else {
        return Err("Invalid parameter url: must not be empty".into());
    };
    if url.len() > MAX_BROWSER_URL_BYTES {
        return Err(DispatchError::PayloadTooLarge {
            field: "url",
            limit: MAX_BROWSER_URL_BYTES,
            actual: url.len(),
        });
    }
    Ok(url)
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

#[derive(Debug, Clone, Copy)]
struct BrowserImportSelection {
    history: bool,
    bookmarks: bool,
    cookies: bool,
}

impl BrowserImportSelection {
    fn any(self) -> bool {
        self.history || self.bookmarks || self.cookies
    }

    fn read_selection(self) -> forktty_import::ImportReadSelection {
        forktty_import::ImportReadSelection {
            cookies: self.cookies,
            history: self.history,
            bookmarks: self.bookmarks,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct BrowserImportCounts {
    cookies: usize,
    history: usize,
    bookmarks: usize,
    skipped: usize,
}

impl BrowserImportCounts {
    fn add(&mut self, other: BrowserImportCounts) {
        self.cookies += other.cookies;
        self.history += other.history;
        self.bookmarks += other.bookmarks;
        self.skipped += other.skipped;
    }
}

fn browser_family_key(family: forktty_import::BrowserFamily) -> &'static str {
    match family {
        forktty_import::BrowserFamily::Firefox => "firefox",
        forktty_import::BrowserFamily::Chrome => "chrome",
        forktty_import::BrowserFamily::Chromium => "chromium",
        forktty_import::BrowserFamily::Brave => "brave",
        forktty_import::BrowserFamily::Edge => "edge",
        forktty_import::BrowserFamily::Vivaldi => "vivaldi",
    }
}

fn browser_import_source_id(profile: &forktty_import::SourceProfile) -> String {
    format!("{}:{}", browser_family_key(profile.family), profile.path)
}

fn browser_import_profile_json(profile: &forktty_import::SourceProfile) -> Value {
    json!({
        "id": browser_import_source_id(profile),
        "family": profile.family,
        "display_name": profile.display_name,
        "path": profile.path,
        "is_default": profile.is_default,
    })
}

fn browser_import_discover_json() -> Value {
    let browsers = forktty_import::discover();
    let profile_count: usize = browsers.iter().map(|browser| browser.profiles.len()).sum();
    let browsers_json: Vec<Value> = browsers
        .iter()
        .map(|browser| {
            let profiles: Vec<Value> = browser
                .profiles
                .iter()
                .map(browser_import_profile_json)
                .collect();
            json!({
                "family": browser.family,
                "label": browser.family.label(),
                "profiles": profiles,
            })
        })
        .collect();
    json!({
        "browsers": browsers_json,
        "count": profile_count,
    })
}

fn browser_import_bool_param(
    value: Option<&Value>,
    key: &'static str,
    default: bool,
) -> Result<bool, DispatchError> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {key}: expected boolean"
        ))),
    }
}

fn browser_import_selection(params: &Value) -> Result<BrowserImportSelection, DispatchError> {
    let Some(include) = params.get("include") else {
        return Ok(BrowserImportSelection {
            history: true,
            bookmarks: true,
            cookies: true,
        });
    };
    let object = include.as_object().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter include: expected object".to_string())
    })?;
    let selection = BrowserImportSelection {
        history: browser_import_bool_param(object.get("history"), "include.history", true)?,
        bookmarks: browser_import_bool_param(object.get("bookmarks"), "include.bookmarks", true)?,
        cookies: browser_import_bool_param(object.get("cookies"), "include.cookies", true)?,
    };
    if !selection.any() {
        return Err(DispatchError::InvalidParam(
            "select at least one browser data type to import".to_string(),
        ));
    }
    Ok(selection)
}

fn browser_import_all_sources_param(params: &Value) -> Result<bool, DispatchError> {
    browser_import_bool_param(params.get("all"), "all", false)
}

fn browser_import_selected_sources(
    params: &Value,
) -> Result<Vec<forktty_import::SourceProfile>, DispatchError> {
    let all = browser_import_all_sources_param(params)?;
    let discovered: Vec<forktty_import::SourceProfile> = forktty_import::discover()
        .into_iter()
        .flat_map(|browser| browser.profiles.into_iter())
        .collect();
    if all {
        if params
            .get("sources")
            .is_some_and(|sources| !sources.is_null())
        {
            return Err(DispatchError::InvalidParam(
                "cannot combine all and sources".to_string(),
            ));
        }
        return Ok(discovered);
    }

    let Some(sources) = params.get("sources") else {
        return Err(DispatchError::MissingParam("sources"));
    };
    let ids = sources.as_array().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter sources: expected array".to_string())
    })?;
    if ids.is_empty() {
        return Err(DispatchError::InvalidParam(
            "sources must not be empty".to_string(),
        ));
    }

    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for value in ids {
        let id = value.as_str().ok_or_else(|| {
            DispatchError::InvalidParam("Invalid parameter sources: expected strings".to_string())
        })?;
        if id.trim().is_empty() {
            return Err(DispatchError::InvalidParam(
                "sources must not include empty source ids".to_string(),
            ));
        }
        if !seen.insert(id.to_string()) {
            continue;
        }
        let Some(profile) = discovered
            .iter()
            .find(|profile| browser_import_source_id(profile) == id)
        else {
            return Err(DispatchError::NotFound("browser import source".to_string()));
        };
        selected.push(profile.clone());
    }
    Ok(selected)
}

fn browser_import_counts_from_data(
    data: &forktty_import::ImportedData,
    include: BrowserImportSelection,
) -> BrowserImportCounts {
    BrowserImportCounts {
        cookies: if include.cookies {
            data.result.cookies
        } else {
            0
        },
        history: if include.history {
            data.visits.len()
        } else {
            0
        },
        bookmarks: if include.bookmarks {
            data.bookmarks.len()
        } else {
            0
        },
        skipped: if include.cookies {
            data.result.skipped
        } else {
            0
        },
    }
}

fn browser_import_counts_json(counts: BrowserImportCounts) -> Value {
    json!({
        "cookies": counts.cookies,
        "history": counts.history,
        "bookmarks": counts.bookmarks,
        "skipped": counts.skipped,
    })
}

async fn browser_import_preview(params: &Value) -> Result<Value, DispatchError> {
    let include = browser_import_selection(params)?;
    let selected = browser_import_selected_sources(params)?;
    let mut total = BrowserImportCounts::default();
    let mut source_rows = Vec::new();

    for source in selected {
        let data = forktty_import::ImportEngine::read_source_async_with_selection(
            &source,
            include.read_selection(),
        )
        .await
        .map_err(|err| DispatchError::Other(err.to_string()))?;
        let counts = browser_import_counts_from_data(&data, include);
        total.add(counts);
        source_rows.push(json!({
            "source": browser_import_profile_json(&source),
            "counts": browser_import_counts_json(counts),
        }));
    }

    Ok(json!({
        "sources": source_rows,
        "total": browser_import_counts_json(total),
        "cookies_supported": false,
    }))
}

struct BrowserImportSpooledSource {
    source: forktty_import::SourceProfile,
    data_file: tempfile::NamedTempFile,
}

async fn browser_import_read_sources(
    sources: &[forktty_import::SourceProfile],
    include: BrowserImportSelection,
) -> Result<Vec<BrowserImportSpooledSource>, DispatchError> {
    let mut source_data = Vec::with_capacity(sources.len());
    for source in sources {
        let data = forktty_import::ImportEngine::read_source_async_with_selection(
            source,
            include.read_selection(),
        )
        .await
        .map_err(|err| DispatchError::Other(err.to_string()))?;
        let mut data_file =
            tempfile::NamedTempFile::new().map_err(|err| DispatchError::Other(err.to_string()))?;
        serde_json::to_writer(data_file.as_file_mut(), &data)
            .map_err(|err| DispatchError::Other(err.to_string()))?;
        source_data.push(BrowserImportSpooledSource {
            source: source.clone(),
            data_file,
        });
    }
    Ok(source_data)
}

struct BrowserImportReadEntry {
    destination: forktty_import::ImportDestination,
    source_data: Vec<BrowserImportSpooledSource>,
}

async fn browser_import_read_plan(
    plan: forktty_import::ImportPlan,
    include: BrowserImportSelection,
) -> Result<(forktty_import::ImportMode, Vec<BrowserImportReadEntry>), DispatchError> {
    let mode = plan.mode;
    let mut entries = Vec::with_capacity(plan.entries.len());
    for entry in plan.entries {
        let source_data = browser_import_read_sources(&entry.sources, include).await?;
        entries.push(BrowserImportReadEntry {
            destination: entry.destination,
            source_data,
        });
    }
    Ok((mode, entries))
}

fn resolve_profile_value_in_store(
    store: &forktty_core::ProfileStore,
    value: &Value,
) -> Result<forktty_core::ProfileId, DispatchError> {
    let profile = value.as_str().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter profile: expected string".to_string())
    })?;
    store
        .resolve(profile)
        .ok_or(DispatchError::NotFound("profile".to_string()))
}

fn optional_profile_param_in_store(
    store: &forktty_core::ProfileStore,
    params: &Value,
) -> Result<forktty_core::ProfileId, DispatchError> {
    match params.get("profile") {
        None | Some(Value::Null) => Ok(forktty_core::ProfileId::default()),
        Some(value) => resolve_profile_value_in_store(store, value),
    }
}

fn browser_import_destination_from_params(
    params: &Value,
    store: &forktty_core::ProfileStore,
) -> Result<Option<forktty_import::ImportDestination>, DispatchError> {
    let Some(destination) = params.get("destination") else {
        return Ok(None);
    };
    let object = destination.as_object().ok_or_else(|| {
        DispatchError::InvalidParam("Invalid parameter destination: expected object".to_string())
    })?;
    let kind = match object.get("kind") {
        None | Some(Value::Null) => return Err(DispatchError::MissingParam("destination.kind")),
        Some(Value::String(kind)) => kind.as_str(),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "Invalid parameter destination.kind: expected string".to_string(),
            ));
        }
    };
    match kind {
        "existing" => {
            let value = object
                .get("profile")
                .or_else(|| object.get("id"))
                .ok_or(DispatchError::MissingParam("destination.profile"))?;
            Ok(Some(forktty_import::ImportDestination::Existing(
                resolve_profile_value_in_store(store, value)?,
            )))
        }
        "create" => {
            let name = match object.get("display_name").or_else(|| object.get("name")) {
                None | Some(Value::Null) => {
                    return Err(DispatchError::MissingParam("destination.display_name"));
                }
                Some(Value::String(name)) => name.trim().to_string(),
                Some(_) => {
                    return Err(DispatchError::InvalidParam(
                        "Invalid parameter destination.display_name: expected string".to_string(),
                    ));
                }
            };
            if name.is_empty() {
                return Err(DispatchError::InvalidParam(
                    "destination.display_name must not be empty".to_string(),
                ));
            }
            Ok(Some(forktty_import::ImportDestination::Create(name)))
        }
        other => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter destination.kind: expected existing or create, got {other}"
        ))),
    }
}

fn browser_import_plan_from_params(
    params: &Value,
    selected: &[forktty_import::SourceProfile],
    store: &forktty_core::ProfileStore,
) -> Result<forktty_import::ImportPlan, DispatchError> {
    if let Some(destination) = browser_import_destination_from_params(params, store)? {
        return Ok(forktty_import::ImportPlan {
            mode: forktty_import::ImportMode::SingleDestination,
            entries: vec![forktty_import::ImportEntry {
                sources: selected.to_vec(),
                destination,
            }],
        });
    }

    let mode = match params.get("mode") {
        None | Some(Value::Null) => "default",
        Some(Value::String(mode)) => mode.as_str(),
        Some(_) => {
            return Err(DispatchError::InvalidParam(
                "Invalid parameter mode: expected string".to_string(),
            ));
        }
    };
    match mode {
        "default" => {
            let preferred = optional_profile_param_in_store(store, params)?;
            Ok(forktty_import::resolve_default_plan(
                selected,
                store.list(),
                preferred,
            ))
        }
        "single_destination" => {
            let profile = optional_profile_param_in_store(store, params)?;
            Ok(forktty_import::ImportPlan {
                mode: forktty_import::ImportMode::SingleDestination,
                entries: vec![forktty_import::ImportEntry {
                    sources: selected.to_vec(),
                    destination: forktty_import::ImportDestination::Existing(profile),
                }],
            })
        }
        "separate_profiles" => Ok(forktty_import::resolve_separate_profiles_plan(
            selected,
            store.list(),
        )),
        other => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter mode: expected default, single_destination, or separate_profiles, got {other}"
        ))),
    }
}

fn browser_import_profile_meta_json(
    id: forktty_core::ProfileId,
    display_name: &str,
    created: bool,
) -> Value {
    json!({
        "id": id.to_string(),
        "display_name": display_name,
        "created": created,
    })
}

fn browser_import_prepare_destination(
    store: &mut forktty_core::ProfileStore,
    destination: &forktty_import::ImportDestination,
) -> Result<(forktty_core::ProfileId, String, bool), DispatchError> {
    match destination {
        forktty_import::ImportDestination::Existing(id) => {
            let Some(meta) = store.list().iter().find(|profile| profile.id == *id) else {
                return Err(DispatchError::NotFound("profile".to_string()));
            };
            Ok((meta.id, meta.display_name.clone(), false))
        }
        forktty_import::ImportDestination::Create(display_name) => {
            let meta = store.create(display_name).map_err(|err| match err {
                forktty_core::ProfileError::InvalidInput(message) => {
                    DispatchError::InvalidParam(message)
                }
                other => DispatchError::Other(other.to_string()),
            })?;
            Ok((meta.id, meta.display_name, true))
        }
    }
}

fn rollback_browser_import_created_profile(
    state: &SocketAppState,
    profile_id: forktty_core::ProfileId,
) -> Result<(), String> {
    {
        let _profile_store_guard = state
            .profile_store_lock
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let mut store = profiles_store().map_err(|err| err.to_string())?;
        store.delete(&profile_id).map_err(|err| err.to_string())?;
    }

    if let Some(profile_dir) = forktty_core::browser_history::history_path(profile_id)
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        match fs::remove_dir_all(&profile_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "profile data cleanup failed for {}: {err}",
                    profile_dir.display()
                ));
            }
        }
    }
    Ok(())
}

async fn browser_import_run(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let include = browser_import_selection(params)?;
    let selected = browser_import_selected_sources(params)?;
    let plan = {
        let _profile_store_guard = state
            .profile_store_lock
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let store = profiles_store()?;
        browser_import_plan_from_params(params, &selected, &store)?
    };
    let (mode, read_entries) = browser_import_read_plan(plan, include).await?;

    let mut total_read = BrowserImportCounts::default();
    let mut total_written = BrowserImportCounts::default();
    let mut total_unsupported_cookies = 0usize;
    let mut entries_json = Vec::new();

    for entry in read_entries {
        let (profile_id, display_name, created) = {
            let _profile_store_guard = state
                .profile_store_lock
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let mut store = profiles_store()?;
            browser_import_prepare_destination(&mut store, &entry.destination)?
        };
        let entry_result: Result<
            (Value, BrowserImportCounts, BrowserImportCounts, usize),
            DispatchError,
        > = async {
            let history_store = if include.history {
                Some(
                    forktty_core::HistoryStore::for_profile(profile_id)
                        .map_err(|err| DispatchError::Other(err.to_string()))?,
                )
            } else {
                None
            };
            let mut bookmark_store = if include.bookmarks {
                Some(
                    forktty_core::BookmarkStore::for_profile(profile_id)
                        .map_err(|err| DispatchError::Other(err.to_string()))?,
                )
            } else {
                None
            };

            let mut entry_read = BrowserImportCounts::default();
            let mut entry_written = BrowserImportCounts::default();
            let mut entry_unsupported_cookies = 0usize;
            let mut entry_sources = Vec::new();

            for mut spooled in entry.source_data {
                spooled
                    .data_file
                    .as_file_mut()
                    .seek(SeekFrom::Start(0))
                    .map_err(|err| DispatchError::Other(err.to_string()))?;
                let data: forktty_import::ImportedData =
                    serde_json::from_reader(spooled.data_file.as_file_mut())
                        .map_err(|err| DispatchError::Other(err.to_string()))?;
                let read_counts = browser_import_counts_from_data(&data, include);
                entry_read.add(read_counts);
                entry_sources.push(browser_import_profile_json(&spooled.source));

                if let Some(history_store) = &history_store {
                    for visit in &data.visits {
                        if history_store
                            .import_visit(&visit.url, &visit.title, visit.visit_count)
                            .map_err(|err| DispatchError::Other(err.to_string()))?
                        {
                            entry_written.history += 1;
                        }
                    }
                }

                if let Some(bookmark_store) = bookmark_store.as_mut() {
                    for bookmark in &data.bookmarks {
                        bookmark_store
                            .add(&bookmark.url, &bookmark.title)
                            .map_err(|err| DispatchError::Other(err.to_string()))?;
                        entry_written.bookmarks += 1;
                    }
                }

                if include.cookies {
                    entry_unsupported_cookies += data.result.cookies;
                }
            }

            let entry_json = json!({
                "destination": browser_import_profile_meta_json(profile_id, &display_name, created),
                "sources": entry_sources,
                "read": browser_import_counts_json(entry_read),
                "written": browser_import_counts_json(entry_written),
                "cookies": {
                    "read": entry_read.cookies,
                    "written": 0,
                    "unsupported": entry_unsupported_cookies,
                    "skipped": entry_read.skipped,
                },
            });
            Ok((
                entry_json,
                entry_read,
                entry_written,
                entry_unsupported_cookies,
            ))
        }
        .await;

        let (entry_json, entry_read, entry_written, entry_unsupported_cookies) = match entry_result
        {
            Ok(result) => result,
            Err(err) => {
                if created {
                    if let Err(cleanup_err) =
                        rollback_browser_import_created_profile(state, profile_id)
                    {
                        return Err(DispatchError::Other(format!(
                            "{err}; created profile cleanup failed: {cleanup_err}"
                        )));
                    }
                }
                return Err(err);
            }
        };

        total_read.add(entry_read);
        total_written.add(entry_written);
        total_unsupported_cookies += entry_unsupported_cookies;
        entries_json.push(entry_json);
    }

    Ok(json!({
        "mode": mode,
        "entries": entries_json,
        "total": {
            "read": browser_import_counts_json(total_read),
            "written": browser_import_counts_json(total_written),
            "cookies": {
                "read": total_read.cookies,
                "written": 0,
                "unsupported": total_unsupported_cookies,
                "skipped": total_read.skipped,
            },
        },
        "cookies_supported": false,
    }))
}

fn history_limit_from_params(params: &Value) -> Result<usize, DispatchError> {
    match params.get("limit") {
        None | Some(Value::Null) => Ok(100),
        Some(value) => value
            .as_u64()
            .map(|n| n.min(10_000) as usize)
            .ok_or_else(|| {
                DispatchError::InvalidParam(
                    "Invalid parameter limit: expected unsigned integer".to_string(),
                )
            }),
    }
}

fn optional_u64_param(params: &Value, key: &'static str) -> Result<Option<u64>, DispatchError> {
    match params.get(key) {
        Some(Value::Number(number)) => number.as_u64().map(Some).ok_or_else(|| {
            DispatchError::InvalidParam(format!(
                "Invalid parameter {key}: expected unsigned integer"
            ))
        }),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(DispatchError::InvalidParam(format!(
            "Invalid parameter {key}: expected unsigned integer"
        ))),
    }
}

fn terminal_text_capture_from_params(params: &Value) -> Result<TerminalTextCapture, DispatchError> {
    match params.get("scope") {
        None | Some(Value::Null) => Ok(TerminalTextCapture::Visible),
        Some(Value::String(scope)) => match scope.trim() {
            "" | "visible" => Ok(TerminalTextCapture::Visible),
            "all" | "full" => Ok(TerminalTextCapture::All),
            other => Err(DispatchError::InvalidParam(format!(
                "Invalid parameter scope: expected visible or all, got {other:?}"
            ))),
        },
        Some(_) => Err(DispatchError::InvalidParam(
            "Invalid parameter scope: expected string".to_string(),
        )),
    }
}

fn terminal_text_max_bytes_from_params(params: &Value) -> Result<usize, DispatchError> {
    let Some(value) = optional_u64_param(params, "max_bytes")? else {
        return Ok(MAX_TERMINAL_TEXT_BYTES);
    };
    if value == 0 || value > MAX_TERMINAL_TEXT_BYTES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter max_bytes: expected 1..={MAX_TERMINAL_TEXT_BYTES}"
        )));
    }
    Ok(value as usize)
}

fn terminal_tail_lines_from_params(params: &Value) -> Result<usize, DispatchError> {
    let lines = optional_u64_param(params, "lines")?.unwrap_or(DEFAULT_CAPTURE_TAIL_LINES as u64);
    if lines == 0 || lines > MAX_CAPTURE_TAIL_LINES as u64 {
        return Err(DispatchError::InvalidParam(format!(
            "Invalid parameter lines: expected 1..={MAX_CAPTURE_TAIL_LINES}"
        )));
    }
    Ok(lines as usize)
}

fn optional_bookmark_title(params: &Value) -> Result<String, DispatchError> {
    match params.get("title") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(value) => value
            .as_str()
            .map(|title| title.trim().to_string())
            .ok_or_else(|| {
                DispatchError::InvalidParam("Invalid parameter title: expected string".to_string())
            }),
    }
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

fn agent_kind_from_status_key(key: &str) -> Option<AgentKind> {
    let mut parts = key.split(':');
    if parts.next()? != "agent" {
        return None;
    }
    let provider = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    match provider {
        "claude" | "claude-code" | "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        "antigravity" | "agy" => Some(AgentKind::Antigravity),
        "opencode" | "open-code" | "open_code" => Some(AgentKind::OpenCode),
        "gemini" => Some(AgentKind::Gemini),
        "custom" => Some(AgentKind::Custom),
        _ => None,
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
    // Treat an explicit JSON `null` as absent, matching `optional_u64_param`,
    // so clients that always serialize optional fields (as `null` when unset)
    // are not rejected with a type error before the method handler runs.
    if value.is_null() {
        return Ok(None);
    }
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

struct HookSessionEndGuard {
    targets: Arc<Mutex<HookSessionTargets>>,
    session_id: Option<String>,
}

impl HookSessionEndGuard {
    fn none(state: &SocketAppState) -> Self {
        Self {
            targets: state.hook_session_targets.clone(),
            session_id: None,
        }
    }

    fn new(state: &SocketAppState, session_id: Option<String>) -> Self {
        Self {
            targets: state.hook_session_targets.clone(),
            session_id,
        }
    }
}

impl Drop for HookSessionEndGuard {
    fn drop(&mut self) {
        let Some(session_id) = self.session_id.as_deref() else {
            return;
        };
        if let Ok(mut targets) = self.targets.lock() {
            targets.remove_session(session_id);
        }
    }
}

fn optional_hook_session_cwd(params: &Value) -> Result<Option<PathBuf>, DispatchError> {
    let Some(raw) = optional_non_blank_string_param(params, "hook_session_cwd")? else {
        return Ok(None);
    };
    ensure_max_text_size("hook_session_cwd", raw)?;
    let path = Path::new(raw);
    if path.is_absolute() {
        match canonical_existing_dir(path, "hook_session_cwd") {
            Ok(canonical) => return Ok(Some(canonical)),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

fn prepare_hook_session_targets(
    state: &SocketAppState,
    method: &str,
    params: &mut Value,
) -> Result<HookSessionEndGuard, DispatchError> {
    if !is_hook_targetable_method(method) {
        return Ok(HookSessionEndGuard::none(state));
    }
    let Some(session_id) = optional_non_blank_string_param(params, "hook_session_id")? else {
        return Ok(HookSessionEndGuard::none(state));
    };
    ensure_max_text_size("hook_session_id", session_id)?;
    let session_id = session_id.to_string();
    let event_name = optional_non_blank_string_param(params, "hook_event_name")?;
    let evict_on_return = event_name == Some("session-end");

    let surface_id = optional_surface_id_param(params)?.map(str::to_string);
    let workspace_selectors = workspace_selector_params(params)?;
    let has_explicit_workspace = !workspace_selectors.is_empty();
    let workspace_id = workspace_selectors
        .iter()
        .find(|selector| {
            matches!(selector.kind, WorkspaceSelectorKind::Id)
                && matches!(selector.key, "workspace_id" | "workspaceId")
        })
        .map(|selector| selector.value.to_string());

    if let (Some(workspace_id), Some(surface_id)) = (workspace_id.as_deref(), surface_id.as_deref())
    {
        let target = HookSessionTarget {
            workspace_id: workspace_id.to_string(),
            surface_id: surface_id.to_string(),
        };
        if hook_session_target_is_live(state, &target)? {
            state
                .hook_session_targets
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?
                .learn(session_id.clone(), target);
        }
        return Ok(HookSessionEndGuard::new(
            state,
            evict_on_return.then_some(session_id),
        ));
    }

    if surface_id.is_none() && !has_explicit_workspace {
        let mapped = state
            .hook_session_targets
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?
            .get(&session_id)
            .cloned();
        if let Some(target) = mapped {
            if hook_session_target_is_live(state, &target)? {
                insert_hook_session_target(params, &target)?;
            } else {
                state
                    .hook_session_targets
                    .lock()
                    .map_err(|_| "Lock poisoned".to_string())?
                    .remove_session(&session_id);
                return Err(DispatchError::NotFound("surface".to_string()));
            }
        }
    }

    Ok(HookSessionEndGuard::new(
        state,
        evict_on_return.then_some(session_id),
    ))
}

fn is_hook_targetable_method(method: &str) -> bool {
    matches!(
        method,
        "metadata.set_status"
            | "metadata.clear_status"
            | "metadata.set_progress"
            | "metadata.log"
            | "notification.create"
    )
}

fn hook_session_target_is_live(
    state: &SocketAppState,
    target: &HookSessionTarget,
) -> Result<bool, DispatchError> {
    let model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    Ok(model
        .surface(&target.surface_id)
        .is_some_and(|surface| surface.workspace_id == target.workspace_id))
}

fn insert_hook_session_target(
    params: &mut Value,
    target: &HookSessionTarget,
) -> Result<(), DispatchError> {
    let Some(params) = params.as_object_mut() else {
        return Err(DispatchError::InvalidParam(
            "Invalid hook target params: expected object".to_string(),
        ));
    };
    params.insert(
        "workspace_id".to_string(),
        Value::String(target.workspace_id.clone()),
    );
    params.insert(
        "surface_id".to_string(),
        Value::String(target.surface_id.clone()),
    );
    Ok(())
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
    spawn_workspace_terminal(state, &workspace)
}

async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
) -> Result<(), SocketError> {
    handle_connection_with_write_timeout(stream, state, RESPONSE_WRITE_TIMEOUT).await
}

// The write timeout is injectable so tests of the stalled-client behavior can
// pass a short one instead of waiting out the production
// [`RESPONSE_WRITE_TIMEOUT`].
async fn handle_connection_with_write_timeout(
    stream: tokio::net::UnixStream,
    state: SocketAppState,
    write_timeout: Duration,
) -> Result<(), SocketError> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        let read = tokio::time::timeout(
            REQUEST_READ_TIMEOUT,
            read_limited_line(&mut reader, MAX_REQUEST_SIZE),
        )
        .await;
        let line = match read {
            // Idle or slow-loris client never finished a request line; drop the
            // connection so its permit is returned to the pool.
            Err(_elapsed) => break,
            Ok(None) => break,
            Ok(Some(Err(ReadLineError::TooLarge))) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "request_too_large",
                    "Request exceeds 1 MiB",
                );
                write_response(&mut writer, &response, write_timeout).await?;
                break;
            }
            Ok(Some(Err(ReadLineError::InvalidUtf8))) => {
                let response = JsonRpcResponse::error(
                    Value::Null,
                    "parse_error",
                    "Request must be valid UTF-8 JSON",
                );
                write_response(&mut writer, &response, write_timeout).await?;
                break;
            }
            Ok(Some(Err(ReadLineError::Io(err)))) => return Err(err.into()),
            Ok(Some(Ok(line))) => line,
        };
        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                let response = JsonRpcResponse::error(Value::Null, "parse_error", err.to_string());
                write_response(&mut writer, &response, write_timeout).await?;
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
            return stream_events(
                &state,
                replay,
                &mut reader,
                &mut writer,
                EVENTS_WRITE_TIMEOUT,
            )
            .await;
        }
        let id = request.id.clone();
        let response = if method_allowed_from_socket(&request.method) {
            match dispatch(&state, &request.method, request.params).await {
                Ok(result) => JsonRpcResponse::ok(id, result),
                Err(err) => JsonRpcResponse::error(id, err.code(), err.to_string()),
            }
        } else {
            JsonRpcResponse::error(
                id,
                "method_not_found",
                format!("Unknown method: {}", request.method),
            )
        };
        write_response(&mut writer, &response, write_timeout).await?;
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
    write_timeout: Duration,
) -> Result<(), SocketError> {
    let mut receiver = state.events.subscribe();
    write_ndjson_bounded(writer, &json!({"event": "subscribed"}), write_timeout).await?;
    if replay {
        let snapshot = current_snapshot(&state.model);
        for event in events::diff(&Snapshot::default(), &snapshot) {
            write_ndjson_bounded(writer, &json!(event), write_timeout).await?;
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
                Ok(event) => write_ndjson_bounded(writer, &json!(event), write_timeout).await?,
                Err(broadcast::error::RecvError::Lagged(dropped)) => {
                    write_ndjson_bounded(writer, &lagged_notice(dropped), write_timeout).await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
    Ok(())
}

/// [`write_ndjson`] bounded by `timeout`: a subscriber that stopped reading
/// must produce an error (ending its connection and releasing the permit)
/// instead of parking the stream task on a full socket buffer forever.
async fn write_ndjson_bounded(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    value: &Value,
    timeout: Duration,
) -> Result<(), SocketError> {
    tokio::time::timeout(timeout, write_ndjson(writer, value))
        .await
        .map_err(|_| {
            SocketError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                "subscriber stopped reading; dropping the event stream connection",
            ))
        })?
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

/// Write one response line, bounded by `timeout`: a client that sent a request
/// and then stopped reading must produce an error (ending its connection and
/// releasing the permit) instead of parking the connection task on a full
/// socket buffer forever.
async fn write_response(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    response: &JsonRpcResponse,
    timeout: Duration,
) -> Result<(), io::Error> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    tokio::time::timeout(timeout, async {
        writer.write_all(&bytes).await?;
        writer.flush().await
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "client stopped reading; dropping the connection",
        )
    })?
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

fn probe_forktty_socket(stream: StdUnixStream) -> io::Result<bool> {
    probe_forktty_socket_with_timeout(stream, SOCKET_PROBE_TIMEOUT)
}

// The timeout is injectable so tests of the protocol/parsing behavior can pass
// a generous one: the responding peer is a freshly spawned thread, and under
// scheduler starvation the production 250ms can elapse before it ever runs.
fn probe_forktty_socket_with_timeout(
    mut stream: StdUnixStream,
    timeout: Duration,
) -> io::Result<bool> {
    use std::io::{Read, Write};
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(br#"{"id":"probe","method":"system.ping","params":{}}"#)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let mut response = Vec::with_capacity(256);
    let mut buf = [0u8; 256];
    // Overall deadline on top of the per-read timeouts: a peer that trickles
    // one byte per read window never trips the read timeout and would stretch
    // the probe (which runs on the GTK main thread at startup) almost
    // indefinitely. A real ForkTTY answers a ping in one write, so anything
    // still dribbling after a few timeout periods is a foreign peer.
    let started = std::time::Instant::now();
    let overall_deadline = timeout * 4;
    loop {
        if started.elapsed() > overall_deadline {
            return Ok(false);
        }
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
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    #[cfg(feature = "browser")]
    use std::sync::Barrier;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn optional_non_blank_string_param_treats_null_as_absent() {
        let params = serde_json::json!({ "key": null });
        assert_eq!(
            optional_non_blank_string_param(&params, "key").unwrap(),
            None
        );

        let missing = serde_json::json!({});
        assert_eq!(
            optional_non_blank_string_param(&missing, "key").unwrap(),
            None
        );

        let present = serde_json::json!({ "key": "value" });
        assert_eq!(
            optional_non_blank_string_param(&present, "key").unwrap(),
            Some("value")
        );

        let wrong_type = serde_json::json!({ "key": 7 });
        assert!(optional_non_blank_string_param(&wrong_type, "key").is_err());
    }

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

    #[test]
    fn spawn_terminal_surfaces_respawns_ssh_surfaces() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let surface = forktty_core::Surface {
            id: "surface-ssh".to_string(),
            workspace_id: "workspace-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: "ssh".to_string(),
            unread: false,
            needs_attention: false,
            kind: forktty_core::SurfaceKind::Ssh {
                host: "user@example.test".to_string(),
            },
            agent_session: None,
        };

        spawn_terminal_surfaces(&state, &[surface]).unwrap();

        assert!(backend.spawn_shell("surface-ssh").unwrap().ends_with("ssh"));
        assert_eq!(
            backend.spawn_args("surface-ssh").unwrap(),
            vec!["user@example.test"]
        );
    }

    #[test]
    fn spawn_request_revalidates_ssh_host_from_restored_surface() {
        let base = SpawnRequest::for_surface(
            &forktty_core::Surface {
                id: "surface-ssh".to_string(),
                workspace_id: "workspace-1".to_string(),
                cwd: PathBuf::from("/tmp"),
                title: "ssh".to_string(),
                unread: false,
                needs_attention: false,
                kind: forktty_core::SurfaceKind::Terminal,
                agent_session: None,
            },
            "/bin/sh",
            "/tmp/forktty.sock",
        );

        // A valid host is spawned as a single argv element after the ssh binary.
        let valid = spawn_request_for_surface_kind(
            base.clone(),
            &forktty_core::SurfaceKind::Ssh {
                host: "user@example.test".to_string(),
            },
        )
        .expect("valid ssh host should spawn");
        assert_eq!(valid.args, vec!["user@example.test".to_string()]);

        // A tampered/persisted host that would smuggle ssh options must not be
        // spawned, even though it never passed `workspace.create_ssh`.
        for malicious in [
            "-oProxyCommand=touch /tmp/pwned",
            "-l root",
            "host with space",
        ] {
            assert!(
                spawn_request_for_surface_kind(
                    base.clone(),
                    &forktty_core::SurfaceKind::Ssh {
                        host: malicious.to_string(),
                    },
                )
                .is_none(),
                "malicious ssh host {malicious:?} must be rejected on respawn"
            );
        }
    }

    #[test]
    fn spawn_request_resumes_restored_agent_terminal_surface() {
        let surface = forktty_core::Surface {
            id: "surface-agent".to_string(),
            workspace_id: "workspace-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: "agent".to_string(),
            unread: false,
            needs_attention: false,
            kind: forktty_core::SurfaceKind::Terminal,
            agent_session: Some(forktty_core::AgentSession {
                agent: AgentKind::Codex,
                session_id: "codex-session-1".to_string(),
                resume_cwd: Some(PathBuf::from("/tmp/forktty-project")),
                permission_mode: None,
                lifecycle: AgentSessionLifecycle::Running,
                last_activity_ms: 12_345,
            }),
        };

        let request = spawn_request_for_surface(
            SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
            &surface,
        )
        .expect("agent terminal surface should spawn");

        assert_eq!(request.shell, "codex");
        assert_eq!(
            request.args,
            vec![
                "resume".to_string(),
                "-C".to_string(),
                "/tmp/forktty-project".to_string(),
                "codex-session-1".to_string(),
            ]
        );
    }

    #[test]
    fn spawn_request_ignores_persisted_bypass_permission_mode() {
        let surface = forktty_core::Surface {
            id: "surface-agent".to_string(),
            workspace_id: "workspace-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: "agent".to_string(),
            unread: false,
            needs_attention: false,
            kind: forktty_core::SurfaceKind::Terminal,
            agent_session: Some(forktty_core::AgentSession {
                agent: AgentKind::ClaudeCode,
                session_id: "claude-session-1".to_string(),
                resume_cwd: Some(PathBuf::from("/tmp/forktty-project")),
                permission_mode: Some("bypassPermissions".to_string()),
                lifecycle: AgentSessionLifecycle::Running,
                last_activity_ms: 12_345,
            }),
        };

        let request = spawn_request_for_surface(
            SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
            &surface,
        )
        .expect("agent terminal surface should spawn");

        assert_eq!(request.shell, "claude");
        assert_eq!(
            request.args,
            vec!["--resume".to_string(), "claude-session-1".to_string()]
        );
        assert_eq!(request.cwd, PathBuf::from("/tmp/forktty-project"));
    }

    #[test]
    #[serial_test::serial]
    fn spawn_request_uses_codex_session_cwd_fallback_when_not_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let codex_home = dir.path().join("codex");
        let sessions_dir = codex_home.join("sessions/2026/06/12");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join("rollout-2026-06-12T15-21-07-codex-session-fallback.jsonl"),
            format!(
                "{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "codex-session-fallback",
                        "cwd": project.to_string_lossy(),
                    }
                })
            ),
        )
        .unwrap();
        let _env = EnvGuard::set("CODEX_HOME", codex_home.to_str().unwrap());
        let surface = forktty_core::Surface {
            id: "surface-agent".to_string(),
            workspace_id: "workspace-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: "agent".to_string(),
            unread: false,
            needs_attention: false,
            kind: forktty_core::SurfaceKind::Terminal,
            agent_session: Some(forktty_core::AgentSession {
                agent: AgentKind::Codex,
                session_id: "codex-session-fallback".to_string(),
                resume_cwd: None,
                permission_mode: None,
                lifecycle: AgentSessionLifecycle::Running,
                last_activity_ms: 12_345,
            }),
        };

        let request = spawn_request_for_surface(
            SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
            &surface,
        )
        .expect("agent terminal surface should spawn");

        assert_eq!(request.shell, "codex");
        assert_eq!(
            request.args,
            vec![
                "resume".to_string(),
                "-C".to_string(),
                project.to_string_lossy().into_owned(),
                "codex-session-fallback".to_string(),
            ]
        );
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

    #[derive(Debug)]
    struct DirtyOnCloseBackend {
        surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
        active_children: Mutex<BTreeSet<String>>,
        dirty_on_close: PathBuf,
    }

    impl DirtyOnCloseBackend {
        fn new(initial: TerminalSurfaceState, dirty_on_close: PathBuf) -> Self {
            let mut surfaces = BTreeMap::new();
            let mut active_children = BTreeSet::new();
            active_children.insert(initial.surface_id.clone());
            surfaces.insert(initial.surface_id.clone(), initial);
            Self {
                surfaces: Mutex::new(surfaces),
                active_children: Mutex::new(active_children),
                dirty_on_close,
            }
        }

        fn active_children(&self) -> BTreeSet<String> {
            self.active_children.lock().unwrap().clone()
        }
    }

    impl TerminalBackend for DirtyOnCloseBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.active_children
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .insert(request.surface_id.clone());
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
            fs::write(&self.dirty_on_close, "dirty\n")
                .map_err(|err| TerminalError::Backend(err.to_string()))?;
            self.active_children
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id);
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .remove(surface_id)
                .ok_or_else(|| TerminalError::NotFound(surface_id.to_string()))?;
            Ok(())
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

    #[cfg(unix)]
    fn commit_failing_setup_hook(dir: &Path) {
        use std::os::unix::fs::PermissionsExt as _;

        let hook_dir = dir.join(".forktty");
        fs::create_dir_all(&hook_dir).unwrap();
        let hook_path = hook_dir.join("setup");
        fs::write(&hook_path, "#!/bin/sh\nexit 9\n").unwrap();
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

        let repo = Repository::open(dir).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(".forktty/setup")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "add hook", &tree, &[&parent])
            .unwrap();
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

        let result = probe_forktty_socket_with_timeout(client, Duration::from_secs(30)).unwrap();
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

        let result = probe_forktty_socket_with_timeout(client, Duration::from_secs(30)).unwrap();
        let _ = server_thread.join();
        assert!(!result);
    }

    #[test]
    fn probe_gives_up_on_peer_that_trickles_bytes() {
        use std::io::{BufRead as _, Write as _};

        let (client, server) = StdUnixStream::pair().unwrap();
        let server_thread = std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(server);
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            let mut server = reader.into_inner();
            // Dribble one byte per interval, faster than the per-read timeout
            // so no individual read ever times out, and never send a newline.
            // Writes fail once the probe gives up and drops its end.
            for _ in 0..300 {
                if server.write_all(b"x").is_err() {
                    return;
                }
                let _ = server.flush();
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let started = std::time::Instant::now();
        let result = probe_forktty_socket_with_timeout(client, Duration::from_millis(50)).unwrap();
        let elapsed = started.elapsed();
        let _ = server_thread.join();

        assert!(!result, "trickling peer must be treated as foreign");
        assert!(
            elapsed < Duration::from_secs(2),
            "probe must hit its overall deadline, took {elapsed:?}"
        );
    }

    #[test]
    fn transient_accept_errors_cover_fd_exhaustion() {
        assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
            libc::EMFILE
        )));
        assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
            libc::ENFILE
        )));
        assert!(is_transient_accept_error(&io::Error::from(
            io::ErrorKind::ConnectionAborted
        )));
        assert!(is_transient_accept_error(&io::Error::from(
            io::ErrorKind::ConnectionReset
        )));
        assert!(is_transient_accept_error(&io::Error::from(
            io::ErrorKind::Interrupted
        )));
        assert!(!is_transient_accept_error(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
        assert!(!is_transient_accept_error(&io::Error::from(
            io::ErrorKind::InvalidInput
        )));
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
    fn bind_socket_listener_creates_owner_only_socket() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");

        let listener = bind_socket_listener(&socket_path, false).unwrap();

        assert_eq!(
            fs::symlink_metadata(&socket_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(listener);
    }

    #[test]
    fn bind_socket_listener_cleans_up_staging_and_stays_connectable() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("forktty.sock");

        let listener = bind_socket_listener(&socket_path, false).unwrap();

        // Only the public socket may remain: the staging directory used to
        // bind with private permissions must be gone on success.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name != "forktty.sock")
            .collect();
        assert_eq!(leftovers, Vec::<std::ffi::OsString>::new());

        // The hard-linked path must reach the bound listener.
        let _client = StdUnixStream::connect(&socket_path).unwrap();
        drop(listener);
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
    async fn dispatches_system_ping() {
        let (state, _) = test_state();
        assert_eq!(
            dispatch(&state, "system.ping", json!({})).await.unwrap(),
            json!("pong")
        );
    }

    #[tokio::test]
    async fn dispatches_surface_send_text() {
        let (state, backend) = test_state();
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
    }

    #[tokio::test]
    async fn dispatches_surface_read_text() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        backend.send_text(surface_id, "alpha\nbeta\n").unwrap();

        let result = dispatch(
            &state,
            "surface.read_text",
            json!({"surface_id": surface_id}),
        )
        .await
        .unwrap();

        assert_eq!(result["surface_id"], surface_id);
        assert_eq!(result["scope"], "visible");
        assert_eq!(result["text"], "alpha\nbeta\n");
        assert_eq!(result["truncated"], false);
    }

    #[tokio::test]
    async fn dispatches_surface_capture_tail() {
        let (state, backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        backend.send_text(surface_id, "one\ntwo\nthree\n").unwrap();

        let result = dispatch(
            &state,
            "surface.capture_tail",
            json!({"surface_id": surface_id, "lines": 2}),
        )
        .await
        .unwrap();

        assert_eq!(result["surface_id"], surface_id);
        assert_eq!(result["scope"], "tail");
        assert_eq!(result["lines"], 2);
        assert_eq!(result["text"], "two\nthree\n");
        assert_eq!(result["truncated"], false);
    }

    #[tokio::test]
    async fn dispatches_topology_tree() {
        let (state, _) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let result = dispatch(&state, "topology.tree", json!({})).await.unwrap();

        assert_eq!(result["workspaces"][0]["id"], workspace_id);
        assert_eq!(result["workspaces"][0]["focused_surface_id"], surface_id);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["id"], surface_id);
        assert_eq!(result["workspaces"][0]["pane_tree"]["type"], "leaf");
    }

    #[tokio::test]
    async fn dispatches_notification_methods() {
        let (state, _) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

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
    }

    #[tokio::test]
    async fn dispatches_metadata_status_methods() {
        let (state, _) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();

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
    }

    #[tokio::test]
    async fn dispatches_metadata_progress_methods() {
        let (state, _) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();

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
    }

    #[tokio::test]
    async fn dispatches_metadata_log_methods() {
        let (state, _) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();

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
    async fn hook_session_status_learns_and_reuses_explicit_surface_target() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
        let surface_id = workspaces[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:claude",
                "label": "Claude",
                "value": "Ready",
                "hook_session_id": "session-a",
                "hook_event_name": "session-start",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();
        let other = dispatch(
            &state,
            "workspace.create",
            json!({"name": "other", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "key": "agent:claude",
                "label": "Claude",
                "value": "Running",
                "hook_session_id": "session-a",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let original_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let other_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": other["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(original_statuses[0]["value"], "Running");
        assert!(other_statuses.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn hook_status_persists_surface_agent_session_binding() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let resume_cwd = tempfile::tempdir().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Ready",
                "hook_session_id": "codex-session-9",
                "hook_session_cwd": resume_cwd.path(),
                "hook_event_name": "session-start",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();

        {
            let model = state.model.lock().unwrap();
            let session = model
                .surface(surface_id)
                .unwrap()
                .agent_session
                .as_ref()
                .unwrap();
            assert_eq!(session.agent, AgentKind::Codex);
            assert_eq!(session.session_id, "codex-session-9");
            assert_eq!(
                session.lifecycle,
                forktty_core::AgentSessionLifecycle::Running
            );
        }

        let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
        assert_eq!(
            agents[0]["resume_cwd"],
            resume_cwd.path().to_string_lossy().as_ref()
        );
        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
        assert_eq!(
            health[0]["resume_cwd"],
            resume_cwd.path().to_string_lossy().as_ref()
        );
        assert_eq!(
            health[0]["argv"],
            json!([
                "codex",
                "resume",
                "-C",
                resume_cwd.path().to_string_lossy().as_ref(),
                "codex-session-9"
            ])
        );
    }

    #[tokio::test]
    async fn hook_status_updates_persisted_agent_session_lifecycle() {
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
                "value": "Done",
                "hook_session_id": "codex-session-9",
                "hook_event_name": "stop",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();

        let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
        assert_eq!(agents[0]["lifecycle"], "idle");
        assert!(agents[0]["last_activity_ms"].as_u64().unwrap() > 0);

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Ended",
                "hook_session_id": "codex-session-9",
                "hook_event_name": "session-end",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
        assert_eq!(health[0]["lifecycle"], "ended");
        assert!(health[0]["last_activity_ms"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn hook_session_status_explicit_target_beats_learned_target() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
        let surface_id = workspaces[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:claude",
                "label": "Claude",
                "value": "Ready",
                "hook_session_id": "session-b",
                "hook_event_name": "session-start",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();
        let other = dispatch(
            &state,
            "workspace.create",
            json!({"name": "other", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let other_workspace_id = other["id"].as_str().unwrap().to_string();
        let other_surface_id = other["focused_surface_id"].as_str().unwrap().to_string();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": other_workspace_id,
                "surface_id": other_surface_id,
                "key": "agent:claude",
                "label": "Claude",
                "value": "Running elsewhere",
                "hook_session_id": "session-b",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let original_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let other_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": other_workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(original_statuses[0]["value"], "Ready");
        assert_eq!(other_statuses[0]["value"], "Running elsewhere");
    }

    #[tokio::test]
    async fn hook_session_status_surface_close_invalidates_learned_target() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
        let surface_id = workspaces[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();
        let split = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap();
        let mapped_surface_id = split["id"].as_str().unwrap().to_string();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": mapped_surface_id,
                "key": "agent:claude",
                "label": "Claude",
                "value": "Ready",
                "hook_session_id": "session-c",
                "hook_event_name": "session-start",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();
        let other = dispatch(
            &state,
            "workspace.create",
            json!({"name": "other", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "surface.close",
            json!({"surface_id": mapped_surface_id}),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "key": "agent:claude",
                "label": "Claude",
                "value": "Untargeted after close",
                "hook_session_id": "session-c",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let original_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let other_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": other["id"]}),
        )
        .await
        .unwrap();
        assert_eq!(original_statuses[0]["value"], "Ready");
        assert_eq!(other_statuses[0]["value"], "Untargeted after close");
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
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
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
            (
                "notification.create",
                json!({
                    "workspace_id": workspace_id,
                    "surface_id": surface_id,
                    "hook_session_id": oversized,
                    "title": "Prompt",
                    "body": "body"
                }),
                "hook_session_id",
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
    async fn metadata_set_status_rejects_invalid_required_fields() {
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
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains(message));
        }

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
    }

    #[tokio::test]
    async fn metadata_set_progress_rejects_invalid_required_fields() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for (method, params, message) in [
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
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "error");
            assert!(error.to_string().contains(message));
        }

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
    }

    #[tokio::test]
    async fn metadata_log_rejects_invalid_required_fields() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        for (method, params, message) in [
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
    async fn metadata_set_progress_rejects_missing_required_fields() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

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
            "metadata.list_progress",
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
    async fn workspace_create_rejects_oversized_name() {
        let (state, _backend) = test_state();

        let oversized = "x".repeat(MAX_METADATA_TEXT_BYTES + 1);
        let error = dispatch(
            &state,
            "workspace.create",
            json!({"name": oversized, "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "payload_too_large");

        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_last_workspace_closes_leave_exactly_one_replacement() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();

        // Two concurrent closes of the same last workspace: both may pass the
        // is_last_workspace check and spawn a replacement, but only one
        // commit can succeed; the loser must roll its replacement back.
        let state_a = state.clone();
        let state_b = state.clone();
        let id_a = workspace_id.clone();
        let id_b = workspace_id;
        let (a, b) = tokio::join!(
            tokio::spawn(async move {
                dispatch(&state_a, "workspace.close", json!({"id": id_a})).await
            }),
            tokio::spawn(async move {
                dispatch(&state_b, "workspace.close", json!({"id": id_b})).await
            }),
        );
        let (a, b) = (a.unwrap(), b.unwrap());

        assert!(
            a.is_ok() != b.is_ok(),
            "exactly one close must win the race: {a:?} / {b:?}"
        );
        let workspaces = {
            let model = state.model.lock().unwrap();
            model.list_workspaces()
        };
        assert_eq!(
            workspaces.len(),
            1,
            "the race must leave exactly one workspace, got {workspaces:?}"
        );
        assert_eq!(workspaces[0].name, "main");
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
    async fn agent_list_returns_only_surfaces_with_agent_sessions() {
        let (state, _backend) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            (workspace.id, surface_id)
        };
        let _plain = dispatch(
            &state,
            "workspace.create",
            json!({"name": "plain", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();

        assert_eq!(agents.as_array().unwrap().len(), 1);
        assert_eq!(agents[0]["workspace_id"], workspace_id);
        assert_eq!(agents[0]["surface_id"], surface_id);
        assert_eq!(agents[0]["agent"], "codex");
        assert_eq!(agents[0]["session_id"], "codex-session-1");

        let scoped = dispatch(&state, "agent.list", json!({"workspace_id": workspace_id}))
            .await
            .unwrap();
        assert_eq!(scoped.as_array().unwrap().len(), 1);

        let missing = dispatch(&state, "agent.list", json!({"workspace_name": "missing"}))
            .await
            .unwrap_err();
        assert_eq!(missing.code(), "not_found");
    }

    #[tokio::test]
    async fn agent_health_dispatches_rows_for_persisted_sessions() {
        let (state, _backend) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Custom,
                "custom-session-1",
            ));
            (workspace.id, surface_id)
        };

        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();

        assert_eq!(health.as_array().unwrap().len(), 1);
        assert_eq!(health[0]["workspace_id"], workspace_id);
        assert_eq!(health[0]["surface_id"], surface_id);
        assert_eq!(health[0]["agent"], "custom");
        assert_eq!(health[0]["session_id"], "custom-session-1");
        assert_eq!(health[0]["ready"], false);
        assert_eq!(health[0]["reason"], "unsupported_agent");
        assert_eq!(health[0]["argv"], json!([]));
    }

    #[test]
    fn agent_health_marks_resume_command_ready_when_provider_is_on_path() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        {
            let mut file = fs::File::create(&codex).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
        }
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));

        let health = agent_health_rows_with_path(&model, None, Some(dir.path().as_os_str()));

        assert_eq!(health.len(), 1);
        assert_eq!(health[0]["ready"], true);
        assert_eq!(health[0]["reason"], "ready");
        assert_eq!(health[0]["program"], "codex");
        assert_eq!(health[0]["executable"], codex.to_string_lossy().as_ref());
        assert_eq!(
            health[0]["argv"],
            json!(["codex", "resume", "codex-session-1"])
        );
    }

    #[test]
    #[serial_test::serial]
    fn agent_health_uses_codex_session_cwd_fallback_when_not_persisted() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let codex_home = dir.path().join("codex");
        let sessions_dir = codex_home.join("sessions/2026/06/12");
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::write(
            sessions_dir.join("rollout-2026-06-12T15-21-07-codex-session-health-fallback.jsonl"),
            format!(
                "{}\n",
                json!({
                    "type": "session_meta",
                    "payload": {
                        "id": "codex-session-health-fallback",
                        "cwd": project.to_string_lossy(),
                    }
                })
            ),
        )
        .unwrap();
        let _env = EnvGuard::set("CODEX_HOME", codex_home.to_str().unwrap());
        let path_dir = tempfile::tempdir().unwrap();
        let codex = path_dir.path().join("codex");
        {
            let mut file = fs::File::create(&codex).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
        }
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &surface_id,
            AgentKind::Codex,
            "codex-session-health-fallback",
        ));

        let health = agent_health_rows_with_path(&model, None, Some(path_dir.path().as_os_str()));

        assert_eq!(health.len(), 1);
        assert_eq!(health[0]["ready"], true);
        assert_eq!(health[0]["resume_cwd"], project.to_string_lossy().as_ref());
        assert_eq!(
            health[0]["argv"],
            json!([
                "codex",
                "resume",
                "-C",
                project.to_string_lossy().as_ref(),
                "codex-session-health-fallback"
            ])
        );
    }

    #[test]
    fn agent_health_marks_supported_agent_not_ready_when_provider_is_missing() {
        let empty_path = tempfile::tempdir().unwrap();
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));

        let health = agent_health_rows_with_path(&model, None, Some(empty_path.path().as_os_str()));

        assert_eq!(health.len(), 1);
        assert_eq!(health[0]["ready"], false);
        assert_eq!(health[0]["reason"], "program_not_found");
        assert_eq!(health[0]["program"], "codex");
        assert_eq!(health[0]["executable"], Value::Null);
        assert_eq!(
            health[0]["argv"],
            json!(["codex", "resume", "codex-session-1"])
        );
    }

    #[test]
    fn agent_reclaim_plan_marks_only_old_idle_ready_sessions_as_candidates() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let codex = dir.path().join("codex");
        {
            let mut file = fs::File::create(&codex).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
        }
        fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let candidate_surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &candidate_surface_id,
            AgentKind::Codex,
            "codex-session-1",
        ));
        assert!(model.set_surface_agent_session_lifecycle(
            &candidate_surface_id,
            AgentSessionLifecycle::Idle,
        ));
        assert!(model.set_surface_agent_session_last_activity_ms(&candidate_surface_id, 1_000));

        let protected_surface_id = model.add_tab(&candidate_surface_id).unwrap().id;
        assert!(model.set_surface_agent_session(
            &protected_surface_id,
            AgentKind::Codex,
            "codex-session-2",
        ));
        assert!(model.set_surface_agent_session_lifecycle(
            &protected_surface_id,
            AgentSessionLifecycle::NeedsInput,
        ));
        assert!(model.set_surface_agent_session_last_activity_ms(&protected_surface_id, 500));

        let plan =
            agent_reclaim_plan_with_path(&model, None, Some(dir.path().as_os_str()), 10_000, 5_000);

        assert_eq!(plan["policy"]["now_ms"], 10_000);
        assert_eq!(plan["policy"]["min_idle_ms"], 5_000);
        assert_eq!(plan["candidates"].as_array().unwrap().len(), 1);
        assert_eq!(plan["candidates"][0]["surface_id"], candidate_surface_id);
        assert_eq!(plan["candidates"][0]["idle_ms"], 9_000);
        assert_eq!(plan["candidates"][0]["ready"], true);
        assert_eq!(plan["protected"].as_array().unwrap().len(), 1);
        assert_eq!(plan["protected"][0]["surface_id"], protected_surface_id);
        assert_eq!(plan["protected"][0]["protect_reason"], "needs_input");
    }

    #[tokio::test]
    async fn status_summary_includes_workspace_agents_status_and_progress() {
        let (state, _backend) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            model
                .set_status(
                    &workspace.id,
                    "agent:codex",
                    "Codex",
                    "Running",
                    Some("blue".into()),
                )
                .unwrap();
            model
                .set_progress(&workspace.id, "build", "Build", 2.0, Some(4.0))
                .unwrap();
            (workspace.id, surface_id)
        };

        let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();

        assert_eq!(summary["workspace"]["id"], workspace_id);
        assert_eq!(summary["workspace"]["focused_surface_id"], surface_id);
        assert_eq!(summary["agents"][0]["agent"], "codex");
        assert_eq!(summary["agents"][0]["session_id"], "codex-session-1");
        assert_eq!(summary["status"][0]["key"], "agent:codex");
        assert_eq!(summary["status"][0]["value"], "Running");
        assert_eq!(summary["progress"][0]["key"], "build");
        assert_eq!(summary["progress"][0]["value"], 2.0);

        let scoped = dispatch(
            &state,
            "status.summary",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(scoped["workspace"]["id"], summary["workspace"]["id"]);
    }

    #[tokio::test]
    async fn agent_resume_opens_new_tab_with_provider_resume_argv() {
        let (state, backend) = test_state();
        let source_surface_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let source_surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &source_surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            source_surface_id
        };

        let resumed = dispatch(
            &state,
            "agent.resume",
            json!({"surface_id": source_surface_id}),
        )
        .await
        .unwrap();

        let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
        assert_ne!(resumed_surface_id, source_surface_id);
        assert_eq!(resumed["agent"], "codex");
        assert_eq!(resumed["session_id"], "codex-session-1");
        assert_eq!(
            resumed["argv"],
            json!(["codex", "resume", "codex-session-1"])
        );
        assert_eq!(backend.spawn_shell(resumed_surface_id).unwrap(), "codex");
        assert_eq!(
            backend.spawn_args(resumed_surface_id).unwrap(),
            vec!["resume", "codex-session-1"]
        );

        let model = state.model.lock().unwrap();
        let persisted = model
            .surface(resumed_surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap();
        assert_eq!(persisted.agent, AgentKind::Codex);
        assert_eq!(persisted.session_id, "codex-session-1");
    }

    #[tokio::test]
    async fn agent_resume_opens_claude_tab_from_persisted_session_cwd() {
        let (state, backend) = test_state();
        let resume_cwd = tempfile::tempdir().unwrap();
        let source_surface_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let source_surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &source_surface_id,
                AgentKind::ClaudeCode,
                "claude-session-1",
            ));
            assert!(model.set_surface_agent_session_resume_cwd(
                &source_surface_id,
                resume_cwd.path().to_path_buf()
            ));
            source_surface_id
        };

        let resumed = dispatch(
            &state,
            "agent.resume",
            json!({"surface_id": source_surface_id}),
        )
        .await
        .unwrap();

        let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
        assert_ne!(resumed_surface_id, source_surface_id);
        assert_eq!(resumed["agent"], "claude_code");
        assert_eq!(resumed["session_id"], "claude-session-1");
        assert_eq!(
            resumed["argv"],
            json!(["claude", "--resume", "claude-session-1"])
        );
        assert_eq!(backend.spawn_shell(resumed_surface_id).unwrap(), "claude");
        assert_eq!(
            backend.spawn_args(resumed_surface_id).unwrap(),
            vec!["--resume", "claude-session-1"]
        );
        let spawned = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == resumed_surface_id)
            .unwrap();
        assert_eq!(spawned.cwd, resume_cwd.path());

        let model = state.model.lock().unwrap();
        let persisted = model
            .surface(resumed_surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap();
        assert_eq!(persisted.resume_cwd.as_deref(), Some(resume_cwd.path()));
    }

    #[tokio::test]
    async fn hook_permission_mode_does_not_change_claude_resume_argv() {
        let (state, backend) = test_state();
        let (workspace_id, source_surface_id) = {
            let model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            (workspace.id.clone(), workspace.focused_surface_id.clone())
        };

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": source_surface_id,
                "key": "agent:claude",
                "label": "Claude",
                "value": "Ready",
                "color": "green",
                "hook_session_id": "claude-session-1",
                "hook_session_cwd": "/tmp",
                "hook_event_name": "session-start",
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": source_surface_id,
                "key": "agent:claude:permission",
                "label": "Claude mode",
                "value": "bypassPermissions",
                "color": "red",
                "hook_session_id": "claude-session-1",
                "hook_event_name": "session-start",
            }),
        )
        .await
        .unwrap();

        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
        assert_eq!(
            health[0]["argv"],
            json!(["claude", "--resume", "claude-session-1"])
        );

        let resumed = dispatch(
            &state,
            "agent.resume",
            json!({"surface_id": source_surface_id}),
        )
        .await
        .unwrap();

        let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
        assert_eq!(
            resumed["argv"],
            json!(["claude", "--resume", "claude-session-1"])
        );
        assert_eq!(
            backend.spawn_args(resumed_surface_id).unwrap(),
            vec!["--resume", "claude-session-1"]
        );
    }

    #[tokio::test]
    async fn hook_permission_mode_does_not_change_codex_resume_argv() {
        let (state, _backend) = test_state();
        let (workspace_id, source_surface_id) = {
            let model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            (workspace.id.clone(), workspace.focused_surface_id.clone())
        };

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": source_surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Ready",
                "color": "green",
                "hook_session_id": "codex-session-1",
                "hook_session_cwd": "/tmp",
                "hook_event_name": "session-start",
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": source_surface_id,
                "key": "agent:codex:permission",
                "label": "Codex mode",
                "value": "bypassPermissions",
                "color": "red",
                "hook_session_id": "codex-session-1",
                "hook_event_name": "session-start",
            }),
        )
        .await
        .unwrap();

        let resumed = dispatch(
            &state,
            "agent.resume",
            json!({"surface_id": source_surface_id}),
        )
        .await
        .unwrap();

        assert_eq!(
            resumed["argv"],
            json!(["codex", "resume", "-C", "/tmp", "codex-session-1"])
        );
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
    async fn worktree_create_preserves_existing_worktree_when_spawn_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/existing-spawn-rollback-{}", std::process::id());
        let created = worktree::create(
            repo_dir.path().to_str().unwrap(),
            &branch_name,
            "../forktty-worktrees/{name}",
        )
        .unwrap();
        let existing_path = created.path.clone();
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
        assert!(Path::new(&existing_path).exists());
        let repo = Repository::open(repo_dir.path()).unwrap();
        assert!(repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .is_ok());
        let worktrees = worktree::list(repo_dir.path().to_str().unwrap()).unwrap();
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].branch, branch_name);
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
    async fn worktree_create_reopens_existing_worktree_after_workspace_close() {
        let repo_dir = make_temp_repo();
        let (state, _backend) = test_state();
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
            json!({"name": "topic/retry", "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();
        let first_workspace_id = created["id"].as_str().unwrap().to_string();

        dispatch(&state, "workspace.close", json!({"id": first_workspace_id}))
            .await
            .unwrap();
        let reopened = dispatch(
            &state,
            "worktree.create",
            json!({"name": "topic/retry", "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();

        assert_eq!(reopened["branch"], "topic/retry");
        assert_eq!(reopened["path"], created["path"]);
        assert_eq!(reopened["worktree_name"], created["worktree_name"]);
        assert_ne!(reopened["id"], created["id"]);
        assert_eq!(
            worktree::list(repo_dir.path().to_str().unwrap())
                .unwrap()
                .len(),
            1
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn worktree_create_surfaces_setup_hook_failure_as_warning_and_notification() {
        let repo_dir = make_temp_repo();
        commit_failing_setup_hook(repo_dir.path());
        let (state, _backend) = test_state();
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
            json!({"name": "topic/hook-fail", "cwd": repo_dir.path()}),
        )
        .await
        .unwrap();

        // The worktree is still created (setup hook failure is non-fatal)...
        assert_eq!(created["branch"], "topic/hook-fail");
        // ...but the failure is now visible as a structured warning.
        let warning = created["setup_warning"].as_str().unwrap();
        assert!(
            warning.contains("setup hook failed"),
            "warning should explain the failure: {warning}"
        );

        // ...and as a workspace-scoped error notification.
        let workspace_id = created["id"].as_str().unwrap();
        let notifications = dispatch(&state, "notification.list", json!({}))
            .await
            .unwrap();
        let hook_notification = notifications
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["title"] == "Worktree Setup Hook Failed")
            .expect("setup hook failure should produce a notification");
        assert_eq!(hook_notification["workspace_id"], workspace_id);
        assert_eq!(hook_notification["kind"], "error");
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
        assert_eq!(
            worktree::list(repo_dir.path().to_str().unwrap())
                .unwrap()
                .len(),
            1
        );
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
        assert_eq!(
            worktree::list(repo_dir.path().to_str().unwrap())
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn worktree_remove_last_workspace_closes_replacement_when_finish_fails() {
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/socket-remove-finish-{}", std::process::id());
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
        let backend = Arc::new(DirtyOnCloseBackend::new(
            TerminalSurfaceState {
                surface_id: surface_id.clone(),
                workspace_id: workspace.id.clone(),
                cwd: worktree_cwd.clone(),
                shell: "/bin/sh".to_string(),
                cols: 80,
                rows: 24,
            },
            worktree_cwd.join("dirty-after-close.txt"),
        ));
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

        assert!(error.contains("uncommitted changes"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        assert_eq!(workspaces.as_array().unwrap().len(), 1);
        assert_eq!(workspaces[0]["id"], workspace.id);
        let backend_surfaces = backend.surfaces().unwrap();
        assert_eq!(backend_surfaces.len(), 1);
        assert_eq!(backend_surfaces[0].surface_id, surface_id);
        assert_eq!(backend.active_children(), BTreeSet::from([surface_id]));
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

        assert_eq!(error.code(), "precondition_failed");
        let error = error.to_string();
        assert!(error.contains("open workspace"));
        // The rejection must tell the caller how to satisfy the precondition.
        assert!(error.contains("create-workspace"));
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

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn random_param_value(rng: &mut u64, depth: u32) -> Value {
        // Keys real handlers look for, plus garbage; values span every JSON
        // type so each parameter extraction path sees the wrong type too.
        const KEYS: &[&str] = &[
            "surface_id",
            "surfaceId",
            "workspace",
            "workspace_id",
            "name",
            "cwd",
            "text",
            "host",
            "message",
            "title",
            "kind",
            "level",
            "label",
            "value",
            "id",
            "axis",
            "branch",
            "path",
            "url",
            "garbage \u{0} key",
        ];
        const STRINGS: &[&str] = &[
            "",
            " ",
            "x",
            "workspace-1",
            "surface-1",
            "../../../etc/passwd",
            "-1",
            "0",
            "999999999999999999999",
            "*",
            "/",
            "\\",
            "🦀\u{7f}",
            "{\"nested\":true}",
        ];
        let variants = if depth == 0 { 7 } else { 9 };
        match xorshift(rng) % variants {
            0 => Value::Null,
            1 => json!(true),
            2 => json!(-1),
            3 => json!(u64::MAX),
            4 => json!(f64::MAX),
            5 => json!(STRINGS[(xorshift(rng) % STRINGS.len() as u64) as usize]),
            6 => json!("a".repeat((xorshift(rng) % 8192) as usize)),
            7 => Value::Array(
                (0..xorshift(rng) % 3)
                    .map(|_| random_param_value(rng, depth - 1))
                    .collect(),
            ),
            _ => {
                let mut object = serde_json::Map::new();
                for _ in 0..xorshift(rng) % 4 {
                    object.insert(
                        KEYS[(xorshift(rng) % KEYS.len() as u64) as usize].to_string(),
                        random_param_value(rng, depth - 1),
                    );
                }
                Value::Object(object)
            }
        }
    }

    /// Deterministic randomized sweep over every dispatchable method with
    /// adversarial params: the socket accepts NDJSON from any local client,
    /// so no params shape may panic the server (errors are fine).
    #[tokio::test]
    #[serial_test::serial]
    async fn dispatch_never_panics_on_adversarial_params() {
        let data_dir = tempfile::tempdir().unwrap();
        let _data_env = EnvGuard::set("XDG_DATA_HOME", data_dir.path().to_str().unwrap());

        let fixed = [
            Value::Null,
            json!({}),
            json!([]),
            json!(""),
            json!(0),
            json!(true),
            json!({"surface_id": null, "workspace": [], "text": {}}),
        ];
        let mut rng = 0x5eed_2026_0610_f00du64;
        for method in METHODS {
            // Fresh state per method so earlier mutations (closed workspaces,
            // split surfaces) cannot mask a later parameter path.
            let (state, _backend) = test_state();
            for params in &fixed {
                let _ = dispatch(&state, method, params.clone()).await;
            }
            for _ in 0..60 {
                let params = random_param_value(&mut rng, 2);
                let _ = dispatch(&state, method, params).await;
            }
        }
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
    async fn pane_new_tab_adds_tab_and_returns_surface() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        let new_surface = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
            .await
            .unwrap();

        // The result is a new Surface with its own id
        let new_id = new_surface["id"].as_str().unwrap();
        assert_ne!(new_id, surface_id);
        assert_eq!(new_surface["workspace_id"].as_str().unwrap(), workspace_id);

        // Both surfaces now appear in the list
        let surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        let ids: Vec<_> = surfaces
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&surface_id));
        assert!(ids.contains(&new_id));
    }

    #[tokio::test]
    async fn pane_new_tab_returns_not_found_for_unknown_surface() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": "no-such-surface"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "not_found");
    }

    #[tokio::test]
    async fn pane_select_tab_selects_existing_tab() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        // Create a second tab in the same pane
        let new_surface = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
            .await
            .unwrap();
        let new_id = new_surface["id"].as_str().unwrap().to_string();

        // Select back the original tab
        let result = dispatch(&state, "pane.select_tab", json!({"surface_id": surface_id}))
            .await
            .unwrap();
        assert_eq!(result["selected"], true);

        // Select the new tab
        let result = dispatch(&state, "pane.select_tab", json!({"surface_id": new_id}))
            .await
            .unwrap();
        assert_eq!(result["selected"], true);
    }

    #[tokio::test]
    async fn pane_select_tab_returns_not_found_for_unknown_surface() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "pane.select_tab",
            json!({"surface_id": "no-such-tab"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "not_found");
    }

    #[tokio::test]
    async fn pane_new_tab_rolls_back_model_when_spawn_fails() {
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
        let workspace_id = workspaces[0]["id"].as_str().unwrap();

        let error = dispatch(&state, "pane.new_tab", json!({"surface_id": surface_id}))
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("spawn failed"));
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
        assert!(validate_worktree_name("-flag").is_err());
        assert!(validate_worktree_name("feature\nname").is_err());
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

    #[test]
    fn rejects_ambiguous_directory_param_aliases() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        let workspace_error = resolve_workspace_cwd_param(&json!({
            "workingDir": first.path(),
            "cwd": second.path(),
        }))
        .unwrap_err();

        assert!(workspace_error.contains("Ambiguous path parameter"));
        assert!(workspace_error.contains("workingDir and cwd"));

        let repo_error = resolve_required_existing_dir_param(
            &json!({
                "path": first.path(),
                "cwd": second.path(),
            }),
            &["path", "cwd"],
            "path or cwd",
        )
        .unwrap_err();

        assert_eq!(repo_error.code(), "error");
        assert!(repo_error.to_string().contains("Ambiguous path parameter"));
        assert!(repo_error.to_string().contains("path and cwd"));
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

    #[cfg(feature = "browser")]
    #[tokio::test]
    async fn socket_connection_rejects_browser_import_methods() {
        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(handle_connection(server, state));
        let (read_half, mut write_half) = client.into_split();

        write_half
            .write_all(br#"{"id":1,"method":"browser.import.discover","params":{}}"#)
            .await
            .unwrap();
        write_half.write_all(b"\n").await.unwrap();
        write_half.shutdown().await.unwrap();

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        let response: JsonRpcResponse = serde_json::from_str(line.trim_end()).unwrap();

        assert!(!response.ok);
        assert_eq!(response.id, json!(1));
        assert_eq!(response.error.unwrap().code, "method_not_found");
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
        #[cfg(not(feature = "browser"))]
        assert!(!methods.iter().any(|m| {
            m.as_str()
                .is_some_and(|method| method.starts_with("browser."))
        }));
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

    #[cfg(not(feature = "browser"))]
    #[tokio::test]
    async fn browser_methods_are_not_available_without_browser_feature() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "browser.open",
            json!({"workspace_id": "workspace-1", "url": "https://example.com"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "method_not_found");
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
    async fn poisoned_model_lock_does_not_broadcast_false_removals() {
        let (state, _backend) = test_state();
        let mut receiver = state.events.subscribe();
        spawn_event_tick(state.clone());
        // Let at least one healthy tick run so the tick task's previous
        // snapshot contains the bootstrapped workspace.
        tokio::time::sleep(EVENTS_TICK * 2).await;

        // Poison the model lock from a thread that panics while holding it.
        let model = state.model.clone();
        std::thread::spawn(move || {
            let _guard = model.lock().unwrap();
            panic!("poison the model lock");
        })
        .join()
        .unwrap_err();
        assert!(state.model.lock().is_err(), "lock must be poisoned");

        // Ticks against the poisoned lock must be skipped, not diffed against
        // an empty snapshot (which would broadcast a removal of every
        // workspace and surface to all subscribers).
        tokio::time::sleep(EVENTS_TICK * 4).await;
        assert!(
            matches!(
                receiver.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "no events may be broadcast while the lock is poisoned"
        );
    }

    #[tokio::test]
    async fn stalled_subscriber_is_dropped_by_the_write_timeout() {
        use tokio::io::AsyncReadExt;

        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let state_for_stream = state.clone();
        let stream_task = tokio::spawn(async move {
            let (read_half, mut write_half) = server.into_split();
            let mut reader = BufReader::new(read_half);
            stream_events(
                &state_for_stream,
                false,
                &mut reader,
                &mut write_half,
                Duration::from_millis(200),
            )
            .await
        });

        // Read the handshake so the subscription is live, then stop reading.
        // The client write half must stay open: a stalled client, not a
        // disconnected one (EOF would end the stream cleanly via peer_closed).
        let (mut client_read, _client_write_keepalive) = client.into_split();
        let mut buf = [0u8; 32];
        let read = client_read.read(&mut buf).await.unwrap();
        assert!(read > 0, "handshake reached the client");

        // Saturate the kernel socket buffer with large events until the
        // stream's write stalls and the timeout fires.
        let big_title = "x".repeat(64 * 1024);
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let _ = state.events.send(ModelEvent::SurfaceTitleChanged {
                    id: "s1".to_string(),
                    title: big_title.clone(),
                });
                if stream_task.is_finished() {
                    return stream_task.await;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("stalled subscriber must be dropped, not held forever");

        let err = result
            .unwrap()
            .expect_err("stream must end with a timeout error");
        assert!(
            err.to_string().contains("stopped reading"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn stalled_request_client_is_dropped_by_the_response_write_timeout() {
        let (state, _backend) = test_state();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();

        // Enough log payload that the serialized metadata.list_logs response
        // cannot fit in the kernel socket buffers, so the response write
        // stalls until the timeout fires.
        let big_message = "x".repeat(16_000);
        for _ in 0..64 {
            dispatch(
                &state,
                "metadata.log",
                json!({"workspace_id": workspace_id, "message": big_message}),
            )
            .await
            .unwrap();
        }

        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let connection_task = tokio::spawn(handle_connection_with_write_timeout(
            server,
            state,
            Duration::from_millis(200),
        ));

        // Send a request whose response is huge, then never read. Both client
        // halves stay open: a stalled client, not a disconnected one (EOF or
        // EPIPE would end the connection without the write timeout).
        let (_client_read_keepalive, mut client_write) = client.into_split();
        let request = format!(
            "{{\"id\":1,\"method\":\"metadata.list_logs\",\"params\":{{\"workspace_id\":\"{workspace_id}\"}}}}\n"
        );
        client_write.write_all(request.as_bytes()).await.unwrap();

        let result = tokio::time::timeout(Duration::from_secs(10), connection_task)
            .await
            .expect("stalled client must be dropped, not held forever")
            .unwrap();
        let err = result.expect_err("connection must end with a timeout error");
        assert!(
            err.to_string().contains("stopped reading"),
            "unexpected error: {err}"
        );
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

    #[cfg(feature = "browser")]
    mod browser_tests {
        use super::*;

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

            let same_url = dispatch(
                &state,
                "browser.navigate",
                json!({"surface_id": surface_id, "url": "https://other.com"}),
            )
            .await
            .unwrap();
            assert_eq!(same_url["navigated"], json!(true));

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

        #[tokio::test]
        async fn browser_url_limit_applies_after_default_scheme() {
            let (state, _backend) = test_state();
            let created = dispatch(&state, "workspace.create", json!({"name": "w"}))
                .await
                .unwrap();
            let ws_id = created["id"].as_str().unwrap();
            let bare_url = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len() + 1);

            let err = dispatch(
                &state,
                "browser.open",
                json!({"workspace_id": ws_id, "url": bare_url}),
            )
            .await
            .unwrap_err();

            assert_eq!(err.code(), "payload_too_large");
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
        async fn browser_eval_is_not_exposed() {
            let (state, _rx) = state_with_browser_channel();
            let sid = open_browser_surface(&state).await;
            let err = dispatch(
                &state,
                "browser.eval",
                json!({"surface_id": sid, "script": "document.title"}),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "method_not_found");
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

        #[tokio::test]
        async fn browser_fill_maps_not_interactable_reply() {
            let (state, rx) = state_with_browser_channel();
            let sid = open_browser_surface(&state).await;
            let responder = tokio::spawn(async move {
                let cmd = rx.recv().await.unwrap();
                assert_eq!(
                    cmd.op,
                    forktty_core::BrowserOp::Fill {
                        reference: "e1".to_string(),
                        value: "hello".to_string(),
                    }
                );
                cmd.reply
                    .send(forktty_core::CmdResult::Err(
                        forktty_core::BrowserCmdError::ElementNotInteractable,
                    ))
                    .unwrap();
            });
            let err = dispatch(
                &state,
                "browser.fill",
                json!({"surface_id": sid, "ref": "e1", "value": "hello"}),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "invalid_param");
            assert_eq!(err.to_string(), "element is not interactable");
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
            assert_eq!(history_limit_from_params(&json!({})).unwrap(), 100);
            assert_eq!(
                history_limit_from_params(&json!({"limit": null})).unwrap(),
                100
            );
            assert_eq!(history_limit_from_params(&json!({"limit": 5})).unwrap(), 5);
            assert_eq!(
                history_limit_from_params(&json!({"limit": u64::MAX})).unwrap(),
                10_000
            );
            assert!(matches!(
                history_limit_from_params(&json!({"limit": "5"})),
                Err(DispatchError::InvalidParam(_))
            ));
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

        fn create_firefox_import_source(home: &Path, name: &str) -> forktty_import::SourceProfile {
            let profile_dir = home.join(".mozilla/firefox").join(name);
            fs::create_dir_all(&profile_dir).unwrap();

            let cookies = rusqlite::Connection::open(profile_dir.join("cookies.sqlite")).unwrap();
            cookies
                .execute_batch(
                    "CREATE TABLE moz_cookies (
                    name TEXT, value TEXT, host TEXT, path TEXT,
                    expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER
                 );
                 INSERT INTO moz_cookies VALUES ('sid','cookie-value','.example.test','/',0,0,1);",
                )
                .unwrap();
            drop(cookies);

            let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
            places
                .execute_batch(
                    "CREATE TABLE moz_places (
                    id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
                 );
                 CREATE TABLE moz_bookmarks (
                    id INTEGER PRIMARY KEY, fk INTEGER, title TEXT, type INTEGER
                 );
                 INSERT INTO moz_places (id,url,title,visit_count)
                    VALUES (1,'https://example.test/','Example',2);
                 INSERT INTO moz_bookmarks (fk,title,type)
                    VALUES (1,'Example Bookmark',1);",
                )
                .unwrap();
            drop(places);

            let profile = forktty_import::SourceProfile {
                family: forktty_import::BrowserFamily::Firefox,
                display_name: name.to_string(),
                path: profile_dir.to_string_lossy().into_owned(),
                is_default: false,
            };
            let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
            fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
            profile
        }

        fn create_corrupt_firefox_import_source(
            home: &Path,
            name: &str,
        ) -> forktty_import::SourceProfile {
            let profile_dir = home.join(".mozilla/firefox").join(name);
            fs::create_dir_all(&profile_dir).unwrap();
            let cookies = rusqlite::Connection::open(profile_dir.join("cookies.sqlite")).unwrap();
            cookies
                .execute_batch(
                    "CREATE TABLE moz_cookies (
                    name TEXT, value TEXT, host TEXT, path TEXT,
                    expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER
                 );",
                )
                .unwrap();
            drop(cookies);
            fs::write(profile_dir.join("places.sqlite"), b"not a sqlite database").unwrap();
            let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
            fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
            forktty_import::SourceProfile {
                family: forktty_import::BrowserFamily::Firefox,
                display_name: name.to_string(),
                path: profile_dir.to_string_lossy().into_owned(),
                is_default: true,
            }
        }

        fn create_firefox_import_source_with_corrupt_cookies(
            home: &Path,
            name: &str,
        ) -> forktty_import::SourceProfile {
            let profile_dir = home.join(".mozilla/firefox").join(name);
            fs::create_dir_all(&profile_dir).unwrap();
            fs::write(profile_dir.join("cookies.sqlite"), b"not a sqlite database").unwrap();
            let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
            places
                .execute_batch(
                    "CREATE TABLE moz_places (
                    id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
                 );
                 INSERT INTO moz_places (id,url,title,visit_count)
                    VALUES (1,'https://history-only.test/','History Only',3);",
                )
                .unwrap();
            drop(places);
            let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
            fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
            forktty_import::SourceProfile {
                family: forktty_import::BrowserFamily::Firefox,
                display_name: name.to_string(),
                path: profile_dir.to_string_lossy().into_owned(),
                is_default: true,
            }
        }

        fn create_firefox_import_source_with_long_history_url(
            home: &Path,
            name: &str,
        ) -> forktty_import::SourceProfile {
            let profile_dir = home.join(".mozilla/firefox").join(name);
            fs::create_dir_all(&profile_dir).unwrap();
            let long_url = format!("https://{}.test/", "a".repeat(MAX_BROWSER_URL_BYTES + 1));
            let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
            places
                .execute(
                    "CREATE TABLE moz_places (
                        id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
                    );",
                    [],
                )
                .unwrap();
            places
                .execute(
                    "INSERT INTO moz_places (id,url,title,visit_count)
                        VALUES (1,?1,'Too Long',4);",
                    [&long_url],
                )
                .unwrap();
            drop(places);
            write_firefox_profiles_ini(home, &[name]);
            forktty_import::SourceProfile {
                family: forktty_import::BrowserFamily::Firefox,
                display_name: name.to_string(),
                path: profile_dir.to_string_lossy().into_owned(),
                is_default: true,
            }
        }

        fn write_firefox_profiles_ini(home: &Path, names: &[&str]) {
            let mut profiles_ini = String::new();
            for (index, name) in names.iter().enumerate() {
                profiles_ini.push_str(&format!(
                    "[Profile{index}]\nName={name}\nPath={name}\nDefault={}\n",
                    usize::from(index == 0)
                ));
            }
            fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_discover_preview_and_run_imports_history_bookmarks() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let source = create_firefox_import_source(&home, "default-release");
            let source_id = browser_import_source_id(&source);
            let (state, _backend) = test_state();

            let discovered = dispatch(&state, "browser.import.discover", json!({}))
                .await
                .unwrap();
            assert_eq!(discovered["count"], json!(1));
            assert_eq!(
                discovered["browsers"][0]["profiles"][0]["id"],
                json!(source_id)
            );

            let preview = dispatch(
                &state,
                "browser.import.preview",
                json!({"sources": [source_id.clone()]}),
            )
            .await
            .unwrap();
            assert_eq!(preview["total"]["history"], json!(1));
            assert_eq!(preview["total"]["bookmarks"], json!(1));
            assert_eq!(preview["total"]["cookies"], json!(1));

            let imported = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id.clone()],
                    "destination": {"kind": "existing", "profile": "Default"}
                }),
            )
            .await
            .unwrap();
            assert_eq!(imported["total"]["written"]["history"], json!(1));
            assert_eq!(imported["total"]["written"]["bookmarks"], json!(1));
            assert_eq!(imported["total"]["cookies"]["written"], json!(0));
            assert_eq!(imported["total"]["cookies"]["unsupported"], json!(1));

            let history = dispatch(
                &state,
                "browser.history.search",
                json!({"query": "example.test", "limit": 10}),
            )
            .await
            .unwrap();
            assert_eq!(history.as_array().unwrap().len(), 1);
            assert_eq!(history[0]["visit_count"], json!(2));
            let bookmarks = dispatch(&state, "browser.bookmark.list", json!({}))
                .await
                .unwrap();
            assert_eq!(bookmarks.as_array().unwrap().len(), 1);

            let imported_again = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id],
                    "destination": {"kind": "existing", "profile": "Default"}
                }),
            )
            .await
            .unwrap();
            assert_eq!(imported_again["total"]["written"]["history"], json!(1));
            let history_after = dispatch(
                &state,
                "browser.history.search",
                json!({"query": "example.test", "limit": 10}),
            )
            .await
            .unwrap();
            assert_eq!(history_after.as_array().unwrap().len(), 1);
            assert_eq!(history_after[0]["visit_count"], json!(2));
            let bookmarks_after = dispatch(&state, "browser.bookmark.list", json!({}))
                .await
                .unwrap();
            assert_eq!(bookmarks_after.as_array().unwrap().len(), 1);
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_reports_skipped_oversized_history_urls() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let source = create_firefox_import_source_with_long_history_url(&home, "long-url");
            let source_id = browser_import_source_id(&source);
            let (state, _backend) = test_state();

            let imported = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id],
                    "destination": {"kind": "existing", "profile": "Default"}
                }),
            )
            .await
            .unwrap();

            assert_eq!(imported["total"]["read"]["history"], json!(1));
            assert_eq!(imported["total"]["written"]["history"], json!(0));
            assert_eq!(imported["entries"][0]["written"]["history"], json!(0));
            let history = dispatch(&state, "browser.history.list", json!({}))
                .await
                .unwrap();
            assert!(history.as_array().unwrap().is_empty());
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_run_creates_new_profile_from_plan() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let source = create_firefox_import_source(&home, "work");
            let source_id = browser_import_source_id(&source);
            let (state, _backend) = test_state();

            let imported = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id],
                    "destination": {"kind": "create", "display_name": "Imported Work"}
                }),
            )
            .await
            .unwrap();
            assert_eq!(
                imported["entries"][0]["destination"]["created"],
                json!(true)
            );
            let profile_id = imported["entries"][0]["destination"]["id"]
                .as_str()
                .unwrap()
                .to_string();

            let profiles = dispatch(&state, "browser.profile.list", json!({}))
                .await
                .unwrap();
            assert!(profiles.as_array().unwrap().iter().any(|profile| {
                profile["id"] == json!(profile_id)
                    && profile["display_name"] == json!("Imported Work")
            }));
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_skips_unselected_corrupt_cookie_db() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let source = create_firefox_import_source_with_corrupt_cookies(&home, "history-only");
            let source_id = browser_import_source_id(&source);
            let (state, _backend) = test_state();

            let preview = dispatch(
                &state,
                "browser.import.preview",
                json!({
                    "sources": [source_id.clone()],
                    "include": {"history": true, "bookmarks": false, "cookies": false}
                }),
            )
            .await
            .unwrap();
            assert_eq!(preview["total"]["history"], json!(1));
            assert_eq!(preview["total"]["cookies"], json!(0));

            let imported = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id],
                    "include": {"history": true, "bookmarks": false, "cookies": false},
                    "destination": {"kind": "existing", "profile": "Default"}
                }),
            )
            .await
            .unwrap();
            assert_eq!(imported["total"]["written"]["history"], json!(1));
            assert_eq!(imported["total"]["cookies"]["read"], json!(0));
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_missing_and_corrupt_sources_are_errors_not_crashes() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let (state, _backend) = test_state();

            let missing = dispatch(
                &state,
                "browser.import.preview",
                json!({"sources": ["firefox:/does/not/exist"]}),
            )
            .await
            .unwrap_err();
            assert_eq!(missing.code(), "not_found");

            let source = create_corrupt_firefox_import_source(&home, "corrupt");
            let corrupt = dispatch(
                &state,
                "browser.import.preview",
                json!({"sources": [browser_import_source_id(&source)]}),
            )
            .await
            .unwrap_err();
            assert_eq!(corrupt.code(), "error");
            assert!(corrupt.to_string().contains("import database error"));
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_run_does_not_create_profile_for_unreadable_source() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let source = create_corrupt_firefox_import_source(&home, "corrupt-create");
            let source_id = browser_import_source_id(&source);
            let (state, _backend) = test_state();

            let err = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [source_id],
                    "destination": {"kind": "create", "display_name": "Should Roll Back"}
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "error");

            let profiles = dispatch(&state, "browser.profile.list", json!({}))
                .await
                .unwrap();
            assert!(!profiles
                .as_array()
                .unwrap()
                .iter()
                .any(|profile| { profile["display_name"] == json!("Should Roll Back") }));
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_run_does_not_partially_write_existing_profile_on_later_read_error()
        {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let valid = create_firefox_import_source(&home, "valid");
            let corrupt = create_corrupt_firefox_import_source(&home, "corrupt");
            write_firefox_profiles_ini(&home, &["valid", "corrupt"]);
            let (state, _backend) = test_state();

            let err = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [
                        browser_import_source_id(&valid),
                        browser_import_source_id(&corrupt)
                    ],
                    "destination": {"kind": "existing", "profile": "Default"}
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "error");

            let history = dispatch(&state, "browser.history.list", json!({}))
                .await
                .unwrap();
            assert!(history.as_array().unwrap().is_empty());
            let bookmarks = dispatch(&state, "browser.bookmark.list", json!({}))
                .await
                .unwrap();
            assert!(bookmarks.as_array().unwrap().is_empty());
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_run_does_not_create_earlier_separate_profile_on_later_read_error() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let valid = create_firefox_import_source(&home, "valid");
            let corrupt = create_corrupt_firefox_import_source(&home, "corrupt");
            write_firefox_profiles_ini(&home, &["valid", "corrupt"]);
            let (state, _backend) = test_state();

            let err = dispatch(
                &state,
                "browser.import.run",
                json!({
                    "sources": [
                        browser_import_source_id(&valid),
                        browser_import_source_id(&corrupt)
                    ],
                    "mode": "separate_profiles"
                }),
            )
            .await
            .unwrap_err();
            assert_eq!(err.code(), "error");

            let profiles = dispatch(&state, "browser.profile.list", json!({}))
                .await
                .unwrap();
            assert!(!profiles.as_array().unwrap().iter().any(|profile| {
                profile["display_name"] == json!("valid")
                    || profile["display_name"] == json!("corrupt")
            }));
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_import_rejects_ambiguous_and_invalid_params() {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join("home");
            fs::create_dir_all(&home).unwrap();
            let _home = EnvGuard::set("HOME", home.to_str().unwrap());
            let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
            let (state, _backend) = test_state();

            for (method, params, expected) in [
                (
                    "browser.import.preview",
                    json!({"all": true, "sources": ["firefox:/tmp/profile"]}),
                    "cannot combine all and sources",
                ),
                (
                    "browser.import.preview",
                    json!({"sources": [" \t "]}),
                    "sources must not include empty source ids",
                ),
                (
                    "browser.import.run",
                    json!({"all": true, "mode": 42}),
                    "Invalid parameter mode",
                ),
                (
                    "browser.import.run",
                    json!({"all": true, "destination": {"kind": 42}}),
                    "Invalid parameter destination.kind",
                ),
                (
                    "browser.import.run",
                    json!({"all": true, "destination": {"kind": "create", "display_name": 42}}),
                    "Invalid parameter destination.display_name",
                ),
                (
                    "browser.import.preview",
                    json!({
                        "sources": ["firefox:/tmp/profile"],
                        "include": {"history": false, "bookmarks": false, "cookies": false}
                    }),
                    "select at least one browser data type",
                ),
            ] {
                let err = dispatch(&state, method, params).await.unwrap_err();
                assert_eq!(err.code(), "invalid_param");
                assert!(
                    err.to_string().contains(expected),
                    "expected {expected:?}, got {err}"
                );
            }
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

            for query in [json!(""), json!(" \t "), json!(42)] {
                let err = dispatch(&state, "browser.history.search", json!({"query": query}))
                    .await
                    .unwrap_err();
                assert_eq!(err.code(), "error");
                assert!(err.to_string().contains("Invalid parameter query"));
            }
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
        async fn browser_history_rejects_invalid_limit() {
            let tmp = tempfile::tempdir().unwrap();
            let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
            let (state, _backend) = test_state();

            for (method, params) in [
                ("browser.history.list", json!({"limit": "5"})),
                ("browser.history.list", json!({"limit": -1})),
                (
                    "browser.history.search",
                    json!({"query": "example", "limit": 1.5}),
                ),
            ] {
                let err = dispatch(&state, method, params).await.unwrap_err();
                assert_eq!(err.code(), "invalid_param");
                assert!(err.to_string().contains("Invalid parameter limit"));
            }
        }

        #[tokio::test]
        #[serial_test::serial]
        async fn browser_bookmark_trims_url_and_title_and_rejects_bad_remove_url() {
            let tmp = tempfile::tempdir().unwrap();
            let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
            let (state, _backend) = test_state();

            let added = dispatch(
                &state,
                "browser.bookmark.add",
                json!({"url": " https://trim.test/ ", "title": " Trimmed "}),
            )
            .await
            .unwrap();
            assert_eq!(added["added"], json!(true));

            let listed = dispatch(&state, "browser.bookmark.list", json!({}))
                .await
                .unwrap();
            assert_eq!(listed[0]["url"], json!("https://trim.test/"));
            assert_eq!(listed[0]["title"], json!("Trimmed"));

            let invalid_title = dispatch(
                &state,
                "browser.bookmark.add",
                json!({"url": "https://title.test/", "title": 42}),
            )
            .await
            .unwrap_err();
            assert_eq!(invalid_title.code(), "invalid_param");
            assert!(invalid_title
                .to_string()
                .contains("Invalid parameter title"));

            let empty_remove = dispatch(&state, "browser.bookmark.remove", json!({"url": "  "}))
                .await
                .unwrap_err();
            assert_eq!(empty_remove.code(), "invalid_param");
            assert!(empty_remove.to_string().contains("url must not be empty"));

            let removed = dispatch(
                &state,
                "browser.bookmark.remove",
                json!({"url": " https://trim.test/ "}),
            )
            .await
            .unwrap();
            assert_eq!(removed["removed"], json!(true));
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

    #[tokio::test]
    async fn workspace_create_ssh_returns_workspace_and_spawns_ssh_process() {
        let (state, backend) = test_state();
        let result = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "user@example.com", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();

        assert_eq!(result["name"], "workspace");
        let surface_id = result["focused_surface_id"].as_str().unwrap();

        // The spawned shell should be the ssh binary and args should be the host.
        let shell = backend.spawn_shell(surface_id).unwrap();
        assert!(
            shell.ends_with("/ssh") || shell == "ssh",
            "expected ssh binary, got {shell}"
        );
        let args = backend.spawn_args(surface_id).unwrap();
        assert_eq!(args, vec!["user@example.com"]);
    }

    #[tokio::test]
    async fn workspace_select_respawns_ssh_workspace_with_ssh_process() {
        let (state, backend) = test_state();
        let result = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "user@example.com", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let workspace_id = result["id"].as_str().unwrap();
        let surface_id = result["focused_surface_id"].as_str().unwrap();
        backend.close(surface_id).unwrap();

        dispatch(&state, "workspace.select", json!({"id": workspace_id}))
            .await
            .unwrap();

        let shell = backend.spawn_shell(surface_id).unwrap();
        assert!(
            shell.ends_with("/ssh") || shell == "ssh",
            "expected ssh binary, got {shell}"
        );
        assert_eq!(
            backend.spawn_args(surface_id).unwrap(),
            vec!["user@example.com"]
        );
    }

    #[test]
    fn bootstrap_default_workspace_creates_new_workspace_if_none_exists() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        assert!(model.lock().unwrap().active_workspace().is_none());

        bootstrap_default_workspace(&state, PathBuf::from("/foo/bar")).unwrap();

        let m = model.lock().unwrap();
        let workspace = m.active_workspace().unwrap();
        assert_eq!(workspace.working_dir, PathBuf::from("/foo/bar"));

        let surfaces = m.list_surfaces(Some(&workspace.id));
        assert_eq!(surfaces.len(), 1);
        let surface_id = &surfaces[0].id;

        let shell = backend.spawn_shell(surface_id).unwrap();
        assert_eq!(shell, "/bin/sh");
    }

    #[test]
    fn bootstrap_default_workspace_respawns_existing_ssh_workspace_with_ssh_process() {
        let (state, backend) = test_state();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = runtime
            .block_on(dispatch(
                &state,
                "workspace.create_ssh",
                json!({"host": "server.local", "workingDir": "/tmp"}),
            ))
            .unwrap();
        let surface_id = result["focused_surface_id"].as_str().unwrap();
        backend.close(surface_id).unwrap();

        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();

        let shell = backend.spawn_shell(surface_id).unwrap();
        assert!(
            shell.ends_with("/ssh") || shell == "ssh",
            "expected ssh binary, got {shell}"
        );
        assert_eq!(
            backend.spawn_args(surface_id).unwrap(),
            vec!["server.local"]
        );
    }

    #[tokio::test]
    async fn workspace_create_ssh_with_custom_name() {
        let (state, _backend) = test_state();
        let result = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "server.local", "name": "my-server", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        assert_eq!(result["name"], "my-server");
    }

    #[tokio::test]
    async fn workspace_create_ssh_rejects_empty_host() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "", "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
    }

    #[tokio::test]
    async fn workspace_create_ssh_rejects_missing_host() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "missing_param");
    }

    #[tokio::test]
    async fn workspace_create_ssh_rejects_host_with_leading_dash() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "-oProxyCommand=x", "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
    }

    #[tokio::test]
    async fn workspace_create_ssh_rejects_host_with_whitespace() {
        let (state, _backend) = test_state();
        let err = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "a b", "workingDir": "/tmp"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
    }
}
