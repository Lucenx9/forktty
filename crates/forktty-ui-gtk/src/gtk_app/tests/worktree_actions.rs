//! GTK worktree action rollback, cwd resolution, and validation regression tests.

use super::*;

struct IsolatedUserDirs {
    state_home: PathBuf,
}

impl IsolatedUserDirs {
    fn session_path(&self) -> PathBuf {
        self.state_home.join("forktty").join("session-v2.json")
    }
}

fn with_isolated_user_dirs<T>(f: impl FnOnce(&IsolatedUserDirs) -> T) -> T {
    let home_dir = tempfile::tempdir().unwrap();
    let home_path = home_dir.path().to_path_buf();
    let state_home = home_path.join("state");
    let data_home = home_path.join("data");
    let config_home = home_path.join("config");
    let cache_home = home_path.join("cache");
    let dirs = IsolatedUserDirs {
        state_home: state_home.clone(),
    };
    let home = home_path.to_string_lossy().into_owned();
    let state = state_home.to_string_lossy().into_owned();
    let data = data_home.to_string_lossy().into_owned();
    let config = config_home.to_string_lossy().into_owned();
    let cache = cache_home.to_string_lossy().into_owned();

    crate::test_env::with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("XDG_STATE_HOME", Some(state.as_str())),
            ("XDG_DATA_HOME", Some(data.as_str())),
            ("XDG_CONFIG_HOME", Some(config.as_str())),
            ("XDG_CACHE_HOME", Some(cache.as_str())),
        ],
        || f(&dirs),
    )
}

