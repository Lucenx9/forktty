//! GTK app regression tests for controller behavior, styling, dialogs, and renderer helpers.

use super::*;

use git2::Repository;

mod agent_hud;
mod backend_io;
mod backend_runtime;
mod design_system;
mod notifications;
mod restart_and_restore;
mod sidebar;
mod surface_actions;
mod tab_drag;
mod terminal_config;
mod terminal_lifecycle;
mod workspace_navigation;
mod worktree_actions;

fn make_temp_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("note.txt"), "base\n").unwrap();
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

#[cfg(target_os = "linux")]
struct SleepingTestChild(std::process::Child);

#[cfg(target_os = "linux")]
impl SleepingTestChild {
    fn spawn_in(cwd: &Path) -> Self {
        Self(
            Command::new("/bin/sleep")
                .arg("60")
                .current_dir(cwd)
                .spawn()
                .unwrap(),
        )
    }

    fn id(&self) -> u32 {
        self.0.id()
    }
}

#[cfg(target_os = "linux")]
impl Drop for SleepingTestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn create_local_branch(repo_dir: &Path, branch_name: &str) {
    let repo = Repository::open(repo_dir).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(branch_name, &head, false).unwrap();
}

fn test_spawn_request() -> SpawnRequest {
    SpawnRequest {
        surface_id: "surface-1".to_string(),
        workspace_id: "workspace-1".to_string(),
        shell: "/bin/sh".to_string(),
        args: Vec::new(),
        cwd: PathBuf::from("/tmp"),
        socket_path: PathBuf::from("/tmp/forktty.sock"),
        extra_env: Vec::new(),
        eligible_for_pty_persistence: false,
    }
}

fn drive_backend_send_text(
    backend: &GtkTerminalBackend,
    rx: &mpsc::Receiver<GtkTerminalCommand>,
    expected_surface_id: &str,
    expected_text: &str,
    reply_result: Result<(), TerminalError>,
) -> Result<(), String> {
    let backend = backend.clone();
    let surface_id = expected_surface_id.to_string();
    let text = expected_text.to_string();
    let (result_tx, result_rx) = mpsc::channel();
    let sender = std::thread::spawn(move || {
        let result = backend
            .send_text(&surface_id, &text)
            .map_err(|err| err.to_string());
        result_tx.send(result).unwrap();
    });

    let command = rx
        .recv_timeout(Duration::from_secs(1))
        .expect("send-text command should be queued");
    match command {
        GtkTerminalCommand::SendText {
            surface_id,
            text,
            reply,
            ..
        } => {
            assert_eq!(surface_id, expected_surface_id);
            assert_eq!(text, expected_text);
            assert!(
                result_rx.recv_timeout(Duration::from_millis(50)).is_err(),
                "send_text returned before the GTK controller replied"
            );
            reply.send(reply_result).unwrap();
        }
        _ => panic!("expected send-text command"),
    }

    let result = result_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("send_text should return after controller reply");
    sender.join().unwrap();
    result
}

#[derive(Debug, Default)]
struct SecondSpawnFailsBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    spawn_count: Mutex<usize>,
}

impl TerminalBackend for SecondSpawnFailsBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let mut spawn_count = self
            .spawn_count
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        *spawn_count += 1;
        if *spawn_count > 1 {
            return Err(TerminalError::Backend("spawn failed".to_string()));
        }
        drop(spawn_count);
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
struct CloseFailsBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
}

