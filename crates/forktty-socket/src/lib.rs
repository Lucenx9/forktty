//! Owner-only Unix socket JSON-RPC server, method dispatcher, and shared runtime state.

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
mod dispatcher;
mod errors;
mod hook_session;
mod metadata_helpers;
mod metadata_params;
mod metadata_runtime;
mod methods;
mod notification_dispatch;
mod notification_view;
mod param_helpers;
mod path_resolver;
mod project_action_params;
mod project_action_runtime;
mod remote;
mod response_encoding;
mod socket_bind;
mod status_runtime;
mod surface_lifecycle;
mod surface_runtime;
mod system_runtime;
mod terminal_text_params;
mod topology_params;
mod topology_runtime;
mod topology_view;
mod unix_connect;
mod workspace_runtime;
mod worktree_params;
mod worktree_runtime;

pub(crate) use agent_runtime::{
    agent_health_rows, agent_session_identify_row, agent_session_lifecycle_from_hook,
    agent_session_rows,
};
#[cfg(test)]
pub(crate) use agent_runtime::{
    agent_health_rows_with_path, agent_reclaim_plan_with_path, surface_status_key,
};
#[cfg(all(test, feature = "browser"))]
use browser_import::browser_import_spool_data;
#[cfg(all(test, feature = "browser"))]
use browser_import_params::browser_import_source_id;
use connection::{
    handle_connection_with_event_limit, reject_over_capacity_connection_until_shutdown,
    ConnectionControl,
};
pub(crate) use context_runtime::workspace_effective_project_cwd;
#[cfg(test)]
pub(crate) use context_runtime::{context_snapshot_risk_flags, ContextSnapshotRiskInputs};
use coordinator::SocketCoordinator;
pub use coordinator::{
    AutoSpawnSuppressionGuard, SurfaceSetGuard, WorktreeReadGuard, WorktreeWriteGuard,
};
use forktty_core::events::{self, ModelEvent, Snapshot};
use forktty_core::protocol_limits;
#[cfg(test)]
use forktty_core::worktree;
#[cfg(test)]
use forktty_core::AgentKind;
#[cfg(test)]
use forktty_core::AgentSessionLifecycle;
#[cfg(test)]
use forktty_core::JsonRpcResponse;
#[cfg(all(test, feature = "browser"))]
use forktty_core::MAX_BROWSER_URL_BYTES;
use forktty_core::{BrowserCommand, WorkspaceModel};
use forktty_terminal::SharedTerminalBackend;
#[cfg(test)]
use forktty_terminal::SpawnRequest;
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
use std::future::Future;
use std::io;
#[cfg(all(test, feature = "browser"))]
use std::io::{Seek, SeekFrom};
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener as StdUnixListener;
#[cfg(test)]
use std::os::unix::net::UnixStream as StdUnixStream;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use tokio::io::AsyncBufRead;
use tokio::net::UnixListener;
use tokio::sync::{broadcast, watch, Semaphore};
use tokio::task::JoinSet;

#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct ServerShutdownTestHooks {
    connection_accepted: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    shutdown_started: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    dispatch_pause: Option<Arc<DispatchAdmissionTestPause>>,
    pre_admission_error_pause: Option<Arc<PreAdmissionErrorTestPause>>,
    partial_bytes_consumed: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    buffered_followup: Option<tokio::sync::mpsc::UnboundedSender<bool>>,
}

#[cfg(not(test))]
#[derive(Clone, Default)]
struct ServerShutdownTestHooks {
    _private: (),
}

#[cfg(test)]
pub(crate) struct DispatchAdmissionTestPause {
    pause_next: AtomicBool,
    admitted: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl DispatchAdmissionTestPause {
    async fn pause_after_admission(&self) {
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.admitted.wait().await;
            self.release.notified().await;
        }
    }
}

#[cfg(test)]
pub(crate) struct PreAdmissionErrorTestPause {
    pause_next: AtomicBool,
    classified: Arc<tokio::sync::Barrier>,
    release: Arc<tokio::sync::Notify>,
}

#[cfg(test)]
impl PreAdmissionErrorTestPause {
    async fn pause_after_classification(&self) {
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.classified.wait().await;
            self.release.notified().await;
        }
    }
}