#[cfg(unix)]
fn commit_blocking_setup_hook(dir: &Path, started_fifo: &Path, release_fifo: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let hook_dir = dir.join(".forktty");
    std::fs::create_dir_all(&hook_dir).unwrap();
    let hook_path = hook_dir.join("setup");
    std::fs::write(
        &hook_path,
        format!(
            "#!/bin/sh\nprintf x > '{}'\nread _ < '{}'\n",
            started_fifo.display(),
            release_fifo.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let repo = Repository::open(dir).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(".forktty/setup")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "add blocking setup hook",
        &tree,
        &[&parent],
    )
    .unwrap();
}

#[derive(Debug)]
struct FirstCloseFailsBackend {
    inner: forktty_terminal::HeadlessTerminalBackend,
    fail_next_close: AtomicBool,
}

impl Default for FirstCloseFailsBackend {
    fn default() -> Self {
        Self {
            inner: forktty_terminal::HeadlessTerminalBackend::new(),
            fail_next_close: AtomicBool::new(true),
        }
    }
}

impl TerminalBackend for FirstCloseFailsBackend {
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
        if self.fail_next_close.swap(false, Ordering::SeqCst) {
            return Err(TerminalError::Backend("close failed".to_string()));
        }
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[derive(Debug)]
struct GtkBlocksAfterCloseBackend {
    inner: forktty_terminal::HeadlessTerminalBackend,
    close_started: Mutex<Option<mpsc::Sender<()>>>,
    release_close: Mutex<mpsc::Receiver<()>>,
}

impl GtkBlocksAfterCloseBackend {
    fn new(close_started: mpsc::Sender<()>, release_close: mpsc::Receiver<()>) -> Self {
        Self {
            inner: forktty_terminal::HeadlessTerminalBackend::new(),
            close_started: Mutex::new(Some(close_started)),
            release_close: Mutex::new(release_close),
        }
    }
}

impl TerminalBackend for GtkBlocksAfterCloseBackend {
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
        self.inner.close(surface_id)?;
        if let Some(started) = self
            .close_started
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .take()
        {
            let _ = started.send(());
            self.release_close
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .recv()
                .map_err(|err| TerminalError::Backend(err.to_string()))?;
        }
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[cfg(unix)]
#[test]
fn gtk_create_and_socket_worktree_list_share_transaction_guard() {
    use std::future::Future as _;
    use std::io::{Read as _, Write as _};
    use std::task::{Context, Poll, Waker};

    with_isolated_user_dirs(|_| {
        let repo_dir = make_temp_repo();
        let fifo_dir = tempfile::tempdir().unwrap();
        let started_fifo = fifo_dir.path().join("gtk-create-started");
        let release_fifo = fifo_dir.path().join("gtk-create-release");
        for fifo in [&started_fifo, &release_fifo] {
            assert!(std::process::Command::new("mkfifo")
                .arg(fifo)
                .status()
                .unwrap()
                .success());
        }
        commit_blocking_setup_hook(repo_dir.path(), &started_fifo, &release_fifo);

        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model,
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();

        let (setup_started_tx, setup_started_rx) = mpsc::channel();
        let started_reader = std::thread::spawn(move || {
            let mut fifo = std::fs::File::open(started_fifo).unwrap();
            let mut byte = [0_u8; 1];
            fifo.read_exact(&mut byte).unwrap();
            setup_started_tx.send(()).unwrap();
        });

        let gtk_state = state.clone();
        let gtk_cwd = repo_dir.path().to_path_buf();
        let gtk_action = std::thread::spawn(move || {
            glib::MainContext::new().block_on(open_worktree_from_gtk_async_at_cwd(
                &gtk_state,
                &gtk_cwd,
                "feature/gtk-shared-guard",
                WorktreeAction::Create,
            ))
        });
        setup_started_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("GTK Create should reach its setup hook");

        // A valid discovery request reaches the cancellation-shielded read
        // worker. Polling once proves it is pending on the shared GTK writer,
        // rather than relying on thread scheduling or a timeout.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut socket_action = Box::pin(forktty_socket::dispatch(
            &state,
            "worktree.list",
            serde_json::json!({"cwd": repo_dir.path()}),
        ));
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        let first_poll = {
            let _runtime_context = runtime.enter();
            socket_action.as_mut().poll(&mut context)
        };
        assert!(
            matches!(first_poll, Poll::Pending),
            "socket discovery should be pending on the GTK transaction guard"
        );

        let mut release = std::fs::OpenOptions::new()
            .write(true)
            .open(release_fifo)
            .unwrap();
        release.write_all(b"x\n").unwrap();

        gtk_action.join().unwrap().unwrap();
        let worktrees = runtime.block_on(socket_action).unwrap();
        started_reader.join().unwrap();

        assert_eq!(
            worktrees.as_array().map(Vec::len),
            Some(1),
            "socket discovery should complete after GTK releases the guard"
        );
    });
}

#[test]
fn gtk_last_worktree_close_failure_keeps_model_and_cleans_replacement() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-remove-close-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(FirstCloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        );
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    let error = remove_worktree_from_gtk(&state, &branch_name).unwrap_err();

    assert!(error.contains("close failed"), "{error}");
    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert!(workspaces[0].active);
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    drop(model);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
    assert!(Path::new(&info.path).exists());
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .iter()
        .any(|worktree| worktree.worktree_name == info.worktree_name));
}

#[test]
fn close_last_worktree_workspace_keeps_old_workspace_when_replacement_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-remove-spawn-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let worktree_cwd = PathBuf::from(&info.path);
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(SecondSpawnFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_worktree_workspace(
            &info.branch,
            &worktree_cwd,
            &info.branch,
            &info.worktree_name,
        );
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();

    let error = remove_worktree_from_gtk(&state, &branch_name).unwrap_err();

    assert!(error.contains("spawn failed"), "{error}");
    let model = model.lock().unwrap();
    let workspaces = model.list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, workspace_id);
    assert!(workspaces[0].active);
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    let backend_surfaces = terminal.surfaces().unwrap();
    assert_eq!(backend_surfaces.len(), 1);
    assert_eq!(backend_surfaces[0].surface_id, surface_id);
    assert!(Path::new(&info.path).exists());
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .iter()
        .any(|worktree| worktree.worktree_name == info.worktree_name));
}