impl TerminalBackend for CloseFailsBackend {
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

#[derive(Debug, Default)]
struct SecondCloseFailsBackend {
    inner: forktty_terminal::HeadlessTerminalBackend,
    close_count: Mutex<usize>,
}

impl TerminalBackend for SecondCloseFailsBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        self.inner.spawn(request)
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        self.inner.send_text(surface_id, text)
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        self.inner.resize(surface_id, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let mut close_count = self
            .close_count
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?;
        *close_count += 1;
        if *close_count == 2 {
            return Err(TerminalError::Backend("second close failed".to_string()));
        }
        drop(close_count);
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[derive(Debug)]
struct CloseObservesModelLockBackend {
    surfaces: Mutex<BTreeMap<String, TerminalSurfaceState>>,
    model: Arc<Mutex<WorkspaceModel>>,
    observed_model_unlocked: Mutex<bool>,
}

impl CloseObservesModelLockBackend {
    fn new(model: Arc<Mutex<WorkspaceModel>>) -> Self {
        Self {
            surfaces: Mutex::new(BTreeMap::new()),
            model,
            observed_model_unlocked: Mutex::new(false),
        }
    }

    fn observed_model_unlocked(&self) -> bool {
        *self.observed_model_unlocked.lock().unwrap()
    }
}

impl TerminalBackend for CloseObservesModelLockBackend {
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
        if self.model.try_lock().is_ok() {
            *self.observed_model_unlocked.lock().unwrap() = true;
        }
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

#[test]
fn update_stamp_waits_24h_and_honors_rate_limit_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let stamp = dir.path().join("update-check.json");
    let now = 1_800_000_000_000;

    assert!(update_check_due_at(&stamp, now));
    record_update_attempt_at(&stamp, now, Some(now + 3_600_000)).unwrap();

    assert!(!update_check_due_at(&stamp, now + 3_599_999));
    assert!(!update_check_due_at(&stamp, now + 3_600_000));
    assert!(update_check_due_at(&stamp, now + 86_400_000 + 1));
}

#[test]
fn update_rate_limit_deadline_prefers_retry_after_then_reset_header() {
    let now = 1_800_000_000_000;

    assert_eq!(
        rate_limit_retry_after_ms(429, Some("120"), Some("0"), Some("1800000300"), now),
        Some(now + 120_000)
    );
    assert_eq!(
        rate_limit_retry_after_ms(403, None, Some("0"), Some("1800000300"), now),
        Some(1_800_000_300_000)
    );
    assert_eq!(
        rate_limit_retry_after_ms(500, Some("120"), Some("0"), Some("1800000300"), now),
        None
    );
}

#[test]
fn appimage_target_rejects_extract_and_run_and_non_files() {
    let dir = tempfile::tempdir().unwrap();
    let appimage = dir.path().join("ForkTTY.AppImage");
    std::fs::write(&appimage, b"old").unwrap();

    assert!(appimage_target_from_env(Some(&appimage), Some("1")).is_none());
    assert!(appimage_target_from_env(Some(dir.path()), None).is_none());

    let target = appimage_target_from_env(Some(&appimage), None).expect("valid appimage target");
    assert_eq!(target.canonical_path, appimage.canonicalize().unwrap());
}

#[cfg(unix)]
#[test]
fn appimage_update_temp_file_is_owner_only_even_with_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let temp = dir.path().join(".forktty-update.tmp");
    let previous_umask = unsafe { libc::umask(0) };
    let result = create_private_update_file(&temp).map(|_| ());
    unsafe {
        libc::umask(previous_umask);
    }
    result.unwrap();

    let mode = std::fs::metadata(&temp).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn appimage_update_replaces_only_after_checksum_matches() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ForkTTY.AppImage");
    let temp = dir.path().join(".forktty-update.tmp");
    std::fs::write(&target, b"old image").unwrap();
    std::fs::write(&temp, b"new image").unwrap();

    let wrong = "0000000000000000000000000000000000000000000000000000000000000000  forktty-new-x86_64.AppImage\n";
    let err =
        replace_appimage_with_verified_temp(&target, &temp, "forktty-new-x86_64.AppImage", wrong)
            .unwrap_err();
    assert!(err.to_string().contains("checksum"));
    assert_eq!(std::fs::read(&target).unwrap(), b"old image");

    std::fs::write(&temp, b"new image").unwrap();
    let digest = "41c04b3f2d0c69a66494faa96f05b622709379f3a5251078e0cd39fa9c44d67e";
    let sums = format!("{digest}  forktty-new-x86_64.AppImage\n");
    replace_appimage_with_verified_temp(&target, &temp, "forktty-new-x86_64.AppImage", &sums)
        .unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new image");
    assert!(!temp.exists());
}

#[test]
fn uses_configured_shell_for_gtk_spawn() {
    let mut config = config::AppConfig::default();
    config.general.shell = "/bin/sh".to_string();

    assert_eq!(configured_shell(&config), "/bin/sh");
}

#[test]
fn configured_shell_ignores_non_executable_paths() {
    let mut config = config::AppConfig::default();
    config.general.shell = "relative-shell".to_string();

    let shell = configured_shell(&config);

    assert!(is_executable_file(Path::new(&shell)));
}

#[test]
fn socket_path_env_ignores_blank_and_relative_values() {
    assert_eq!(socket_path_from_env(None), default_socket_path());
    assert_eq!(
        socket_path_from_env(Some("  /tmp/forktty-custom.sock  ".to_string())),
        PathBuf::from("/tmp/forktty-custom.sock")
    );
    assert_eq!(
        socket_path_from_env(Some("  ".to_string())),
        default_socket_path()
    );
    assert_eq!(
        socket_path_from_env(Some("relative.sock".to_string())),
        default_socket_path()
    );
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_rejects_empty_source_selection() {
    let err =
        browser_import_dialog_params_from_parts(Vec::new(), true, true, true, None).unwrap_err();

    assert_eq!(err, BrowserImportDialogParamError::NoSources);
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_rejects_empty_data_selection() {
    let err = browser_import_dialog_params_from_parts(
        vec![serde_json::json!("firefox:/tmp/profile")],
        false,
        false,
        false,
        None,
    )
    .unwrap_err();

    assert_eq!(err, BrowserImportDialogParamError::NoData);
}

#[cfg(feature = "browser")]
#[test]
fn browser_import_dialog_params_builds_include_and_destination() {
    let params = browser_import_dialog_params_from_parts(
        vec![serde_json::json!("firefox:/tmp/profile")],
        true,
        false,
        true,
        Some(serde_json::json!({"kind": "existing", "profile": "Default"})),
    )
    .unwrap();

    assert_eq!(
        params["sources"][0],
        serde_json::json!("firefox:/tmp/profile")
    );
    assert_eq!(params["include"]["history"], serde_json::json!(true));
    assert_eq!(params["include"]["bookmarks"], serde_json::json!(false));
    assert_eq!(params["include"]["cookies"], serde_json::json!(true));
    assert_eq!(
        params["destination"],
        serde_json::json!({"kind": "existing", "profile": "Default"})
    );
}

#[test]
fn terminal_focus_click_focuses_when_terminal_needs_focus() {
    assert!(!terminal_focus_click_should_focus(
        true,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_focus(
        false,
        Some("pane-1"),
        "pane-1"
    ));
    assert!(terminal_focus_click_should_focus(
        true,
        Some("pane-2"),
        "pane-1"
    ));
    assert!(!terminal_focus_click_should_focus(true, None, "pane-1"));
}

#[test]
fn startup_workspace_prefers_home_over_launch_directory() {
    assert_eq!(
        default_startup_workspace_dir_from(
            Some(PathBuf::from("/home/tester")),
            Some(PathBuf::from("/tmp/launch-dir")),
        ),
        PathBuf::from("/home/tester")
    );
    assert_eq!(
        default_startup_workspace_dir_from(None, Some(PathBuf::from("/tmp/launch-dir"))),
        PathBuf::from("/tmp/launch-dir")
    );
    assert_eq!(
        default_startup_workspace_dir_from(None, None),
        PathBuf::from("/")
    );
}

#[test]
fn first_launch_bootstrap_opens_main_workspace_in_startup_home() {
    let home_dir = tempfile::tempdir().unwrap();
    let launch_dir = tempfile::tempdir().unwrap();
    let startup_dir = default_startup_workspace_dir_from(
        Some(home_dir.path().to_path_buf()),
        Some(launch_dir.path().to_path_buf()),
    );
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    bootstrap_default_workspace(&state, startup_dir).unwrap();

    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].name, "main");
    assert_eq!(workspaces[0].working_dir, home_dir.path());
    let surfaces = model.list_surfaces(Some(&workspaces[0].id));
    assert_eq!(surfaces.len(), 1);
    assert_eq!(surfaces[0].cwd, home_dir.path());

    let backend_surfaces = backend.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].cwd, home_dir.path());
}

#[test]
fn settings_change_rebases_onto_externally_modified_config() {
    // While the Settings dialog is open, an external save (e.g. the F9
    // sidebar toggle) can change other fields; a dialog save must apply only
    // its own field on top of the latest config instead of reverting them.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut base = config::AppConfig::default();
    base.general.shell = "/bin/sh".to_string();
    base.appearance.sidebar_visible = !base.appearance.sidebar_visible;
    let external_sidebar = base.appearance.sidebar_visible;
    config::save_config_to_path(&path, &base).unwrap();

    let next =
        config::update_config_at_path(&path, |config| config.appearance.scrollback_lines = 18_000)
            .unwrap();

    assert_eq!(next.appearance.scrollback_lines, 18_000);
    assert_eq!(next.appearance.sidebar_visible, external_sidebar);
}

#[test]
fn settings_change_preserves_telemetry_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut base = config::AppConfig::default();
    base.general.shell = "/bin/sh".to_string();
    base.telemetry.anonymous_ping = false;
    config::save_config_to_path(&path, &base).unwrap();

    let next =
        config::update_config_at_path(&path, |config| config.appearance.scrollback_lines = 18_000)
            .unwrap();

    assert!(!next.telemetry.anonymous_ping);
}

