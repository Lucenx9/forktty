//! Worktree removal transaction, coordination, and rollback regression tests.

use super::*;
use crate::hook_session::hook_target_gate_for_surface;

#[cfg(unix)]
fn commit_signaling_setup_hook(dir: &Path, signal_fifo: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let hook_dir = dir.join(".forktty");
    fs::create_dir_all(&hook_dir).unwrap();
    let hook_path = hook_dir.join("setup");
    fs::write(
        &hook_path,
        format!("#!/bin/sh\nprintf x > '{}'\n", signal_fifo.display()),
    )
    .unwrap();
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

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
        "add signaling hook",
        &tree,
        &[&parent],
    )
    .unwrap();
}

#[derive(Debug)]
struct BlocksAfterCloseBackend {
    inner: HeadlessTerminalBackend,
    close_started: Mutex<Option<mpsc::Sender<()>>>,
    release_close: Mutex<mpsc::Receiver<()>>,
}

#[derive(Debug)]
struct PanicsAfterCloseBackend {
    inner: HeadlessTerminalBackend,
    panic_on_close: AtomicBool,
}

#[derive(Debug)]
struct PanicsOnRollbackSpawnBackend {
    inner: HeadlessTerminalBackend,
    close_completed: AtomicBool,
    panic_on_rollback_spawn: AtomicBool,
    panic_on_every_rollback_spawn: bool,
    restore_permissions: Mutex<Vec<PathBuf>>,
}

impl PanicsAfterCloseBackend {
    fn new() -> Self {
        Self {
            inner: HeadlessTerminalBackend::new(),
            panic_on_close: AtomicBool::new(true),
        }
    }
}

impl PanicsOnRollbackSpawnBackend {
    fn new(panic_on_every_rollback_spawn: bool) -> Self {
        Self {
            inner: HeadlessTerminalBackend::new(),
            close_completed: AtomicBool::new(false),
            panic_on_rollback_spawn: AtomicBool::new(true),
            panic_on_every_rollback_spawn,
            restore_permissions: Mutex::new(Vec::new()),
        }
    }

    fn restore_permissions_on_rollback(&self, paths: Vec<PathBuf>) {
        *self.restore_permissions.lock().unwrap() = paths;
    }
}