#[test]
fn gtk_last_worktree_irreversible_finish_commits_replacement() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-cleanup-replacement-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let removal =
        worktree::prepare_remove(repo_dir.path().to_str().unwrap(), &branch_name).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let target_workspace = model.lock().unwrap().create_worktree_workspace(
        &info.branch,
        PathBuf::from(&info.path),
        &info.branch,
        &info.worktree_name,
    );
    spawn_workspace_terminal_gtk(&state, &target_workspace).unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        GtkTerminalCommand::Spawn { .. }
    ));
    worktree::remove(repo_dir.path().to_str().unwrap(), &branch_name, false).unwrap();

    let error = glib::MainContext::new()
        .block_on(async {
            let worktree_guard = state.worktree_write_guard().await;
            let transaction = state.worktree_surface_transaction(worktree_guard).await;
            forktty_socket::finish_prepared_worktree_removal(
                transaction,
                repo_dir.path().to_path_buf(),
                removal,
            )
            .await
        })
        .unwrap_err()
        .to_string();

    assert!(error.contains("not found"), "{error}");
    assert!(error.contains("became irreversible"), "{error}");
    assert!(error.contains("modeled removal was committed"), "{error}");
    let commands = rx.try_iter().collect::<Vec<_>>();
    let replacement_id = match commands.first() {
        Some(GtkTerminalCommand::Spawn { request, .. }) => request.surface_id.clone(),
        _ => panic!("committed removal should first queue the replacement spawn"),
    };
    assert!(matches!(
        commands.get(1),
        Some(GtkTerminalCommand::Close { surface_id, .. })
            if surface_id == &target_workspace.focused_surface_id
    ));
    assert_eq!(commands.len(), 2);
    let workspaces = model.lock().unwrap().list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].focused_surface_id, replacement_id);
    assert!(workspaces[0].active);
    assert_ne!(workspaces[0].id, target_workspace.id);
}

#[test]
fn worktree_create_removes_created_worktree_when_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/spawn-rollback-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = with_isolated_user_dirs(|_| {
        open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err()
    });

    assert!(error.contains("sending on a closed channel"));
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

#[test]
fn worktree_create_preserves_existing_branch_when_gtk_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/existing-branch-rollback-{}", std::process::id());
    create_local_branch(repo_dir.path(), &branch_name);
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = with_isolated_user_dirs(|_| {
        open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err()
    });

    assert!(error.contains("sending on a closed channel"));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
    let repo = Repository::open(repo_dir.path()).unwrap();
    assert!(repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .is_ok());
}

#[test]
fn worktree_create_preserves_existing_worktree_when_gtk_spawn_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/existing-spawn-rollback-{}", std::process::id());
    let created = worktree::create(
        repo_dir.path().to_str().unwrap(),
        &branch_name,
        "../forktty-worktrees/{name}",
    )
    .unwrap();
    let existing_path = created.path.clone();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let (tx, rx) = mpsc::channel();
    drop(rx);
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = with_isolated_user_dirs(|_| {
        open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap_err()
    });

    assert!(error.contains("sending on a closed channel"));
    assert!(Path::new(&existing_path).exists());
    let repo = Repository::open(repo_dir.path()).unwrap();
    assert!(repo
        .find_branch(&branch_name, git2::BranchType::Local)
        .is_ok());
    let worktrees = worktree::list(repo_dir.path().to_str().unwrap()).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].branch, branch_name);
}

#[test]
fn worktree_open_uses_captured_base_cwd_after_active_workspace_changes() {
    let repo_one = make_temp_repo();
    let repo_two = make_temp_repo();
    let branch_name = format!("feature/captured-cwd-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let captured_cwd = repo_one.path().to_path_buf();
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("captured", &captured_cwd);
        model.create_workspace("active-now", repo_two.path());
    }

    with_isolated_user_dirs(|dirs| {
        glib::MainContext::new()
            .block_on(open_worktree_from_gtk_async_at_cwd(
                &state,
                &captured_cwd,
                &branch_name,
                WorktreeAction::Create,
            ))
            .unwrap();
        assert!(dirs.session_path().exists());
    });

    assert!(repo_one.path().join(".worktrees").exists());
    assert!(!repo_two.path().join(".worktrees").exists());
}