#[test]
fn settings_dialog_does_not_expose_shell_editor() {
    let source = include_str!("settings_dialog.rs");

    assert!(!source.contains("Shell command"));
    assert!(!source.contains("Shell saved."));
    assert!(!source.contains("saved shell"));
}

#[test]
fn settings_dialog_does_not_expose_runtime_scrollback_limit() {
    let source = include_str!("settings_dialog.rs");

    assert!(!source.contains("Scrollback lines"));
    assert!(!source.contains("Scrollback saved."));
    assert!(!source.contains("Audible bell"));
    assert!(!source.contains("Terminal bell updated."));
    assert!(!source.contains("\"Terminal\""));
}

#[test]
fn settings_dialog_exposes_pty_persistence_toggle() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("Persist terminal processes"));
    assert!(source.contains("config.general.persist_terminal_processes = is_enabled"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state, true)"));
    assert!(source.contains("was_persisting_before_reset"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state_for_reset, true)"));
    assert!(source.contains("PTY process persistence updated."));
}

#[test]
fn settings_dialog_setup_buttons_wait_for_status_poll() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("button.set_sensitive(false);"));
    assert!(source.contains("button.set_sensitive(true);"));
    assert!(source.contains("button.set_label(\"...\");"));
}

#[test]
fn settings_apply_refreshes_pr_lookup_when_enabled() {
    let source = include_str!("app.rs");

    assert!(source.contains("spawn_pr_refresh(pr_model.clone(), pr_in_flight.clone())"));
    assert!(source.contains("clear_pr_hints(&model);"));
}

#[test]
fn settings_agent_hooks_initial_page_targets_agent_hooks_stack() {
    assert_eq!(SettingsInitialPage::Interface.stack_name(), "interface");
    assert_eq!(SettingsInitialPage::AgentHooks.stack_name(), "agent-hooks");
}

#[test]
fn settings_agent_hooks_nav_uses_agent_semantic_icon() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains(
        "\"forktty-terminal-symbolic\",\n        \"Agent hooks\",\n        \"Lifecycle metadata\""
    ));
}

#[test]
fn agent_hook_settings_explain_opt_in_and_offer_managed_removal() {
    let source = include_str!("settings_dialog.rs");
    assert!(source.contains("settings_section(\"Lifecycle hooks\", \"\")"));
    assert!(source.contains("settings_section(\"What hooks do\", \"\")"));
    assert!(source.contains("ForkTTY never installs or updates hooks in the background."));
    assert!(source.contains("Hooks report lifecycle and attention state; they never move focus"));
    assert!(source.contains("run_agent_integrations_remove"));
    assert!(source.contains("show_agent_hook_setup_confirmation"));
    assert!(source.contains("set_settings_task_controls_sensitive(&controls, false)"));
    assert!(source.contains("set_settings_task_controls_sensitive(&controls, true)"));
    assert!(source.contains("status_kind: AgentSetupStatusKind"));
    assert!(source.contains("\"Update agent hooks?\""));
    assert!(source.contains("\"Remove agent hooks?\""));
    assert!(!source.contains("\"Update Agent Hooks?\""));
    assert!(!source.contains("\"Remove Agent Hooks?\""));
}

#[test]
fn settings_navigation_groups_core_worktrees_before_optional_integrations() {
    let source = include_str!("settings_dialog.rs");
    let general = source.find("settings_nav_heading(\"General\")").unwrap();
    let interface = source[general..]
        .find("nav.append(&interface_nav)")
        .unwrap()
        + general;
    let worktrees = source[interface..]
        .find("nav.append(&worktrees_nav)")
        .unwrap()
        + interface;
    let integrations = source[worktrees..]
        .find("settings_nav_heading(\"Integrations\")")
        .unwrap()
        + worktrees;
    let agent_hooks = source[integrations..]
        .find("nav.append(&agent_hooks_nav)")
        .unwrap()
        + integrations;
    let system = source[agent_hooks..]
        .find("settings_nav_heading(\"System\")")
        .unwrap()
        + agent_hooks;

    assert!(general < interface);
    assert!(interface < worktrees);
    assert!(worktrees < integrations);
    assert!(integrations < agent_hooks);
    assert!(agent_hooks < system);
}

#[test]
fn close_phase_keeps_ui_alive_until_socket_drain_completes() {
    let first = close_request_transition(ClosePhase::Running, true);
    assert_eq!(first.phase, ClosePhase::Draining);
    assert!(first.ui_alive);
    assert_eq!(first.action, CloseRequestAction::StartDrain);

    let repeated = close_request_transition(first.phase, first.ui_alive);
    assert_eq!(repeated.phase, ClosePhase::Draining);
    assert!(repeated.ui_alive);
    assert_eq!(repeated.action, CloseRequestAction::Stop);
}

#[test]
fn close_phase_finalizes_exactly_once_and_then_proceeds() {
    let finalized = close_drain_completed_transition(ClosePhase::Draining, true)
        .expect("draining close should finalize");
    assert_eq!(finalized.phase, ClosePhase::Finalizing);
    assert!(!finalized.ui_alive);
    assert!(close_drain_completed_transition(finalized.phase, finalized.ui_alive).is_none());

    let final_request = close_request_transition(finalized.phase, finalized.ui_alive);
    assert_eq!(final_request.action, CloseRequestAction::Proceed);
    assert_eq!(final_request.phase, ClosePhase::Finalizing);
    assert!(!final_request.ui_alive);
}

#[test]
fn close_persistence_snapshots_scrollback_before_session_serialization() {
    let order = RefCell::new(Vec::new());
    glib::MainContext::new().block_on(run_close_persistence_steps(
        || order.borrow_mut().push("scrollback"),
        || order.borrow_mut().push("cwd"),
        async { order.borrow_mut().push("session") },
        || order.borrow_mut().push("pty"),
    ));

    assert_eq!(order.into_inner(), ["scrollback", "cwd", "session", "pty"]);
}

#[test]
fn socket_server_shutdown_waits_for_full_thread_completion() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let (completion_tx, completion_rx) = tokio::sync::oneshot::channel();
        let handle = SocketServerHandle::new(shutdown_tx, completion_rx);

        let mut waiting = tokio::spawn(handle.shutdown_and_wait());
        shutdown_rx.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err(),
            "shutdown completion must not resolve before the server thread reports full drain"
        );

        completion_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap();
    });
}

#[test]
fn socket_server_completion_follows_runtime_drop() {
    struct DropProbe<'a>(&'a Cell<bool>);

    impl Drop for DropProbe<'_> {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }

    let runtime_dropped = Cell::new(false);
    let (completion_tx, mut completion_rx) = tokio::sync::oneshot::channel();
    run_socket_server_thread(completion_tx, || {
        let _runtime = DropProbe(&runtime_dropped);
        assert_eq!(
            completion_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
    });

    assert!(runtime_dropped.get());
    assert_eq!(completion_rx.try_recv(), Ok(()));
}

