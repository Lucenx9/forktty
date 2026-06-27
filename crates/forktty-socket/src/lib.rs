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
mod feed_events;
mod feed_params;
mod feed_runtime;
mod feed_view;
mod hook_session;
mod metadata_helpers;
mod metadata_params;
mod metadata_runtime;
mod methods;
mod notification_dispatch;
mod param_helpers;
mod path_resolver;
mod project_action_params;
mod project_action_runtime;
mod provider_runtime;
mod remote;
mod response_encoding;
mod socket_bind;
mod status_runtime;
mod store_access;
mod surface_lifecycle;
mod surface_runtime;
mod system_runtime;
mod team_dispatch;
mod team_params;
mod team_provider;
mod team_runtime;
mod team_state;
mod terminal_text_params;
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
use forktty_core::AgentKind;
#[cfg(test)]
use forktty_core::AgentSessionLifecycle;
#[cfg(test)]
use forktty_core::JsonRpcResponse;
#[cfg(test)]
use forktty_core::SplitAxis;
#[cfg(all(test, feature = "browser"))]
use forktty_core::MAX_BROWSER_URL_BYTES;
use forktty_core::{
    BrowserCommand, FeedApprovalState, FeedStore, NotificationItem, NotificationKind,
    WorkspaceModel,
};
#[cfg(test)]
use forktty_core::{FeedEntry, FeedEntryType};
use forktty_terminal::SharedTerminalBackend;
#[cfg(test)]
use forktty_terminal::SpawnRequest;
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use serde_json::Value;
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
#[cfg(test)]
use tokio::io::AsyncBufRead;
use tokio::net::UnixListener;
use tokio::sync::{broadcast, Semaphore};

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
    optional_limit_param, optional_non_blank_string_param, optional_string_array_param,
    optional_string_param, optional_surface_id_param, optional_u64_param,
    optional_workspace_create_name_from_params, required_f64, required_string,
    required_string_param, required_surface_id, required_trimmed_string, split_axis_from_params,
    workspace_selector_from_params, workspace_selector_params, WorkspaceSelectorKind,
};
pub use socket_bind::{bind_socket_listener, default_socket_path, socket_path_from_env};
#[cfg(test)]
pub(crate) use socket_bind::{
    default_socket_dir_from_env, effective_uid, probe_forktty_socket_with_timeout,
    PROBE_RESPONSE_MAX_BYTES,
};
pub use surface_lifecycle::{
    bootstrap_default_workspace, resolve_ssh_binary, spawn_request_for_surface,
    spawn_request_for_surface_kind,
};
pub(crate) use surface_lifecycle::{
    close_replacement_terminal_surface_if_present, close_surface_request,
    close_terminal_surface_if_present, close_terminal_surfaces_or_restore,
    ensure_model_surface_exists, ensure_terminal_for_active_workspace,
    evict_hook_session_targets_for_surfaces, required_ssh_host_param,
    rollback_replacement_if_redundant, rollback_surface_creation, rollback_workspace_creation,
    spawn_surface_terminal, spawn_terminal_surfaces, spawn_workspace_terminal,
    surface_effective_project_cwd,
};
#[cfg(test)]
pub(crate) use team_provider::{team_worker_launch_command, CLAUDE_TEAM_REVIEW_ALLOWED_TOOLS};
pub(crate) use terminal_text_params::{
    terminal_tail_lines_from_params, terminal_text_capture_from_params,
    terminal_text_max_bytes_from_params, MAX_CAPTURE_TAIL_LINES, MAX_TERMINAL_TEXT_BYTES,
};

const MAX_REQUEST_SIZE: usize = protocol_limits::SOCKET_REQUEST_MAX_BYTES;
const MAX_SEND_TEXT_BYTES: usize = protocol_limits::SOCKET_SEND_TEXT_MAX_BYTES;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_LINES: usize = 40;
const DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES: usize =
    protocol_limits::DEFAULT_CONTEXT_SNAPSHOT_TAIL_MAX_BYTES;
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
const DEFAULT_TEAM_WORKER_STALE_MS: u64 = 5 * 60 * 1_000;

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

    mod agent_session;
    mod context_snapshot;
    mod event_stream;
    mod metadata;
    mod metadata_hooks;
    mod notification_feed;
    mod protocol_dispatch;
    mod remote;
    mod socket_bind;
    mod spawn_request;
    mod surface_pane;
    mod system;
    mod team_health_finish;
    mod team_message_dispatch;
    mod team_provider;
    mod team_worker_runtime;
    mod workflow;
    mod workspace_surface;
    mod worktree_project;

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
}