#[test]
fn worktree_dialog_prefers_focused_surface_cwd_over_workspace_launch_dir() {
    // Importers: gtk_app tests via super::*; helpers under test:
    // active_workspace_cwd_string / active_workspace_repo_cwd_string.
    // No data-file schema. User: "vai procedi"
    let launch_dir = tempfile::tempdir().unwrap();
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", launch_dir.path());
        assert!(model.set_surface_cwd(&workspace.focused_surface_id, repo_dir.path().to_path_buf()));
    }

    // Create/Attach discovery may follow the live shell CWD.
    assert_eq!(
        active_workspace_cwd_string(&state).unwrap(),
        repo_dir.path().to_string_lossy()
    );
    // Remove/Merge must stay on the modeled workspace checkout.
    assert_eq!(
        active_workspace_repo_cwd_string(&state).unwrap(),
        launch_dir.path().to_string_lossy()
    );
}

#[test]
fn remove_and_merge_use_modeled_workspace_checkout_not_live_shell_cwd() {
    // Surface CWD left the repo entirely (e.g. `cd /tmp`). Create/Attach may
    // still discover from that path, but Remove/Merge must not.
    let repo_dir = make_temp_repo();
    let outside_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", repo_dir.path());
        assert!(model.set_surface_cwd(
            &workspace.focused_surface_id,
            outside_dir.path().to_path_buf()
        ));
    }

    assert_eq!(
        active_workspace_cwd_string(&state).unwrap(),
        outside_dir.path().to_string_lossy()
    );
    assert_eq!(
        active_workspace_repo_cwd_string(&state).unwrap(),
        repo_dir.path().to_string_lossy()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn worktree_dialog_prefers_live_child_pid_cwd_over_recorded_surface_cwd() {
    let launch_dir = tempfile::tempdir().unwrap();
    let recorded_dir = tempfile::tempdir().unwrap();
    let repo_dir = make_temp_repo();
    let (tx, _rx) = mpsc::channel();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(GtkTerminalBackend::new(tx));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", launch_dir.path());
        assert!(model.set_surface_cwd(
            &workspace.focused_surface_id,
            recorded_dir.path().to_path_buf()
        ));
        (workspace.id, workspace.focused_surface_id)
    };
    terminal
        .spawn(SpawnRequest {
            surface_id: surface_id.clone(),
            workspace_id,
            shell: "/bin/sh".to_string(),
            args: Vec::new(),
            cwd: recorded_dir.path().to_path_buf(),
            socket_path: PathBuf::from("/tmp/forktty.sock"),
            extra_env: Vec::new(),
            eligible_for_pty_persistence: false,
        })
        .unwrap();
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg("sleep 60")
        .current_dir(repo_dir.path())
        .spawn()
        .unwrap();
    terminal.mark_surface_pid(&surface_id, child.id()).unwrap();

    let cwd = active_workspace_cwd_string(&state).unwrap();

    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(cwd, repo_dir.path().to_string_lossy());
}

#[cfg(target_os = "linux")]
#[test]
fn worktree_dialog_preserves_synced_managed_workload_cwd() {
    crate::test_env::with_isolated_user_dirs(|| {
        let launch_dir = tempfile::tempdir().unwrap();
        let workload_dir = make_temp_repo();
        let runtime_dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
        let socket_path = runtime_dir.join("forktty.sock");
        let (tx, _rx) = mpsc::channel();
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(GtkTerminalBackend::new(tx));
        let state = SocketAppState::new(
            model.clone(),
            terminal.clone(),
            "/bin/sh",
            socket_path.clone(),
        )
        .with_notification_dispatch(false);
        let (workspace_id, surface_id) = {
            let mut model = model.lock().unwrap();
            let workspace = model.create_workspace("main", launch_dir.path());
            assert!(model.set_surface_cwd(
                &workspace.focused_surface_id,
                workload_dir.path().to_path_buf()
            ));
            (workspace.id, workspace.focused_surface_id)
        };
        terminal
            .spawn(SpawnRequest {
                surface_id: surface_id.clone(),
                workspace_id,
                shell: "/bin/sh".to_string(),
                args: Vec::new(),
                cwd: launch_dir.path().to_path_buf(),
                socket_path,
                extra_env: Vec::new(),
                eligible_for_pty_persistence: true,
            })
            .unwrap();
        let managed_socket =
            forktty_core::pty_persistence::session_socket_path(&runtime_dir, &surface_id).unwrap();
        forktty_core::pty_persistence::ensure_private_session_dir(&managed_socket).unwrap();
        let _managed_listener = std::os::unix::net::UnixListener::bind(managed_socket).unwrap();
        let client = SleepingTestChild::spawn_in(launch_dir.path());
        terminal.mark_surface_pid(&surface_id, client.id()).unwrap();

        let cwd = active_workspace_cwd_string(&state).unwrap();

        assert_eq!(cwd, workload_dir.path().to_string_lossy());
    });
}