#[test]
fn window_close_cleans_pty_persistence_when_disabled() {
    let source = include_str!("app.rs");

    assert!(source.contains("config::load_config()"));
    assert!(source.contains("!config.general.persist_terminal_processes"));
    assert!(source.contains("if cleanup_pty_persistence"));
    assert!(source.contains("cleanup_pty_persistence_sessions(&state, false)"));
}

#[test]
fn chrome_micro_polish_css_stays_gtk_414_compatible() {
    let source = include_str!("../style.css");

    assert!(!source.contains("var("));
    for (line_number, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        assert!(
            !(trimmed.starts_with("--") && trimmed.contains(':')),
            "CSS custom property definition on line {}: {}",
            line_number + 1,
            line
        );
    }
}

#[test]
fn sidebar_footer_keeps_secondary_navigation_compact() {
    let sidebar_source = include_str!("sidebar.rs");
    let app_source = include_str!("app.rs");
    let css = include_str!("../style.css");

    assert!(app_source.contains("build_sidebar_sections()"));
    assert!(
        app_source.contains("show_worktree_dialog(&window_for_git_repos, &state_for_git_repos)")
    );
    assert!(!app_source.contains("is not available yet"));
    assert!(app_source.contains("show_about_dialog(&window_for_about)"));
    assert!(sidebar_source.contains("sidebar_nav_row(\"forktty-merge-symbolic\", \"Worktrees\")"));
    assert!(!sidebar_source.contains("Knowledge Base"));
    assert!(!sidebar_source.contains("Snippets"));
    assert!(!sidebar_source.contains("Environments"));
    assert!(!sidebar_source.contains("Secrets"));
    assert!(sidebar_source.contains("settings_row.set_action_name(Some(\"app.settings\"));"));
    assert!(!sidebar_source.contains("sidebar_section_label(\"Resources\")"));
    assert!(!css.contains(".sidebar-fixed-section {"));
    assert!(css.contains("button.flat.sidebar-nav-row {"));
    assert!(css.contains(".sidebar-footer {"));
}

#[test]
fn workbench_sidebar_overlays_terminal_content_and_preserves_configured_side() {
    let actions_source = include_str!("actions.rs");
    assert!(!actions_source.contains("Sidebar shown"));
    assert!(!actions_source.contains("Sidebar hidden"));
    assert!(actions_source.contains("next.appearance.sidebar_visible = visible;"));
    assert!(actions_source
        .contains("app.set_accels_for_action(\"app.toggle-sidebar\", &[\"F9\", \"<Control>b\"]);"));

    let _ = crate::test_env::with_gtk_test(|| {
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let workspace_area = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let overlay = build_workbench_overlay(&sidebar, &workspace_area, "left", true, false);

        assert!(overlay.is_collapsed());
        assert!(!overlay.is_pin_sidebar());
        assert!(overlay.shows_sidebar());
        assert!(!overlay.enables_hide_gesture());
        assert!(!overlay.enables_show_gesture());
        assert_eq!(overlay.min_sidebar_width(), 204.0);
        assert_eq!(overlay.max_sidebar_width(), 216.0);
        assert_eq!(overlay.sidebar_position(), gtk::PackType::Start);
        assert_eq!(overlay.sidebar().as_ref(), Some(sidebar.upcast_ref()));
        assert_eq!(
            overlay.content().as_ref(),
            Some(workspace_area.upcast_ref())
        );

        apply_sidebar_position(&overlay, &sidebar, "right");
        assert_eq!(overlay.sidebar_position(), gtk::PackType::End);
        assert!(sidebar.has_css_class("right"));
        assert!(!sidebar.has_css_class("left"));

        overlay.set_show_sidebar(false);
        assert!(!overlay.shows_sidebar());
        assert!(workspace_area.is_visible());
    });
}

#[test]
fn workbench_sidebar_switches_between_overlay_and_pinned_presentations() {
    let _ = crate::test_env::with_gtk_test(|| {
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let workspace_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let overlay = build_workbench_overlay(&sidebar, &workspace_area, "left", true, false);

        apply_sidebar_pinned(&overlay, true);
        assert!(!overlay.is_collapsed());
        assert!(overlay.is_pin_sidebar());
        assert!(overlay.shows_sidebar());
        assert_eq!(overlay.sidebar_position(), gtk::PackType::Start);

        apply_sidebar_position(&overlay, &sidebar, "right");
        assert_eq!(overlay.sidebar_position(), gtk::PackType::End);
        assert!(sidebar.has_css_class("right"));

        assert!(!toggle_sidebar_visibility(&overlay));
        assert!(overlay.is_pin_sidebar());
        assert!(!overlay.is_collapsed());
        apply_sidebar_pinned(&overlay, false);
        assert!(overlay.is_collapsed());
        assert!(!overlay.is_pin_sidebar());
        assert!(!overlay.shows_sidebar());

        apply_sidebar_pinned(&overlay, true);
        assert!(!overlay.is_collapsed());
        assert!(overlay.is_pin_sidebar());
        assert!(!overlay.shows_sidebar());

        assert!(toggle_sidebar_visibility(&overlay));
        assert!(overlay.is_pin_sidebar());
    });
}

#[test]
fn applied_sidebar_config_syncs_layout_visibility_and_pin_control() {
    let _ = crate::test_env::with_gtk_test(|| {
        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let workspace_area = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let overlay = build_workbench_overlay(&sidebar, &workspace_area, "left", true, false);
        let sidebar_pin = build_sidebar_pin_button(false);
        let shells = WorkbenchShells {
            overlay: overlay.clone(),
            sidebar: sidebar.clone(),
            sidebar_pin: sidebar_pin.clone(),
        };
        let appearance = config::AppearanceConfig {
            sidebar_position: "right".to_string(),
            sidebar_visible: false,
            sidebar_pinned: true,
            ..config::AppearanceConfig::default()
        };

        apply_sidebar_config(&shells, &appearance);

        assert_eq!(overlay.sidebar_position(), gtk::PackType::End);
        assert!(!overlay.is_collapsed());
        assert!(overlay.is_pin_sidebar());
        assert!(!overlay.shows_sidebar());
        assert!(sidebar_pin.is_active());
        assert_eq!(sidebar_pin.tooltip_text().as_deref(), Some("Unpin Sidebar"));

        apply_sidebar_config(&shells, &config::AppearanceConfig::default());

        assert_eq!(overlay.sidebar_position(), gtk::PackType::Start);
        assert!(overlay.is_collapsed());
        assert!(!overlay.is_pin_sidebar());
        assert!(overlay.shows_sidebar());
        assert!(!sidebar_pin.is_active());
    });
}

