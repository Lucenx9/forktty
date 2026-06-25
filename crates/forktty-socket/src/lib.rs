mod agent_params;
mod agent_runtime;
mod browser_import;
mod browser_import_params;
mod browser_params;
mod browser_profile;
mod browser_runtime;
mod connection;
mod context_params;
mod context_runtime;
mod coordinator;
mod feed_events;
mod feed_params;
mod feed_runtime;
mod feed_view;
mod hook_session;
mod metadata_params;
mod metadata_runtime;
mod methods;
mod notification_dispatch;
mod path_resolver;
mod project_action_params;
mod project_action_runtime;
mod provider_runtime;
mod remote;
mod response_encoding;
mod socket_bind;
mod status_runtime;
mod store_access;
mod surface_runtime;
mod system_runtime;
mod team_dispatch;
mod team_params;
mod team_provider;
mod team_runtime;
mod team_state;
mod topology_params;
mod topology_runtime;
mod topology_view;
mod workflow_params;
mod workflow_runtime;
mod workspace_runtime;
mod worktree_params;
mod worktree_runtime;

pub(crate) use agent_runtime::{
    agent_health_rows, agent_session_identify_row, agent_session_lifecycle_from_hook,
    agent_session_rows, effective_agent_resume_cwd,
};
#[cfg(test)]
pub(crate) use agent_runtime::{
    agent_health_rows_with_path, agent_reclaim_plan_with_path, surface_status_key,
};
#[cfg(all(test, feature = "browser"))]
use browser_import::browser_import_spool_data;
#[cfg(all(test, feature = "browser"))]
use browser_import_params::browser_import_source_id;
use connection::{handle_connection_with_event_limit, reject_over_capacity_connection};
pub(crate) use context_runtime::workspace_effective_project_cwd;
#[cfg(test)]
pub(crate) use context_runtime::{context_snapshot_risk_flags, ContextSnapshotRiskInputs};
use coordinator::SocketCoordinator;
use forktty_core::events::{self, ModelEvent, Snapshot};
use forktty_core::protocol_limits;
#[cfg(test)]
use forktty_core::worktree;
#[cfg(test)]
use forktty_core::JsonRpcResponse;
#[cfg(all(test, feature = "browser"))]
use forktty_core::MAX_BROWSER_URL_BYTES;
use forktty_core::{
    agent_resume_command_with_cwd_and_permission_mode, command_safety::is_valid_ssh_host,
    AgentKind, AgentSessionLifecycle, BrowserCommand, FeedApprovalState, FeedStore, LogLevel,
    NotificationItem, NotificationKind, SplitAxis, StatusHookMetadata, WorkspaceModel,
    WorkspaceSelector,
};
#[cfg(test)]
use forktty_core::{FeedEntry, FeedEntryType};
use forktty_terminal::{SharedTerminalBackend, SpawnRequest, TerminalError, TerminalTextCapture};
use serde_json::{json, Value};
use std::io;
#[cfg(all(test, feature = "browser"))]
use std::io::{Seek, SeekFrom};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener as StdUnixListener;
#[cfg(test)]
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::Ordering;
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
#[cfg(test)]
use tokio::io::AsyncBufRead;
use tokio::net::UnixListener;
use tokio::sync::{broadcast, Semaphore};

#[cfg(test)]
mod env_access {
    use std::ffi::OsString;
    use std::sync::{Condvar, Mutex, OnceLock};
    use std::thread::ThreadId;

    static ENV_TEST_LOCK: OnceLock<EnvTestLock> = OnceLock::new();

    struct EnvTestLock {
        state: Mutex<EnvTestLockState>,
        cvar: Condvar,
    }

    #[derive(Default)]
    struct EnvTestLockState {
        owner: Option<ThreadId>,
        depth: usize,
    }

    pub(crate) struct EnvTestLockGuard {
        lock: &'static EnvTestLock,
    }

    pub(crate) fn lock_env_for_test() -> EnvTestLockGuard {
        let lock = ENV_TEST_LOCK.get_or_init(|| EnvTestLock {
            state: Mutex::new(EnvTestLockState::default()),
            cvar: Condvar::new(),
        });
        let current = std::thread::current().id();
        let mut state = lock.state.lock().expect("env test lock poisoned");
        loop {
            match state.owner {
                Some(owner) if owner == current => {
                    state.depth += 1;
                    return EnvTestLockGuard { lock };
                }
                Some(_) => {
                    state = lock.cvar.wait(state).expect("env test lock poisoned");
                }
                None => {
                    state.owner = Some(current);
                    state.depth = 1;
                    return EnvTestLockGuard { lock };
                }
            }
        }
    }

    impl Drop for EnvTestLockGuard {
        fn drop(&mut self) {
            let current = std::thread::current().id();
            let mut state = self.lock.state.lock().expect("env test lock poisoned");
            debug_assert_eq!(state.owner, Some(current));
            state.depth = state.depth.saturating_sub(1);
            if state.depth == 0 {
                state.owner = None;
                self.lock.cvar.notify_all();
            }
        }
    }

    pub(crate) fn with_env_read_lock<T>(f: impl FnOnce() -> T) -> T {
        let _guard = lock_env_for_test();
        f()
    }

    pub(crate) fn var_os(key: &str) -> Option<OsString> {
        with_env_read_lock(|| std::env::var_os(key))
    }

    pub(crate) fn var(key: &str) -> Result<String, std::env::VarError> {
        with_env_read_lock(|| std::env::var(key))
    }
}

#[cfg(test)]
pub(crate) use env_access::{
    lock_env_for_test, var as env_var, var_os as env_var_os, with_env_read_lock, EnvTestLockGuard,
};

#[cfg(not(test))]
pub(crate) fn env_var_os(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key)
}