impl TerminalBackend for PanicsAfterCloseBackend {
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
        if self.panic_on_close.swap(false, Ordering::SeqCst) {
            panic!("injected panic after terminal close");
        }
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

impl TerminalBackend for PanicsOnRollbackSpawnBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        if self.close_completed.load(Ordering::SeqCst)
            && (self.panic_on_every_rollback_spawn
                || self.panic_on_rollback_spawn.swap(false, Ordering::SeqCst))
        {
            use std::os::unix::fs::PermissionsExt as _;

            for path in self.restore_permissions.lock().unwrap().iter() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            panic!("injected panic during runtime rollback");
        }
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
        self.close_completed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

impl BlocksAfterCloseBackend {
    fn new(close_started: mpsc::Sender<()>, release_close: mpsc::Receiver<()>) -> Self {
        Self {
            inner: HeadlessTerminalBackend::new(),
            close_started: Mutex::new(Some(close_started)),
            release_close: Mutex::new(release_close),
        }
    }
}

impl TerminalBackend for BlocksAfterCloseBackend {
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

#[derive(Debug, Default)]
struct FailsThirdCloseAndFirstRollbackSpawnBackend {
    inner: HeadlessTerminalBackend,
    close_count: AtomicUsize,
    rollback_spawn_count: AtomicUsize,
}

impl TerminalBackend for FailsThirdCloseAndFirstRollbackSpawnBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        if self.close_count.load(Ordering::SeqCst) >= 3
            && self.rollback_spawn_count.fetch_add(1, Ordering::SeqCst) == 0
        {
            return Err(TerminalError::Backend(
                "first rollback spawn failed".to_string(),
            ));
        }
        self.inner.spawn(request)
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        self.inner.send_text(surface_id, text)
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        self.inner.resize(surface_id, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        if self.close_count.fetch_add(1, Ordering::SeqCst) == 2 {
            return Err(TerminalError::Backend("third close failed".to_string()));
        }
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[derive(Debug)]
struct DirtyOnCloseBlocksRollbackBackend {
    inner: HeadlessTerminalBackend,
    dirty_on_close: Mutex<PathBuf>,
    close_completed: AtomicBool,
    restore_started: Mutex<Option<mpsc::Sender<()>>>,
    release_restore: Mutex<mpsc::Receiver<()>>,
}

impl DirtyOnCloseBlocksRollbackBackend {
    fn new(
        dirty_on_close: PathBuf,
        restore_started: mpsc::Sender<()>,
        release_restore: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            inner: HeadlessTerminalBackend::new(),
            dirty_on_close: Mutex::new(dirty_on_close),
            close_completed: AtomicBool::new(false),
            restore_started: Mutex::new(Some(restore_started)),
            release_restore: Mutex::new(release_restore),
        }
    }
}

impl TerminalBackend for DirtyOnCloseBlocksRollbackBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        if self.close_completed.load(Ordering::SeqCst) {
            if let Some(started) = self
                .restore_started
                .lock()
                .map_err(|_| TerminalError::LockPoisoned)?
                .take()
            {
                let _ = started.send(());
                self.release_restore
                    .lock()
                    .map_err(|_| TerminalError::LockPoisoned)?
                    .recv()
                    .map_err(|err| TerminalError::Backend(err.to_string()))?;
            }
        }
        self.inner.spawn(request)
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        self.inner.send_text(surface_id, text)
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        self.inner.resize(surface_id, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        let dirty_on_close = self
            .dirty_on_close
            .lock()
            .map_err(|_| TerminalError::LockPoisoned)?
            .clone();
        fs::write(dirty_on_close, "dirty\n")
            .map_err(|err| TerminalError::Backend(err.to_string()))?;
        self.inner.close(surface_id)?;
        self.close_completed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[tokio::test]
async fn worktree_remove_matches_exact_name_and_target_path() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-exact-remove-{}", std::process::id());
    let info = worktree::create(
        repo_dir.path().to_str().unwrap(),
        &branch_name,
        &path_resolver::worktree_layout(),
    )
    .unwrap();
    let unrelated_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();
    let (unrelated_workspace, target_workspace) = {
        let mut model = model.lock().unwrap();
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
        (unrelated_workspace, target_workspace)
    };
    spawn_workspace_terminal(&state, &unrelated_workspace).unwrap();
    spawn_workspace_terminal(&state, &target_workspace).unwrap();

    dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name, "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();

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
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert!(runtime_surface_ids.contains(&unrelated_workspace.focused_surface_id));
    assert!(!runtime_surface_ids.contains(&target_workspace.focused_surface_id));
}

#[cfg(unix)]
#[tokio::test]
async fn worktree_remove_matches_missing_target_through_canonical_parent_symlink() {
    use std::os::unix::fs::symlink;

    let repo_dir = make_temp_repo();
    let real_parent = tempfile::tempdir().unwrap();
    let symlink_parent = repo_dir.path().join("linked-worktrees");
    symlink(real_parent.path(), &symlink_parent).unwrap();
    let branch_name = format!("topic/socket-missing-remove-{}", std::process::id());
    let layout = symlink_parent.join("{name}").to_string_lossy().to_string();
    let info = worktree::create(repo_dir.path().to_str().unwrap(), &branch_name, &layout).unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();
    let target_workspace = {
        let mut model = model.lock().unwrap();
        model.create_worktree_workspace(
            &info.branch,
            PathBuf::from(&info.path),
            &info.branch,
            &info.worktree_name,
        )
    };
    spawn_workspace_terminal(&state, &target_workspace).unwrap();
    fs::remove_dir_all(&info.path).unwrap();

    dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();

    let model = model.lock().unwrap();
    assert!(!model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == target_workspace.id));
    drop(model);
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != target_workspace.focused_surface_id));
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_create_waits_for_remove_transaction_rollback_or_commit() {
    let (first_close_started_tx, first_close_started_rx) = mpsc::channel();
    let (release_first_close_tx, release_first_close_rx) = mpsc::channel();
    let (spawn_after_close_started_tx, spawn_after_close_started_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let remove_branch = format!("topic/socket-serialize-remove-{}", std::process::id());
    let create_branch = format!("topic/socket-serialize-create-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlockingFirstCloseBackend::new(
        first_close_started_tx,
        release_first_close_rx,
        spawn_after_close_started_tx,
    ));
    let state = SocketAppState::new(
        model,
        backend,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();
    dispatch(
        &state,
        "worktree.create",
        json!({"name": remove_branch, "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let fifo_dir = tempfile::tempdir().unwrap();
    let setup_started_fifo = fifo_dir.path().join("setup-started");
    make_fifo(&setup_started_fifo);
    commit_signaling_setup_hook(repo_dir.path(), &setup_started_fifo);
    let (setup_started_tx, setup_started_rx) = mpsc::channel();
    let setup_reader = std::thread::spawn(move || {
        use std::io::Read as _;

        let mut fifo = fs::File::open(setup_started_fifo).unwrap();
        let mut byte = [0_u8; 1];
        fifo.read_exact(&mut byte).unwrap();
        setup_started_tx.send(()).unwrap();
    });

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = remove_branch.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    first_close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should reach terminal close");

    let create_state = state.clone();
    let create_cwd = repo_dir.path().to_path_buf();
    let create_name = create_branch.clone();
    let create_start = Arc::new(tokio::sync::Barrier::new(2));
    let create_task_start = create_start.clone();
    let create = tokio::spawn(async move {
        create_task_start.wait().await;
        dispatch(
            &create_state,
            "worktree.create",
            json!({"name": create_name, "cwd": create_cwd}),
        )
        .await
    });
    create_start.wait().await;
    let create_mutated_git_before_remove_released = setup_started_rx
        .recv_timeout(Duration::from_millis(300))
        .is_ok();
    let create_spawned_before_remove_released = spawn_after_close_started_rx.try_recv().is_ok();
    release_first_close_tx.send(()).unwrap();
    remove.await.unwrap().unwrap();
    create.await.unwrap().unwrap();
    spawn_after_close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("Create should spawn only after Remove releases the transaction");
    setup_reader.join().unwrap();

    assert!(
        !create_mutated_git_before_remove_released,
        "Create must wait until Remove commits or fully rolls back"
    );
    assert!(
        !create_spawned_before_remove_released,
        "Create must not spawn while Remove owns the transaction"
    );
    let listed = worktree::list(repo_dir.path().to_str().unwrap()).unwrap();
    assert!(listed.iter().all(|info| info.branch != remove_branch));
    assert!(listed.iter().any(|info| info.branch == create_branch));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_remove_suppresses_modeled_surface_after_runtime_close() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-suppress-remove-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should close the target runtime");

    let suppressed_while_modeled = state
        .suppressed_auto_spawn_surface_ids()
        .contains(&surface_id);
    let still_modeled = model.lock().unwrap().surface(&surface_id).is_some();
    let runtime_closed = backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id);
    release_close_tx.send(()).unwrap();
    remove.await.unwrap().unwrap();

    assert!(still_modeled);
    assert!(runtime_closed);
    assert!(
        suppressed_while_modeled,
        "modeled surfaces must remain suppressed until removal commits"
    );
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_worktree_remove_still_commits_model_and_runtime_transaction() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-cancel-remove-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should close the target runtime");

    remove.abort();
    release_close_tx.send(()).unwrap();
    assert!(remove.await.unwrap_err().is_cancelled());

    let completed = (0..200).any(|_| {
        let modeled = model.lock().unwrap().surface(&surface_id).is_some();
        if modeled || !state.suppressed_auto_spawn_surface_ids().is_empty() {
            std::thread::sleep(Duration::from_millis(10));
            false
        } else {
            true
        }
    });

    assert!(
        completed,
        "cancelling the response future must not strand a modeled workspace after filesystem removal"
    );
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .iter()
        .all(|info| info.branch != branch_name));
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_worktree_remove_still_restores_runtime_after_filesystem_failure() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-cancel-remove-rollback-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should close the target runtime");

    fs::write(target_path.join("dirty-after-close.txt"), "dirty\n").unwrap();
    let poison_model = model.clone();
    assert!(std::thread::spawn(move || {
        let _model = poison_model.lock().unwrap();
        panic!("poison model before worktree rollback");
    })
    .join()
    .is_err());
    remove.abort();
    release_close_tx.send(()).unwrap();
    assert!(remove.await.unwrap_err().is_cancelled());

    let restored = (0..200).any(|_| {
        let runtime_restored = backend
            .surfaces()
            .unwrap()
            .iter()
            .any(|surface| surface.surface_id == surface_id);
        if runtime_restored && state.suppressed_auto_spawn_surface_ids().is_empty() {
            true
        } else {
            std::thread::sleep(Duration::from_millis(10));
            false
        }
    });

    assert!(
        restored,
        "cancelling the response future must not skip runtime rollback after filesystem failure"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_remove_error_after_target_deletion_still_commits_model() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-delete-then-error-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_name = created["worktree_name"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should close the target runtime");

    fs::remove_dir_all(&target_path).unwrap();
    fs::remove_dir_all(repo_dir.path().join(".git/worktrees").join(worktree_name)).unwrap();
    release_close_tx.send(()).unwrap();
    let error = remove.await.unwrap().unwrap_err().to_string();

    assert!(error.contains("modeled removal was committed"), "{error}");
    assert!(model.lock().unwrap().surface(&surface_id).is_none());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_remove_commits_when_an_initially_missing_target_is_pruned_concurrently() {
    let (close_started_tx, close_started_rx) = mpsc::channel();
    let (release_close_tx, release_close_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-missing-pruned-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(BlocksAfterCloseBackend::new(
        close_started_tx,
        release_close_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_name = created["worktree_name"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    fs::remove_dir_all(&target_path).unwrap();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    close_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("worktree.remove should close the target runtime");

    fs::remove_dir_all(repo_dir.path().join(".git/worktrees").join(worktree_name)).unwrap();
    release_close_tx.send(()).unwrap();
    let error = remove.await.unwrap().unwrap_err().to_string();

    assert!(error.contains("modeled removal was committed"), "{error}");
    assert!(model.lock().unwrap().surface(&surface_id).is_none());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_panic_after_runtime_close_completes_rollback() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-panic-close-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(PanicsAfterCloseBackend::new());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic after terminal close"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .iter()
        .any(|info| info.branch == branch_name));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_missing_target_panic_before_filesystem_keeps_registration() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-missing-panic-close-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(PanicsAfterCloseBackend::new());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_name = created["worktree_name"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    fs::remove_dir_all(&target_path).unwrap();

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic after terminal close"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(
        repo_dir
            .path()
            .join(".git/worktrees")
            .join(worktree_name)
            .is_dir(),
        "panic before filesystem removal must keep the Git worktree registration"
    );
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_missing_target_rollback_panic_does_not_retry_filesystem() {
    use std::os::unix::fs::PermissionsExt as _;

    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-missing-panic-rollback-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(PanicsOnRollbackSpawnBackend::new(false));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_name = created["worktree_name"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    let registrations_dir = repo_dir.path().join(".git/worktrees");
    let registration_dir = registrations_dir.join(worktree_name);
    fs::remove_dir_all(&target_path).unwrap();
    fs::set_permissions(&registration_dir, fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(&registrations_dir, fs::Permissions::from_mode(0o500)).unwrap();
    backend.restore_permissions_on_rollback(vec![registrations_dir, registration_dir.clone()]);

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic during runtime rollback"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(
        registration_dir.is_dir(),
        "panic during rollback must not retry and prune the Git registration"
    );
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_persistent_restore_panic_records_blocking_surface_status() {
    use std::os::unix::fs::PermissionsExt as _;

    let repo_dir = make_temp_repo();
    let branch_name = format!(
        "topic/socket-persistent-restore-panic-{}",
        std::process::id()
    );
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(PanicsOnRollbackSpawnBackend::new(true));
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_name = created["worktree_name"].as_str().unwrap().to_string();
    let target_path = PathBuf::from(created["path"].as_str().unwrap());
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    let registrations_dir = repo_dir.path().join(".git/worktrees");
    let registration_dir = registrations_dir.join(worktree_name);
    fs::remove_dir_all(&target_path).unwrap();
    fs::set_permissions(&registration_dir, fs::Permissions::from_mode(0o500)).unwrap();
    fs::set_permissions(&registrations_dir, fs::Permissions::from_mode(0o500)).unwrap();
    backend.restore_permissions_on_rollback(vec![registrations_dir, registration_dir.clone()]);

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic during runtime rollback"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_some());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(registration_dir.is_dir());
    let model = model.lock().unwrap();
    let blocker = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|status| status.key == surface_status_key(&surface_id))
        .expect("persistent rollback panic must record a surface-scoped blocker");
    assert_eq!(blocker.label, "Terminal");
    assert_eq!(blocker.color.as_deref(), Some("red"));
    assert!(blocker.value.starts_with("Spawn failed:"));
    drop(model);
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_remove_panic_restores_surface_snapshot_taken_after_hook_quiescence() {
    let repo_dir = make_temp_repo();
    let resumed_dir = tempfile::tempdir().unwrap();
    let branch_name = format!("topic/socket-panic-fresh-snapshot-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(PanicsAfterCloseBackend::new());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    let target_gate = hook_target_gate_for_surface(&state, &surface_id).unwrap();
    let target_guard = target_gate.lock().unwrap();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    assert!(target_gate.wait_until_contended(Duration::from_secs(2)));
    assert!(model
        .lock()
        .unwrap()
        .set_surface_cwd(&surface_id, resumed_dir.path().to_path_buf()));
    drop(target_guard);

    let error = remove.await.unwrap().unwrap_err().to_string();

    assert!(
        error.contains("injected panic after terminal close"),
        "{error}"
    );
    let restored = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("panic recovery should restore the target runtime");
    assert_eq!(restored.cwd, resumed_dir.path());
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_panic_after_filesystem_finish_completes_model_commit() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-panic-finish-{}", std::process::id());
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
    let created = dispatch(
        &state,
        "worktree.create",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = state
        .model
        .lock()
        .unwrap()
        .list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    state.panic_after_worktree_filesystem_finish_and_poison_model_once();

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic after worktree filesystem finish with model poison"),
        "{error}"
    );
    assert!(state.model.lock().unwrap().surface(&surface_id).is_none());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .iter()
        .all(|info| info.branch != branch_name));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_recovers_and_clears_hook_gate_poison_during_commit() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-poison-gate-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    let target_gate = hook_target_gate_for_surface(&state, &surface_id).unwrap();
    target_gate.panic_next_completion_lock();

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("injected panic while reacquiring a removal hook target gate"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_none());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(target_gate.lock().is_ok(), "gate poison must be cleared");
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_commit_recovers_hook_target_store_poison() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-poison-targets-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    let targets = state.hook_session_targets.clone();
    assert!(std::thread::spawn(move || {
        let _targets = targets.lock().unwrap();
        panic!("poison hook session target store before removal commit");
    })
    .join()
    .is_err());

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("hook session target lock was poisoned"),
        "{error}"
    );
    assert!(model.lock().unwrap().surface(&surface_id).is_none());
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));
    assert!(state.hook_session_targets.lock().is_ok());
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
}

#[tokio::test]
async fn worktree_remove_partial_close_restores_each_possible_runtime_and_blocks_missing_ones() {
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-partial-remove-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailsThirdCloseAndFirstRollbackSpawnBackend::default());
    let state = SocketAppState::new(
        model.clone(),
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
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let target_surfaces = {
        let mut model = model.lock().unwrap();
        let first = model.list_surfaces(Some(&workspace_id))[0].clone();
        let second = model
            .split_surface(&first.id, forktty_core::SplitAxis::Horizontal)
            .unwrap();
        model
            .split_surface(&second.id, forktty_core::SplitAxis::Vertical)
            .unwrap();
        model.list_surfaces(Some(&workspace_id))
    };
    for surface in target_surfaces.iter().skip(1) {
        spawn_surface_terminal(&state, surface).unwrap();
    }

    let error = dispatch(
        &state,
        "worktree.remove",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("third close failed"));
    assert!(error.contains("first rollback spawn failed"));
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert!(!runtime_surface_ids.contains(&target_surfaces[0].id));
    assert!(
        runtime_surface_ids.contains(&target_surfaces[1].id),
        "a later rollback spawn must still run after an earlier one fails"
    );
    assert!(runtime_surface_ids.contains(&target_surfaces[2].id));
    let model = model.lock().unwrap();
    let statuses = model.list_status(&workspace_id);
    let blocked = statuses
        .iter()
        .find(|status| status.key == surface_status_key(&target_surfaces[0].id))
        .expect("missing rollback runtime should get an independent blocking status");
    assert_eq!(blocked.label, "Terminal");
    assert_eq!(blocked.color.as_deref(), Some("red"));
    assert!(blocked.value.starts_with("Spawn failed"));
    drop(model);
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
    assert!(Path::new(created["path"].as_str().unwrap()).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_remove_finish_failure_restores_selection_and_runtime_before_unsuppressing() {
    let (restore_started_tx, restore_started_rx) = mpsc::channel();
    let (release_restore_tx, release_restore_rx) = mpsc::channel();
    let repo_dir = make_temp_repo();
    let branch_name = format!("topic/socket-finish-rollback-{}", std::process::id());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(DirtyOnCloseBlocksRollbackBackend::new(
        repo_dir.path().join(".worktrees-placeholder"),
        restore_started_tx,
        release_restore_rx,
    ));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();
    let base_workspace_id = model.lock().unwrap().active_workspace_id().unwrap();
    let created = dispatch(
        &state,
        "worktree.create",
        json!({"name": branch_name.as_str(), "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let surface_id = model.lock().unwrap().list_surfaces(Some(&workspace_id))[0]
        .id
        .clone();
    *backend.dirty_on_close.lock().unwrap() =
        PathBuf::from(created["path"].as_str().unwrap()).join("dirty-after-close.txt");
    model
        .lock()
        .unwrap()
        .select_workspace(forktty_core::WorkspaceSelector::Id(&base_workspace_id))
        .unwrap();

    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove_name = branch_name.clone();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({"name": remove_name, "cwd": remove_cwd}),
        )
        .await
    });
    restore_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("finish failure should begin explicit runtime restore");

    let suppressed_during_restore = state
        .suppressed_auto_spawn_surface_ids()
        .contains(&surface_id);
    let (still_modeled, active_workspace_id) = {
        let model = model.lock().unwrap();
        (
            model.surface(&surface_id).is_some(),
            model.active_workspace_id(),
        )
    };
    let runtime_missing_during_restore = backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id);
    release_restore_tx.send(()).unwrap();
    let error = remove.await.unwrap().unwrap_err().to_string();

    assert!(error.contains("uncommitted changes"));
    assert!(suppressed_during_restore);
    assert!(still_modeled);
    assert!(runtime_missing_during_restore);
    assert_eq!(
        active_workspace_id.as_deref(),
        Some(base_workspace_id.as_str())
    );
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
    assert!(state.suppressed_auto_spawn_surface_ids().is_empty());
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