#[cfg(test)]
mod env_access;

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
pub use dispatcher::dispatch;
pub(crate) use dispatcher::method_allowed_from_socket;
pub use errors::{DispatchError, SocketError};
pub(crate) use metadata_helpers::{
    agent_kind_from_permission_status_key, agent_kind_from_status_key, log_level_from_params,
    notification_body_from_params, notification_kind_from_params, notification_title_from_params,
    optional_hook_status_metadata, resolve_notification_target, resolve_workspace_id_for_metadata,
    status_color_from_params,
};
#[cfg(test)]
pub(crate) use param_helpers::MAX_METADATA_TEXT_BYTES;
pub(crate) use param_helpers::{
    ensure_max_text_size, format_param_names, optional_bool_param, optional_f64,
    optional_non_blank_string_param, optional_surface_id_param, optional_u64_param,
    optional_workspace_create_name_from_params, required_f64, required_string,
    required_string_param, required_surface_id, required_trimmed_string, split_axis_from_params,
    workspace_selector_from_params, workspace_selector_params,
};
pub use remote::ready_surface_ids;
use socket_bind::verify_peer_credentials;
pub use socket_bind::{bind_socket_listener, default_socket_path, socket_path_from_env};
#[cfg(test)]
pub(crate) use socket_bind::{
    default_socket_dir_from_env, effective_uid, peer_uid_allowed,
    probe_forktty_socket_with_timeout, PROBE_RESPONSE_MAX_BYTES,
};
pub use surface_lifecycle::{
    bootstrap_default_workspace, deferred_surface_creation_failure_handler,
    evict_hook_session_targets_for_surfaces, resolve_ssh_binary, session_data_from_state,
    spawn_request_for_surface, spawn_request_for_surface_kind, sync_live_surface_cwds,
    PersistedSurfaceSpawnError, SurfaceCreationLayoutSnapshot,
};
pub(crate) use surface_lifecycle::{
    close_replacement_terminal_surface_if_present, close_surface_request,
    close_terminal_surface_if_present, close_terminal_surfaces_or_restore, current_model_surfaces,
    ensure_model_surface_exists, ensure_terminal_for_active_workspace,
    ensure_terminal_for_active_workspace_now, record_terminal_spawn_failure_for_completion,
    required_ssh_host_param, restore_terminal_surfaces_after_failure,
    rollback_replacement_if_redundant, rollback_surface_creation, rollback_workspace_creation,
    spawn_surface_terminal, spawn_surface_terminal_with_failure_handler, spawn_workspace_terminal,
    surface_effective_project_cwd,
};
#[cfg(test)]
pub(crate) use surface_lifecycle::{
    restore_current_terminal_surfaces_after_failure, spawn_terminal_surfaces,
};
pub(crate) use terminal_text_params::{
    terminal_tail_lines_from_params, terminal_text_capture_from_params,
    terminal_text_max_bytes_from_params, MAX_CAPTURE_TAIL_LINES, MAX_TERMINAL_TEXT_BYTES,
};
pub use unix_connect::connect_unix_stream_with_timeout;
#[cfg(test)]
pub(crate) use unix_connect::unix_socket_address;
pub use worktree_runtime::{
    finish_prepared_worktree_removal, open_worktree_transaction, open_worktree_workspace,
    remove_worktree_transaction, rollback_created_worktree_after_runtime_failure,
    run_guarded_worktree_read, run_guarded_worktree_write,
};

const MAX_REQUEST_SIZE: usize = protocol_limits::SOCKET_REQUEST_MAX_BYTES;
const MAX_SEND_TEXT_BYTES: usize = protocol_limits::SOCKET_SEND_TEXT_MAX_BYTES;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_LINES: usize = 40;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES: usize =
    protocol_limits::DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES;
const MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS: usize = 100;
const MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES: usize = 16;
const MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES: usize =
    protocol_limits::MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
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

/// Opaque capability for a worktree transaction that may change terminal
/// topology.
///
/// This capability owns both the worktree write guard and the subsequently
/// acquired surface-set guard. Passing it to worktree runtime helpers therefore
/// makes their mandatory lock ordering a type-level caller requirement and
/// allows destructive work to retain both guards after a caller is cancelled.
#[must_use = "dropping the transaction ends surface-set coordination"]
pub struct WorktreeSurfaceTransaction {
    state: SocketAppState,
    _worktree_guard: WorktreeWriteGuard,
    _surface_set_guard: SurfaceSetGuard,
}