#[test]
fn sidebar_pin_button_reflects_the_selected_presentation() {
    let app_source = include_str!("app.rs");
    let sidebar_source = include_str!("sidebar.rs");
    let css = include_str!("../style.css");
    assert!(app_source.contains("sidebar_header.append(&sidebar_pin);"));
    assert!(app_source.contains("next.appearance.sidebar_pinned = pinned;"));
    assert!(css.contains(".sidebar-header-action:checked {"));
    assert!(sidebar_source.contains("\"Keep Sidebar Beside Terminal\""));

    let _ = crate::test_env::with_gtk_test(|| {
        let button = build_sidebar_pin_button(false);

        assert!(!button.is_active());
        assert!(button.is_focusable());
        assert!(button.has_css_class("sidebar-header-action"));
        assert_eq!(button.icon_name().as_deref(), Some("view-pin-symbolic"));
        assert_eq!(
            button.tooltip_text().as_deref(),
            Some("Pin Sidebar Beside Terminal")
        );

        sync_sidebar_pin_button(&button, true);
        assert!(button.is_active());
        assert_eq!(button.tooltip_text().as_deref(), Some("Unpin Sidebar"));
    });
}

#[test]
fn settings_dialog_covers_workbench_layout_and_privacy_sections() {
    let settings_source = include_str!("settings_dialog.rs");

    assert!(settings_source
        .contains("show_worktree_dialog(&parent_for_worktrees, &state_for_worktrees)"));
    assert!(settings_source.contains("model.clear_notifications()"));
    assert!(settings_source.contains("run_agent_integrations_setup"));
    assert!(settings_source.contains("if !status_kind.should_run_setup()"));
    assert!(settings_source.contains("settings_section(\"Lifecycle hooks\", \"\")"));
    assert!(settings_source.contains("settings_section(\"Local-first by design\", \"\")"));
    assert!(settings_source.contains(
        "Terminal automation and agent lifecycle metadata use an owner-only local Unix socket."
    ));
    assert!(!settings_source.contains("Agent coordination runs"));
    assert!(settings_source.contains("No cloud sync; anonymous daily ping is controlled below."));
    assert!(!settings_source.contains("No cloud sync. No analytics."));
    assert!(settings_source.contains("settings_section(\"Stored data locations\", \"\")"));
}

#[test]
fn workbench_state_uses_one_periodic_refresh_source() {
    let app_source = include_str!("app.rs");

    assert_eq!(
        app_source
            .matches("timeout_add_local(WORKBENCH_REFRESH_INTERVAL")
            .count(),
        1
    );
    assert!(!app_source.contains("alive_for_layout_timer"));
    assert!(!app_source.contains("alive_for_sidebar_timer"));
    assert!(!app_source.contains("alive_for_notifications_timer"));
    assert!(!app_source.contains("alive_for_agents_timer"));
}

#[test]
fn pane_chrome_keeps_focus_and_agent_state_in_one_header() {
    let pane_source = include_str!("pane_chrome.rs");
    let controller_source = include_str!("controller.rs");
    let css = include_str!("../style.css");

    assert!(pane_source.contains("agent_badge.add_css_class(\"pane-agent-badge\")"));
    assert!(!pane_source.contains("footer_revealer"));
    assert!(!controller_source.contains("footer_revealer"));
    assert!(!css.contains(".terminal-pane-footer"));
    assert!(!css.contains(".rail-dot"));
}

#[test]
fn terminal_empty_stage_surfaces_workspace_actions() {
    let source = include_str!("placeholders.rs");
    let css = include_str!("../style.css");

    assert!(source.contains("create_plain_workspace(&state_for_create)"));
    assert!(source.contains("open_workspace_dialog(&parent_for_open, &state_for_open)"));
    assert!(css.contains(".terminal-empty-actions {"));
    assert!(css.contains(".terminal-empty-stage {"));
}