#[test]
fn gtk_worktree_remove_matches_exact_name_and_target_path() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-exact-remove-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let unrelated_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (base_workspace, unrelated_workspace, target_workspace) = {
        let mut model = model.lock().unwrap();
        let base_workspace = model.create_workspace("repo", repo_dir.path());
        let unrelated_workspace = model.create_worktree_workspace(
            "unrelated",
            unrelated_dir.path(),
            "unrelated",
            &info.worktree_name,
        );
        let target_workspace = model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        );
        (base_workspace, unrelated_workspace, target_workspace)
    };
    for workspace in [&base_workspace, &unrelated_workspace, &target_workspace] {
        spawn_workspace_terminal_gtk(&state, workspace).unwrap();
    }

    with_isolated_user_dirs(|_| {
        remove_worktree_from_gtk(&state, &branch_name).unwrap();
    });

    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == unrelated_workspace.id));
    assert!(!model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == target_workspace.id));
    drop(model);
    let runtime_surface_ids = terminal
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert!(runtime_surface_ids.contains(&unrelated_workspace.focused_surface_id));
    assert!(!runtime_surface_ids.contains(&target_workspace.focused_surface_id));
}

#[cfg(unix)]
#[test]
fn gtk_worktree_remove_matches_missing_target_through_canonical_parent_symlink() {
    use std::os::unix::fs::symlink;

    let repo_dir = make_temp_repo();
    let real_parent = tempfile::tempdir().unwrap();
    let symlink_parent = repo_dir.path().join("linked-worktrees");
    symlink(real_parent.path(), &symlink_parent).unwrap();
    let branch_name = format!("feature/gtk-missing-remove-{}", std::process::id());
    let layout = symlink_parent.join("{name}").to_string_lossy().to_string();
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, &layout).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (base_workspace, target_workspace) = {
        let mut model = model.lock().unwrap();
        let base_workspace = model.create_workspace("repo", repo_dir.path());
        let target_workspace = model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        );
        (base_workspace, target_workspace)
    };
    spawn_workspace_terminal_gtk(&state, &base_workspace).unwrap();
    spawn_workspace_terminal_gtk(&state, &target_workspace).unwrap();
    model
        .lock()
        .unwrap()
        .select_workspace(WorkspaceSelector::Id(&base_workspace.id))
        .unwrap();
    std::fs::remove_dir_all(&info.path).unwrap();

    with_isolated_user_dirs(|_| {
        remove_worktree_from_gtk(&state, &branch_name).unwrap();
    });

    let model = model.lock().unwrap();
    assert!(!model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == target_workspace.id));
    drop(model);
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != target_workspace.focused_surface_id));
}