impl WorktreeSurfaceTransaction {
    pub(crate) fn state(&self) -> &SocketAppState {
        &self.state
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
    desktop_notification_closer: Arc<dyn Fn(&str) + Send + Sync>,
    /// Broadcast channel feeding `events.subscribe` connections. The background
    /// tick task in [`serve`] is the sole producer.
    pub events: broadcast::Sender<ModelEvent>,
    /// Sends scripting commands to the GTK WebView. `None` when no browser
    /// engine is wired (no `browser` feature, or headless), in which case the
    /// browser scripting verbs report unavailable.
    pub browser_cmd: Option<async_channel::Sender<BrowserCommand>>,
    hook_session_targets: Arc<Mutex<hook_session::HookSessionTargets>>,
    hook_target_gates: Arc<Mutex<hook_session::HookTargetGates>>,
    #[cfg(test)]
    panic_after_worktree_filesystem_finish: Arc<AtomicU8>,
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
            notification_dispatch: true,
            desktop_notification_closer: Arc::new(forktty_core::close_desktop_notification),
            events,
            browser_cmd: None,
            hook_session_targets: Arc::new(Mutex::new(hook_session::HookSessionTargets::default())),
            hook_target_gates: Arc::new(Mutex::new(hook_session::HookTargetGates::default())),
            #[cfg(test)]
            panic_after_worktree_filesystem_finish: Arc::new(AtomicU8::new(0)),
            coordinator: Arc::new(SocketCoordinator::default()),
        }
    }

    pub fn with_notification_dispatch(mut self, enabled: bool) -> Self {
        self.notification_dispatch = enabled;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_desktop_notification_closer<F>(mut self, closer: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.desktop_notification_closer = Arc::new(closer);
        self
    }

    pub(crate) fn close_desktop_notification(&self, notification_id: &str) {
        (self.desktop_notification_closer)(notification_id);
    }

    pub fn with_browser_cmd(mut self, sender: async_channel::Sender<BrowserCommand>) -> Self {
        self.browser_cmd = Some(sender);
        self
    }

    #[cfg(test)]
    pub(crate) fn panic_after_worktree_filesystem_finish_and_poison_model_once(&self) {
        self.panic_after_worktree_filesystem_finish
            .store(2, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn panic_after_worktree_filesystem_finish_if_requested(&self) {
        if self
            .panic_after_worktree_filesystem_finish
            .swap(0, Ordering::SeqCst)
            == 2
        {
            let _model = self.model.lock().unwrap();
            panic!("injected panic after worktree filesystem finish with model poison");
        }
    }

    #[cfg(not(test))]
    fn panic_after_worktree_filesystem_finish_if_requested(&self) {}

    /// Acquire shared process-local access to worktree discovery and reads.
    ///
    /// Acquire this before [`Self::surface_set_guard`] when both are needed.
    pub async fn worktree_read_guard(&self) -> WorktreeReadGuard {
        self.coordinator.worktree_read_guard().await
    }

    /// Acquire exclusive process-local access to a worktree transaction.
    ///
    /// Create, attach, remove, and merge operations retain this through either
    /// commit or complete rollback. Acquire it before
    /// [`Self::surface_set_guard`] when both are needed.
    pub async fn worktree_write_guard(&self) -> WorktreeWriteGuard {
        self.coordinator.worktree_write_guard().await
    }

    /// Try to acquire exclusive worktree access without waiting.
    ///
    /// Returns `None` while another worktree read or write transaction is
    /// active. GTK session autosave uses this (with
    /// [`Self::try_surface_set_guard`]) so a mid-transaction snapshot cannot
    /// run `repair_session_invariants` against a half-removed worktree.
    pub fn try_worktree_write_guard(&self) -> Option<WorktreeWriteGuard> {
        self.coordinator.try_worktree_write_guard()
    }

    /// Serialize changes to the modeled and runtime terminal surface set.
    pub async fn surface_set_guard(&self) -> SurfaceSetGuard {
        self.coordinator.surface_set_guard().await
    }

    /// Try to serialize a synchronous surface reconciliation without waiting.
    ///
    /// Returns `None` while another model/runtime surface-set transaction is
    /// active. GTK refresh paths use this to defer reconciliation instead of
    /// blocking the main loop or deadlocking on a guard they already own.
    pub fn try_surface_set_guard(&self) -> Option<SurfaceSetGuard> {
        self.coordinator.try_surface_set_guard()
    }

    /// Enter the surface-changing phase of an exclusive worktree transaction.
    ///
    /// The returned capability takes ownership of `worktree_guard`, so the
    /// write guard cannot be dropped before the surface-set guard. It is
    /// accepted by the shared socket/GTK worktree runtime helpers that require
    /// both guards.
    ///
    /// # Panics
    ///
    /// Panics if `worktree_guard` came from an unrelated app-state
    /// coordinator. Guards acquired from clones of this state are accepted.
    pub async fn worktree_surface_transaction(
        &self,
        worktree_guard: WorktreeWriteGuard,
    ) -> WorktreeSurfaceTransaction {
        assert!(
            worktree_guard.belongs_to(&self.coordinator),
            "worktree write guard belongs to another SocketAppState coordinator"
        );
        let surface_set_guard = self.surface_set_guard().await;
        WorktreeSurfaceTransaction {
            state: self.clone(),
            _worktree_guard: worktree_guard,
            _surface_set_guard: surface_set_guard,
        }
    }

    pub(crate) fn blocking_worktree_surface_transaction(
        &self,
        worktree_guard: WorktreeWriteGuard,
    ) -> WorktreeSurfaceTransaction {
        assert!(
            worktree_guard.belongs_to(&self.coordinator),
            "worktree write guard belongs to another SocketAppState coordinator"
        );
        let surface_set_guard = self.coordinator.blocking_surface_set_guard();
        WorktreeSurfaceTransaction {
            state: self.clone(),
            _worktree_guard: worktree_guard,
            _surface_set_guard: surface_set_guard,
        }
    }

    /// Suppress controller auto-spawn for each unique surface ID until drop.
    ///
    /// Nested registrations are reference-counted across cloned app states.
    pub fn suppress_surface_auto_spawn<I, S>(&self, surface_ids: I) -> AutoSpawnSuppressionGuard
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.coordinator.suppress_surface_auto_spawn(surface_ids)
    }

    /// Snapshot surface IDs whose controller auto-spawn is currently disabled.
    ///
    /// This synchronous snapshot is intended to be collected before locking
    /// the workspace model during GTK reconciliation.
    pub fn suppressed_auto_spawn_surface_ids(&self) -> std::collections::BTreeSet<String> {
        self.coordinator.suppressed_auto_spawn_surface_ids()
    }
}

pub async fn serve(listener: StdUnixListener, state: SocketAppState) -> Result<(), SocketError> {
    serve_until_shutdown(listener, state, std::future::pending()).await
}

/// Serve socket connections until `shutdown` resolves.
///
/// The plain [`serve`] wrapper is intentionally endless for tests and
/// embeddings that own the process lifetime. GUI hosts should use this variant
/// so closing the last window can stop the socket thread cleanly.
pub async fn serve_until_shutdown(
    listener: StdUnixListener,
    state: SocketAppState,
    shutdown: impl Future<Output = ()>,
) -> Result<(), SocketError> {
    serve_until_shutdown_inner(
        listener,
        state,
        shutdown,
        ServerShutdownTestHooks::default(),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn serve_until_shutdown_with_hooks(
    listener: StdUnixListener,
    state: SocketAppState,
    shutdown: impl Future<Output = ()>,
    hooks: ServerShutdownTestHooks,
) -> Result<(), SocketError> {
    serve_until_shutdown_inner(listener, state, shutdown, hooks).await
}

async fn serve_until_shutdown_inner(
    listener: StdUnixListener,
    state: SocketAppState,
    shutdown: impl Future<Output = ()>,
    hooks: ServerShutdownTestHooks,
) -> Result<(), SocketError> {
    let listener = UnixListener::from_std(listener)?;
    spawn_event_tick(state.clone());
    let connection_limit = Arc::new(Semaphore::new(MAX_SOCKET_CONNECTIONS));
    let event_subscription_limit = Arc::new(Semaphore::new(MAX_EVENT_SUBSCRIBERS));
    let dispatch_admission = Arc::new(AtomicBool::new(true));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut connections = JoinSet::new();
    let mut server_result = Ok(());
    tokio::pin!(shutdown);
    'accept: loop {
        let stream = tokio::select! {
            biased;
            _ = &mut shutdown => {
                begin_cooperative_shutdown(&dispatch_admission, &shutdown_tx, &hooks);
                break 'accept;
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                report_connection_join(joined);
                continue;
            }
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _)) => {
                        #[cfg(test)]
                        if let Some(accepted) = &hooks.connection_accepted {
                            let _ = accepted.send(());
                        }
                        stream
                    },
                    // A client aborting mid-handshake (or a transient kernel hiccup)
                    // must not take the whole IPC server down for the rest of the
                    // process lifetime; only give up on genuinely fatal errors.
                    Err(err) if is_transient_accept_error(&err) => {
                        // Brief pause so accept() does not hot-spin while fds are
                        // exhausted (EMFILE/ENFILE persists until something closes).
                        tokio::select! {
                            biased;
                            _ = &mut shutdown => {
                                begin_cooperative_shutdown(
                                    &dispatch_admission,
                                    &shutdown_tx,
                                    &hooks,
                                );
                                break 'accept;
                            },
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        }
                        continue;
                    }
                    Err(err) => {
                        begin_cooperative_shutdown(&dispatch_admission, &shutdown_tx, &hooks);
                        server_result = Err(err.into());
                        break 'accept;
                    },
                }
            }
        };
        // The 0600 socket mode is the primary access barrier, but unix(7)
        // explicitly warns that socket-file permissions are not a portable
        // security guarantee; SO_PEERCRED (captured at connect time, not
        // spoofable) keeps foreign-uid peers out even if the path or its
        // parent directory ever ends up more exposed than intended (e.g. the
        // world-writable /tmp fallback used without XDG_RUNTIME_DIR).
        if let Err(reason) = verify_peer_credentials(&stream) {
            eprintln!("forktty socket: rejected connection: {reason}");
            continue;
        }
        let state = state.clone();
        let event_subscription_limit = event_subscription_limit.clone();
        let control = ConnectionControl::new(
            shutdown_rx.clone(),
            dispatch_admission.clone(),
            #[cfg(test)]
            hooks.dispatch_pause.clone(),
            #[cfg(test)]
            hooks.pre_admission_error_pause.clone(),
            #[cfg(test)]
            hooks.partial_bytes_consumed.clone(),
            #[cfg(test)]
            hooks.buffered_followup.clone(),
        );
        let Ok(permit) = connection_limit.clone().try_acquire_owned() else {
            connections.spawn(async move {
                reject_over_capacity_connection_until_shutdown(stream, control).await;
            });
            continue;
        };
        connections.spawn(async move {
            let _permit = permit;
            if let Err(err) =
                handle_connection_with_event_limit(stream, state, event_subscription_limit, control)
                    .await
            {
                // We can't return errors to a client whose connection has
                // already dropped, but the operator should still see the
                // underlying I/O or JSON failure on stderr.
                eprintln!("forktty socket connection ended with error: {err}");
            }
        });
    }

    // `JoinSet` aborts all remaining tasks when dropped. Cooperative shutdown
    // must instead let admitted dispatches finish, so every exit path drains
    // the owned set explicitly.
    while let Some(joined) = connections.join_next().await {
        report_connection_join(Some(joined));
    }
    server_result
}