#[test]
fn workspace_sidebar_polish_quiets_status_counts_and_reorder_grip() {
    let source = include_str!("../style.css");
    let badge = source
        .split(".workspace-status-badge {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("workspace-status-badge block");

    assert!(!badge.contains("text-transform: uppercase;"));

    let count = source
        .split(".workspace-count-badge {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("workspace-count-badge block");
    assert!(count.contains("background: transparent;"));
    assert!(count.contains("border: 0;"));

    let active_grip = source
        .split(".workspace-row.active .workspace-drag-grip {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("active workspace-drag-grip block");
    assert!(active_grip.contains("opacity: 0.24;"));
    assert!(!source.contains("row:focus-visible .workspace-drag-grip"));
}

#[test]
fn workspace_rows_keep_compact_height_and_pane_tabs_keep_navigation_rhythm() {
    let controller_source = include_str!("controller.rs");
    let css = include_str!("../style.css");

    assert!(controller_source.contains("tab.set_hexpand(false);"));
    assert!(controller_source.contains("select.set_hexpand(true);"));
    assert!(controller_source.contains("let tabstrip_scroller = gtk::ScrolledWindow::new();"));
    assert!(controller_source
        .contains("set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);"));
    assert!(controller_source.contains("queue_reveal_tab(&strip.scroller, &strip.tabstrip, tab);"));
    assert!(css.contains(".workspace-card {\n  min-height: 36px;"));
    assert!(css.contains(".workspace-row .workspace-card.drop-before {"));
    assert!(css.contains(".pane-tab {\n  min-width: 96px;"));
    assert!(css.contains(".pane-tab-grip {\n  -gtk-icon-size: 11px;\n  min-width: 14px;\n  color: @ft_text_4;\n  opacity: 0.24;"));
}

#[test]
fn active_pane_tab_keeps_accent_out_of_rounded_border() {
    let css = include_str!("../style.css");
    let active_tab = css
        .split(".pane-tab.active {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("pane-tab active block");

    assert!(active_tab.contains("background-image: linear-gradient("));
    assert!(active_tab.contains("border-top-color: transparent;"));
    assert!(active_tab.contains("box-shadow: none;"));
    assert!(!active_tab.contains("border-top-color: alpha(@accent_color"));
    assert!(!active_tab.contains("box-shadow: inset 0 1px 0 alpha(@accent_color"));
}

#[test]
fn pane_tabs_expose_native_accessible_tab_semantics() {
    let controller_source = include_str!("controller.rs");

    assert!(controller_source.contains(".accessible_role(gtk::AccessibleRole::TabList)"));
    assert!(controller_source.contains(".accessible_role(gtk::AccessibleRole::Tab)"));
    assert!(controller_source.contains("select.update_state(&[gtk::accessible::State::Selected"));
    assert!(controller_source.contains("tab_update.active"));
    assert!(controller_source.contains("gtk::accessible::Relation::Controls"));
    assert!(controller_source.contains("queue_tab_focus_after_selection("));
}

#[test]
fn chrome_micro_polish_keyboard_focus_matches_hover() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .rsplit(selector)
            .next()
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block(".pane-tab-close:focus-visible {").contains("opacity: 1;"));
    // Focus background matches hover through the shared @ft_bg_1 tier.
    assert!(
        block("button.flat.terminal-pane-action:focus-visible {").contains("background: @ft_bg_1;")
    );
    assert!(block("button.flat.sidebar-header-action:focus-visible {")
        .contains("background: @ft_bg_1;"));
}

#[test]
fn chrome_micro_polish_unifies_pane_hover_and_hairline_tone() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    // Hover and hairline colors stay on the shared surface tokens.
    assert!(block("button.flat.terminal-pane-action:hover {").contains("background: @ft_bg_1;"));
    assert!(block("button.flat.pane-close-action:hover {").contains("background: @ft_danger_bg;"));
    assert!(block(".pane-action-separator {").contains("background: @ft_line;"));
    let separator = block(".terminal-stage paned > separator {");
    assert!(separator.contains("min-width: 1px;"));
    assert!(separator.contains("background-clip: padding-box;"));
    let horizontal_separator = block(".terminal-stage paned.horizontal > separator {");
    assert!(horizontal_separator.contains("border-left: 3px solid transparent;"));
    assert!(horizontal_separator.contains("border-right: 3px solid transparent;"));
    let vertical_separator = block(".terminal-stage paned.vertical > separator {");
    assert!(vertical_separator.contains("border-top: 3px solid transparent;"));
    assert!(vertical_separator.contains("border-bottom: 3px solid transparent;"));
    assert!(block(".terminal-pane.active .terminal-pane-header {")
        .contains("border-bottom-color: @ft_line;"));
    assert!(block(".terminal-pane.active .terminal-pane-header {")
        .contains("box-shadow: inset 0 1px 0 alpha(@accent_color, 0.78);"));
    assert!(
        block(".terminal-pane.needs-attention .terminal-pane-header {")
            .contains("box-shadow: inset 0 2px 0 alpha(@accent_color, 0.82);")
    );
    assert!(block(".pane-attention-dot {").contains("min-width: 6px;"));
}

#[test]
fn split_pane_headers_stay_compact_and_reveal_controls_on_demand() {
    let pane_source = include_str!("pane_chrome.rs");
    let css = include_str!("../style.css");
    let block = |selector: &str| {
        css.split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(pane_source.contains("let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);"));
    let header = block(".terminal-pane-header {");
    assert!(header.contains("min-height: 22px;"));
    assert!(header.contains("padding: 0 8px;"));
    assert!(header.contains("background: @ft_bg_stage;"));
    assert!(block(".pane-drag-grip {").contains("opacity: 0.18;"));
    assert!(block(".terminal-pane-actions {").contains("opacity: 0;"));
    assert!(block(".terminal-pane-actions.revealed,").contains("opacity: 1;"));
    let action = block(".terminal-pane-action {");
    assert!(action.contains("min-height: 18px;"));
    assert!(action.contains("margin: 2px 1px;"));
}

#[test]
fn app_header_actions_use_readable_contrast() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block(".app-header .header-action,").contains("color: @ft_text_2;"));
}

#[test]
fn command_palette_microtext_uses_readable_muted_contrast() {
    let source = include_str!("../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    // Muted microtext and disabled text stay on their readable shared tiers.
    assert!(block(".ft-menu-shortcut {").contains("color: @ft_text_3;"));
    assert!(block(".command-item .keycap {").contains("color: @ft_text_3;"));
    assert!(
        block(".command-list row:disabled .command-item .keycap {").contains("color: @ft_text_4;")
    );
}

#[test]
fn maximized_layout_signature_tracks_focused_pane() {
    // In maximize mode only the focused pane is rendered, so a focus-only
    // change must produce a different signature and trigger a rebuild.
    assert_eq!(
        effective_layout_signature("ws-1:L(s1|s2)", false, "s1"),
        "ws-1:L(s1|s2)"
    );
    assert_eq!(
        effective_layout_signature("ws-1:L(s1|s2)", true, "s1"),
        "ws-1:L(s1|s2)#max:s1"
    );
    assert_ne!(
        effective_layout_signature("ws-1:L(s1|s2)", true, "s1"),
        effective_layout_signature("ws-1:L(s1|s2)", true, "s2")
    );
}

#[test]
fn single_pane_maximize_is_noop_and_later_split_is_visible() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let single = model.active_workspace().unwrap().pane_tree;

    assert!(!toggle_maximized_pane_state(false, Some(&single)));

    model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let split = model.active_workspace().unwrap().pane_tree;
    assert!(toggle_maximized_pane_state(false, Some(&split)));
}

#[test]
fn maximize_counts_panes_not_tabs() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.add_tab(&workspace.focused_surface_id).unwrap();
    let tabbed_leaf = model.active_workspace().unwrap().pane_tree;

    assert_eq!(collect_panes(&tabbed_leaf).len(), 1);
    assert!(!toggle_maximized_pane_state(false, Some(&tabbed_leaf)));
}

#[test]
fn collapse_to_one_pane_clears_maximize() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let split = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let split_tree = model.active_workspace().unwrap().pane_tree;
    assert!(toggle_maximized_pane_state(false, Some(&split_tree)));

    model.close_surface(&split.id).unwrap();
    let collapsed = model.active_workspace().unwrap().pane_tree;
    assert!(!normalize_maximized_pane_state(true, Some(&collapsed)));
}

#[test]
fn empty_layout_clears_maximize() {
    assert!(!normalize_maximized_pane_state(true, None));
}

#[test]
fn settings_choice_mapping_round_trips_known_values() {
    assert_eq!(settings_choice_index(WORKTREE_LAYOUT_ITEMS, "sibling"), 1);
    assert_eq!(
        settings_choice_value(WORKTREE_LAYOUT_ITEMS, 1),
        Some("sibling")
    );
    assert_eq!(settings_choice_value(WINDOW_MODE_ITEMS, 1), Some("quake"));
}

#[test]
fn settings_choice_mapping_falls_back_for_unknown_values() {
    assert_eq!(settings_choice_index(SIDEBAR_POSITION_ITEMS, "top"), 0);
    assert_eq!(settings_choice_value(SIDEBAR_POSITION_ITEMS, 99), None);
}