#[test]
fn gtk_worktree_remove_suppresses_modeled_surface_until_commit() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-suppress-remove-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(GtkBlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (base_workspace, target_workspace) = {
        let mut model = model.lock().unwrap();
        let base_workspace = model.create_workspace("repo", repo_dir.path());
        let target_workspace = model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        );
        (base_workspace, target_workspace)
    };
    spawn_workspace_terminal_gtk(&state, &base_workspace).unwrap();
    spawn_workspace_terminal_gtk(&state, &target_workspace).unwrap();
    let surface_id = target_workspace.focused_surface_id.clone();

    let remove_state = state.clone();
    let remove_name = branch_name.clone();
    let remove = std::thread::spawn(move || {
        with_isolated_user_dirs(|_| remove_worktree_from_gtk(&remove_state, &remove_name))
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("GTK removal should close the target runtime");

    let suppressed_while_modeled = state
        .suppressed_auto_spawn_surface_ids()
        .contains(&surface_id);
    let still_modeled = model.lock().unwrap().surface(&surface_id).is_some();
    let runtime_closed = terminal
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id);
    release_close_tx.send(()).unwrap();
    remove.join().unwrap().unwrap();

    assert!(still_modeled);
    assert!(runtime_closed);
    assert!(suppressed_while_modeled);
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[test]
fn gtk_worktree_remove_discards_replacement_when_target_closes_during_runtime_close() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/gtk-redundant-replacement-{}", std::process::id());
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, "nested").unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(GtkBlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let target_workspace = model.lock().unwrap().create_worktree_workspace(
        &info.branch,
        PathBuf::from(&info.path),
        &info.branch,
        &info.worktree_name,
    );
    spawn_workspace_terminal_gtk(&state, &target_workspace).unwrap();

    let remove_state = state.clone();
    let remove_name = branch_name.clone();
    let remove = std::thread::spawn(move || {
        with_isolated_user_dirs(|_| remove_worktree_from_gtk(&remove_state, &remove_name))
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("GTK removal should close the old runtime after spawning a replacement");

    let concurrent_workspace = {
        let mut model = model.lock().unwrap();
        let closed = model.close_workspace(WorkspaceSelector::Id(&target_workspace.id));
        assert!(closed.is_some());
        model.create_workspace("concurrent", repo_dir.path())
    };
    release_close_tx.send(()).unwrap();

    remove.join().unwrap().unwrap();

    let workspaces = model.lock().unwrap().list_workspaces();
    assert_eq!(workspaces.len(), 1);
    assert_eq!(workspaces[0].id, concurrent_workspace.id);
    let runtime_surface_ids = terminal
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        runtime_surface_ids,
        BTreeSet::from([concurrent_workspace.focused_surface_id])
    );
}

#[test]
fn gtk_worktree_remove_keeps_worktree_when_terminal_close_fails() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("feature/remove-close-fails-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    {
        let mut model = model.lock().unwrap();
        model.create_workspace("repo", repo_dir.path());
    }
    let terminal = Arc::new(CloseFailsBackend::default());
    let state = SocketAppState::new(
        model.clone(),
        terminal.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    with_isolated_user_dirs(|_| {
        open_worktree_from_gtk(&state, &branch_name, WorktreeAction::Create).unwrap();
    });
    let info = worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .into_iter()
        .find(|info| info.branch == branch_name)
        .unwrap();
    let (workspace_id, surface_id) = {
        let model = model.lock().unwrap();
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.worktree_name.as_deref() == Some(&info.worktree_name))
            .unwrap();
        let surface = model
            .list_surfaces(Some(&workspace.id))
            .into_iter()
            .next()
            .unwrap();
        (workspace.id, surface.id)
    };

    let error = remove_worktree_from_gtk(&state, &branch_name).unwrap_err();

    assert!(error.contains("close failed"));
    assert!(Path::new(&info.path).exists());
    assert_eq!(
        worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .len(),
        1
    );
    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
}

#[test]
fn validates_worktree_names_for_gtk_actions() {
    assert_eq!(
        validate_worktree_name_for_gtk(" feature/login ").unwrap(),
        "feature/login"
    );
    assert!(validate_worktree_name_for_gtk("../escape").is_err());
    assert!(validate_worktree_name_for_gtk("feature//empty").is_err());
    assert!(validate_worktree_name_for_gtk("feature\\windows").is_err());
    assert!(validate_worktree_name_for_gtk("").is_err());
}

#[test]
fn gtk_worktree_actions_require_active_workspace() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model,
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    for result in [
        active_workspace_cwd_string(&state),
        open_worktree_from_gtk(&state, "feature/test", WorktreeAction::Create)
            .map(|_| String::new()),
        merge_worktree_from_gtk(&state, "feature/test"),
        remove_worktree_from_gtk(&state, "feature/test").map(|_| String::new()),
    ] {
        assert!(result
            .unwrap_err()
            .contains("No active workspace is available"));
    }
}