fn begin_cooperative_shutdown(
    dispatch_admission: &AtomicBool,
    shutdown: &watch::Sender<bool>,
    hooks: &ServerShutdownTestHooks,
) {
    dispatch_admission.store(false, Ordering::SeqCst);
    shutdown.send_replace(true);
    #[cfg(not(test))]
    let _ = hooks;
    #[cfg(test)]
    if let Some(started) = &hooks.shutdown_started {
        let _ = started.send(());
    }
}

fn report_connection_join(joined: Option<Result<(), tokio::task::JoinError>>) {
    if let Some(Err(err)) = joined {
        eprintln!("forktty socket connection task failed: {err}");
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

fn current_unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use forktty_core::validate_worktree_name;
    use forktty_terminal::{
        DeferredSpawnFailureHandler, HeadlessTerminalBackend, TerminalBackend, TerminalError,
        TerminalSurfaceState, TerminalTextCapture, TerminalTextSnapshot,
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

    mod agent_session;
    #[cfg(feature = "browser")]
    mod browser;
    mod context_snapshot;
    mod event_stream;
    mod hook_ingress;
    mod metadata;
    mod metadata_hooks;
    mod notification_feed;
    mod protocol_dispatch;
    mod remote;
    mod server_shutdown;
    mod socket_bind;
    mod spawn_request;
    mod surface_pane;
    mod system;
    mod test_runtime;
    mod workspace_surface;
    mod worktree_project;
    mod worktree_removal;

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
    struct DeferredSpawnFailureBackend {
        failure_handlers: Mutex<Vec<DeferredSpawnFailureHandler>>,
    }

    impl DeferredSpawnFailureBackend {
        fn fail_next_spawn(&self) {
            self.failure_handlers.lock().unwrap().remove(0).run();
        }

        fn pending_failure_count(&self) -> usize {
            self.failure_handlers.lock().unwrap().len()
        }
    }

    impl TerminalBackend for DeferredSpawnFailureBackend {
        fn spawn(&self, _request: SpawnRequest) -> Result<(), TerminalError> {
            Ok(())
        }

        fn spawn_with_failure_handler(
            &self,
            _request: SpawnRequest,
            failure_handler: DeferredSpawnFailureHandler,
        ) -> Result<(), TerminalError> {
            self.failure_handlers.lock().unwrap().push(failure_handler);
            Ok(())
        }

        fn send_text(&self, _surface_id: &str, _text: &str) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _surface_id: &str, _cols: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn close(&self, _surface_id: &str) -> Result<(), TerminalError> {
            Ok(())
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
}