#[test]
fn command_palette_search_matches_labels_and_shortcuts() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let new_tab = command_search_text("New Tab", Some("Ctrl+Shift+T"));
    let settings = command_search_text("Settings", Some("Ctrl+,"));
    let sidebar = command_search_text("Toggle Sidebar", Some("Ctrl+B / F9"));
    let shortcuts = command_search_text("Keyboard Shortcuts", Some("Ctrl+?"));
    let zoom = command_search_text("Zoom In", Some(TERMINAL_ZOOM_IN_SHORTCUT));

    assert!(command_matches(&copy, "copy"));
    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(command_matches(&copy, "ctrl+c"));
    assert!(command_matches(&new_tab, "new tab"));
    assert!(command_matches(&new_tab, "ctrl shift t"));
    assert!(command_matches(&settings, "ctrl,"));
    assert!(command_matches(&sidebar, "f9"));
    assert!(command_matches(&shortcuts, "ctrl?"));
    assert!(command_matches(&shortcuts, "ctrl question"));
    assert!(command_matches(&zoom, "zoom in"));
    assert!(command_matches(&zoom, "ctrl+"));
    assert!(!command_matches(&copy, "paste"));
}

#[test]
fn command_palette_shortcut_search_does_not_match_modifier_letters() {
    let copy = command_search_text("Copy", Some("Ctrl+Shift+C"));
    let split_right = command_search_text("Split Right", Some("Ctrl+Shift+H"));
    let split_down = command_search_text("Split Down", Some("Ctrl+Shift+J"));

    assert!(command_matches(&copy, "ctrl shift c"));
    assert!(!command_matches(&split_right, "ctrl shift c"));
    assert!(!command_matches(&split_down, "ctrl shift c"));
}

#[test]
fn command_palette_search_supports_fuzzy_words() {
    let split = command_search_text("Split Right", Some("Ctrl+Shift+H"));

    assert!(command_matches(&split, "sr"));
    assert!(command_matches(&split, "sp ri"));
    assert!(!command_matches(&split, "split down"));
}

#[test]
fn accessible_shortcut_text_uses_accessibility_key_names() {
    assert_eq!(accessible_shortcut_text("Ctrl+Shift+P"), "Control+Shift+P");
    assert_eq!(
        accessible_shortcut_text("Ctrl+L / Alt+D"),
        "Control+L Alt+D"
    );
    assert_eq!(accessible_shortcut_text("Ctrl+,"), "Control+,");
    assert_eq!(accessible_shortcut_text("Ctrl+?"), "Control+?");
    assert_eq!(accessible_shortcut_text("Esc"), "Escape");
}

#[test]
fn app_menu_uses_the_logo_without_duplicate_brand_or_hamburger() {
    let source = include_str!("app.rs");

    assert!(source
        .contains("let app_menu = gtk::MenuButton::builder()\n        .icon_name(\"forktty\")"));
    assert!(!source.contains(".label(\"forktty\")"));
    assert!(!source.contains("forktty-menu-symbolic"));
    assert!(!source.contains("brand_separator"));
    assert!(source.contains("gtk::accessible::Property::Label(\"Main menu\")"));
    let menu_position = source.find("header.pack_start(&app_menu);").unwrap();
    let location_position = source
        .find("header.pack_start(&location_cluster);")
        .unwrap();
    assert!(menu_position < location_position);
    assert!(source.contains("gtk::gdk::Key::F10"));
    assert!(source.contains("app_menu.popup()"));
    assert!(source.contains("main_menu_shortcut_should_open"));
    assert!(source.contains("\"ghostty-terminal\""));
    assert!(source.contains("\"forktty-terminal-focus-boundary\""));
}