#[cfg(not(test))]
pub(crate) fn env_var(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

#[cfg(test)]
pub(crate) use connection::{
    handle_connection, handle_connection_with_limits, handle_connection_with_write_timeout,
    lagged_notice, read_limited_line, stream_events, ReadLineError,
};
pub use socket_bind::{bind_socket_listener, default_socket_path, socket_path_from_env};
#[cfg(test)]
pub(crate) use socket_bind::{
    default_socket_dir_from_env, effective_uid, probe_forktty_socket_with_timeout,
    PROBE_RESPONSE_MAX_BYTES,
};
#[cfg(test)]
pub(crate) use team_provider::{team_worker_launch_command, CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS};

const MAX_REQUEST_SIZE: usize = protocol_limits::SOCKET_REQUEST_MAX_BYTES;
const MAX_SEND_TEXT_BYTES: usize = protocol_limits::SOCKET_SEND_TEXT_MAX_BYTES;
const MAX_TERMINAL_TEXT_BYTES: usize = protocol_limits::SOCKET_TERMINAL_TEXT_MAX_BYTES;
const DEFAULT_CAPTURE_TAIL_LINES: usize = 80;
const MAX_CAPTURE_TAIL_LINES: usize = 5_000;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_LINES: usize = 40;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES: usize =
    protocol_limits::DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES;
const MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES: usize = 16;
const MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES: usize =
    protocol_limits::MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
const MAX_METADATA_TEXT_BYTES: usize = protocol_limits::SOCKET_METADATA_TEXT_MAX_BYTES;
const MAX_SOCKET_CONNECTIONS: usize = 64;
/// Max time to wait for a client to deliver a complete request line before the
/// connection is dropped. Clients send a full `{json}\n` immediately, so this
/// only fires on idle or slow-loris connections that would otherwise hold one
/// of the [`MAX_SOCKET_CONNECTIONS`] permits indefinitely.
const REQUEST_READ_TIMEOUT: Duration = protocol_limits::SOCKET_REQUEST_READ_TIMEOUT;
/// Buffered events per subscriber before a slow client gets a `Lagged` notice.
const EVENTS_CHANNEL_CAPACITY: usize = 256;
const MAX_EVENT_SUBSCRIBERS: usize = 32;
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
const RESPONSE_WRITE_TIMEOUT: Duration = protocol_limits::SOCKET_RESPONSE_WRITE_TIMEOUT;
const HOOK_SESSION_TARGET_CAPACITY: usize = 256;
const DEFAULT_AGENT_RECLAIM_MIN_IDLE_MS: u64 = 10 * 60 * 1_000;
const DEFAULT_TEAM_WORKER_STALE_MS: u64 = 5 * 60 * 1_000;

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
/// `conflict`, `already_exists`, `not_ready`, `invalid_param`,
/// `precondition_failed`, `error`). Existing handlers that return ad-hoc `String`
/// errors keep working via the [`From<String>`] impl below; new sites should
/// prefer the structured variants so the response carries a useful `error.code`.
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

impl From<forktty_core::TeamError> for DispatchError {
    fn from(err: forktty_core::TeamError) -> Self {
        use forktty_core::TeamError as T;
        match err {
            T::TeamNotFound(_) => DispatchError::NotFound("team".to_string()),
            T::WorkerNotFound(_) => DispatchError::NotFound("worker".to_string()),
            T::TaskNotFound(_) => DispatchError::NotFound("task".to_string()),
            T::MessageNotFound(_) => DispatchError::NotFound("message".to_string()),
            T::Invalid(message) => DispatchError::InvalidParam(message),
            T::UnsupportedVersion(_) | T::Json(_) | T::Io(_) => {
                DispatchError::Other(err.to_string())
            }
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
        if is_invalid_param_message(&message) {
            DispatchError::InvalidParam(message)
        } else {
            DispatchError::Other(message)
        }
    }
}

impl From<&str> for DispatchError {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

fn is_invalid_param_message(message: &str) -> bool {
    message.starts_with("Invalid parameter ")
        || (message.starts_with("Ambiguous ")
            && (message.contains(" selector:") || message.contains(" parameter:")))
}

#[derive(Clone)]
pub struct SocketAppState {
    pub model: Arc<Mutex<WorkspaceModel>>,
    pub profile_store_lock: Arc<Mutex<()>>,
    pub terminal: SharedTerminalBackend,
    pub shell: String,
    pub socket_path: PathBuf,
    pub workflow_store_path: Option<PathBuf>,
    pub notification_dispatch: bool,
    pub team_store_path: Option<PathBuf>,
    /// Broadcast channel feeding `events.subscribe` connections. The background
    /// tick task in [`serve`] is the sole producer.
    pub events: broadcast::Sender<ModelEvent>,
    /// Sends scripting commands to the GTK WebView. `None` when no browser
    /// engine is wired (no `browser` feature, or headless), in which case the
    /// browser scripting verbs report unavailable.
    pub browser_cmd: Option<async_channel::Sender<BrowserCommand>>,
    feed_store: Arc<Mutex<Option<FeedStore>>>,
    hook_session_targets: Arc<Mutex<hook_session::HookSessionTargets>>,
    coordinator: Arc<SocketCoordinator>,
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
            workflow_store_path: forktty_core::workflow_store_path().ok(),
            notification_dispatch: true,
            team_store_path: if cfg!(test) {
                None
            } else {
                forktty_core::team_store_path().ok()
            },
            events,
            browser_cmd: None,
            feed_store: Arc::new(Mutex::new(None)),
            hook_session_targets: Arc::new(Mutex::new(hook_session::HookSessionTargets::default())),
            coordinator: Arc::new(SocketCoordinator::default()),
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

    pub fn with_workflow_store_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.workflow_store_path = Some(path.into());
        self
    }

    pub fn with_default_feed_store(self) -> Self {
        match FeedStore::open_default() {
            Ok(Some(store)) => self.with_feed_store(store),
            Ok(None) => self,
            Err(err) => {
                eprintln!("forktty feed history disabled: {err}");
                self
            }
        }
    }

    pub fn with_feed_store_path(self, path: impl AsRef<Path>) -> Result<Self, String> {
        FeedStore::open_at(path)
            .map(|store| self.with_feed_store(store))
            .map_err(|err| err.to_string())
    }

    fn with_feed_store(self, store: FeedStore) -> Self {
        if let Ok(mut feed_store) = self.feed_store.lock() {
            *feed_store = Some(store);
        }
        self
    }

    pub fn mark_notification_feed_entries_dismissed(&self, notifications: &[NotificationItem]) {
        let ids = notifications
            .iter()
            .filter(|notification| notification.kind == NotificationKind::Prompt)
            .map(feed_events::feed_notification_entry_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        let Ok(mut store) = self.feed_store.lock() else {
            return;
        };
        let Some(store) = store.as_mut() else {
            return;
        };
        if let Err(err) = store.mark_approvals(ids, FeedApprovalState::Dismissed) {
            eprintln!("forktty feed history dismiss update failed: {err}");
        }
    }
}

pub async fn serve(listener: StdUnixListener, state: SocketAppState) -> Result<(), SocketError> {
    let listener = UnixListener::from_std(listener)?;
    spawn_event_tick(state.clone());
    let connection_limit = Arc::new(Semaphore::new(MAX_SOCKET_CONNECTIONS));
    let event_subscription_limit = Arc::new(Semaphore::new(MAX_EVENT_SUBSCRIBERS));
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
        let event_subscription_limit = event_subscription_limit.clone();
        let Ok(permit) = connection_limit.clone().try_acquire_owned() else {
            tokio::spawn(async move {
                reject_over_capacity_connection(stream).await;
            });
            continue;
        };
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(err) =
                handle_connection_with_event_limit(stream, state, event_subscription_limit).await
            {
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
            let events = events::diff(&prev, &next);
            if let Err(err) = feed_events::record_feed_events(&state, &events) {
                eprintln!("forktty feed history update failed: {err}");
            }
            for event in events {
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
    let _hook_session_end = hook_session::prepare_hook_session_targets(state, method, &mut params)?;

    match method {
        "system.ping" => Ok(system_runtime::ping()),
        "system.capabilities" => Ok(system_runtime::capabilities()),
        "system.identify" => system_runtime::identify(state, &params),
        "context.snapshot" => context_runtime::snapshot(state, &params).await,
        "feed.approval.respond" => feed_runtime::approval_respond(state, &params),
        "feed.list" => feed_runtime::list(state, &params),
        "workflow.list" => workflow_runtime::list(state, &params).await,
        "workflow.get" => workflow_runtime::get(state, &params).await,
        "workflow.upsert" => workflow_runtime::upsert(state, &params).await,
        "workflow.loop.set" => workflow_runtime::loop_set(state, &params).await,
        "workflow.plan.set" => workflow_runtime::plan_set(state, &params).await,
        "workflow.evidence.add" => workflow_runtime::evidence_add(state, &params).await,
        "workflow.replay" => workflow_runtime::replay(state, &params).await,
        "team.list" => team_runtime::list(state, &params).await,
        "team.get" => team_runtime::get(state, &params).await,
        "team.upsert" => team_runtime::upsert(state, &params).await,
        "team.finish" => team_runtime::finish(state, &params).await,
        "team.worker.upsert" => team_runtime::worker_upsert(state, &params).await,
        "team.worker.heartbeat" => team_runtime::worker_heartbeat(state, &params).await,
        "team.worker.launch" => team_runtime::worker_launch(state, &params).await,
        "team.worker.health" => team_runtime::worker_health(state, &params).await,
        "team.worker.nudge" => team_runtime::worker_nudge(state, &params).await,
        "team.worker.shutdown" => team_runtime::worker_shutdown(state, &params).await,
        "team.task.upsert" => team_runtime::task_upsert(state, &params).await,
        "team.message.send" => team_runtime::message_send(state, &params).await,
        "team.message.dispatch" => team_runtime::message_dispatch(state, &params).await,
        "team.message.ack" => team_runtime::message_ack(state, &params).await,
        "team.inbox" => team_runtime::inbox(state, &params).await,
        "team.summary" => team_runtime::summary(state, &params).await,
        "team.events" => team_runtime::events(state, &params).await,

        "agent.health" => agent_runtime::health(state, &params),
        "agent.hibernate" => agent_runtime::hibernate(state, &params),
        "agent.list" => agent_runtime::list(state, &params),
        "agent.reclaim.plan" => agent_runtime::reclaim_plan(state, &params),
        "agent.reclaim" => agent_runtime::reclaim(state, &params),
        "agent.resume" => agent_runtime::resume(state, &params).await,
        "status.summary" => status_runtime::summary(state, &params),
        "remote.list" => remote::list(state, &params),
        "remote.status" => remote::status(state, &params),
        "system.top" => topology_runtime::system_top(state, &params),
        "workspace.list" => topology_runtime::workspace_list(state),
        "workspace.create" => workspace_runtime::create(state, &params),
        "workspace.create_ssh" => workspace_runtime::create_ssh(state, &params),
        "workspace.select" => workspace_runtime::select(state, &params).await,
        "workspace.close" => workspace_runtime::close(state, &params).await,
        "worktree.list" => worktree_runtime::list(state, &params).await,
        "worktree.status" => worktree_runtime::status(state, &params).await,
        "worktree.create" => worktree_runtime::create(state, &params).await,
        "worktree.attach" => worktree_runtime::attach(state, &params).await,
        "worktree.remove" => worktree_runtime::remove(state, &params).await,
        "worktree.merge" => worktree_runtime::merge(state, &params).await,
        "project.action.list" => project_action_runtime::list(state, &params).await,
        "project.action.run" => project_action_runtime::run(state, &params).await,
        "surface.list" => topology_runtime::surface_list(state, &params),
        "surface.read_text" => topology_runtime::surface_read_text(state, &params),
        "surface.capture_tail" => topology_runtime::surface_capture_tail(state, &params),
        "surface.send_text" => topology_runtime::surface_send_text(state, &params),
        "topology.tree" => topology_runtime::tree(state, &params),
        "surface.split" => surface_runtime::split(state, &params).await,
        "browser.open" => browser_runtime::open(state, &params),
        "browser.navigate" => browser_runtime::navigate(state, &params),
        "browser.snapshot" => browser_runtime::snapshot(state, &params).await,
        "browser.click" => browser_runtime::click(state, &params).await,
        "browser.fill" => browser_runtime::fill(state, &params).await,
        "browser.back" => browser_runtime::back(state, &params).await,
        "browser.forward" => browser_runtime::forward(state, &params).await,
        "browser.reload" => browser_runtime::reload(state, &params).await,
        "browser.profile.list" => browser_runtime::profile_list(state, &params),
        "browser.profile.create" => browser_runtime::profile_create(state, &params),
        "browser.profile.delete" => browser_runtime::profile_delete(state, &params),
        "browser.history.list" => browser_runtime::history_list(state, &params),
        "browser.history.search" => browser_runtime::history_search(state, &params),
        "browser.history.clear" => browser_runtime::history_clear(state, &params),
        "browser.bookmark.add" => browser_runtime::bookmark_add(state, &params),
        "browser.bookmark.list" => browser_runtime::bookmark_list(state, &params),
        "browser.bookmark.remove" => browser_runtime::bookmark_remove(state, &params),
        "browser.import.discover" => Ok(browser_import::browser_import_discover_json()),
        "browser.import.preview" => browser_import::browser_import_preview(&params).await,
        "browser.import.run" => browser_import::browser_import_run(state, &params).await,
        "pane.new_tab" => surface_runtime::new_tab(state, &params).await,
        "pane.select_tab" => surface_runtime::select_tab(state, &params),
        "surface.focus" => surface_runtime::focus(state, &params),
        "surface.close" => surface_runtime::close(state, &params).await,
        "notification.create" => metadata_runtime::notification_create(state, &params),
        "notification.list" => metadata_runtime::notification_list(state),
        "notification.clear" => metadata_runtime::notification_clear(state),
        "metadata.set_status" => metadata_runtime::set_status(state, &params),
        "metadata.list_status" => metadata_runtime::list_status(state, &params),
        "metadata.clear_status" => metadata_runtime::clear_status(state, &params),
        "metadata.set_progress" => metadata_runtime::set_progress(state, &params),
        "metadata.list_progress" => metadata_runtime::list_progress(state, &params),
        "metadata.clear_progress" => metadata_runtime::clear_progress(state, &params),
        "metadata.log" => metadata_runtime::log(state, &params),
        "metadata.list_logs" => metadata_runtime::list_logs(state, &params),
        "metadata.clear_logs" => metadata_runtime::clear_logs(state, &params),
        _ => Err(DispatchError::MethodNotFound(method.to_string())),
    }
}

fn method_allowed_from_socket(method: &str) -> bool {
    methods::allowed_from_socket(method)
}

pub(crate) fn surface_effective_project_cwd(surface: &forktty_core::Surface) -> PathBuf {
    surface
        .agent_session
        .as_ref()
        .and_then(effective_agent_resume_cwd)
        .unwrap_or_else(|| surface.cwd.clone())
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

pub(crate) fn spawn_workspace_terminal(
    state: &SocketAppState,
    workspace: &forktty_core::Workspace,
) -> Result<(), String> {
    let Some(request) = spawn_request_for_workspace(state, workspace)? else {
        return Ok(());
    };
    state.terminal.spawn(request).map_err(|err| err.to_string())
}

pub(crate) fn spawn_surface_terminal(
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
    if agent_session.lifecycle == AgentSessionLifecycle::Suspended {
        return None;
    }
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

pub(crate) fn spawn_terminal_surfaces(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    for surface in surfaces {
        spawn_surface_terminal(state, surface)?;
    }
    Ok(())
}

pub(crate) fn close_terminal_surface_if_present(
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

pub(crate) fn close_replacement_terminal_surface_if_present(
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

pub(crate) fn close_terminal_surfaces_or_restore(
    state: &SocketAppState,
    surfaces: &[forktty_core::Surface],
) -> Result<(), String> {
    let mut closed = Vec::new();
    for surface in surfaces {
        if let Err(err) = close_terminal_surface_if_present(state, &surface.id) {
            if !closed.is_empty() {
                if let Err(respawn_err) = spawn_terminal_surfaces(state, &closed) {
                    return Err(format!("{err}; terminal restore failed: {respawn_err}"));
                }
            }
            return Err(err);
        }
        closed.push(surface.clone());
    }
    Ok(())
}

pub(crate) async fn close_surface_request(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<Value, DispatchError> {
    let _surface_set_guard = state.coordinator.surface_set.lock().await;
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

fn evict_hook_session_targets_for_surface(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), DispatchError> {
    state
        .hook_session_targets
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?
        .remove_surface(surface_id);
    team_state::forget_team_launch_owned_surface_for_surface(state, surface_id)?;
    Ok(())
}

pub(crate) fn evict_hook_session_targets_for_surfaces(
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
    drop(targets);
    team_state::forget_team_launch_owned_surfaces_for_surfaces(state, surface_ids)?;
    Ok(())
}

pub(crate) fn rollback_workspace_creation(
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
pub(crate) fn rollback_replacement_if_redundant(
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

pub(crate) fn rollback_surface_creation(
    state: &SocketAppState,
    surface_id: &str,
) -> Result<(), String> {
    let mut model = state
        .model
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let _ = model.close_surface(surface_id);
    Ok(())
}

pub(crate) async fn ensure_terminal_for_active_workspace(
    state: &SocketAppState,
) -> Result<(), String> {
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

fn optional_string_param<'a>(
    params: &'a Value,
    key: &'static str,
) -> Result<Option<&'a str>, DispatchError> {
    match params.get(key) {
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected string").into()),
    }
}

fn optional_bool_param(params: &Value, key: &'static str) -> Result<Option<bool>, DispatchError> {
    match params.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected boolean").into()),
    }
}

fn optional_string_array_param(
    params: &Value,
    key: &'static str,
) -> Result<Option<Vec<String>>, DispatchError> {
    match params.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        DispatchError::InvalidParam(format!(
                            "Invalid parameter {key}: expected array of non-empty strings"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("Invalid parameter {key}: expected array").into()),
    }
}

fn optional_workspace_create_name_from_params(
    params: &Value,
) -> Result<Option<&str>, DispatchError> {
    let Some(value) = params.get("name") else {
        return Ok(None);
    };
    match value.as_str().map(str::trim) {
        Some(name) if !name.is_empty() => {
            ensure_max_text_size("name", name)?;
            Ok(Some(name))
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

/// Maximum number of items an explicit `limit` parameter may request on list
/// methods that otherwise return every match. Mirrors the cap applied to
/// browser history (`history_limit_from_params`) so no single request can ask
/// for an unbounded result set.
const MAX_LIST_LIMIT: u64 = 10_000;

/// Read an optional `limit` parameter, clamping any explicit value to
/// [`MAX_LIST_LIMIT`]. Returns `None` when the caller omits `limit`, preserving
/// the "no limit" semantics of the underlying list stores.
fn optional_limit_param(params: &Value, key: &'static str) -> Result<Option<usize>, DispatchError> {
    Ok(optional_u64_param(params, key)?.map(|limit| limit.min(MAX_LIST_LIMIT) as usize))
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
        "pi" => Some(AgentKind::Pi),
        "opencode" | "open-code" | "open_code" => Some(AgentKind::OpenCode),
        "custom" => Some(AgentKind::Custom),
        _ => None,
    }
}

fn agent_kind_from_permission_status_key(key: &str) -> Option<AgentKind> {
    let mut parts = key.split(':');
    if parts.next()? != "agent" {
        return None;
    }
    let provider = parts.next()?;
    if parts.next()? != "permission" {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    match provider {
        "claude" | "claude-code" | "claude_code" => Some(AgentKind::ClaudeCode),
        "codex" => Some(AgentKind::Codex),
        "antigravity" | "agy" => Some(AgentKind::Antigravity),
        "pi" => Some(AgentKind::Pi),
        "opencode" | "open-code" | "open_code" => Some(AgentKind::OpenCode),
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

pub(crate) fn format_param_names<'a>(names: impl Iterator<Item = &'a str>) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_core::validate_worktree_name;
    use forktty_terminal::{
        HeadlessTerminalBackend, TerminalBackend, TerminalError, TerminalSurfaceState,
        TerminalTextCapture, TerminalTextSnapshot,
    };
    use git2::Repository;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    #[cfg(feature = "browser")]
    use std::sync::Barrier;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
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

    #[test]
    fn gemini_status_keys_are_not_agent_bindings() {
        assert!(agent_kind_from_status_key("agent:gemini").is_none());
        assert!(agent_kind_from_permission_status_key("agent:gemini:permission").is_none());
    }

    #[test]
    fn pi_status_keys_bind_to_pi_agent() {
        assert_eq!(agent_kind_from_status_key("agent:pi"), Some(AgentKind::Pi));
        assert_eq!(
            agent_kind_from_permission_status_key("agent:pi:permission"),
            Some(AgentKind::Pi)
        );
    }

    #[test]
    fn env_guard_serializes_with_guarded_readers() {
        let original_path = with_env_read_lock(|| std::env::var("PATH").ok());
        let dir = tempfile::tempdir().unwrap();
        let temp_path = dir.path().to_str().unwrap().to_string();
        let (writer_ready_tx, writer_ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let writer = std::thread::spawn(move || {
            let _path = EnvGuard::set("PATH", &temp_path);
            writer_ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        writer_ready_rx.recv().unwrap();

        let (reader_tx, reader_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            reader_tx
                .send(with_env_read_lock(|| std::env::var("PATH").ok()))
                .unwrap();
        });
        assert!(
            reader_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "guarded env readers must wait while EnvGuard holds a temporary value"
        );
        release_tx.send(()).unwrap();
        writer.join().unwrap();
        let observed_path = reader_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        reader.join().unwrap();
        assert_eq!(observed_path, original_path);
    }

    #[test]
    fn team_worker_launch_rejects_removed_gemini_provider() {
        let err = team_worker_launch_command("gemini", None, Vec::new()).unwrap_err();
        assert!(
            err.to_string()
                .contains("unsupported team worker agent: gemini"),
            "{err}"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn capabilities_report_provider_detection_and_team_policy() {
        let dir = tempfile::tempdir().unwrap();
        let codex = write_fake_program(dir.path(), "codex");
        let pi = write_fake_program(dir.path(), "pi");
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (state, _backend) = test_state();

        let result = dispatch(&state, "system.capabilities", json!({}))
            .await
            .unwrap();

        let policy = &result["team_provider_policy"];
        assert_eq!(policy["default_agent"], "auto");
        assert_eq!(
            policy["provider_order"],
            json!(["codex", "claude", "pi", "opencode", "antigravity"])
        );
        assert_eq!(policy["auto_fallback"], true);
        assert_eq!(policy["disabled_agents"], json!([]));

        let providers = &result["provider_capabilities"];
        assert_eq!(providers["codex"]["available_on_path"], true);
        assert_eq!(
            providers["codex"]["executable"].as_str().unwrap(),
            codex.to_string_lossy()
        );
        assert_eq!(providers["pi"]["available_on_path"], true);
        assert_eq!(
            providers["pi"]["executable"].as_str().unwrap(),
            pi.to_string_lossy()
        );
        assert_eq!(providers["claude"]["available_on_path"], false);
        assert_eq!(
            providers["claude"]["unavailable_reason"],
            "program_not_found"
        );
        assert_eq!(result["pty_persistence"]["config_enabled"], false);
        assert_eq!(result["pty_persistence"]["available"], false);
        assert_eq!(result["pty_persistence"]["active"], false);
        assert_eq!(
            result["pty_persistence"]["unavailable_reason"],
            "broker_not_found"
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn capabilities_report_pty_persistence_broker_status() {
        let dir = tempfile::tempdir().unwrap();
        let dtach = write_fake_program(dir.path(), "dtach");
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let forktty_config_dir = config_home.path().join("forktty");
        fs::create_dir_all(&forktty_config_dir).unwrap();
        fs::write(
            forktty_config_dir.join("config.toml"),
            r#"
            [general]
            persist_terminal_processes = true
            "#,
        )
        .unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (state, _backend) = test_state();

        let result = dispatch(&state, "system.capabilities", json!({}))
            .await
            .unwrap();

        let pty = &result["pty_persistence"];
        assert_eq!(pty["config_enabled"], true);
        assert_eq!(pty["available"], true);
        assert_eq!(pty["active"], true);
        assert_eq!(pty["broker"], "dtach");
        assert_eq!(
            pty["broker_executable"].as_str().unwrap(),
            dtach.to_string_lossy()
        );
        assert_eq!(pty["scope"], "plain_terminal_surfaces");
        assert_eq!(pty["unavailable_reason"], Value::Null);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn capabilities_report_configured_provider_command_outside_path() {
        let path_dir = tempfile::tempdir().unwrap();
        let custom_dir = tempfile::tempdir().unwrap();
        let custom_codex = write_fake_program(custom_dir.path(), "codex-custom");
        let _path = EnvGuard::set("PATH", path_dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let forktty_config_dir = config_home.path().join("forktty");
        fs::create_dir_all(&forktty_config_dir).unwrap();
        fs::write(
            forktty_config_dir.join("config.toml"),
            format!(
                r#"
            [general]
            shell = "/bin/sh"

            [team]
            provider_commands = {{ codex = "{}" }}
            "#,
                custom_codex.display()
            ),
        )
        .unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (state, _backend) = test_state();

        let result = dispatch(&state, "system.capabilities", json!({}))
            .await
            .unwrap();

        let custom_codex = custom_codex.to_string_lossy().into_owned();
        let provider = &result["provider_capabilities"]["codex"];
        assert_eq!(provider["launchable"], true);
        assert_eq!(provider["available_on_path"], false);
        assert_eq!(provider["program"].as_str().unwrap(), custom_codex);
        assert_eq!(provider["default_program"], "codex");
        assert_eq!(
            provider["configured_command"].as_str().unwrap(),
            custom_codex
        );
        assert_eq!(provider["executable"].as_str().unwrap(), custom_codex);
        assert_eq!(
            result["team_provider_policy"]["provider_commands"]["codex"]
                .as_str()
                .unwrap(),
            custom_codex
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_launch_auto_selects_first_available_configured_provider() {
        let dir = tempfile::tempdir().unwrap();
        let pi = write_fake_program(dir.path(), "pi");
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let forktty_config_dir = config_home.path().join("forktty");
        fs::create_dir_all(&forktty_config_dir).unwrap();
        fs::write(
            forktty_config_dir.join("config.toml"),
            r#"
            [general]
            shell = "/bin/sh"

            [team]
            default_agent = "auto"
            provider_order = ["claude", "pi", "codex"]
            auto_fallback = true
            "#,
        )
        .unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (mut state, _backend) = test_state();
        let team_store = tempfile::tempdir().unwrap();
        state.team_store_path = Some(team_store.path().join("team-v1.json"));
        let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": main_workspace_id,
                "name": "Launch",
            }),
        )
        .await
        .unwrap();

        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "role": "reviewer",
            }),
        )
        .await
        .unwrap();

        assert_eq!(launched["worker"]["agent"], "pi");
        assert_eq!(launched["argv"][0], "pi");
        assert_eq!(launched["selection"]["requested_agent"], "auto");
        assert_eq!(launched["selection"]["selected_agent"], "pi");
        assert_eq!(
            launched["selection"]["executable"].as_str().unwrap(),
            pi.to_string_lossy()
        );
        let first_considered = &launched["selection"]["considered"][0];
        assert_eq!(first_considered["agent"], "claude");
        assert_eq!(first_considered["program"], "claude");
        assert_eq!(first_considered["available"], false);
        assert_eq!(first_considered["reason"], "program_not_found");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_launch_auto_uses_configured_command_outside_path() {
        let path_dir = tempfile::tempdir().unwrap();
        let custom_dir = tempfile::tempdir().unwrap();
        let custom_codex = write_fake_program(custom_dir.path(), "codex-custom");
        let _path = EnvGuard::set("PATH", path_dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let forktty_config_dir = config_home.path().join("forktty");
        fs::create_dir_all(&forktty_config_dir).unwrap();
        fs::write(
            forktty_config_dir.join("config.toml"),
            format!(
                r#"
            [general]
            shell = "/bin/sh"

            [team]
            default_agent = "auto"
            provider_order = ["codex"]
            provider_commands = {{ codex = "{}" }}
            "#,
                custom_codex.display()
            ),
        )
        .unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let team_store = tempfile::tempdir().unwrap();
        state.team_store_path = Some(team_store.path().join("team-v1.json"));
        let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": main_workspace_id,
                "name": "Launch",
            }),
        )
        .await
        .unwrap();

        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
            }),
        )
        .await
        .unwrap();

        let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
        let custom_codex = custom_codex.to_string_lossy().into_owned();
        assert_eq!(launched["worker"]["agent"], "codex");
        assert_eq!(launched["argv"][0].as_str().unwrap(), custom_codex);
        assert_eq!(
            backend.spawn_shell(launched_surface_id).unwrap(),
            custom_codex
        );
        assert_eq!(
            launched["selection"]["program"].as_str().unwrap(),
            custom_codex
        );
        assert_eq!(launched["selection"]["default_program"], "codex");
        assert_eq!(
            launched["selection"]["configured_command"]
                .as_str()
                .unwrap(),
            custom_codex
        );
        assert_eq!(launched["selection"]["considered"][0]["available"], true);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_launch_auto_rejects_when_no_provider_is_available() {
        let dir = tempfile::tempdir().unwrap();
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let config_home = tempfile::tempdir().unwrap();
        let _config_home = EnvGuard::set("XDG_CONFIG_HOME", config_home.path().to_str().unwrap());
        let (mut state, _backend) = test_state();
        let team_store = tempfile::tempdir().unwrap();
        state.team_store_path = Some(team_store.path().join("team-v1.json"));
        let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": main_workspace_id,
                "name": "Launch",
            }),
        )
        .await
        .unwrap();

        let err = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "precondition_failed");
        assert!(err
            .to_string()
            .contains("No team worker provider is available"));
    }

    #[test]
    fn team_worker_launch_accepts_documented_provider_aliases() {
        for (alias, expected_program) in [
            ("codex", "codex"),
            ("claude", "claude"),
            ("claude_code", "claude"),
            ("claude-code", "claude"),
            ("opencode", "opencode"),
            ("open_code", "opencode"),
            ("open-code", "opencode"),
            ("antigravity", "agy"),
            ("agy", "agy"),
            ("pi", "pi"),
        ] {
            let (program, args) = team_worker_launch_command(alias, None, Vec::new()).unwrap();
            assert_eq!(program, expected_program, "{alias}");
            if expected_program == "claude" {
                assert_eq!(
                    args,
                    vec!["--permission-mode".to_string(), "auto".to_string()],
                    "{alias}"
                );
            } else {
                assert!(args.is_empty(), "{alias}");
            }
        }
    }

    #[test]
    fn team_worker_launch_uses_dont_ask_for_claude_review_roles() {
        let (program, args) =
            team_worker_launch_command("claude-code", Some("code-reviewer"), Vec::new()).unwrap();

        assert_eq!(program, "claude");
        assert_eq!(
            &args[0..2],
            ["--permission-mode".to_string(), "dontAsk".to_string()]
        );
        let tools = args
            .windows(2)
            .find(|pair| pair[0] == "--allowedTools")
            .map(|pair| pair[1].as_str())
            .expect("review profile should set Claude allowed tools");
        assert_eq!(tools, CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS);
        assert!(tools.contains("Read"));
        assert!(tools.contains("Grep"));
        assert!(tools.contains("Glob"));
        assert!(!tools.contains("Bash("));
        assert!(!tools.contains("cargo test"));
    }

    #[test]
    fn team_worker_launch_uses_read_only_tools_for_pi_review_roles() {
        let (program, args) =
            team_worker_launch_command("pi", Some("code-reviewer"), Vec::new()).unwrap();

        assert_eq!(program, "pi");
        assert_eq!(args, ["--tools", "read,grep,find,ls"]);
    }

    #[test]
    fn team_worker_launch_does_not_add_pi_tools_for_non_review_roles() {
        let (program, args) =
            team_worker_launch_command("pi", Some("builder"), Vec::new()).unwrap();

        assert_eq!(program, "pi");
        assert!(args.is_empty());
    }

    #[test]
    fn team_worker_launch_preserves_explicit_pi_tool_args() {
        for args in [
            vec!["--tools".to_string(), "read".to_string()],
            vec!["--tools=read".to_string()],
            vec!["-t".to_string(), "read".to_string()],
            vec!["--exclude-tools=bash".to_string()],
            vec!["-xt".to_string(), "bash".to_string()],
            vec!["--no-tools".to_string()],
            vec!["-nt".to_string()],
            vec!["--no-builtin-tools".to_string()],
            vec!["-nbt".to_string()],
        ] {
            let (_, actual) =
                team_worker_launch_command("pi", Some("reviewer"), args.clone()).unwrap();
            assert_eq!(actual, args);
        }
    }

    #[test]
    fn team_worker_launch_preserves_explicit_claude_permission_args() {
        let explicit_args = vec![
            "--permission-mode".to_string(),
            "default".to_string(),
            "--model".to_string(),
            "opus".to_string(),
        ];

        let (program, args) =
            team_worker_launch_command("claude", Some("reviewer"), explicit_args.clone()).unwrap();

        assert_eq!(program, "claude");
        assert_eq!(args, explicit_args);

        let equals_args = vec!["--permission-mode=default".to_string()];
        let (_, args) =
            team_worker_launch_command("claude", Some("reviewer"), equals_args.clone()).unwrap();
        assert_eq!(args, equals_args);
    }

    #[test]
    fn team_worker_launch_preserves_each_claude_permission_override() {
        for args in [
            vec!["--allowedTools".to_string(), "Read".to_string()],
            vec!["--allowed-tools=Read".to_string()],
            vec!["--disallowedTools".to_string(), "Bash".to_string()],
            vec!["--disallowed-tools=Bash".to_string()],
            vec!["--settings".to_string(), "/tmp/settings.json".to_string()],
            vec!["--settings=/tmp/settings.json".to_string()],
            vec!["--dangerously-skip-permissions".to_string()],
            vec!["--allow-dangerously-skip-permissions".to_string()],
        ] {
            let (_, actual) =
                team_worker_launch_command("claude", Some("reviewer"), args.clone()).unwrap();
            assert_eq!(actual, args);
        }
    }

    #[test]
    fn team_worker_launch_prepends_review_defaults_before_extra_args() {
        let extra = vec!["--model".to_string(), "opus".to_string()];
        let (_, args) =
            team_worker_launch_command("claude", Some("code-reviewer"), extra.clone()).unwrap();

        assert_eq!(
            &args[0..2],
            ["--permission-mode".to_string(), "dontAsk".to_string()]
        );
        assert_eq!(&args[args.len() - 2..], extra.as_slice());
    }

    #[test]
    fn team_worker_launch_does_not_treat_preview_as_review_role() {
        let (_, args) = team_worker_launch_command("claude", Some("preview"), Vec::new()).unwrap();

        assert_eq!(
            args,
            vec!["--permission-mode".to_string(), "auto".to_string()]
        );
    }

    #[test]
    fn optional_limit_param_clamps_and_preserves_none() {
        assert_eq!(optional_limit_param(&json!({}), "limit").unwrap(), None);
        assert_eq!(
            optional_limit_param(&json!({"limit": null}), "limit").unwrap(),
            None
        );
        assert_eq!(
            optional_limit_param(&json!({"limit": 5}), "limit").unwrap(),
            Some(5)
        );
        assert_eq!(
            optional_limit_param(&json!({"limit": u64::MAX}), "limit").unwrap(),
            Some(10_000)
        );
        assert!(matches!(
            optional_limit_param(&json!({"limit": "5"}), "limit"),
            Err(DispatchError::InvalidParam(_))
        ));
    }

    /// RAII guard that sets an environment variable for the duration of a test
    /// and restores the previous value (or removes it) on drop, even on panic.
    ///
    /// Use together with `#[serial_test::serial]` so that tests touching
    /// process-global env vars do not race with each other.
    struct EnvGuard {
        _guard: EnvTestLockGuard,
        key: &'static str,
        prev: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, val: &str) -> Self {
            let guard = lock_env_for_test();
            let prev = std::env::var(key).ok();
            // SAFETY: test-only; access serialized by ENV_TEST_LOCK.
            unsafe { std::env::set_var(key, val) };
            Self {
                _guard: guard,
                key,
                prev,
            }
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

    fn write_fake_program(dir: &Path, name: &str) -> PathBuf {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt as _;

        let program = dir.join(name);
        {
            let mut file = fs::File::create(&program).unwrap();
            writeln!(file, "#!/bin/sh").unwrap();
        }
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    fn write_fake_codex(dir: &Path) -> PathBuf {
        write_fake_program(dir, "codex")
    }

    fn test_state() -> (SocketAppState, Arc<HeadlessTerminalBackend>) {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
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
            persisted_scrollback: None,
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
                persisted_scrollback: None,
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
            persisted_scrollback: None,
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
    fn spawn_request_skips_suspended_agent_terminal_surface() {
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
                lifecycle: AgentSessionLifecycle::Suspended,
                last_activity_ms: 12_345,
            }),
            persisted_scrollback: None,
        };

        assert!(spawn_request_for_surface(
            SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
            &surface,
        )
        .is_none());
    }

    #[test]
    fn spawn_request_reapplies_persisted_bypass_permission_mode() {
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
            persisted_scrollback: None,
        };

        let request = spawn_request_for_surface(
            SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
            &surface,
        )
        .expect("agent terminal surface should spawn");

        assert_eq!(request.shell, "claude");
        assert_eq!(
            request.args,
            vec![
                "--dangerously-skip-permissions".to_string(),
                "--resume".to_string(),
                "claude-session-1".to_string()
            ]
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
            persisted_scrollback: None,
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
                        pid: None,
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

        fn surface_ready(&self, _surface_id: &str) -> Result<bool, TerminalError> {
            self.surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)
                .map(|_| false)
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
    struct ShowReadyBackend {
        inner: HeadlessTerminalBackend,
        not_ready_sends_after_show: AtomicUsize,
        shown_surfaces: Mutex<Vec<String>>,
    }

    impl ShowReadyBackend {
        fn with_not_ready_sends_after_show(count: usize) -> Self {
            Self {
                inner: HeadlessTerminalBackend::new(),
                not_ready_sends_after_show: AtomicUsize::new(count),
                shown_surfaces: Mutex::new(Vec::new()),
            }
        }

        fn sent_text(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
            self.inner.sent_text(surface_id)
        }

        fn shown_surfaces(&self) -> Vec<String> {
            self.shown_surfaces.lock().unwrap().clone()
        }
    }

    impl TerminalBackend for ShowReadyBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            if !self
                .shown_surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .iter()
                .any(|shown| shown == surface_id)
            {
                return Err(TerminalError::NotReady(surface_id.to_string()));
            }
            if self.not_ready_sends_after_show.load(Ordering::SeqCst) > 0 {
                self.not_ready_sends_after_show
                    .fetch_sub(1, Ordering::SeqCst);
                return Err(TerminalError::NotReady(surface_id.to_string()));
            }
            self.inner.send_text(surface_id, text)
        }

        fn show_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.shown_surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .push(surface_id.to_string());
            Ok(())
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    #[derive(Debug, Default)]
    struct FailingEnterBackend {
        inner: HeadlessTerminalBackend,
    }

    impl FailingEnterBackend {
        fn sent_text(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
            self.inner.sent_text(surface_id)
        }
    }

    impl TerminalBackend for FailingEnterBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            self.inner.send_text(surface_id, text)
        }

        fn send_enter(&self, _surface_id: &str) -> Result<(), TerminalError> {
            Err(TerminalError::Backend("enter failed".to_string()))
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    #[derive(Debug, Default)]
    struct RecordingEnterBackend {
        inner: HeadlessTerminalBackend,
        entered_surfaces: Mutex<Vec<String>>,
    }

    impl RecordingEnterBackend {
        fn sent_text(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
            self.inner.sent_text(surface_id)
        }

        fn entered_surfaces(&self) -> Vec<String> {
            self.entered_surfaces.lock().unwrap().clone()
        }
    }

    #[derive(Debug)]
    struct BlockingFirstSendBackend {
        inner: HeadlessTerminalBackend,
        first_send_started: Mutex<Option<mpsc::Sender<()>>>,
        release_first_send: Mutex<mpsc::Receiver<()>>,
    }

    impl BlockingFirstSendBackend {
        fn new(
            first_send_started: mpsc::Sender<()>,
            release_first_send: mpsc::Receiver<()>,
        ) -> Self {
            Self {
                inner: HeadlessTerminalBackend::new(),
                first_send_started: Mutex::new(Some(first_send_started)),
                release_first_send: Mutex::new(release_first_send),
            }
        }

        fn sent_text(&self, surface_id: &str) -> Result<Vec<String>, TerminalError> {
            self.inner.sent_text(surface_id)
        }
    }

    impl TerminalBackend for BlockingFirstSendBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            if let Some(started) = self
                .first_send_started
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .take()
            {
                let _ = started.send(());
                self.release_first_send
                    .lock()
                    .map_err(|_| TerminalError::LockPoisoned)?
                    .recv()
                    .map_err(|err| TerminalError::Backend(err.to_string()))?;
            }
            self.inner.send_text(surface_id, text)
        }

        fn show_surface(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.inner.show_surface(surface_id)
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    impl TerminalBackend for RecordingEnterBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            self.inner.send_text(surface_id, text)
        }

        fn send_enter(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.entered_surfaces
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .push(surface_id.to_string());
            Ok(())
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
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
                        pid: None,
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

    #[derive(Debug, Default)]
    struct FailsSecondCloseBackend {
        inner: HeadlessTerminalBackend,
        close_count: AtomicUsize,
    }

    #[derive(Debug)]
    struct BlockingFirstCloseBackend {
        inner: HeadlessTerminalBackend,
        close_started: AtomicBool,
        first_close_started: Mutex<Option<mpsc::Sender<()>>>,
        release_first_close: Mutex<mpsc::Receiver<()>>,
        spawn_after_close_started: Mutex<Option<mpsc::Sender<()>>>,
    }

    impl BlockingFirstCloseBackend {
        fn new(
            first_close_started: mpsc::Sender<()>,
            release_first_close: mpsc::Receiver<()>,
            spawn_after_close_started: mpsc::Sender<()>,
        ) -> Self {
            Self {
                inner: HeadlessTerminalBackend::new(),
                close_started: AtomicBool::new(false),
                first_close_started: Mutex::new(Some(first_close_started)),
                release_first_close: Mutex::new(release_first_close),
                spawn_after_close_started: Mutex::new(Some(spawn_after_close_started)),
            }
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    impl TerminalBackend for BlockingFirstCloseBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            if self.close_started.load(Ordering::SeqCst) {
                if let Some(started) = self
                    .spawn_after_close_started
                    .lock()
                    .map_err(|_| TerminalError::LockPoisoned)?
                    .take()
                {
                    let _ = started.send(());
                }
            }
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            self.inner.send_text(surface_id, text)
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            if let Some(started) = self
                .first_close_started
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .take()
            {
                self.close_started.store(true, Ordering::SeqCst);
                let _ = started.send(());
                self.release_first_close
                    .lock()
                    .map_err(|_| TerminalError::LockPoisoned)?
                    .recv()
                    .map_err(|err| TerminalError::Backend(err.to_string()))?;
            }
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    impl FailsSecondCloseBackend {
        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
    }

    impl TerminalBackend for FailsSecondCloseBackend {
        fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
            self.inner.spawn(request)
        }

        fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
            self.inner.send_text(surface_id, text)
        }

        fn read_text(
            &self,
            surface_id: &str,
            capture: TerminalTextCapture,
            max_bytes: usize,
        ) -> Result<TerminalTextSnapshot, TerminalError> {
            self.inner.read_text(surface_id, capture, max_bytes)
        }

        fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
            self.inner.resize(surface_id, cols, rows)
        }

        fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
            if self.close_count.fetch_add(1, Ordering::SeqCst) == 1 {
                return Err(TerminalError::Backend("second close failed".to_string()));
            }
            self.inner.close(surface_id)
        }

        fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
            self.inner.surfaces()
        }
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
                        pid: None,
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
                        pid: None,
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
    async fn dispatches_system_top_summary() {
        let (state, backend) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.mark_surface_unread(&surface_id, true));
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model
                .set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,));
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));
            model
                .set_status(
                    &workspace.id,
                    "agent:codex",
                    "Codex",
                    "Idle",
                    Some("green".into()),
                )
                .unwrap();
            (workspace.id, surface_id)
        };
        backend.resize(&surface_id, 120, 40).unwrap();

        let result = dispatch(&state, "system.top", json!({})).await.unwrap();

        assert_eq!(result["totals"]["workspaces"], 1);
        assert_eq!(result["totals"]["surfaces"], 1);
        assert_eq!(result["totals"]["unread_surfaces"], 1);
        assert_eq!(result["totals"]["agents"], 1);
        assert_eq!(result["workspaces"][0]["id"], workspace_id);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["id"], surface_id);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["focused"], true);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["unread"], true);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["kind"], "terminal");
        assert_eq!(result["workspaces"][0]["surfaces"][0]["shell"], "/bin/sh");
        assert_eq!(result["workspaces"][0]["surfaces"][0]["cols"], 120);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["rows"], 40);
        assert_eq!(result["workspaces"][0]["surfaces"][0]["pid"], Value::Null);
        assert_eq!(
            result["workspaces"][0]["surfaces"][0]["agent"]["session_id"],
            "codex-session-1"
        );
        assert_eq!(
            result["workspaces"][0]["surfaces"][0]["agent"]["lifecycle"],
            "idle"
        );
        assert_eq!(result["workspaces"][0]["status"][0]["key"], "agent:codex");
    }

    #[tokio::test]
    async fn dispatches_context_snapshot_with_bounded_terminal_tails() {
        let (state, backend) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model
                .set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Running,));
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));
            model
                .set_status(
                    &workspace.id,
                    "agent:codex",
                    "Codex",
                    "Running",
                    Some("blue".into()),
                )
                .unwrap();
            (workspace.id, surface_id)
        };
        backend
            .send_text(&surface_id, "one\ntwo\nthree\n")
            .expect("write terminal text");

        let result = dispatch(
            &state,
            "context.snapshot",
            json!({"tail_lines": 2, "tail_max_bytes": 1024}),
        )
        .await
        .unwrap();

        assert_eq!(result["workspace"]["id"], workspace_id);
        assert_eq!(result["workspace"]["focused_surface_id"], surface_id);
        assert_eq!(result["surfaces"][0]["id"], surface_id);
        assert_eq!(result["status"]["status"][0]["key"], "agent:codex");
        assert_eq!(result["agents"][0]["surface_id"], surface_id);
        assert_eq!(result["agents"][0]["lifecycle"], "running");
        assert_eq!(
            result["agents"][0]["lifecycle_evidence"]["status_key"],
            "agent:codex"
        );
        assert_eq!(
            result["agents"][0]["lifecycle_evidence"]["status_value"],
            "Running"
        );
        assert_eq!(
            result["agent_health"][0]["lifecycle_evidence"]["readiness_reason"],
            result["agent_health"][0]["reason"]
        );
        assert_eq!(
            result["agents"][0]["observed_at_ms"],
            result["status"]["agents"][0]["observed_at_ms"]
        );
        assert_eq!(
            result["agents"][0]["age_ms"],
            result["status"]["agents"][0]["age_ms"]
        );
        assert_eq!(result["terminal_tails"][0]["surface_id"], surface_id);
        assert_eq!(result["terminal_tails"][0]["text"], "two\nthree\n");
        assert_eq!(result["terminal_tails"][0]["untrusted"], true);
        assert_eq!(result["terminal_tails"][0]["truncated"], false);
        assert!(result["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("terminal_text_untrusted")));
    }

    #[tokio::test]
    async fn context_snapshot_exposes_effective_project_cwd_from_resume_cwd() {
        let (state, _) = test_state();
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_string_lossy().to_string();
        {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model.set_surface_agent_session_resume_cwd(
                &surface_id,
                project_dir.path().to_path_buf()
            ));
        }

        let snapshot = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
            .await
            .unwrap();

        assert_eq!(snapshot["workspace"]["working_dir"], "/tmp");
        assert_eq!(snapshot["workspace"]["effective_project_cwd"], project_cwd);
        assert_eq!(snapshot["surfaces"][0]["cwd"], "/tmp");
        assert_eq!(
            snapshot["surfaces"][0]["effective_project_cwd"],
            project_cwd
        );
        assert_eq!(snapshot["agents"][0]["effective_project_cwd"], project_cwd);
        assert_eq!(
            snapshot["agent_health"][0]["effective_project_cwd"],
            project_cwd
        );
    }

    #[tokio::test]
    async fn system_identify_reports_target_and_caller_context() {
        let (state, _) = test_state();
        let project_dir = tempfile::tempdir().unwrap();
        let project_cwd = project_dir.path().to_string_lossy().to_string();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model.set_surface_agent_session_resume_cwd(
                &surface_id,
                project_dir.path().to_path_buf()
            ));
            (workspace.id, surface_id)
        };

        let identify = dispatch(
            &state,
            "system.identify",
            json!({
                "caller_workspace_id": workspace_id,
                "caller_surface_id": surface_id,
            }),
        )
        .await
        .unwrap();

        assert_eq!(identify["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(identify["workspace"]["id"], workspace_id);
        assert_eq!(identify["workspace"]["effective_project_cwd"], project_cwd);
        assert_eq!(identify["surface"]["id"], surface_id);
        assert_eq!(identify["surface"]["effective_project_cwd"], project_cwd);
        assert_eq!(identify["agent"]["agent"], "codex");
        assert_eq!(identify["agent"]["session_id"], "codex-session-1");
        assert_eq!(identify["caller"]["workspace_id"], workspace_id);
        assert_eq!(identify["caller"]["surface_id"], surface_id);
        assert_eq!(identify["caller"]["workspace_known"], true);
        assert_eq!(identify["caller"]["surface_known"], true);
        assert_eq!(identify["caller"]["matches_workspace"], true);
        assert_eq!(identify["caller"]["matches_surface"], true);
    }

    #[tokio::test]
    async fn system_identify_defaults_to_known_caller_surface() {
        let (state, _) = test_state();
        let caller_surface_id = {
            let model = state.model.lock().unwrap();
            model.active_workspace().unwrap().focused_surface_id.clone()
        };
        let focused = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": caller_surface_id}),
        )
        .await
        .unwrap();
        let focused_surface_id = focused["id"].as_str().unwrap().to_string();
        assert_ne!(caller_surface_id, focused_surface_id);

        let identify = dispatch(
            &state,
            "system.identify",
            json!({"caller_surface_id": caller_surface_id}),
        )
        .await
        .unwrap();

        assert_eq!(identify["target_source"], "caller_surface");
        assert_eq!(identify["surface"]["id"], caller_surface_id);
        assert_eq!(identify["caller"]["surface_known"], true);
        assert_eq!(identify["caller"]["matches_surface"], true);
    }

    #[tokio::test]
    async fn system_identify_degrades_stale_caller_surface_to_active_focus() {
        let (state, _) = test_state();
        let (workspace_id, focused_surface_id) = {
            let model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            (workspace.id.clone(), workspace.focused_surface_id.clone())
        };

        let identify = dispatch(
            &state,
            "system.identify",
            json!({
                "caller_workspace_id": workspace_id,
                "caller_surface_id": "surface-missing"
            }),
        )
        .await
        .unwrap();

        assert_eq!(identify["target_source"], "active_workspace");
        assert_eq!(identify["workspace"]["id"], workspace_id);
        assert_eq!(identify["surface"]["id"], focused_surface_id);
        assert_eq!(identify["caller"]["workspace_known"], true);
        assert_eq!(identify["caller"]["surface_known"], false);
        assert_eq!(identify["caller"]["matches_workspace"], true);
        assert_eq!(identify["caller"]["matches_surface"], false);
    }

    #[tokio::test]
    async fn context_snapshot_limits_terminal_tail_surface_count() {
        let surface_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES;
        let (state, backend) = test_state();
        let mut surface_ids = Vec::new();
        let mut focused_surface_id = {
            let model = state.model.lock().unwrap();
            model.active_workspace().unwrap().focused_surface_id.clone()
        };
        surface_ids.push(focused_surface_id.clone());
        for _ in 0..surface_limit {
            let created = dispatch(
                &state,
                "pane.new_tab",
                json!({"surface_id": focused_surface_id}),
            )
            .await
            .unwrap();
            focused_surface_id = created["id"].as_str().unwrap().to_string();
            surface_ids.push(focused_surface_id.clone());
        }
        for (index, surface_id) in surface_ids.iter().enumerate() {
            backend
                .send_text(surface_id, &format!("surface-{index}\n"))
                .expect("write terminal text");
        }

        let result = dispatch(
            &state,
            "context.snapshot",
            json!({"tail_lines": 1, "tail_max_bytes": 1024}),
        )
        .await
        .unwrap();

        assert_eq!(
            result["terminal_tails"].as_array().unwrap().len(),
            surface_limit
        );
        let errors = result["terminal_tail_errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["skipped_surfaces"], 1);
        assert!(errors[0]["error"]
            .as_str()
            .unwrap()
            .contains("surface limit"));
    }

    #[tokio::test]
    async fn context_snapshot_limits_aggregate_terminal_tail_bytes() {
        let aggregate_byte_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
        let (state, backend) = test_state();
        let mut surface_ids = Vec::new();
        let mut focused_surface_id = {
            let model = state.model.lock().unwrap();
            model.active_workspace().unwrap().focused_surface_id.clone()
        };
        surface_ids.push(focused_surface_id.clone());
        for _ in 0..2 {
            let created = dispatch(
                &state,
                "pane.new_tab",
                json!({"surface_id": focused_surface_id}),
            )
            .await
            .unwrap();
            focused_surface_id = created["id"].as_str().unwrap().to_string();
            surface_ids.push(focused_surface_id.clone());
        }
        let large_tail = format!("{}\n", "x".repeat(MAX_TERMINAL_TEXT_BYTES));
        for surface_id in &surface_ids {
            backend
                .send_text(surface_id, &large_tail)
                .expect("write terminal text");
        }

        let result = dispatch(
            &state,
            "context.snapshot",
            json!({"tail_lines": 1, "tail_max_bytes": MAX_TERMINAL_TEXT_BYTES}),
        )
        .await
        .unwrap();

        let total_tail_bytes: usize = result["terminal_tails"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tail| tail["text"].as_str().unwrap().len())
            .sum();
        assert!(total_tail_bytes <= aggregate_byte_limit);
        assert!(result["terminal_tails"].as_array().unwrap().len() < surface_ids.len());
        let errors = result["terminal_tail_errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["skipped_surfaces"], 1);
        assert!(errors[0]["error"].as_str().unwrap().contains("byte limit"));
    }

    #[tokio::test]
    async fn context_snapshot_surface_id_selects_inactive_workspace() {
        let (state, _) = test_state();
        let review_dir = tempfile::tempdir().unwrap();
        let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let first_workspace_id = first[0]["id"].as_str().unwrap().to_string();
        let second = dispatch(
            &state,
            "workspace.create",
            json!({"name": "review", "workingDir": review_dir.path()}),
        )
        .await
        .unwrap();
        let second_workspace_id = second["id"].as_str().unwrap().to_string();
        let second_surface_id = second["focused_surface_id"].as_str().unwrap().to_string();
        dispatch(
            &state,
            "workspace.select",
            json!({"workspace_id": first_workspace_id}),
        )
        .await
        .unwrap();

        let result = dispatch(
            &state,
            "context.snapshot",
            json!({"surface_id": second_surface_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(result["workspace"]["id"], second_workspace_id);
        assert_eq!(result["workspace"]["focused_surface_id"], second_surface_id);
        assert_eq!(result["surfaces"][0]["id"], second_surface_id);
    }

    #[tokio::test]
    async fn context_snapshot_review_gap_rejects_invalid_tail_bounds() {
        let (state, _) = test_state();
        for (params, expected_message) in [
            (
                json!({"tail_lines": MAX_CAPTURE_TAIL_LINES + 1}),
                "Invalid parameter tail_lines",
            ),
            (
                json!({"tail_max_bytes": 0}),
                "Invalid parameter tail_max_bytes",
            ),
            (
                json!({"tail_max_bytes": MAX_TERMINAL_TEXT_BYTES + 1}),
                "Invalid parameter tail_max_bytes",
            ),
        ] {
            let error = dispatch(&state, "context.snapshot", params)
                .await
                .unwrap_err();
            assert_eq!(error.code(), "invalid_param");
            assert!(
                error.to_string().contains(expected_message),
                "expected {expected_message:?}, got {error}"
            );
        }
    }

    #[tokio::test]
    async fn context_snapshot_review_gap_tail_lines_zero_skips_terminal_tails() {
        let (state, backend) = test_state();
        let surface_id = {
            let model = state.model.lock().unwrap();
            model.active_workspace().unwrap().focused_surface_id.clone()
        };
        backend
            .send_text(&surface_id, "one\ntwo\nthree\n")
            .expect("write terminal text");

        let result = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
            .await
            .unwrap();

        assert!(result["terminal_tails"].as_array().unwrap().is_empty());
        assert!(result["terminal_tail_errors"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(!result["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("terminal_text_untrusted")));
    }

    #[test]
    fn context_snapshot_review_gap_risk_flags_pin_field_shapes() {
        let approval = FeedEntry {
            id: "approval-1".to_string(),
            entry_type: FeedEntryType::Approval,
            kind: None,
            read: false,
            key: None,
            value: None,
            total: None,
            title: "Approval".to_string(),
            body: "Run command?".to_string(),
            workspace_id: Some("workspace-1".to_string()),
            surface_id: Some("surface-1".to_string()),
            created_at_ms: 123,
            approval_state: Some(FeedApprovalState::Pending),
        };
        let approval_json = json!(approval);
        assert_eq!(approval_json["type"], "approval");
        assert!(approval_json.get("entry_type").is_none());

        let truncated_tail = json!(forktty_terminal::TerminalTextSnapshot::from_text(
            "surface-1",
            "alpha\nbeta\n",
            80,
            24,
            TerminalTextCapture::Tail { lines: 2 },
            4,
        ));
        assert_eq!(truncated_tail["truncated"], true);

        let status = json!({
            "status": [{
                "key": "agent:codex:permission",
                "value": "bypassPermissions",
            }],
        });
        let agent_health = [json!({
            "surface_id": "surface-1",
            "permission_mode": "bypassPermissions",
        })];
        let feed = [approval_json];
        let remotes = [json!({"surface_id": "surface-ssh", "kind": "ssh"})];
        let terminal_tails = [truncated_tail];
        let terminal_tail_errors = [json!({"surface_id": "surface-missing", "error": "not ready"})];
        let workflow_summaries = json!([{"consistency_warnings": ["running_with_completed_plan"]}]);
        let loop_summaries = json!([
            {
                "gates_failed": 1,
                "stage": "needs_human",
            },
            {
                "stage": "blocked",
                "stop_reason": "budget_exhausted",
                "stale_binding": true,
            }
        ]);
        let team_summaries = json!([{"consistency_warnings": ["done_with_active_workers"]}]);
        let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
            status: &status,
            agent_health: &agent_health,
            feed: &feed,
            remotes: &remotes,
            terminal_tails: &terminal_tails,
            terminal_tail_errors: &terminal_tail_errors,
            workflow_summaries: &workflow_summaries,
            loop_summaries: &loop_summaries,
            team_summaries: &team_summaries,
        });

        for expected in [
            "terminal_text_untrusted",
            "terminal_tail_truncated",
            "terminal_tail_unavailable",
            "team_consistency_warning",
            "workflow_consistency_warning",
            "loop_gate_failed",
            "loop_needs_human",
            "loop_blocked",
            "loop_stale_binding",
            "loop_budget_exhausted",
            "remote_surface",
            "pending_approval",
            "permission_bypass",
        ] {
            assert!(
                flags.contains(&expected),
                "expected risk flag {expected:?} in {flags:?}"
            );
        }
    }

    #[test]
    fn context_snapshot_review_gap_permission_bypass_reads_agent_health() {
        let status = json!({"status": []});
        let agent_health = [json!({"permission_mode": "bypassPermissions"})];
        let workflow_summaries = json!([]);
        let loop_summaries = json!([]);
        let team_summaries = json!([]);
        let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
            status: &status,
            agent_health: &agent_health,
            feed: &[],
            remotes: &[],
            terminal_tails: &[],
            terminal_tail_errors: &[],
            workflow_summaries: &workflow_summaries,
            loop_summaries: &loop_summaries,
            team_summaries: &team_summaries,
        });

        assert_eq!(flags, vec!["permission_bypass"]);
    }

    #[test]
    fn context_snapshot_risk_flags_only_pending_approvals() {
        let status = json!({"status": []});
        let agent_health = [];
        let remotes = [];
        let terminal_tails = [];
        let terminal_tail_errors = [];
        let workflow_summaries = json!([]);
        let loop_summaries = json!([]);
        let team_summaries = json!([]);
        let non_pending_feed = [
            json!({"type": "approval", "approval_state": "approved"}),
            json!({"type": "approval", "approval_state": "denied"}),
            json!({"type": "approval", "approval_state": "dismissed"}),
            json!({"type": "approval", "approval_state": "stale"}),
        ];

        let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
            status: &status,
            agent_health: &agent_health,
            feed: &non_pending_feed,
            remotes: &remotes,
            terminal_tails: &terminal_tails,
            terminal_tail_errors: &terminal_tail_errors,
            workflow_summaries: &workflow_summaries,
            loop_summaries: &loop_summaries,
            team_summaries: &team_summaries,
        });
        assert!(!flags.contains(&"pending_approval"));

        let pending_feed = [json!({"type": "approval", "approval_state": "pending"})];
        let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
            status: &status,
            agent_health: &agent_health,
            feed: &pending_feed,
            remotes: &remotes,
            terminal_tails: &terminal_tails,
            terminal_tail_errors: &terminal_tail_errors,
            workflow_summaries: &workflow_summaries,
            loop_summaries: &loop_summaries,
            team_summaries: &team_summaries,
        });
        assert!(flags.contains(&"pending_approval"));
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
    async fn notification_clear_marks_persisted_approvals_dismissed() {
        let dir = tempfile::tempdir().unwrap();
        let feed_path = dir.path().join("feed.json");
        let (state, _) = test_state();
        let state = state.with_feed_store_path(&feed_path).unwrap();
        let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

        dispatch(
            &state,
            "notification.create",
            json!({
                "workspace_id": workspace_id,
                "kind": "prompt",
                "title": "Permission",
                "body": "Run command?"
            }),
        )
        .await
        .unwrap();

        dispatch(&state, "notification.clear", json!({}))
            .await
            .unwrap();
        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 10}),
        )
        .await
        .unwrap();

        assert_eq!(feed[0]["type"], "approval");
        assert_eq!(feed[0]["approval_state"], "dismissed");
    }

    #[tokio::test]
    async fn context_snapshot_marks_missing_surface_approvals_stale() {
        let dir = tempfile::tempdir().unwrap();
        let feed_path = dir.path().join("feed.json");
        let (state, _) = test_state();
        let state = state.with_feed_store_path(&feed_path).unwrap();
        let (workspace_id, stale_surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let split = model
                .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
                .unwrap();
            (workspace.id, split.id)
        };
        {
            let prev = current_snapshot(&state.model);
            let mut model = state.model.lock().unwrap();
            model.create_notification(
                "Permission",
                "Run stale command?",
                NotificationKind::Prompt,
                Some(workspace_id.clone()),
                Some(stale_surface_id.clone()),
            );
            drop(model);
            let next = current_snapshot(&state.model);
            feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
            let mut model = state.model.lock().unwrap();
            model.close_surface(&stale_surface_id).unwrap();
        }

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();
        let feed = snapshot["feed"].as_array().unwrap();

        assert_eq!(feed[0]["surface_id"], stale_surface_id);
        assert_eq!(feed[0]["approval_state"], "stale");
        assert!(!snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("pending_approval")));
    }

    #[tokio::test]
    async fn dispatches_minimal_feed_list() {
        let (state, _) = test_state();
        let (workspace_id, surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            model
                .set_status(&workspace.id, "agent:codex", "Codex", "Running", None)
                .unwrap();
            model
                .set_progress(&workspace.id, "build", "Build", 2.0, Some(4.0))
                .unwrap();
            let note = model.create_notification(
                "Permission",
                "Run command?",
                NotificationKind::Prompt,
                Some(workspace.id.clone()),
                Some(surface_id.clone()),
            );
            assert!(note.created_at_ms > 0);
            for title in ["One", "Two", "Three"] {
                model.create_notification(
                    title,
                    "Plain notification",
                    NotificationKind::Info,
                    Some(workspace.id.clone()),
                    None,
                );
            }
            (workspace.id, surface_id)
        };

        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 3}),
        )
        .await
        .unwrap();
        let items = feed.as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["type"], "approval");
        assert_eq!(items[0]["title"], "Permission");
        assert_eq!(items[0]["body"], "Run command?");
        assert_eq!(items[0]["surface_id"], surface_id);
        assert!(!items.iter().any(|item| item["type"] == "notification"));
        assert!(items
            .iter()
            .any(|item| item["type"] == "status" && item["title"] == "Codex"));
        assert!(items
            .iter()
            .any(|item| item["type"] == "progress" && item["title"] == "Build"));
    }

    #[tokio::test]
    async fn context_snapshot_compacts_feed_trace_by_default_and_expands_on_request() {
        let (state, _) = test_state();
        let workspace_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            model
                .set_status(
                    &workspace.id,
                    "tool:mcp__forktty__context_snapshot",
                    "Tool",
                    "Running mcp__forktty__context_snapshot",
                    None,
                )
                .unwrap();
            model
                .set_progress(&workspace.id, "tool:review", "Review", 1.0, Some(2.0))
                .unwrap();
            model.create_notification(
                "Permission",
                "Run command?",
                NotificationKind::Prompt,
                Some(workspace.id.clone()),
                Some(surface_id),
            );
            workspace.id
        };

        let compact = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();
        let compact_feed = compact["feed"].as_array().unwrap();
        assert!(compact_feed.iter().any(|item| item["type"] == "approval"));
        assert!(!compact_feed
            .iter()
            .any(|item| matches!(item["type"].as_str(), Some("status" | "progress"))));

        let expanded = dispatch(
            &state,
            "context.snapshot",
            json!({
                "workspace_id": workspace_id,
                "tail_lines": 0,
                "include_feed_trace": true
            }),
        )
        .await
        .unwrap();
        let expanded_feed = expanded["feed"].as_array().unwrap();
        assert!(expanded_feed.iter().any(|item| item["type"] == "status"));
        assert!(expanded_feed.iter().any(|item| item["type"] == "progress"));
    }

    #[tokio::test]
    async fn feed_list_live_fallback_limits_status_and_progress_to_newest_entries() {
        let (state, _) = test_state();
        let workspace_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            for i in 0..5 {
                model
                    .set_status(
                        &workspace.id,
                        format!("status-{i}"),
                        "Status",
                        format!("value-{i}"),
                        None,
                    )
                    .unwrap();
            }
            workspace.id
        };

        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 2}),
        )
        .await
        .unwrap();
        let status_keys = feed
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "status")
            .map(|item| item["key"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(status_keys, vec!["status-3", "status-4"]);

        {
            let mut model = state.model.lock().unwrap();
            assert!(model.clear_status(&workspace_id, None));
            for i in 0..5 {
                model
                    .set_progress(
                        &workspace_id,
                        format!("progress-{i}"),
                        "Progress",
                        i as f64,
                        None,
                    )
                    .unwrap();
            }
        }

        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 2}),
        )
        .await
        .unwrap();
        let progress_keys = feed
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "progress")
            .map(|item| item["key"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(progress_keys, vec!["progress-3", "progress-4"]);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn dispatches_team_orchestration_runtime_methods() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
        let leader_resume_cwd = tempfile::tempdir().unwrap();
        let leader_surface_cwd = {
            let model = state.model.lock().unwrap();
            model.surface(surface_id).unwrap().cwd.clone()
        };
        {
            let mut model = state.model.lock().unwrap();
            assert!(model.set_surface_agent_session(
                surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model.set_surface_agent_session_resume_cwd(
                surface_id,
                leader_resume_cwd.path().to_path_buf(),
            ));
        }

        let team = dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id,
                "name": "Launch",
                "goal": "ship runtime"
            }),
        )
        .await
        .unwrap();
        assert_eq!(team["workspace_id"], workspace_id);
        assert_eq!(team["leader_surface_id"], surface_id);

        let worker = dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id,
                "role": "implementer",
                "status": "idle"
            }),
        )
        .await
        .unwrap();
        assert_eq!(worker["surface_id"], surface_id);

        let task = dispatch(
            &state,
            "team.task.upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "title": "Build team runtime",
                "assigned_worker_id": "worker-1"
            }),
        )
        .await
        .unwrap();
        assert_eq!(task["assigned_worker_id"], "worker-1");

        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-2",
                "agent": "codex",
                "role": "reviewer",
                "assigned_task_id": "task-1",
                "args": ["--model", "test"]
            }),
        )
        .await
        .unwrap();
        let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
        assert_eq!(launched["worker"]["surface_id"], launched_surface_id);
        assert_eq!(backend.spawn_shell(launched_surface_id).unwrap(), "codex");
        let launched_runtime_surface = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == launched_surface_id)
            .unwrap();
        assert_eq!(launched_runtime_surface.cwd, leader_surface_cwd);
        assert_eq!(
            backend.spawn_args(launched_surface_id).unwrap(),
            vec!["--model".to_string(), "test".to_string()]
        );

        let heartbeat = dispatch(
            &state,
            "team.worker.heartbeat",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "status": "running",
                "assigned_task_id": "task-1"
            }),
        )
        .await
        .unwrap();
        assert_eq!(heartbeat["status"], "running");
        assert!(heartbeat["last_heartbeat_ms"].as_u64().unwrap() > 0);

        let message = dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "task_id": "task-1",
                "body": "continue\n"
            }),
        )
        .await
        .unwrap();
        assert_eq!(message["delivered"], false);

        let inbox = dispatch(
            &state,
            "team.inbox",
            json!({"team_id": "team-1", "worker_id": "worker-1"}),
        )
        .await
        .unwrap();
        assert_eq!(inbox.as_array().unwrap().len(), 1);

        let ack = dispatch(
            &state,
            "team.message.ack",
            json!({"team_id": "team-1", "message_id": "msg-1", "worker_id": "worker-1"}),
        )
        .await
        .unwrap();
        assert_eq!(ack["delivered"], true);

        let dispatchable = dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-2",
                "from": "leader",
                "to_worker_id": "worker-2",
                "body": "review this\r"
            }),
        )
        .await
        .unwrap();
        assert_eq!(dispatchable["delivered"], false);
        let dispatched = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-2"}),
        )
        .await
        .unwrap();
        assert_eq!(dispatched["message"]["delivered"], true);
        assert_eq!(
            backend.sent_text(launched_surface_id).unwrap(),
            vec!["review this\r".to_string()]
        );

        let nudged = dispatch(
            &state,
            "team.worker.nudge",
            json!({"team_id": "team-1", "worker_id": "worker-2", "text": "ping\r"}),
        )
        .await
        .unwrap();
        assert!(nudged["worker"]["last_nudge_ms"].as_u64().unwrap() > 0);
        let shutdown = dispatch(
            &state,
            "team.worker.shutdown",
            json!({"team_id": "team-1", "worker_id": "worker-2", "text": "stop"}),
        )
        .await
        .unwrap();
        assert_eq!(shutdown["worker"]["status"], "shutdown_requested");
        assert_eq!(shutdown["submitted"], true);
        assert_eq!(shutdown["closed_surface"], false);
        assert_eq!(
            backend.sent_text(launched_surface_id).unwrap(),
            vec![
                "review this\r".to_string(),
                "ping\r".to_string(),
                "stop\r".to_string()
            ]
        );

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();
        let worker_health = health["workers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worker| worker["worker_id"] == "worker-2")
            .unwrap();
        assert_eq!(worker_health["lifecycle"], "shutdown_requested");
        assert_eq!(worker_health["final_state"], "shutdown_requested");
        assert_eq!(worker_health["surface_alive"], true);

        let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(summary["workers_total"], 2);
        assert_eq!(summary["workers_active"], 1);
        assert_eq!(summary["tasks_open"], 1);
        assert_eq!(summary["messages_pending"], 0);

        let listed = dispatch(&state, "team.list", json!({"workspace_id": workspace_id}))
            .await
            .unwrap();
        assert_eq!(listed[0]["id"], "team-1");
        let fetched = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(fetched["workers"].as_array().unwrap().len(), 2);
        let events = dispatch(&state, "team.events", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert!(events.as_array().unwrap().len() >= 10);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_shutdown_can_close_launch_owned_worker_surface() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace_id = state.model.lock().unwrap().active_workspace().unwrap().id;

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "name": "Close Workers",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
            }),
        )
        .await
        .unwrap();
        let surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

        let shutdown = dispatch(
            &state,
            "team.worker.shutdown",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "text": "stop now",
                "close_surface": true,
            }),
        )
        .await
        .unwrap();

        assert_eq!(shutdown["sent"], true);
        assert_eq!(shutdown["submitted"], true);
        assert_eq!(shutdown["closed_surface"], true);
        assert!(backend.sent_text(&surface_id).is_err());
        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();
        assert_eq!(health["workers"][0]["final_state"], "closed");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_launch_does_not_promote_resume_cwd_to_worktree_boundary() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let open_repo = make_temp_repo();
        let unopened_repo = make_temp_repo();
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(
            &state,
            "workspace.create",
            json!({"name": "open", "workingDir": open_repo.path()}),
        )
        .await
        .unwrap();
        let workspace_id = workspace["id"].as_str().unwrap();
        let surface_id = workspace["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_session_id": "spoofed-session",
                "hook_session_cwd": unopened_repo.path(),
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id,
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();

        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
            }),
        )
        .await
        .unwrap();
        let worker_surface_id = launched["surface"]["id"].as_str().unwrap();
        let spawned = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == worker_surface_id)
            .unwrap();
        assert_eq!(spawned.cwd, open_repo.path());

        let error = dispatch(
            &state,
            "worktree.list",
            json!({"cwd": unopened_repo.path()}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "precondition_failed");
    }

    #[tokio::test]
    async fn team_worker_shutdown_rejects_close_for_manually_attached_surface() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let surface_id = state
            .model
            .lock()
            .unwrap()
            .active_workspace()
            .unwrap()
            .focused_surface_id;

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "name": "Manual Worker",
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id,
                "status": "running",
            }),
        )
        .await
        .unwrap();

        let err = dispatch(
            &state,
            "team.worker.shutdown",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "text": "stop now",
                "close_surface": true,
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "precondition_failed");
        assert!(backend.sent_text(&surface_id).unwrap().is_empty());
        assert!(state.model.lock().unwrap().surface(&surface_id).is_some());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_shutdown_rejects_close_after_manual_surface_upsert() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let (workspace_id, manual_surface_id) = {
            let model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            (workspace.id.clone(), workspace.focused_surface_id.clone())
        };

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "name": "Manual Reattach",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
            }),
        )
        .await
        .unwrap();
        let launched_surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

        let worker = dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": manual_surface_id,
                "status": "running",
            }),
        )
        .await
        .unwrap();
        assert_eq!(worker["surface_id"], manual_surface_id);
        assert_eq!(worker["launched_surface_id"], Value::Null);

        let err = dispatch(
            &state,
            "team.worker.shutdown",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "text": "stop now",
                "close_surface": true,
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "precondition_failed");
        assert!(backend.sent_text(&manual_surface_id).unwrap().is_empty());
        assert!(backend.sent_text(&launched_surface_id).unwrap().is_empty());
        let model = state.model.lock().unwrap();
        assert!(model.surface(&manual_surface_id).is_some());
        assert!(model.surface(&launched_surface_id).is_some());
    }

    #[tokio::test]
    async fn team_worker_shutdown_rejects_close_for_persisted_launch_without_runtime_ownership() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let (workspace_id, surface_id) = {
            let model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            (workspace.id.clone(), workspace.focused_surface_id.clone())
        };

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "name": "Stale Runtime Ownership",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        forktty_core::update_teams_at_path(state.team_store_path.as_ref().unwrap(), |store| {
            store.launch_worker(
                forktty_core::TeamWorkerLaunch {
                    team_id: "team-1".to_string(),
                    worker_id: "worker-1".to_string(),
                    role: None,
                    agent: "codex".to_string(),
                    surface_id: surface_id.clone(),
                    worktree_name: None,
                    assigned_task_id: None,
                },
                forktty_core::team_now_ms(),
            )
        })
        .unwrap();

        let err = dispatch(
            &state,
            "team.worker.shutdown",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "text": "stop now",
                "close_surface": true,
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "precondition_failed");
        assert!(backend.sent_text(&surface_id).unwrap().is_empty());
        assert!(state.model.lock().unwrap().surface(&surface_id).is_some());
    }

    #[tokio::test]
    async fn team_worker_health_reports_active_status_as_running_final_state() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "leader_surface_id": surface_id,
                "status": "done"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": surface_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();

        let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(summary["workers_active"], 1);
        assert!(summary["consistency_warnings"]
            .as_array()
            .unwrap()
            .contains(&json!("done_with_active_workers")));

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();
        assert_eq!(health["workers"][0]["lifecycle"], "active");
        assert_eq!(health["workers"][0]["final_state"], "running");
    }

    #[tokio::test]
    async fn team_finish_marks_quiescent_active_team_done() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.task.upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "title": "Review",
                "status": "done"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "status": "shutdown_requested",
                "assigned_task_id": "task-1"
            }),
        )
        .await
        .unwrap();
        let before = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(
            before["consistency_warnings"],
            json!(["active_without_open_work"])
        );

        let finished = dispatch(&state, "team.finish", json!({"team_id": "team-1"}))
            .await
            .unwrap();

        assert_eq!(finished["finished"], true);
        assert_eq!(finished["summary_before"]["status"], "active");
        assert_eq!(
            finished["summary_before"]["consistency_warnings"],
            json!(["active_without_open_work"])
        );
        assert_eq!(finished["summary_after"]["status"], "done");
        assert_eq!(finished["summary_after"]["consistency_warnings"], json!([]));
        let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(summary["status"], "done");
    }

    #[tokio::test]
    async fn team_finish_dry_run_reports_blockers_without_mutating() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.task.upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "title": "Review",
                "status": "open"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": surface_id,
                "status": "running",
                "assigned_task_id": "task-1"
            }),
        )
        .await
        .unwrap();

        let dry_run = dispatch(
            &state,
            "team.finish",
            json!({"team_id": "team-1", "dry_run": true}),
        )
        .await
        .unwrap();

        assert_eq!(dry_run["dry_run"], true);
        assert_eq!(dry_run["finished"], false);
        assert_eq!(dry_run["blockers"], json!(["open_tasks", "active_workers"]));
        assert_eq!(dry_run["summary_before"]["status"], "active");
        assert_eq!(dry_run["summary_before"]["tasks_open"], 1);
        let summary = dispatch(&state, "team.summary", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(summary["status"], "active");
        assert_eq!(summary["tasks_open"], 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_finish_can_close_launch_owned_workers() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();
        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "status": "running"
            }),
        )
        .await
        .unwrap();
        let surface_id = launched["surface"]["id"].as_str().unwrap().to_string();

        let finished = dispatch(
            &state,
            "team.finish",
            json!({"team_id": "team-1", "close_workers": true}),
        )
        .await
        .unwrap();

        assert_eq!(finished["finished"], true);
        assert_eq!(finished["closed"].as_array().unwrap().len(), 1);
        assert_eq!(finished["summary_after"]["status"], "done");
        assert_eq!(
            finished["worker_health_after"]["workers"][0]["final_state"],
            "closed"
        );
        assert!(backend.sent_text(&surface_id).is_err());
    }

    #[tokio::test]
    async fn team_finish_close_workers_reports_uncloseable_active_worker() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "status": "running"
            }),
        )
        .await
        .unwrap();

        let dry_run = dispatch(
            &state,
            "team.finish",
            json!({"team_id": "team-1", "close_workers": true, "dry_run": true}),
        )
        .await
        .unwrap();
        assert_eq!(dry_run["blockers"], json!([]));
        assert_eq!(dry_run["cleanup_errors"].as_array().unwrap().len(), 1);
        assert_eq!(dry_run["cleanup_errors"][0]["worker_id"], "worker-1");
        assert!(dry_run["cleanup_errors"][0]["error"]
            .as_str()
            .unwrap()
            .contains("no launch-owned surface"));

        let error = dispatch(
            &state,
            "team.finish",
            json!({"team_id": "team-1", "close_workers": true}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "precondition_failed");
        assert!(error
            .to_string()
            .contains("team.finish cannot close all workers"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_finish_marks_missing_worker_surface_closed_without_force() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
                "status": "active"
            }),
        )
        .await
        .unwrap();
        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex"
            }),
        )
        .await
        .unwrap();
        let surface_id = launched["surface"]["id"].as_str().unwrap();
        backend.close(surface_id).unwrap();

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();
        assert_eq!(health["workers"][0]["status"], "running");
        assert_eq!(health["workers"][0]["final_state"], "surface_missing");

        let finished = dispatch(&state, "team.finish", json!({"team_id": "team-1"}))
            .await
            .unwrap();

        assert_eq!(finished["finished"], true);
        assert!(finished["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["action"] == "mark_worker_closed"));
        assert_eq!(finished["team"]["workers"][0]["status"], "closed");
        assert_eq!(finished["summary_after"]["status"], "done");
        assert_eq!(finished["summary_after"]["workers_active"], 0);
        assert_eq!(finished["summary_after"]["consistency_warnings"], json!([]));
        assert_eq!(
            finished["worker_health_after"]["workers"][0]["final_state"],
            "closed"
        );
    }

    #[tokio::test]
    async fn team_worker_health_treats_missing_terminal_runtime_as_surface_missing() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": surface_id,
                "status": "running",
            }),
        )
        .await
        .unwrap();

        backend.close(surface_id).unwrap();
        assert!(state.model.lock().unwrap().surface(surface_id).is_some());

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();

        assert_eq!(health["workers"][0]["surface_present"], true);
        assert_eq!(health["workers"][0]["surface_runtime_present"], false);
        assert_eq!(health["workers"][0]["surface_ready"], false);
        assert_eq!(health["workers"][0]["surface_alive"], false);
        assert_eq!(health["workers"][0]["lifecycle"], "surface_missing");
        assert_eq!(health["workers"][0]["final_state"], "surface_missing");
    }

    #[tokio::test]
    async fn team_worker_health_reports_present_unready_runtime_as_starting() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(NotReadySendBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": surface_id,
                "status": "running",
            }),
        )
        .await
        .unwrap();

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 1_000_000}),
        )
        .await
        .unwrap();

        assert_eq!(health["workers"][0]["surface_present"], true);
        assert_eq!(health["workers"][0]["surface_runtime_present"], true);
        assert_eq!(health["workers"][0]["surface_ready"], false);
        assert_eq!(health["workers"][0]["surface_alive"], false);
        assert_eq!(health["workers"][0]["lifecycle"], "starting");
        assert_eq!(health["workers"][0]["final_state"], "starting");
    }

    #[tokio::test]
    async fn team_worker_health_reports_present_unready_stale_runtime_as_stale() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(NotReadySendBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": workspace_id,
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "surface_id": surface_id,
                "status": "running",
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.heartbeat",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "status": "running",
            }),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;

        let health = dispatch(
            &state,
            "team.worker.health",
            json!({"team_id": "team-1", "stale_after_ms": 0}),
        )
        .await
        .unwrap();

        assert_eq!(health["workers"][0]["surface_present"], true);
        assert_eq!(health["workers"][0]["surface_runtime_present"], true);
        assert_eq!(health["workers"][0]["surface_ready"], false);
        assert_eq!(health["workers"][0]["surface_alive"], false);
        assert_eq!(health["workers"][0]["lifecycle"], "stale");
        assert_eq!(health["workers"][0]["final_state"], "stale");
    }

    #[tokio::test]
    async fn context_snapshot_includes_compact_team_summaries() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();
        let worker_surface = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": "vertical"}),
        )
        .await
        .unwrap();
        let worker_surface_id = worker_surface["id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id,
                "goal": "review state"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "claude",
                "surface_id": worker_surface_id,
                "status": "running"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.task.upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "title": "Review"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "status?"
            }),
        )
        .await
        .unwrap();

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(snapshot["team_summaries"][0]["team_id"], "team-1");
        assert_eq!(snapshot["team_summaries"][0]["workers_total"], 1);
        assert_eq!(snapshot["team_summaries"][0]["workers_active"], 1);
        assert_eq!(snapshot["team_summaries"][0]["tasks_open"], 1);
        assert_eq!(snapshot["team_summaries"][0]["messages_pending"], 1);
    }

    #[tokio::test]
    async fn context_snapshot_omits_team_details_by_default() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id,
                "goal": "review state"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "claude",
                "surface_id": surface_id,
                "status": "running"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "large worker prompt that should not ride along by default"
            }),
        )
        .await
        .unwrap();

        let compact = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();
        assert_eq!(compact["teams"].as_array().unwrap().len(), 0);
        assert_eq!(compact["team_summaries"][0]["team_id"], "team-1");

        let detailed = dispatch(
            &state,
            "context.snapshot",
            json!({
                "workspace_id": workspace_id,
                "tail_lines": 0,
                "include_team_details": true
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            detailed["teams"][0]["messages"][0]["body"],
            "large worker prompt that should not ride along by default"
        );
    }

    #[tokio::test]
    async fn context_snapshot_team_summaries_report_done_inconsistencies() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspace[0]["id"].as_str().unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id,
                "status": "done"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "claude",
                "surface_id": surface_id,
                "status": "running"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.task.upsert",
            json!({
                "team_id": "team-1",
                "task_id": "task-1",
                "title": "Review"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "status?"
            }),
        )
        .await
        .unwrap();

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();
        assert_eq!(snapshot["team_summaries"][0]["status"], "done");
        assert_eq!(
            snapshot["team_summaries"][0]["consistency_warnings"],
            json!([
                "done_with_active_workers",
                "done_with_open_tasks",
                "done_with_pending_messages"
            ])
        );
        assert!(snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("team_consistency_warning")));
    }

    #[tokio::test]
    async fn team_message_dispatch_submit_sends_enter_and_marks_team_wide_delivered() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-default",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "paste only"
            }),
        )
        .await
        .unwrap();
        let default_dispatch = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-default"}),
        )
        .await
        .unwrap();
        assert_eq!(default_dispatch["submitted"], false);

        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-submit",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "run status"
            }),
        )
        .await
        .unwrap();
        let submitted = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
        )
        .await
        .unwrap();
        assert_eq!(submitted["submitted"], true);
        assert_eq!(submitted["message"]["delivered"], true);

        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-already-entered",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "already entered\r"
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-already-entered", "submit": true}),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-trailing-lf",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "multi-line prompt\n"
            }),
        )
        .await
        .unwrap();
        let trailing_lf = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-trailing-lf", "submit": true}),
        )
        .await
        .unwrap();
        assert_eq!(trailing_lf["submitted"], true);
        assert_eq!(trailing_lf["message"]["delivered"], true);

        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-team-wide",
                "from": "leader",
                "body": "broadcast"
            }),
        )
        .await
        .unwrap();
        let team_wide = dispatch(
            &state,
            "team.message.dispatch",
            json!({
                "team_id": "team-1",
                "message_id": "msg-team-wide",
                "worker_id": "worker-1",
                "submit": true
            }),
        )
        .await
        .unwrap();
        assert_eq!(team_wide["submitted"], true);
        assert_eq!(team_wide["message"]["delivered"], true);

        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec![
                "paste only".to_string(),
                "run status\r".to_string(),
                "already entered\r".to_string(),
                "multi-line prompt\n\r".to_string(),
                "broadcast\r".to_string()
            ]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn team_message_dispatch_same_message_concurrently_sends_once() {
        let (first_send_started_tx, first_send_started_rx) = mpsc::channel();
        let (release_first_send_tx, release_first_send_rx) = mpsc::channel();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(BlockingFirstSendBackend::new(
            first_send_started_tx,
            release_first_send_rx,
        ));
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "do it"
            }),
        )
        .await
        .unwrap();

        let first_state = state.clone();
        let first = tokio::spawn(async move {
            dispatch(
                &first_state,
                "team.message.dispatch",
                json!({"team_id": "team-1", "message_id": "msg-1"}),
            )
            .await
        });
        first_send_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first dispatch should reach terminal send");

        let second_state = state.clone();
        let second = tokio::spawn(async move {
            dispatch(
                &second_state,
                "team.message.dispatch",
                json!({"team_id": "team-1", "message_id": "msg-1"}),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        release_first_send_tx.send(()).unwrap();

        let first = first.await.unwrap();
        let second = second.await.unwrap();
        assert!(
            first.is_ok() != second.is_ok(),
            "one dispatch must win and one must conflict: {first:?} / {second:?}"
        );
        let conflict = first.err().or_else(|| second.err()).unwrap();
        assert_eq!(conflict.code(), "conflict");
        assert_eq!(
            backend.sent_text(&surface_id).unwrap(),
            vec!["do it".to_string()]
        );
        let store =
            forktty_core::load_teams_from_path(state.team_store_path.as_ref().unwrap()).unwrap();
        let ack_events = store
            .events
            .iter()
            .filter(|event| event.kind == "team.message.acked")
            .count();
        assert_eq!(ack_events, 1);
    }

    #[tokio::test]
    async fn team_message_dispatch_does_not_resend_when_ack_persist_fails() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let team_store_path = dir.path().join("team-v1.json");
        state.team_store_path = Some(team_store_path.clone());
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "do it"
            }),
        )
        .await
        .unwrap();

        let tmp_path = team_store_path.with_extension(format!("json.tmp-{}", std::process::id()));
        fs::create_dir(&tmp_path).unwrap();
        let first = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
        .unwrap_err();

        assert_eq!(first.code(), "error");
        assert_eq!(
            backend.sent_text(&surface_id).unwrap(),
            vec!["do it".to_string()]
        );

        let retry = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
        .unwrap_err();

        assert_eq!(retry.code(), "conflict");
        assert_eq!(
            backend.sent_text(&surface_id).unwrap(),
            vec!["do it".to_string()]
        );
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn team_worker_launch_same_worker_rejects_duplicate_and_rolls_back_surface() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_codex(bin_dir.path());
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let leader_surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": leader_surface_id
            }),
        )
        .await
        .unwrap();
        let first = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex"
            }),
        )
        .await
        .unwrap();
        let first_surface_id = first["surface"]["id"].as_str().unwrap().to_string();

        let duplicate = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(duplicate.code(), "conflict");
        let runtime_surfaces = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        assert!(runtime_surfaces.contains(&leader_surface_id.to_string()));
        assert!(runtime_surfaces.contains(&first_surface_id));
        assert_eq!(
            runtime_surfaces.len(),
            2,
            "duplicate launch surface must be rolled back"
        );
    }

    #[tokio::test]
    async fn team_message_dispatch_shows_worker_surface_before_sending() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(ShowReadyBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "review queued prompt"
            }),
        )
        .await
        .unwrap();

        let dispatched = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
        .unwrap();

        assert_eq!(dispatched["message"]["delivered"], true);
        assert_eq!(backend.shown_surfaces(), vec![surface_id.to_string()]);
        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec!["review queued prompt".to_string()]
        );
    }

    #[tokio::test]
    async fn team_message_dispatch_retries_not_ready_without_repeated_focus() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(ShowReadyBackend::with_not_ready_sends_after_show(1));
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "review queued prompt"
            }),
        )
        .await
        .unwrap();

        let dispatched = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
        .unwrap();

        assert_eq!(dispatched["message"]["delivered"], true);
        assert_eq!(backend.shown_surfaces(), vec![surface_id.to_string()]);
        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec!["review queued prompt".to_string()]
        );
    }

    #[tokio::test]
    async fn team_message_dispatch_activates_worker_workspace_before_sending() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(ShowReadyBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let first_workspace_id = first[0]["id"].as_str().unwrap().to_string();
        let second_dir = tempfile::tempdir().unwrap();
        let second = dispatch(
            &state,
            "workspace.create",
            json!({"name": "review", "workingDir": second_dir.path()}),
        )
        .await
        .unwrap();
        let second_workspace_id = second["id"].as_str().unwrap().to_string();
        let second_surface_id = second["focused_surface_id"].as_str().unwrap().to_string();
        dispatch(
            &state,
            "workspace.select",
            json!({"workspace_id": first_workspace_id}),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": first[0]["focused_surface_id"]
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": second_surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "review queued prompt"
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1"}),
        )
        .await
        .unwrap();

        assert_eq!(
            state.model.lock().unwrap().active_workspace_id().as_deref(),
            Some(second_workspace_id.as_str())
        );
        assert_eq!(backend.shown_surfaces(), vec![second_surface_id.clone()]);
        assert_eq!(
            backend.sent_text(&second_surface_id).unwrap(),
            vec!["review queued prompt".to_string()]
        );
    }

    #[tokio::test]
    async fn team_message_dispatch_rejects_non_boolean_submit() {
        let (mut state, backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-1",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "run"
            }),
        )
        .await
        .unwrap();

        let err = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-1", "submit": "yes"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(backend.sent_text(surface_id).unwrap_or_default().is_empty());
    }

    #[tokio::test]
    async fn team_message_dispatch_submit_does_not_call_separate_enter() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailingEnterBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-submit",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "run status"
            }),
        )
        .await
        .unwrap();

        let result = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
        )
        .await
        .unwrap();
        assert_eq!(result["submitted"], true);
        assert_eq!(result["message"]["delivered"], true);
        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec!["run status\r".to_string()]
        );

        let team = dispatch(&state, "team.get", json!({"team_id": "team-1"}))
            .await
            .unwrap();
        assert_eq!(team["messages"][0]["delivered"], true);
        assert!(team["messages"][0]["acknowledged_at_ms"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn team_message_dispatch_submit_uses_separate_enter_for_claude() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(RecordingEnterBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "claude",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.message.send",
            json!({
                "team_id": "team-1",
                "message_id": "msg-submit",
                "from": "leader",
                "to_worker_id": "worker-1",
                "body": "run status"
            }),
        )
        .await
        .unwrap();

        let result = dispatch(
            &state,
            "team.message.dispatch",
            json!({"team_id": "team-1", "message_id": "msg-submit", "submit": true}),
        )
        .await
        .unwrap();

        assert_eq!(result["submitted"], true);
        assert_eq!(result["message"]["delivered"], true);
        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec!["run status".to_string()]
        );
        assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
    }

    #[tokio::test]
    async fn team_worker_shutdown_submit_uses_separate_enter_for_claude() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(RecordingEnterBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspace = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let surface_id = workspace[0]["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "leader_surface_id": surface_id
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "team.worker.upsert",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "claude",
                "surface_id": surface_id
            }),
        )
        .await
        .unwrap();

        let result = dispatch(
            &state,
            "team.worker.shutdown",
            json!({"team_id": "team-1", "worker_id": "worker-1", "text": "stop"}),
        )
        .await
        .unwrap();

        assert_eq!(result["submitted"], true);
        assert_eq!(result["worker"]["status"], "shutdown_requested");
        assert_eq!(
            backend.sent_text(surface_id).unwrap(),
            vec!["stop".to_string()]
        );
        assert_eq!(backend.entered_surfaces(), vec![surface_id.to_string()]);
    }

    #[tokio::test]
    async fn team_upsert_rejects_leader_surface_from_another_workspace() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(dir.path().join("team-v1.json"));
        let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let first_surface_id = first[0]["focused_surface_id"].as_str().unwrap();
        let other = dispatch(
            &state,
            "workspace.create",
            json!({"name": "other", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let err = dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": other["id"].clone(),
                "leader_surface_id": first_surface_id
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("leader_surface_id"));
    }

    #[test]
    fn team_store_update_does_not_block_current_thread_runtime() {
        let (mut state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("team-v1.json");
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(store_path.with_extension("lock"))
            .unwrap();
        lock_file.lock().unwrap();
        state.team_store_path = Some(store_path);

        let (ping_tx, ping_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let runtime_state = state.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let update_state = runtime_state.clone();
                let update = tokio::spawn(async move {
                    dispatch(
                        &update_state,
                        "team.upsert",
                        json!({"team_id": "team-1", "name": "Runtime", "status": "active"}),
                    )
                    .await
                });
                tokio::task::yield_now().await;
                let ping = dispatch(&runtime_state, "system.ping", json!({})).await;
                ping_tx.send(ping.is_ok()).unwrap();
                done_tx.send(update.await.unwrap().is_ok()).unwrap();
            });
        });

        let ping_before_unlock = ping_rx
            .recv_timeout(Duration::from_millis(200))
            .unwrap_or(false);
        drop(lock_file);
        assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        thread.join().unwrap();
        assert!(
            ping_before_unlock,
            "team store I/O must not block unrelated socket work on a current-thread runtime"
        );
    }

    #[tokio::test]
    async fn dispatches_workflow_control_plane_methods() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "surface_id": surface_id,
                "agent": "codex",
                "session_id": "codex-session-1",
                "mode": "review",
                "status": "running",
                "goal": "Review the current branch",
                "memory": "Watch socket behavior"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();
        assert_eq!(workflow["workspace_id"], workspace_id);
        assert_eq!(workflow["surface_id"], surface_id);
        assert_eq!(workflow_id, "session:codex:codex-session-1:review");

        let planned = dispatch(
            &state,
            "workflow.plan.set",
            json!({
                "workflow_id": workflow_id,
                "steps": [
                    {"id": "inspect", "title": "Inspect socket changes", "status": "done"},
                    {"id": "test", "title": "Run tests", "status": "pending", "detail": "socket"}
                ]
            }),
        )
        .await
        .unwrap();
        assert_eq!(planned["plan"].as_array().unwrap().len(), 2);

        let evidenced = dispatch(
            &state,
            "workflow.evidence.add",
            json!({
                "workflow_id": workflow_id,
                "evidence_id": "socket-test",
                "kind": "test",
                "title": "workflow socket test",
                "text": "  passed\n"
            }),
        )
        .await
        .unwrap();
        assert_eq!(evidenced["evidence"][0]["id"], "socket-test");
        assert_eq!(evidenced["evidence"][0]["text"], "  passed\n");

        let listed = dispatch(
            &state,
            "workflow.list",
            json!({"workspace_id": workspace_id, "query": "socket", "limit": 10}),
        )
        .await
        .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["id"], workflow_id);

        let fetched = dispatch(&state, "workflow.get", json!({"workflow_id": workflow_id}))
            .await
            .unwrap();
        assert_eq!(fetched["memory"], "Watch socket behavior");

        let replay = dispatch(
            &state,
            "workflow.replay",
            json!({"workflow_id": workflow_id}),
        )
        .await
        .unwrap();
        assert_eq!(replay.as_array().unwrap().len(), 3);
        assert_eq!(replay[2]["kind"], "workflow.evidence.added");

        let other_workspace_id = {
            let mut model = state.model.lock().unwrap();
            model.create_workspace("other", "/tmp").id
        };
        let error = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workspace_id": other_workspace_id,
                "surface_id": surface_id,
                "workflow_id": "bad-target"
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
    }

    #[test]
    fn workflow_store_update_does_not_block_current_thread_runtime() {
        let (state, _backend) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let store_path = dir.path().join("workflow-v1.json");
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(store_path.with_extension("lock"))
            .unwrap();
        lock_file.lock().unwrap();
        let state = state.with_workflow_store_path(store_path);

        let (ping_tx, ping_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let runtime_state = state.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let update_state = runtime_state.clone();
                let update = tokio::spawn(async move {
                    dispatch(
                        &update_state,
                        "workflow.upsert",
                        json!({"workflow_id": "workflow-1", "status": "active"}),
                    )
                    .await
                });
                tokio::task::yield_now().await;
                let ping = dispatch(&runtime_state, "system.ping", json!({})).await;
                ping_tx.send(ping.is_ok()).unwrap();
                done_tx.send(update.await.unwrap().is_ok()).unwrap();
            });
        });

        let ping_before_unlock = ping_rx
            .recv_timeout(Duration::from_millis(200))
            .unwrap_or(false);
        drop(lock_file);
        assert!(done_rx.recv_timeout(Duration::from_secs(2)).unwrap());
        thread.join().unwrap();
        assert!(
            ping_before_unlock,
            "workflow store I/O must not block unrelated socket work on a current-thread runtime"
        );
    }

    #[tokio::test]
    async fn context_snapshot_compacts_workflows_by_default_and_expands_on_request() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-1",
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "status": "running",
                "goal": "Review policy",
                "memory": "Large durable workflow memory that should not ride along in compact context snapshots"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();
        dispatch(
            &state,
            "workflow.plan.set",
            json!({
                "workflow_id": workflow_id,
                "steps": [
                    {"id": "inspect", "title": "Inspect", "status": "done"},
                    {"id": "verify", "title": "Verify", "status": "done"}
                ]
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "workflow.evidence.add",
            json!({
                "workflow_id": workflow_id,
                "evidence_id": "evidence-1",
                "kind": "review",
                "title": "Review details",
                "text": "Large evidence body that should be opt-in"
            }),
        )
        .await
        .unwrap();

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(snapshot["workflows"].as_array().unwrap().len(), 0);
        assert_eq!(snapshot["workflow_summaries"][0]["id"], "workflow-1");
        assert_eq!(snapshot["workflow_summaries"][0]["status"], "running");
        assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_total"], 2);
        assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_open"], 0);
        assert_eq!(snapshot["workflow_summaries"][0]["evidence_total"], 1);
        assert_eq!(
            snapshot["workflow_summaries"][0]["consistency_warnings"],
            json!(["running_with_completed_plan"])
        );
        assert!(snapshot["workflow_summaries"][0].get("memory").is_none());
        assert!(snapshot["workflow_summaries"][0].get("plan").is_none());
        assert!(snapshot["workflow_summaries"][0].get("evidence").is_none());
        assert!(snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("workflow_consistency_warning")));

        let detailed = dispatch(
            &state,
            "context.snapshot",
            json!({
                "workspace_id": workspace_id,
                "tail_lines": 0,
                "include_workflow_details": true
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            detailed["workflows"][0]["memory"],
            "Large durable workflow memory that should not ride along in compact context snapshots"
        );
        assert_eq!(
            detailed["workflows"][0]["plan"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            detailed["workflows"][0]["evidence"][0]["text"],
            "Large evidence body that should be opt-in"
        );
        assert_eq!(
            detailed["workflows"][0]["consistency_warnings"],
            json!(["running_with_completed_plan"])
        );
        assert_eq!(detailed["workflow_summaries"][0]["id"], "workflow-1");
        assert!(detailed["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("workflow_consistency_warning")));
    }

    #[tokio::test]
    async fn context_snapshot_treats_completed_workflow_steps_as_terminal() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-completed-steps",
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "status": "done",
                "goal": "Finished review"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();
        dispatch(
            &state,
            "workflow.plan.set",
            json!({
                "workflow_id": workflow_id,
                "steps": [
                    {"id": "inspect", "title": "Inspect", "status": "completed"},
                    {"id": "verify", "title": "Verify", "status": "completed"}
                ]
            }),
        )
        .await
        .unwrap();

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_total"], 2);
        assert_eq!(snapshot["workflow_summaries"][0]["plan_steps_open"], 0);
        assert_eq!(
            snapshot["workflow_summaries"][0]["consistency_warnings"],
            json!([])
        );
        assert!(!snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("workflow_consistency_warning")));
    }

    #[tokio::test]
    async fn workflow_loop_set_updates_state_and_snapshot_loop_summaries() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-1",
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "status": "running",
                "goal": "Review and verify"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();

        let updated = dispatch(
            &state,
            "workflow.loop.set",
            json!({
                "workflow_id": workflow_id,
                "recipe": "review-fix-verify",
                "stage": "verify",
                "iteration": 2,
                "max_iterations": 3,
                "gates": [
                    {
                        "id": "fmt",
                        "kind": "command",
                        "label": "cargo fmt --all --check",
                        "status": "passed",
                        "summary": "formatting clean"
                    },
                    {
                        "id": "review",
                        "kind": "worker_review",
                        "label": "Claude review",
                        "status": "failed",
                        "summary": "one blocker"
                    }
                ]
            }),
        )
        .await
        .unwrap();

        assert_eq!(updated["loop_recipe"], "review-fix-verify");
        assert_eq!(updated["loop_stage"], "verify");
        assert_eq!(updated["loop_iteration"], 2);
        assert_eq!(updated["loop_max_iterations"], 3);
        assert_eq!(updated["loop_gates"][1]["status"], "failed");

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(snapshot["loop_summaries"][0]["workflow_id"], workflow_id);
        assert_eq!(snapshot["loop_summaries"][0]["recipe"], "review-fix-verify");
        assert_eq!(snapshot["loop_summaries"][0]["stage"], "verify");
        assert_eq!(snapshot["loop_summaries"][0]["iteration"], 2);
        assert_eq!(snapshot["loop_summaries"][0]["gates_total"], 2);
        assert_eq!(snapshot["loop_summaries"][0]["gates_failed"], 1);
        assert_eq!(snapshot["loop_summaries"][0].get("goal"), None);
        assert!(snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("loop_gate_failed")));
    }

    #[tokio::test]
    async fn workflow_loop_set_rejects_invalid_loop_payloads() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-1",
                "status": "running"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();

        let zero_max = dispatch(
            &state,
            "workflow.loop.set",
            json!({"workflow_id": workflow_id, "max_iterations": 0}),
        )
        .await
        .unwrap_err();
        assert_eq!(zero_max.code(), "invalid_param");
        assert!(zero_max
            .to_string()
            .contains("loop max_iterations must be greater than 0"));

        let duplicate_gate = dispatch(
            &state,
            "workflow.loop.set",
            json!({
                "workflow_id": workflow_id,
                "gates": [
                    {"id": "same", "kind": "command", "label": "one", "status": "pending"},
                    {"id": "same", "kind": "command", "label": "two", "status": "pending"}
                ]
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(duplicate_gate.code(), "invalid_param");
        assert!(duplicate_gate
            .to_string()
            .contains("duplicate loop gate id"));

        let bad_gates_shape = dispatch(
            &state,
            "workflow.loop.set",
            json!({"workflow_id": workflow_id, "gates": "not-array"}),
        )
        .await
        .unwrap_err();
        assert_eq!(bad_gates_shape.code(), "invalid_param");
        assert!(bad_gates_shape
            .to_string()
            .contains("Invalid parameter gates"));
    }

    #[tokio::test]
    async fn context_snapshot_loop_summaries_report_stale_surface_bindings() {
        let (state, _) = test_state();
        let dir = tempfile::tempdir().unwrap();
        let state = state.with_workflow_store_path(dir.path().join("workflow-v1.json"));
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap();
        let stale_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

        let workflow = dispatch(
            &state,
            "workflow.upsert",
            json!({
                "workflow_id": "workflow-stale-surface",
                "workspace_id": workspace_id,
                "surface_id": stale_surface_id,
                "status": "running"
            }),
        )
        .await
        .unwrap();
        let workflow_id = workflow["id"].as_str().unwrap();

        dispatch(
            &state,
            "workflow.loop.set",
            json!({
                "workflow_id": workflow_id,
                "recipe": "closed-loop",
                "stage": "verify",
                "iteration": 1,
                "max_iterations": 3
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "surface.close",
            json!({"surface_id": stale_surface_id}),
        )
        .await
        .unwrap();

        let snapshot = dispatch(
            &state,
            "context.snapshot",
            json!({"workspace_id": workspace_id, "tail_lines": 0}),
        )
        .await
        .unwrap();

        assert_eq!(snapshot["loop_summaries"][0]["workflow_id"], workflow_id);
        assert_eq!(
            snapshot["loop_summaries"][0]["surface_id"],
            stale_surface_id
        );
        assert_eq!(snapshot["loop_summaries"][0]["surface_present"], false);
        assert_eq!(snapshot["loop_summaries"][0]["stale_binding"], true);
        assert!(snapshot["risk_flags"]
            .as_array()
            .unwrap()
            .contains(&json!("loop_stale_binding")));
    }

    #[tokio::test]
    async fn feed_list_reads_persisted_feed_entries_and_records_approval_decisions() {
        let dir = tempfile::tempdir().unwrap();
        let feed_path = dir.path().join("feed.json");
        let (state, _) = test_state();
        let state = state.with_feed_store_path(&feed_path).unwrap();
        let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();
        {
            let prev = current_snapshot(&state.model);
            state.model.lock().unwrap().create_notification(
                "Permission",
                "Run command?",
                NotificationKind::Prompt,
                Some(workspace_id.clone()),
                None,
            );
            let next = current_snapshot(&state.model);
            feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
        }

        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 10}),
        )
        .await
        .unwrap();
        let approval_id = feed[0]["id"].as_str().unwrap();
        assert_eq!(feed[0]["type"], "approval");
        assert_eq!(feed[0]["kind"], "prompt");
        assert_eq!(feed[0]["read"], false);
        assert_eq!(feed[0]["approval_state"], "pending");

        let decided = dispatch(
            &state,
            "feed.approval.respond",
            json!({"id": approval_id, "decision": "approved"}),
        )
        .await
        .unwrap();

        assert_eq!(decided["approval_state"], "approved");
        assert_eq!(
            forktty_core::FeedStore::open_at(&feed_path)
                .unwrap()
                .list(None, 10)[0]
                .approval_state,
            Some(forktty_core::FeedApprovalState::Approved)
        );
    }

    #[tokio::test]
    async fn feed_list_sees_socket_created_notification_without_waiting_for_event_tick() {
        let dir = tempfile::tempdir().unwrap();
        let feed_path = dir.path().join("feed.json");
        let (state, _) = test_state();
        let state = state.with_feed_store_path(&feed_path).unwrap();
        let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

        dispatch(
            &state,
            "notification.create",
            json!({
                "workspace_id": workspace_id,
                "kind": "prompt",
                "title": "Permission",
                "body": "Run command?"
            }),
        )
        .await
        .unwrap();
        let feed = dispatch(
            &state,
            "feed.list",
            json!({"workspace_id": workspace_id, "limit": 10}),
        )
        .await
        .unwrap();

        assert_eq!(feed[0]["type"], "approval");
        assert_eq!(feed[0]["title"], "Permission");
        assert_eq!(feed[0]["approval_state"], "pending");
    }

    #[tokio::test]
    async fn feed_notification_approval_ids_do_not_collide_after_model_restart() {
        let dir = tempfile::tempdir().unwrap();
        let feed_path = dir.path().join("feed.json");
        let (first_state, _) = test_state();
        let first_state = first_state.with_feed_store_path(&feed_path).unwrap();
        let first_workspace_id = first_state
            .model
            .lock()
            .unwrap()
            .active_workspace_id()
            .unwrap();

        dispatch(
            &first_state,
            "notification.create",
            json!({
                "workspace_id": first_workspace_id,
                "kind": "prompt",
                "title": "Old prompt",
                "body": "Run old command?"
            }),
        )
        .await
        .unwrap();
        let old_feed = dispatch(
            &first_state,
            "feed.list",
            json!({"workspace_id": first_workspace_id, "limit": 10}),
        )
        .await
        .unwrap();
        let old_approval_id = old_feed[0]["id"].as_str().unwrap();
        dispatch(
            &first_state,
            "feed.approval.respond",
            json!({"id": old_approval_id, "decision": "approved"}),
        )
        .await
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let (second_state, _) = test_state();
        let second_state = second_state.with_feed_store_path(&feed_path).unwrap();
        let second_workspace_id = second_state
            .model
            .lock()
            .unwrap()
            .active_workspace_id()
            .unwrap();
        dispatch(
            &second_state,
            "notification.create",
            json!({
                "workspace_id": second_workspace_id,
                "kind": "prompt",
                "title": "New prompt",
                "body": "Run new command?"
            }),
        )
        .await
        .unwrap();
        let feed = dispatch(
            &second_state,
            "feed.list",
            json!({"workspace_id": second_workspace_id, "limit": 10}),
        )
        .await
        .unwrap();

        assert_eq!(feed[0]["title"], "New prompt");
        assert_eq!(feed[0]["approval_state"], "pending");
        assert_ne!(feed[0]["id"], old_approval_id);
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

            assert_eq!(error.code(), "invalid_param");
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

            assert_eq!(error.code(), "invalid_param");
            assert!(error.to_string().contains("Invalid parameter title"));
        }

        let error = dispatch(&state, "notification.create", json!({"body": 42}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
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

            assert_eq!(error.code(), "invalid_param");
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
    async fn metadata_commands_reject_stale_surface_targets_even_with_workspace_id() {
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

        for (method, params) in [
            (
                "metadata.set_status",
                json!({
                    "workspace_id": workspace_id,
                    "surface_id": stale_surface_id,
                    "key": "agent:codex",
                    "label": "Codex",
                    "value": "Running"
                }),
            ),
            (
                "metadata.set_progress",
                json!({
                    "workspace_id": workspace_id,
                    "surface_id": stale_surface_id,
                    "key": "build",
                    "label": "Build",
                    "value": 1
                }),
            ),
            (
                "metadata.log",
                json!({
                    "workspace_id": workspace_id,
                    "surface_id": stale_surface_id,
                    "level": "info",
                    "message": "waiting"
                }),
            ),
        ] {
            let error = dispatch(&state, method, params).await.unwrap_err();
            assert_eq!(error.code(), "not_found");
            assert_eq!(error.to_string(), "Surface not found");
        }
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
    async fn stale_hook_status_does_not_update_persisted_agent_session_lifecycle() {
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
                "hook_session_id": "codex-session-9",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 100,
                "hook_event_clock": "boottime-ns",
            }),
        )
        .await
        .unwrap();
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
                "hook_event_name": "stop",
                "hook_event_order": 200,
                "hook_event_clock": "boottime-ns",
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_session_id": "codex-session-9",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 150,
                "hook_event_clock": "boottime-ns",
            }),
        )
        .await
        .unwrap();

        let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
        assert_eq!(summary["status"][0]["value"], "Ready");
        assert_eq!(summary["agents"][0]["lifecycle"], "idle");

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_session_id": "codex-session-10",
                "hook_event_name": "prompt-submit",
                "hook_event_order": 3_000_000_000u64,
                "hook_event_clock": "boottime-ns",
            }),
        )
        .await
        .unwrap();

        let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
        assert_eq!(summary["status"][0]["value"], "Running");
        assert_eq!(summary["agents"][0]["lifecycle"], "running");

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Needs input",
                "hook_session_id": "codex-session-10",
                "hook_event_name": "permission-request",
                "hook_event_order": 2_500_000_000u64,
                "hook_event_clock": "boottime-ns",
            }),
        )
        .await
        .unwrap();

        let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
        assert_eq!(summary["status"][0]["value"], "Running");
        assert_eq!(summary["agents"][0]["lifecycle"], "running");
    }

    #[tokio::test]
    async fn hook_session_end_clear_marks_persisted_agent_session_ended() {
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
                "key": "agent:claude",
                "label": "Claude",
                "value": "Running",
                "hook_session_id": "claude-session-9",
                "hook_event_name": "session-start",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "key": "agent:claude",
                "hook_session_id": "claude-session-9",
                "hook_event_name": "session-end",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
        assert_eq!(health[0]["lifecycle"], "ended");
        assert!(health[0]["last_activity_ms"].as_u64().unwrap() > 0);

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
    async fn hook_session_end_cleanup_keeps_learned_target_until_aux_status_clear() {
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
                "value": "Running",
                "hook_session_id": "claude-session-10",
                "hook_event_name": "session-start",
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
                "surface_id": surface_id,
                "key": "agent:claude:permission",
                "label": "Claude permissions",
                "value": "bypassPermissions",
                "hook_session_id": "claude-session-10",
                "hook_event_name": "permission-request",
                "hook_event_order": 150
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

        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "key": "agent:claude",
                "hook_session_id": "claude-session-10",
                "hook_event_name": "session-end",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();
        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "key": "agent:claude:permission",
                "hook_session_id": "claude-session-10",
                "hook_event_name": "session-end",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        let health = dispatch(
            &state,
            "agent.health",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(health[0]["lifecycle"], "ended");
        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert!(statuses.as_array().unwrap().is_empty());
        let other_statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": other_workspace_id}),
        )
        .await
        .unwrap();
        assert!(other_statuses.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn stale_hook_session_end_clear_does_not_end_newer_running_session() {
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
                "key": "agent:claude",
                "label": "Claude",
                "value": "Running",
                "hook_session_id": "claude-session-11",
                "hook_event_name": "prompt-submit",
                "hook_event_clock": "boottime-ns",
                "hook_event_order": 200
            }),
        )
        .await
        .unwrap();

        dispatch(
            &state,
            "metadata.clear_status",
            json!({
                "key": "agent:claude",
                "hook_session_id": "claude-session-11",
                "hook_event_name": "session-end",
                "hook_event_clock": "boottime-ns",
                "hook_event_order": 100
            }),
        )
        .await
        .unwrap();

        let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
        assert_eq!(health[0]["lifecycle"], "running");
        let statuses = dispatch(
            &state,
            "metadata.list_status",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(statuses[0]["value"], "Running");
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
    async fn hook_session_status_surface_close_clears_status_and_invalidates_learned_target() {
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
        assert_eq!(original_statuses, json!([]));
        assert_eq!(other_statuses[0]["value"], "Untargeted after close");
    }

    #[tokio::test]
    async fn metadata_hooks_reject_cleanup_after_target_surface_closes() {
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

        let log_error = dispatch(
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
        .unwrap_err();
        let clear_error = dispatch(
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
        .unwrap_err();

        assert_eq!(log_error.code(), "not_found");
        assert_eq!(clear_error.code(), "not_found");

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
        assert_eq!(statuses.as_array().unwrap().len(), 1);
        assert!(logs.as_array().unwrap().is_empty());
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

            assert_eq!(error.code(), "invalid_param");
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

            assert_eq!(error.code(), "invalid_param");
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

            assert_eq!(status_error.code(), "invalid_param");
            assert_eq!(progress_error.code(), "invalid_param");
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
            assert_eq!(error.code(), "invalid_param");
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
            assert_eq!(error.code(), "invalid_param");
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
            assert_eq!(error.code(), "invalid_param");
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

            assert_eq!(error.code(), "invalid_param");
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
            pid: None,
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

            assert_eq!(error.code(), "invalid_param");
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn workspace_close_closes_tab_added_while_close_is_in_flight() {
        let (first_close_started_tx, first_close_started_rx) = mpsc::channel();
        let (release_first_close_tx, release_first_close_rx) = mpsc::channel();
        let (spawn_after_close_started_tx, spawn_after_close_started_rx) = mpsc::channel();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(BlockingFirstCloseBackend::new(
            first_close_started_tx,
            release_first_close_rx,
            spawn_after_close_started_tx,
        ));
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
        let surface_id = workspaces[0]["focused_surface_id"]
            .as_str()
            .unwrap()
            .to_string();

        let close_state = state.clone();
        let close = tokio::spawn(async move {
            dispatch(&close_state, "workspace.close", json!({"id": workspace_id})).await
        });
        first_close_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("workspace.close should reach terminal close");

        let add_tab_state = state.clone();
        let add_tab = tokio::spawn(async move {
            dispatch(
                &add_tab_state,
                "pane.new_tab",
                json!({"surface_id": surface_id}),
            )
            .await
        });
        let spawned_before_close_released = spawn_after_close_started_rx
            .recv_timeout(Duration::from_millis(200))
            .is_ok();
        release_first_close_tx.send(()).unwrap();
        close.await.unwrap().unwrap();
        let added_surface_id = match add_tab.await.unwrap() {
            Ok(added_tab) => Some(added_tab["id"].as_str().unwrap().to_string()),
            Err(err) => {
                assert_eq!(err.code(), "not_found");
                None
            }
        };

        let model_surfaces = {
            let model = state.model.lock().unwrap();
            model
                .list_surfaces(None)
                .into_iter()
                .map(|surface| surface.id)
                .collect::<BTreeSet<_>>()
        };
        let runtime_surfaces = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<BTreeSet<_>>();

        if let Some(added_surface_id) = added_surface_id {
            assert!(
                spawned_before_close_released,
                "successful tab add should have spawned before workspace.close was released"
            );
            assert!(
                !runtime_surfaces.contains(&added_surface_id),
                "tab added during workspace.close must not remain as an orphan runtime"
            );
        }
        assert_eq!(runtime_surfaces, model_surfaces);
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
            pid: None,
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

            assert_eq!(error.code(), "invalid_param");
            assert!(error.to_string().contains("Invalid parameter axis"));
        }

        let non_string_error = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": surface_id, "axis": 42}),
        )
        .await
        .unwrap_err();
        assert_eq!(non_string_error.code(), "invalid_param");
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
    async fn surface_list_includes_runtime_pid_when_known() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let workspace = {
            let mut model = model.lock().unwrap();
            model.create_workspace("project", Path::new("/tmp"))
        };
        let surface_id = workspace.focused_surface_id.clone();
        let backend = Arc::new(SpawnFailsCloseSucceedsBackend::new(TerminalSurfaceState {
            surface_id: surface_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: PathBuf::from("/tmp"),
            shell: "/bin/sh".to_string(),
            cols: 120,
            rows: 40,
            pid: Some(4242),
        }));
        let state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);

        let surfaces = dispatch(&state, "surface.list", json!({})).await.unwrap();

        assert_eq!(surfaces[0]["id"], surface_id);
        assert_eq!(surfaces[0]["pid"], 4242);
        assert_eq!(surfaces[0]["shell"], "/bin/sh");
        assert_eq!(surfaces[0]["cols"], 120);
        assert_eq!(surfaces[0]["rows"], 40);
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
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));
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
        assert_eq!(agents[0]["source"], "persisted_agent_session");
        let observed_at_ms = agents[0]["observed_at_ms"].as_u64().unwrap();
        assert!(observed_at_ms >= 1_000);
        assert_eq!(agents[0]["age_ms"], observed_at_ms - 1_000);
        assert_eq!(
            agents[0]["lifecycle_evidence"],
            json!({
                "source": "persisted_agent_session",
                "lifecycle": "running",
                "last_activity_ms": 1_000,
                "observed_at_ms": observed_at_ms,
                "age_ms": observed_at_ms - 1_000,
                "status_key": Value::Null,
                "status_value": Value::Null,
                "status_source": Value::Null,
                "status_scope": Value::Null,
                "permission_mode": Value::Null,
            })
        );

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
        assert_eq!(health[0]["source"], "persisted_agent_session");
        assert!(health[0]["observed_at_ms"].as_u64().is_some());
        assert!(health[0]["age_ms"].is_null());
        assert_eq!(health[0]["ready"], false);
        assert_eq!(health[0]["reason"], "unsupported_agent");
        assert_eq!(health[0]["argv"], json!([]));
        assert_eq!(
            health[0]["lifecycle_evidence"]["readiness_reason"],
            "unsupported_agent"
        );
        assert_eq!(health[0]["lifecycle_evidence"]["ready"], false);
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

        let health = agent_health_rows_with_path(&model, None, Some(dir.path().as_os_str()), 1_000);

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

        let health =
            agent_health_rows_with_path(&model, None, Some(path_dir.path().as_os_str()), 1_000);

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

        let health =
            agent_health_rows_with_path(&model, None, Some(empty_path.path().as_os_str()), 1_000);

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
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());

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

    #[test]
    fn agent_reclaim_plan_protects_suspended_sessions() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());

        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1"));
        assert!(model
            .set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Suspended,));
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));

        let plan =
            agent_reclaim_plan_with_path(&model, None, Some(dir.path().as_os_str()), 10_000, 5_000);

        assert!(plan["candidates"].as_array().unwrap().is_empty());
        assert_eq!(plan["protected"][0]["surface_id"], surface_id);
        assert_eq!(plan["protected"][0]["protect_reason"], "suspended");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_hibernate_marks_idle_ready_session_suspended_and_closes_backend() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let (state, backend) = test_state();
        let surface_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let workspace_id = workspace.id.clone();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model
                .set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,));
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
            assert!(model
                .set_status(&workspace_id, "agent:codex", "Codex", "Ready", None)
                .is_some());
            assert!(model
                .set_progress(
                    &workspace_id,
                    "agent:codex:tokens",
                    "Codex tokens",
                    42.0,
                    Some(100.0),
                )
                .is_some());
            surface_id
        };

        let hibernated = dispatch(
            &state,
            "agent.hibernate",
            json!({"surface_id": surface_id, "min_idle_ms": 0}),
        )
        .await
        .unwrap();

        assert_eq!(hibernated["surface"]["id"], surface_id);
        assert_eq!(hibernated["agent"], "codex");
        assert_eq!(hibernated["session_id"], "codex-session-1");
        assert_eq!(hibernated["lifecycle"], "suspended");
        assert_eq!(
            hibernated["argv"],
            json!(["codex", "resume", "codex-session-1"])
        );
        assert!(backend
            .surfaces()
            .unwrap()
            .iter()
            .all(|surface| surface.surface_id != surface_id));

        let model = state.model.lock().unwrap();
        let surface = model.surface(&surface_id).unwrap();
        assert_eq!(
            surface.agent_session.as_ref().unwrap().lifecycle,
            AgentSessionLifecycle::Suspended
        );
        assert_eq!(model.list_status(&surface.workspace_id).len(), 1);
        assert_eq!(
            model.list_status(&surface.workspace_id)[0].key,
            surface_status_key(&surface_id)
        );
        assert_eq!(
            model.list_status(&surface.workspace_id)[0].value,
            "Suspended"
        );
        assert!(model.list_progress(&surface.workspace_id).is_empty());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_hibernate_close_failure_rolls_back_visible_state() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailingCloseBackend::default());
        let mut state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        state.workflow_store_path = None;
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let surface_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let workspace_id = workspace.id.clone();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model
                .set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,));
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
            assert!(model.mark_surface_unread(&surface_id, true));
            assert!(model
                .set_status(
                    &workspace_id,
                    surface_status_key(&surface_id),
                    "Agent",
                    "Running",
                    Some("green".to_string()),
                )
                .is_some());
            assert!(model
                .set_status(&workspace_id, "agent:codex", "Codex", "Ready", None)
                .is_some());
            assert!(model
                .set_progress(
                    &workspace_id,
                    "agent:codex:tokens",
                    "Codex tokens",
                    42.0,
                    Some(100.0),
                )
                .is_some());
            surface_id
        };

        let err = dispatch(
            &state,
            "agent.hibernate",
            json!({"surface_id": surface_id, "min_idle_ms": 0}),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "error");
        assert!(err.to_string().contains("close failed"));
        assert!(backend
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id));

        let model = state.model.lock().unwrap();
        let surface = model.surface(&surface_id).unwrap();
        assert_eq!(
            surface.agent_session.as_ref().unwrap().lifecycle,
            AgentSessionLifecycle::Idle
        );
        assert_eq!(surface.agent_session.as_ref().unwrap().last_activity_ms, 1);
        assert!(surface.unread);
        let statuses = model.list_status(&surface.workspace_id);
        assert_eq!(statuses.len(), 2);
        assert!(statuses.iter().any(|status| {
            status.key == surface_status_key(&surface_id)
                && status.value == "Running"
                && status.color.as_deref() == Some("green")
        }));
        assert!(statuses
            .iter()
            .any(|status| status.key == "agent:codex" && status.value == "Ready"));
        let progress = model.list_progress(&surface.workspace_id);
        assert_eq!(progress.len(), 1);
        assert_eq!(progress[0].key, "agent:codex:tokens");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_hibernate_rejects_running_session_without_closing_backend() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let (state, backend) = test_state();
        let surface_id = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
            let surface_id = workspace.focused_surface_id.clone();
            assert!(model.set_surface_agent_session(
                &surface_id,
                AgentKind::Codex,
                "codex-session-1",
            ));
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
            surface_id
        };

        let err = dispatch(&state, "agent.hibernate", json!({"surface_id": surface_id}))
            .await
            .unwrap_err();

        assert_eq!(err.code(), "precondition_failed");
        assert!(err.to_string().contains("Only idle"));
        assert!(backend
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn agent_reclaim_hibernates_only_plan_candidates() {
        let dir = tempfile::tempdir().unwrap();
        write_fake_codex(dir.path());
        let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
        let (state, _backend) = test_state();
        let (candidate_surface_id, protected_surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.active_workspace().unwrap();
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
            assert!(model.set_surface_agent_session_last_activity_ms(&candidate_surface_id, 1));

            let protected_surface_id = model.add_tab(&candidate_surface_id).unwrap().id;
            assert!(model.set_surface_agent_session(
                &protected_surface_id,
                AgentKind::Codex,
                "codex-session-2",
            ));
            assert!(model.set_surface_agent_session_lifecycle(
                &protected_surface_id,
                AgentSessionLifecycle::Running,
            ));
            assert!(model.set_surface_agent_session_last_activity_ms(&protected_surface_id, 1));
            (candidate_surface_id, protected_surface_id)
        };

        let reclaimed = dispatch(
            &state,
            "agent.reclaim",
            json!({"min_idle_ms": 0, "limit": 5}),
        )
        .await
        .unwrap();

        assert_eq!(reclaimed["hibernated"].as_array().unwrap().len(), 1);
        assert_eq!(
            reclaimed["hibernated"][0]["surface"]["id"],
            candidate_surface_id
        );
        assert_eq!(reclaimed["failed"].as_array().unwrap().len(), 0);
        assert_eq!(
            reclaimed["protected"][0]["surface_id"],
            protected_surface_id
        );
        assert_eq!(reclaimed["protected"][0]["protect_reason"], "running");

        let model = state.model.lock().unwrap();
        assert_eq!(
            model
                .surface(&candidate_surface_id)
                .unwrap()
                .agent_session
                .as_ref()
                .unwrap()
                .lifecycle,
            AgentSessionLifecycle::Suspended
        );
        assert_eq!(
            model
                .surface(&protected_surface_id)
                .unwrap()
                .agent_session
                .as_ref()
                .unwrap()
                .lifecycle,
            AgentSessionLifecycle::Running
        );
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
            assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));
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
        assert_eq!(summary["agents"][0]["source"], "persisted_agent_session");
        assert!(summary["agents"][0]["age_ms"].as_u64().is_some());
        assert_eq!(
            summary["agents"][0]["lifecycle_evidence"]["status_key"],
            "agent:codex"
        );
        assert_eq!(
            summary["agents"][0]["lifecycle_evidence"]["status_value"],
            "Running"
        );
        assert_eq!(
            summary["agents"][0]["lifecycle_evidence"]["status_source"],
            "model"
        );
        assert_eq!(
            summary["agents"][0]["lifecycle_evidence"]["status_scope"],
            "workspace_provider"
        );
        assert_eq!(summary["status"][0]["key"], "agent:codex");
        assert_eq!(summary["status"][0]["value"], "Running");
        assert_eq!(summary["status"][0]["source"], "model");
        assert_eq!(summary["progress"][0]["key"], "build");
        assert_eq!(summary["progress"][0]["value"], 2.0);
        assert_eq!(summary["progress"][0]["source"], "model");

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
    #[serial_test::serial]
    async fn team_worker_launch_uses_requested_worktree_workspace() {
        let bin_dir = tempfile::tempdir().unwrap();
        let _codex = write_fake_program(bin_dir.path(), "codex");
        let _path = EnvGuard::set("PATH", bin_dir.path().to_str().unwrap());
        let (mut state, backend) = test_state();
        let team_store = tempfile::tempdir().unwrap();
        let worktree_dir = tempfile::tempdir().unwrap();
        state.team_store_path = Some(team_store.path().join("team-v1.json"));
        let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();
        let (worktree_workspace_id, worktree_surface_id) = {
            let mut model = state.model.lock().unwrap();
            let workspace = model.create_worktree_workspace(
                "feature",
                worktree_dir.path(),
                "feature",
                "feature-x",
            );
            (workspace.id, workspace.focused_surface_id)
        };

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": main_workspace_id,
                "name": "Launch",
            }),
        )
        .await
        .unwrap();

        let launched = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "worktree_name": "feature-x",
            }),
        )
        .await
        .unwrap();

        let launched_surface_id = launched["surface"]["id"].as_str().unwrap();
        assert_eq!(launched["worker"]["worktree_name"], "feature-x");
        assert_ne!(launched_surface_id, worktree_surface_id);
        assert_eq!(launched["surface"]["workspace_id"], worktree_workspace_id);
        let spawned = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .find(|surface| surface.surface_id == launched_surface_id)
            .unwrap();
        assert_eq!(spawned.cwd, worktree_dir.path());
    }

    #[tokio::test]
    async fn team_worker_launch_rejects_invalid_worktree_name() {
        let (mut state, _backend) = test_state();
        let team_store = tempfile::tempdir().unwrap();
        state.team_store_path = Some(team_store.path().join("team-v1.json"));
        let main = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let main_workspace_id = main[0]["id"].as_str().unwrap().to_string();

        dispatch(
            &state,
            "team.upsert",
            json!({
                "team_id": "team-1",
                "workspace_id": main_workspace_id,
                "name": "Launch",
            }),
        )
        .await
        .unwrap();

        let err = dispatch(
            &state,
            "team.worker.launch",
            json!({
                "team_id": "team-1",
                "worker_id": "worker-1",
                "agent": "codex",
                "worktree_name": "../escape",
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(err.code(), "invalid_param");
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
    async fn hook_permission_mode_reapplies_claude_bypass_resume_argv() {
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
            json!([
                "claude",
                "--dangerously-skip-permissions",
                "--resume",
                "claude-session-1"
            ])
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
            json!([
                "claude",
                "--dangerously-skip-permissions",
                "--resume",
                "claude-session-1"
            ])
        );
        assert_eq!(
            backend.spawn_args(resumed_surface_id).unwrap(),
            vec![
                "--dangerously-skip-permissions",
                "--resume",
                "claude-session-1"
            ]
        );
    }

    #[tokio::test]
    async fn hook_permission_mode_reapplies_codex_bypass_resume_argv() {
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
            json!([
                "codex",
                "--dangerously-bypass-approvals-and-sandbox",
                "resume",
                "-C",
                "/tmp",
                "codex-session-1"
            ])
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
            pid: None,
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
                pid: None,
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
    async fn workspace_close_restores_already_closed_surfaces_when_later_close_fails() {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(FailsSecondCloseBackend::default());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
        let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
        let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
        let first_surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
        let second = dispatch(
            &state,
            "surface.split",
            json!({"surface_id": first_surface_id, "axis": "horizontal"}),
        )
        .await
        .unwrap();
        let second_surface_id = second["id"].as_str().unwrap().to_string();

        let error = dispatch(&state, "workspace.close", json!({"id": workspace_id}))
            .await
            .unwrap_err();

        assert_eq!(error.code(), "error");
        assert!(error.to_string().contains("second close failed"));
        let model_surfaces = dispatch(
            &state,
            "surface.list",
            json!({"workspace_id": workspace_id}),
        )
        .await
        .unwrap();
        assert_eq!(model_surfaces.as_array().unwrap().len(), 2);
        let runtime_surfaces = backend
            .surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        assert!(runtime_surfaces.contains(&first_surface_id.to_string()));
        assert!(runtime_surfaces.contains(&second_surface_id));
        assert_eq!(runtime_surfaces.len(), 2);
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
    async fn worktree_create_preserves_preexisting_branch_when_spawn_fails() {
        // A branch can exist with no linked worktree (e.g. its worktree was
        // removed without deleting the branch). `create` adopts it, so a spawn
        // failure must roll back only the worktree it created and never delete
        // the user's pre-existing branch.
        let repo_dir = make_temp_repo();
        let branch_name = format!("topic/adopt-rollback-{}", std::process::id());
        {
            let repo = Repository::open(repo_dir.path()).unwrap();
            let head = repo.head().unwrap().peel_to_commit().unwrap();
            repo.branch(&branch_name, &head, false).unwrap();
        }
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
        // The rolled-back worktree is gone, but the pre-existing branch survives.
        assert!(worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .is_empty());
        let repo = Repository::open(repo_dir.path()).unwrap();
        assert!(repo
            .find_branch(&branch_name, git2::BranchType::Local)
            .is_ok());
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
    async fn worktree_socket_allows_cwd_from_open_surface() {
        let repo_dir = make_temp_repo();
        let (state, _) = test_state();
        {
            let mut model = state.model.lock().unwrap();
            let surface_id = model.active_workspace().unwrap().focused_surface_id.clone();
            assert!(model.set_surface_cwd(&surface_id, repo_dir.path().to_path_buf()));
        }

        let listed = dispatch(&state, "worktree.list", json!({"cwd": repo_dir.path()}))
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 0);

        let status = dispatch(&state, "worktree.status", json!({"path": repo_dir.path()}))
            .await
            .unwrap();
        assert_eq!(status["status"], "clean");
    }

    #[tokio::test]
    async fn worktree_socket_rejects_hook_reported_resume_cwd_for_unopened_repo() {
        let open_repo = make_temp_repo();
        let unopened_repo = make_temp_repo();
        let (state, _) = test_state();
        let workspace = dispatch(
            &state,
            "workspace.create",
            json!({"name": "open", "workingDir": open_repo.path()}),
        )
        .await
        .unwrap();
        let workspace_id = workspace["id"].as_str().unwrap();
        let surface_id = workspace["focused_surface_id"].as_str().unwrap();

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "hook_session_id": "spoofed-session",
                "hook_session_cwd": unopened_repo.path(),
            }),
        )
        .await
        .unwrap();

        let error = dispatch(
            &state,
            "worktree.list",
            json!({"cwd": unopened_repo.path()}),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "precondition_failed");
        assert!(error.to_string().contains("open workspace"));
    }

    #[tokio::test]
    async fn project_actions_list_and_run_from_open_repo_only() {
        let repo_dir = make_temp_repo();
        fs::write(
            repo_dir.path().join("forktty.json"),
            r#"{
                "actions": [
                    {
                        "id": "test",
                        "label": "Run tests",
                        "argv": ["./gradlew", "test"],
                        "cwd": "."
                    }
                ]
            }"#,
        )
        .unwrap();
        fs::write(repo_dir.path().join("gradlew"), "#!/bin/sh\n").unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            backend.clone(),
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();

        let listed = dispatch(
            &state,
            "project.action.list",
            json!({"cwd": repo_dir.path()}),
        )
        .await
        .unwrap();
        assert_eq!(listed[0]["id"], "test");
        assert_eq!(listed[0]["label"], "Run tests");

        let run = dispatch(
            &state,
            "project.action.run",
            json!({"cwd": repo_dir.path(), "id": "test"}),
        )
        .await
        .unwrap();
        let surface_id = run["surface_id"].as_str().unwrap();
        let gradlew = fs::canonicalize(repo_dir.path().join("gradlew")).unwrap();
        assert_eq!(run["argv"], json!([gradlew, "test"]));
        assert_eq!(
            backend.spawn_shell(surface_id).unwrap(),
            gradlew.to_string_lossy()
        );
        assert_eq!(backend.spawn_args(surface_id).unwrap(), vec!["test"]);
        assert_eq!(
            backend
                .surfaces()
                .unwrap()
                .into_iter()
                .find(|surface| surface.surface_id == surface_id)
                .unwrap()
                .cwd,
            repo_dir.path()
        );

        let unopened_repo = make_temp_repo();
        fs::write(
            unopened_repo.path().join("forktty.json"),
            r#"{"actions":[{"id":"x","label":"X","argv":["cargo","test"]}]}"#,
        )
        .unwrap();
        let err = dispatch(
            &state,
            "project.action.list",
            json!({"cwd": unopened_repo.path()}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "precondition_failed");
    }

    #[tokio::test]
    async fn project_actions_run_from_linked_worktree_authorized_repo() {
        let repo_dir = make_temp_repo();
        fs::write(
            repo_dir.path().join("forktty.json"),
            r#"{"actions":[{"id":"test","label":"Run tests","argv":["cargo","test"],"cwd":"."}]}"#,
        )
        .unwrap();
        let created = worktree::create(
            repo_dir.path().to_str().unwrap(),
            "topic/project-action-linked",
            "../forktty-worktrees/{name}",
        )
        .unwrap();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let backend = Arc::new(HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            backend,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, PathBuf::from(&created.path)).unwrap();

        let listed = dispatch(
            &state,
            "project.action.list",
            json!({"cwd": repo_dir.path()}),
        )
        .await
        .unwrap();
        assert_eq!(listed[0]["id"], "test");
        let run = dispatch(
            &state,
            "project.action.run",
            json!({"cwd": repo_dir.path(), "id": "test"}),
        )
        .await
        .unwrap();

        assert_eq!(run["argv"], json!(["cargo", "test"]));
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
            &path_resolver::worktree_layout(),
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
            pid: None,
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
            &path_resolver::worktree_layout(),
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
                pid: None,
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
                "invalid_param",
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
                "invalid_param",
                "Invalid parameter branch: expected string",
            ),
            (
                "worktree.attach",
                json!({"name": 42, "branch": "topic/socket"}),
                "invalid_param",
                "Invalid parameter name: expected string",
            ),
            (
                "worktree.attach",
                json!({"name": "topic/name", "branch": "topic/branch"}),
                "invalid_param",
                "Ambiguous worktree selector: cannot combine name and branch",
            ),
            (
                "worktree.remove",
                json!({"name": 42}),
                "invalid_param",
                "Invalid parameter name: expected string",
            ),
            (
                "worktree.merge",
                json!({"name": 42}),
                "invalid_param",
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
        for method in methods::capability_method_names() {
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
            assert_eq!(err.code(), "invalid_param");
            assert!(err.to_string().contains("Invalid parameter workspace_id"));
        }

        let err = dispatch(
            &state,
            "workspace.select",
            json!({"workspace_id": "workspace-1", "workspace_name": "main"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
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
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("Invalid parameter text"));

        let err = dispatch(
            &state,
            "surface.send_text",
            json!({"surface_id": surface_id, "text": ""}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
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
            assert_eq!(err.code(), "invalid_param");
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
        assert_eq!(err.code(), "invalid_param");
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
        let resolved =
            path_resolver::resolve_workspace_cwd_param(&json!({"workingDir": dir.path()})).unwrap();
        assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());

        let missing = dir.path().join("missing");
        let error = path_resolver::resolve_cwd_param(&json!({"cwd": missing})).unwrap_err();
        assert!(error.contains("cannot resolve path"));
    }

    #[test]
    fn rejects_ambiguous_directory_param_aliases() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        let workspace_error = path_resolver::resolve_workspace_cwd_param(&json!({
            "workingDir": first.path(),
            "cwd": second.path(),
        }))
        .unwrap_err();

        assert!(workspace_error.contains("Ambiguous path parameter"));
        assert!(workspace_error.contains("workingDir and cwd"));

        let repo_error = path_resolver::resolve_required_existing_dir_param(
            &json!({
                "path": first.path(),
                "cwd": second.path(),
            }),
            &["path", "cwd"],
            "path or cwd",
        )
        .unwrap_err();

        assert_eq!(repo_error.code(), "invalid_param");
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
        assert_eq!(response.error.unwrap().code, "payload_too_large");
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
        let providers = &result["provider_capabilities"];
        assert_eq!(providers["codex"]["team_worker_launch"], true);
        assert_eq!(providers["codex"]["safe_resume"], true);
        assert_eq!(providers["claude"]["cwd_resume_flag"], false);
        assert_eq!(providers["antigravity"]["program"], "agy");
        assert_eq!(providers["pi"]["program"], "pi");
        assert_eq!(providers["pi"]["safe_resume"], true);
        assert!(providers.get("gemini").is_none());
        #[cfg(not(feature = "browser"))]
        assert!(!methods.iter().any(|m| {
            m.as_str()
                .is_some_and(|method| method.starts_with("browser."))
        }));
        // Every advertised method except the connection-level events.subscribe
        // must resolve to a dispatch arm (not MethodNotFound).
        for method in methods::capability_method_names() {
            if method == "events.subscribe" {
                continue;
            }
            if let Err(DispatchError::MethodNotFound(_)) = dispatch(&state, method, json!({})).await
            {
                panic!("advertised method {method} has no dispatch handler");
            }
        }
    }

    #[test]
    fn method_registry_classifies_socket_exposure() {
        use methods::MethodExposure;

        let capability_methods = methods::capability_method_names();
        let mut seen = std::collections::BTreeSet::new();
        let mut all_specs = std::collections::BTreeSet::new();
        for spec in methods::method_specs() {
            assert!(
                all_specs.insert(spec.name),
                "duplicate method spec {}",
                spec.name
            );
        }
        for method in &capability_methods {
            assert!(seen.insert(*method), "duplicate capability method {method}");
            #[cfg(feature = "browser")]
            assert_ne!(
                methods::exposure(method),
                Some(MethodExposure::InternalOnly),
                "internal method advertised in capabilities: {method}"
            );
            assert!(
                method_allowed_from_socket(method),
                "capability method rejected by socket filter: {method}"
            );
        }
        assert_eq!(
            methods::exposure("events.subscribe"),
            Some(MethodExposure::ConnectionLevel)
        );
        assert!(method_allowed_from_socket("not.a.real.method"));

        #[cfg(feature = "browser")]
        {
            assert_eq!(
                methods::exposure("browser.open"),
                Some(MethodExposure::Public)
            );
            for method in [
                "browser.import.discover",
                "browser.import.preview",
                "browser.import.run",
            ] {
                assert_eq!(
                    methods::exposure(method),
                    Some(MethodExposure::InternalOnly)
                );
                assert!(!method_allowed_from_socket(method));
                assert!(
                    !capability_methods.contains(&method),
                    "internal method advertised in capabilities: {method}"
                );
            }
        }

        #[cfg(not(feature = "browser"))]
        {
            assert_eq!(methods::exposure("browser.open"), None);
            assert!(method_allowed_from_socket("browser.open"));
            assert!(!method_allowed_from_socket("browser.import.discover"));
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

    #[tokio::test]
    async fn events_subscribe_rejects_non_boolean_replay() {
        let (state, _backend) = test_state();
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(handle_connection(server, state));
        let (read_half, mut write_half) = client.into_split();

        write_half
            .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":"false"}}"#)
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
        assert_eq!(response.error.unwrap().code, "invalid_param");
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn events_subscribe_uses_separate_subscriber_budget() {
        let (state, _backend) = test_state();
        let event_limit = Arc::new(Semaphore::new(1));
        let (first_client, first_server) = tokio::net::UnixStream::pair().unwrap();
        let first_task = tokio::spawn(handle_connection_with_limits(
            first_server,
            state.clone(),
            RESPONSE_WRITE_TIMEOUT,
            event_limit.clone(),
        ));
        let (first_read, mut first_write) = first_client.into_split();
        first_write
            .write_all(br#"{"id":1,"method":"events.subscribe","params":{"replay":false}}"#)
            .await
            .unwrap();
        first_write.write_all(b"\n").await.unwrap();
        let mut first_reader = BufReader::new(first_read);
        let mut first_line = String::new();
        first_reader.read_line(&mut first_line).await.unwrap();
        assert!(first_line.contains("\"subscribed\""));

        let (second_client, second_server) = tokio::net::UnixStream::pair().unwrap();
        let second_task = tokio::spawn(handle_connection_with_limits(
            second_server,
            state,
            RESPONSE_WRITE_TIMEOUT,
            event_limit,
        ));
        let (second_read, mut second_write) = second_client.into_split();
        second_write
            .write_all(br#"{"id":2,"method":"events.subscribe","params":{"replay":false}}"#)
            .await
            .unwrap();
        second_write.write_all(b"\n").await.unwrap();
        second_write.shutdown().await.unwrap();
        let mut second_reader = BufReader::new(second_read);
        let mut second_line = String::new();
        second_reader.read_line(&mut second_line).await.unwrap();
        let response: JsonRpcResponse = serde_json::from_str(second_line.trim_end()).unwrap();

        assert!(!response.ok);
        assert_eq!(response.id, json!(2));
        assert_eq!(response.error.unwrap().code, "server_busy");
        second_task.await.unwrap().unwrap();
        drop(first_write);
        drop(first_reader);
        first_task.abort();
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
            assert_eq!(
                browser_profile::history_limit_from_params(&json!({})).unwrap(),
                100
            );
            assert_eq!(
                browser_profile::history_limit_from_params(&json!({"limit": null})).unwrap(),
                100
            );
            assert_eq!(
                browser_profile::history_limit_from_params(&json!({"limit": 5})).unwrap(),
                5
            );
            assert_eq!(
                browser_profile::history_limit_from_params(&json!({"limit": u64::MAX})).unwrap(),
                10_000
            );
            assert!(matches!(
                browser_profile::history_limit_from_params(&json!({"limit": "5"})),
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

        #[test]
        fn browser_import_spool_data_strips_cookie_values_but_keeps_counts() {
            let data = forktty_import::ImportedData {
                cookies: vec![forktty_import::ImportedCookie {
                    name: "sid".to_string(),
                    value: "secret-cookie-value".to_string(),
                    host: ".example.test".to_string(),
                    path: "/".to_string(),
                    expires: None,
                    secure: false,
                    http_only: true,
                }],
                visits: vec![forktty_import::ImportedVisit {
                    url: "https://example.test/".to_string(),
                    title: "Example".to_string(),
                    visit_count: 2,
                }],
                bookmarks: vec![forktty_import::ImportedBookmark {
                    url: "https://example.test/".to_string(),
                    title: "Example Bookmark".to_string(),
                }],
                result: forktty_import::ImportResult {
                    cookies: 1,
                    history: 1,
                    bookmarks: 1,
                    skipped: 0,
                },
            };

            let mut data_file = browser_import_spool_data(data).unwrap();
            data_file.seek(SeekFrom::Start(0)).unwrap();
            let mut serialized = String::new();
            std::io::Read::read_to_string(&mut data_file, &mut serialized).unwrap();
            assert!(!serialized.contains("secret-cookie-value"));
            assert!(serialized.contains("https://example.test/"));

            data_file.seek(SeekFrom::Start(0)).unwrap();
            let spooled: forktty_import::ImportedData =
                serde_json::from_reader(&mut data_file).unwrap();
            assert!(spooled.cookies.is_empty());
            assert_eq!(spooled.result.cookies, 1);
            assert_eq!(spooled.visits.len(), 1);
            assert_eq!(spooled.bookmarks.len(), 1);
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
                assert_eq!(err.code(), "invalid_param");
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

        assert_eq!(result["name"], "workspace-2");
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
    async fn remote_list_reports_ssh_workspaces_and_connection_state() {
        let (state, backend) = test_state();
        let result = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "user@example.com", "name": "prod", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let surface_id = result["focused_surface_id"].as_str().unwrap().to_string();

        let remotes = dispatch(&state, "remote.list", json!({})).await.unwrap();
        assert_eq!(remotes.as_array().unwrap().len(), 1);
        assert_eq!(remotes[0]["host"], "user@example.com");
        assert_eq!(remotes[0]["workspace_name"], "prod");
        assert_eq!(remotes[0]["surface_id"], surface_id);
        assert_eq!(remotes[0]["connected"], true);

        backend.close(&surface_id).unwrap();
        let remotes = dispatch(&state, "remote.list", json!({})).await.unwrap();
        assert_eq!(remotes[0]["connected"], false);
    }

    #[tokio::test]
    async fn remote_status_uses_selected_or_active_ssh_surface() {
        let (state, _backend) = test_state();
        let result = dispatch(
            &state,
            "workspace.create_ssh",
            json!({"host": "server.local", "name": "remote", "workingDir": "/tmp"}),
        )
        .await
        .unwrap();
        let surface_id = result["focused_surface_id"].as_str().unwrap();

        let status = dispatch(&state, "remote.status", json!({"surface_id": surface_id}))
            .await
            .unwrap();
        assert_eq!(status["host"], "server.local");

        let status = dispatch(&state, "remote.status", json!({})).await.unwrap();
        assert_eq!(status["surface_id"], surface_id);

        let err = dispatch(
            &state,
            "remote.status",
            json!({"surface_id": "missing-surface"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err.code(), "not_found");
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