#[test]
fn titlebar_groups_window_controls_with_spacing_instead_of_a_rule() {
    let app_source = include_str!("app.rs");
    let css = include_str!("../style.css");
    let block = |selector: &str| {
        css.split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(!app_source.contains("header_action_separator"));
    assert!(!css.contains(".header-action-separator"));
    assert!(block(".app-window-controls {").contains("margin: 0 1px 0 5px;"));
    let logo = block(".app-header menubutton.app-logo-menu > button image {");
    assert!(logo.contains("-gtk-icon-size: 19px;"));
    assert!(logo.contains("opacity: 0.82;"));
}

#[test]
fn app_shell_omits_the_redundant_global_status_bar() {
    let app_source = include_str!("app.rs");
    let sidebar_source = include_str!("sidebar.rs");
    let css = include_str!("../style.css");

    assert!(!app_source.contains("let status_bar ="));
    assert!(!app_source.contains("status_location"));
    assert!(!app_source.contains("pane_status"));
    assert!(!app_source.contains("palette_hint"));
    assert!(!sidebar_source.contains("status_location"));
    assert!(!sidebar_source.contains("pane_status"));
    assert!(!css.contains(".app-status-bar"));
}

#[test]
fn embedded_ghostty_surface_marks_terminal_focus_boundary() {
    let source = include_str!("embedded_runtime.rs");

    assert!(source.contains("surface.add_css_class(\"forktty-terminal-focus-boundary\")"));
    assert!(source.contains("build_embedded_ghostty_scroll_view"));
}

#[test]
fn command_palette_source_uses_polished_labels_and_accessibility() {
    let source = include_str!("command_palette.rs");

    assert!(
        source.contains("let title = gtk::Label::builder()\n        .label(\"Command Palette\")")
    );
    assert!(source.contains("gtk::accessible::Property::Label"));
    assert!(source.contains("\"Search commands or shortcuts\""));
    assert!(source.contains("command!(\"Keyboard Shortcuts\", Some(\"Ctrl+? / F1\")"));
    assert!(source.contains("(\"Toggle Maximize Pane\", \"Ctrl+Shift+Enter\")"));
    assert!(source.contains("command!(\"Close Workspace...\""));
    assert!(source.contains("command_enabled!(\n        \"Move Workspace Up\""));
    assert!(source.contains("active_workspace_can_move_relative(state, -1)"));
    assert!(source.contains("active_workspace_can_move_relative(state, 1)"));
}

#[test]
fn app_menu_shortcut_label_matches_shortcuts_dialog() {
    let source = include_str!("app.rs");

    assert!(source.contains("\"Keyboard Shortcuts\""));
    assert!(source.contains("Some(\"Ctrl+? / F1\")"));
}

#[test]
fn worktree_context_failures_use_error_notifications() {
    let source = include_str!("workspace_menu.rs");

    assert!(source.contains("create_local_notification_with_kind("));
    assert!(source.contains("\"Merge failed\""));
    assert!(source.contains("\"Remove failed\""));
    assert!(source.contains("NotificationKind::Error"));
}

#[test]
fn terminal_context_menu_accelerated_items_show_shortcuts() {
    let workspace_menu = include_str!("workspace_menu.rs");
    let embedded_controls = include_str!("embedded_controls.rs");

    for source in [workspace_menu, embedded_controls] {
        assert!(source.contains("add_context_menu_item_with_shortcut("));
        assert!(source.contains("\"Split Right\""));
        assert!(source.contains("Some(\"Ctrl+Shift+H\")"));
        assert!(source.contains("\"Split Down\""));
        assert!(source.contains("Some(SPLIT_VERTICAL_SHORTCUT)"));
        assert!(source.contains("\"Restart Pane\""));
        assert!(source.contains("Some(RESTART_PANE_SHORTCUT)"));
        assert!(source.contains("\"Close Pane\""));
        assert!(source.contains("Some(\"Ctrl+Shift+W\")"));
        assert!(!source
            .contains("\"forktty-folder-symbolic\",\n            \"Copy Working Directory\""));
    }
}

#[test]
fn tab_context_menu_accelerated_items_show_shortcuts() {
    let source = include_str!("controller.rs");

    assert!(source.contains("add_context_menu_item_with_shortcut("));
    assert!(source.contains("\"New Tab\""));
    assert!(source.contains("Some(\"Ctrl+Shift+T\")"));
    assert!(source.contains("\"Split Right\""));
    assert!(source.contains("Some(\"Ctrl+Shift+H\")"));
    assert!(source.contains("\"Split Down\""));
    assert!(source.contains("Some(SPLIT_VERTICAL_SHORTCUT)"));
    assert!(source.contains("\"Close Tab\""));
    assert!(source.contains("Some(\"Ctrl+Shift+W\")"));
}

#[test]
fn pane_tab_selectors_are_keyboard_activatable() {
    let source = include_str!("controller.rs");

    assert!(source.contains("let select = gtk::Button::builder()"));
    assert!(source.contains(".accessible_role(gtk::AccessibleRole::Tab)"));
    assert!(source.contains("select.connect_clicked(move |_|"));
}

#[test]
fn notification_clear_tooltip_and_workspace_popover_accessibility_are_precise() {
    let notifications = include_str!("notifications_panel.rs");
    let popover = include_str!("workspace_popover.rs");

    assert!(notifications.contains("clear.set_tooltip_text(Some(\"Clear all notifications\"));"));
    assert!(popover.contains("set_accessible_button_text(&row"));
    assert!(popover.contains("\"Current workspace {}\""));
    assert!(popover.contains("\"Switch to workspace {}\""));
    assert!(popover.contains("set_accessible_button_text(&new_btn"));
    assert!(popover.contains("\"New Workspace\""));
    assert!(popover.contains("Some(\"Ctrl+Shift+N\")"));
}

#[test]
fn app_shell_polish_keeps_tooltips_and_bundled_icons_precise() {
    let app = include_str!("app.rs");
    let sidebar = include_str!("sidebar.rs");
    let search = include_str!("terminal_search.rs");
    let settings = include_str!("settings_dialog.rs");
    let welcome = include_str!("welcome.rs");

    assert!(app.contains("\"Maximize or Restore\""));
    assert!(app.contains("button.set_tooltip_text(Some(label));"));
    assert!(sidebar.contains("Active workspace: {name}"));
    assert!(search.contains("\"forktty-back-symbolic\""));
    assert!(search.contains("\"forktty-forward-symbolic\""));
    assert!(!search.contains("\"go-up-symbolic\""));
    assert!(!search.contains("\"go-down-symbolic\""));
    assert!(settings.contains("settings_nav_button(\"forktty-info-symbolic\", \"Privacy\""));
    assert!(welcome.contains(".label(\"Set Up\")"));
    assert!(!welcome.contains("setup_button.add_css_class(\"suggested-action\")"));
    assert!(welcome.contains("get_started.add_css_class(\"suggested-action\")"));
}

#[test]
fn terminal_first_startup_avoids_duplicate_notification_nags() {
    let app = include_str!("app.rs");

    assert!(!app.contains("show_hook_setup_reminder"));
    assert!(!app.contains("Agent Hooks Available"));
    assert!(!app.contains("Opened your home directory as the main workspace"));
}

#[test]
fn single_pane_actions_remain_discoverable_at_rest() {
    let css = include_str!("../style.css");
    let actions = css
        .split(".single-pane-actions {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .expect("single-pane-actions block");

    assert!(actions.contains("opacity: 0.56;"));
}

#[test]
fn worktree_dialog_reports_mode_and_load_failure_context() {
    let source = include_str!("worktree_dialog.rs");

    assert!(source.contains("dialog: gtk::Window"));
    assert!(source.contains("controls.dialog.set_title(Some(mode.dialog_title()))"));
    assert!(source.contains("worktree_dialog_hint(mode, state.discovery)"));
    assert!(source.contains("Could not load linked worktrees"));
}

#[test]
fn worktree_dialog_keeps_context_and_target_hierarchy_visible() {
    let source = include_str!("worktree_dialog.rs");
    let css = include_str!("../style.css");

    assert!(source.contains(".label(\"Starting from\")"));
    assert!(source.contains("context_icon.add_css_class(\"worktree-context-icon\")"));
    assert!(source.contains("target_label: gtk::Label"));
    assert!(source.contains("controls.target_label.set_label(mode.target_label())"));
    assert!(css.contains(".worktree-context-icon"));
    assert!(css.contains(".worktree-target-label"));
}

#[test]
fn worktree_actions_use_sentence_case() {
    let palette = include_str!("command_palette.rs");
    let menu = include_str!("workspace_menu.rs");

    assert!(palette.contains("\"New worktree...\""));
    assert!(menu.contains("\"New worktree from here...\""));
    assert!(menu.contains("\"Merge worktree\""));
    assert!(menu.contains("\"Remove worktree\""));
    assert!(menu.contains("\"Merge failed\""));
    assert!(menu.contains("\"Remove failed\""));
    assert!(!palette.contains("\"New Worktree...\""));
    assert!(!menu.contains("\"Merge Worktree\""));
    assert!(!menu.contains("\"Remove Worktree\""));
}

#[test]
fn settings_initial_focus_tracks_requested_page() {
    let source = include_str!("settings_dialog.rs");

    assert!(source.contains("SettingsInitialPage::Interface =>"));
    assert!(source.contains("interface_nav.grab_focus();"));
    assert!(source.contains("SettingsInitialPage::AgentHooks =>"));
    assert!(source.contains("agent_hooks_nav.grab_focus();"));
    assert!(source.contains("Agent hooks"));
}

#[test]
fn custom_menu_items_can_render_shortcut_metadata() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("add_context_menu_item_with_shortcut"));
    assert!(source.contains("ft-menu-shortcut"));
    assert!(source.contains("gtk::accessible::Property::KeyShortcuts"));
}

#[test]
fn about_dialog_source_includes_standard_links() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("Website"));
    assert!(source.contains("Report Issue"));
    assert!(source.contains("Changelog"));
    assert!(source.contains("FORKTTY_WEBSITE_URI"));
    assert!(source.contains("FORKTTY_ISSUES_URI"));
    assert!(source.contains("FORKTTY_CHANGELOG_URI"));
}

#[test]
fn dialog_escape_close_uses_capture_phase() {
    let source = include_str!("ui_common.rs");

    assert!(source.contains("controller.set_propagation_phase(gtk::PropagationPhase::Capture);"));
}
