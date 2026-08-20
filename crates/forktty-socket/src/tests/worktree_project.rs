//! Worktree and project-action socket method regression tests.

use super::*;

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

#[cfg(unix)]
fn commit_blocking_setup_hook(dir: &Path, started_fifo: &Path, release_fifo: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let hook_dir = dir.join(".forktty");
    fs::create_dir_all(&hook_dir).unwrap();
    let hook_path = hook_dir.join("setup");
    fs::write(
        &hook_path,
        format!(
            "#!/bin/sh\nprintf x > '{}'\nread _ < '{}'\n",
            started_fifo.display(),
            release_fifo.display()
        ),
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
        "add blocking setup hook",
        &tree,
        &[&parent],
    )
    .unwrap();
}

#[cfg(unix)]
fn commit_blocking_teardown_hook(dir: &Path, started_fifo: &Path, release_fifo: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let hook_dir = dir.join(".forktty");
    fs::create_dir_all(&hook_dir).unwrap();
    let hook_path = hook_dir.join("teardown");
    fs::write(
        &hook_path,
        format!(
            "#!/bin/sh\nprintf x > '{}'\nread _ < '{}'\n",
            started_fifo.display(),
            release_fifo.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755)).unwrap();

    let repo = Repository::open(dir).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(".forktty/teardown")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("ForkTTY Tests", "tests@forktty.local").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "add blocking teardown hook",
        &tree,
        &[&parent],
    )
    .unwrap();
}

#[derive(Debug, Default)]
struct FailingSurfaceInventoryBackend;

impl TerminalBackend for FailingSurfaceInventoryBackend {
    fn spawn(&self, _request: SpawnRequest) -> Result<(), TerminalError> {
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
        Err(TerminalError::Backend(
            "surface inventory failed".to_string(),
        ))
    }
}

#[derive(Debug)]
struct FailsNthSpawnBackend {
    inner: HeadlessTerminalBackend,
    spawn_count: AtomicUsize,
    fail_on_spawn: usize,
}

impl FailsNthSpawnBackend {
    fn with_initial_surface(surface: &forktty_core::Surface, fail_on_spawn: usize) -> Self {
        let inner = HeadlessTerminalBackend::new();
        inner
            .spawn(SpawnRequest::for_surface(
                surface,
                "/bin/sh",
                PathBuf::from("/tmp/forktty.sock"),
            ))
            .unwrap();
        Self {
            inner,
            spawn_count: AtomicUsize::new(0),
            fail_on_spawn,
        }
    }
}

impl TerminalBackend for FailsNthSpawnBackend {
    fn spawn(&self, request: SpawnRequest) -> Result<(), TerminalError> {
        let spawn_number = self.spawn_count.fetch_add(1, Ordering::SeqCst) + 1;
        if spawn_number == self.fail_on_spawn {
            return Err(TerminalError::Backend(format!(
                "spawn {spawn_number} failed"
            )));
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
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
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
async fn worktree_create_repeatedly_returns_same_workspace_with_one_surface() {
    let repo_dir = make_temp_repo();
    let (state, backend) = test_state();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "repo", "workingDir": repo_dir.path()}),
    )
    .await
    .unwrap();

    let first = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/idempotent", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let second = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/idempotent", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();

    assert_eq!(second["id"], first["id"]);
    let workspace_id = first["id"].as_str().unwrap();
    let model = state.model.lock().unwrap();
    assert_eq!(model.surface_count(Some(workspace_id)), 1);
    assert_eq!(
        model
            .list_workspaces()
            .iter()
            .filter(|workspace| {
                workspace.worktree_name.as_deref() == first["worktree_name"].as_str()
            })
            .count(),
        1
    );
    drop(model);
    assert_eq!(
        backend
            .surfaces()
            .unwrap()
            .iter()
            .filter(|surface| surface.workspace_id == workspace_id)
            .count(),
        1
    );
}

#[tokio::test]
async fn worktree_attach_after_create_returns_same_workspace() {
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
        json!({"name": "topic/create-attach", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let attached = dispatch(
        &state,
        "worktree.attach",
        json!({"branch": "topic/create-attach", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();

    assert_eq!(attached["id"], created["id"]);
    let workspace_id = created["id"].as_str().unwrap();
    assert_eq!(
        backend
            .surfaces()
            .unwrap()
            .iter()
            .filter(|surface| surface.workspace_id == workspace_id)
            .count(),
        1
    );
}

#[tokio::test]
async fn concurrent_worktree_create_requests_converge_on_one_workspace() {
    const BLOCKED_WAIT: Duration = Duration::from_millis(100);

    let repo_dir = make_temp_repo();
    let (state, backend) = test_state();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "repo", "workingDir": repo_dir.path()}),
    )
    .await
    .unwrap();

    let writer = state.worktree_write_guard().await;
    let start = Arc::new(tokio::sync::Barrier::new(3));
    let first_state = state.clone();
    let first_cwd = repo_dir.path().to_path_buf();
    let first_start = start.clone();
    let mut first = tokio::spawn(async move {
        first_start.wait().await;
        dispatch(
            &first_state,
            "worktree.create",
            json!({"name": "topic/concurrent", "cwd": first_cwd}),
        )
        .await
    });
    let second_state = state.clone();
    let second_cwd = repo_dir.path().to_path_buf();
    let second_start = start.clone();
    let mut second = tokio::spawn(async move {
        second_start.wait().await;
        dispatch(
            &second_state,
            "worktree.create",
            json!({"name": "topic/concurrent", "cwd": second_cwd}),
        )
        .await
    });
    start.wait().await;
    assert!(tokio::time::timeout(BLOCKED_WAIT, &mut first)
        .await
        .is_err());
    assert!(tokio::time::timeout(BLOCKED_WAIT, &mut second)
        .await
        .is_err());
    drop(writer);
    let (first, second) = tokio::join!(first, second);
    let first = first.unwrap().unwrap();
    let second = second.unwrap().unwrap();

    assert_eq!(second["id"], first["id"]);
    let workspace_id = first["id"].as_str().unwrap();
    let model = state.model.lock().unwrap();
    assert_eq!(model.surface_count(Some(workspace_id)), 1);
    assert_eq!(
        model
            .list_workspaces()
            .iter()
            .filter(|workspace| workspace.git_branch == "topic/concurrent")
            .count(),
        1
    );
    drop(model);
    assert_eq!(
        backend
            .surfaces()
            .unwrap()
            .iter()
            .filter(|surface| surface.workspace_id == workspace_id)
            .count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_worktree_create_keeps_writer_until_model_and_runtime_commit() {
    use std::io::{Read as _, Write as _};

    const BLOCKED_WAIT: Duration = Duration::from_millis(100);
    const COMPLETION_WAIT: Duration = Duration::from_secs(5);

    let repo_dir = make_temp_repo();
    let fifo_dir = tempfile::tempdir().unwrap();
    let started_fifo = fifo_dir.path().join("socket-create-started");
    let release_fifo = fifo_dir.path().join("socket-create-release");
    for fifo in [&started_fifo, &release_fifo] {
        make_fifo(fifo);
    }
    commit_blocking_setup_hook(repo_dir.path(), &started_fifo, &release_fifo);

    let (state, backend) = test_state();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "repo", "workingDir": repo_dir.path()}),
    )
    .await
    .unwrap();

    let (setup_started_tx, setup_started_rx) = mpsc::channel();
    let started_reader = std::thread::spawn(move || {
        let mut fifo = fs::File::open(started_fifo).unwrap();
        let mut byte = [0_u8; 1];
        fifo.read_exact(&mut byte).unwrap();
        setup_started_tx.send(()).unwrap();
    });

    let create_state = state.clone();
    let create_cwd = repo_dir.path().to_path_buf();
    let create = tokio::spawn(async move {
        dispatch(
            &create_state,
            "worktree.create",
            json!({"name": "topic/cancelled-create", "cwd": create_cwd}),
        )
        .await
    });
    tokio::time::timeout(
        COMPLETION_WAIT,
        tokio::task::spawn_blocking(move || setup_started_rx.recv()),
    )
    .await
    .expect("worktree.create should reach its setup hook")
    .unwrap()
    .unwrap();
    create.abort();
    assert!(create.await.unwrap_err().is_cancelled());

    let writer_state = state.clone();
    let mut competing_writer =
        tokio::spawn(async move { writer_state.worktree_write_guard().await });
    let writer_before_release = tokio::time::timeout(BLOCKED_WAIT, &mut competing_writer).await;
    let writer_was_retained = writer_before_release.is_err();

    let mut release = fs::OpenOptions::new()
        .write(true)
        .open(release_fifo)
        .unwrap();
    release.write_all(b"x\n").unwrap();
    started_reader.join().unwrap();

    let writer = match writer_before_release {
        Ok(writer) => writer.unwrap(),
        Err(_) => tokio::time::timeout(COMPLETION_WAIT, competing_writer)
            .await
            .expect("competing writer should proceed after cancelled create completes")
            .unwrap(),
    };
    assert!(
        writer_was_retained,
        "caller cancellation must not release the worktree writer while Git mutation continues"
    );

    let workspace = state
        .model
        .lock()
        .unwrap()
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.git_branch == "topic/cancelled-create")
        .expect("cancelled requester must not cancel model commit");
    assert_eq!(
        state
            .model
            .lock()
            .unwrap()
            .list_surfaces(Some(&workspace.id))
            .len(),
        1
    );
    assert_eq!(
        backend
            .surfaces()
            .unwrap()
            .into_iter()
            .filter(|surface| surface.workspace_id == workspace.id)
            .count(),
        1,
        "cancelled requester must not cancel runtime commit"
    );
    drop(writer);
}

#[tokio::test]
async fn cancelled_worktree_read_keeps_guard_until_blocking_read_finishes() {
    const BLOCKED_WAIT: Duration = Duration::from_millis(100);
    const COMPLETION_WAIT: Duration = Duration::from_secs(2);

    let (state, _backend) = test_state();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let read_state = state.clone();
    let read = tokio::spawn(async move {
        run_guarded_worktree_read(&read_state, move || {
            let _ = started_tx.send(());
            release_rx.recv().unwrap();
            Ok(())
        })
        .await
    });
    tokio::time::timeout(COMPLETION_WAIT, started_rx)
        .await
        .expect("guarded worktree read should start")
        .unwrap();
    read.abort();
    assert!(read.await.unwrap_err().is_cancelled());

    let writer_state = state.clone();
    let mut writer = tokio::spawn(async move { writer_state.worktree_write_guard().await });
    assert!(
        tokio::time::timeout(BLOCKED_WAIT, &mut writer)
            .await
            .is_err(),
        "caller cancellation must not release a still-running worktree read"
    );

    release_tx.send(()).unwrap();
    let _writer = tokio::time::timeout(COMPLETION_WAIT, writer)
        .await
        .expect("writer should proceed after guarded read finishes")
        .unwrap();
}

#[tokio::test]
async fn cancelled_worktree_write_keeps_guard_until_blocking_mutation_finishes() {
    const BLOCKED_WAIT: Duration = Duration::from_millis(100);
    const COMPLETION_WAIT: Duration = Duration::from_secs(2);

    let (state, _backend) = test_state();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mutation_state = state.clone();
    let mutation = tokio::spawn(async move {
        run_guarded_worktree_write(&mutation_state, move || {
            let _ = started_tx.send(());
            release_rx.recv().unwrap();
            Ok(())
        })
        .await
    });
    tokio::time::timeout(COMPLETION_WAIT, started_rx)
        .await
        .expect("guarded worktree mutation should start")
        .unwrap();
    mutation.abort();
    assert!(mutation.await.unwrap_err().is_cancelled());

    let reader_state = state.clone();
    let mut reader = tokio::spawn(async move { reader_state.worktree_read_guard().await });
    assert!(
        tokio::time::timeout(BLOCKED_WAIT, &mut reader)
            .await
            .is_err(),
        "caller cancellation must not release a still-running worktree mutation"
    );

    release_tx.send(()).unwrap();
    let _reader = tokio::time::timeout(COMPLETION_WAIT, reader)
        .await
        .expect("reader should proceed after guarded mutation finishes")
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_worktree_remove_keeps_writer_during_teardown_and_commits_removal() {
    use std::io::{Read as _, Write as _};

    const BLOCKED_WAIT: Duration = Duration::from_millis(100);
    const COMPLETION_WAIT: Duration = Duration::from_secs(5);

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
        json!({"name": "topic/cancelled-remove-preflight", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_path = PathBuf::from(created["path"].as_str().unwrap());

    let fifo_dir = tempfile::tempdir().unwrap();
    let started_fifo = fifo_dir.path().join("socket-remove-started");
    let release_fifo = fifo_dir.path().join("socket-remove-release");
    for fifo in [&started_fifo, &release_fifo] {
        make_fifo(fifo);
    }
    commit_blocking_teardown_hook(&worktree_path, &started_fifo, &release_fifo);

    let (teardown_started_tx, teardown_started_rx) = mpsc::channel();
    let started_reader = std::thread::spawn(move || {
        let mut fifo = fs::File::open(started_fifo).unwrap();
        let mut byte = [0_u8; 1];
        fifo.read_exact(&mut byte).unwrap();
        teardown_started_tx.send(()).unwrap();
    });
    let remove_state = state.clone();
    let remove_cwd = repo_dir.path().to_path_buf();
    let remove = tokio::spawn(async move {
        dispatch(
            &remove_state,
            "worktree.remove",
            json!({
                "name": "topic/cancelled-remove-preflight",
                "cwd": remove_cwd,
            }),
        )
        .await
    });
    tokio::time::timeout(
        COMPLETION_WAIT,
        tokio::task::spawn_blocking(move || teardown_started_rx.recv()),
    )
    .await
    .expect("worktree.remove should reach its teardown hook")
    .unwrap()
    .unwrap();
    remove.abort();
    assert!(remove.await.unwrap_err().is_cancelled());

    let reader_state = state.clone();
    let mut competing_reader =
        tokio::spawn(async move { reader_state.worktree_read_guard().await });
    assert!(
        tokio::time::timeout(BLOCKED_WAIT, &mut competing_reader)
            .await
            .is_err(),
        "caller cancellation must retain the writer while teardown is running"
    );

    let mut release = fs::OpenOptions::new()
        .write(true)
        .open(release_fifo)
        .unwrap();
    release.write_all(b"x\n").unwrap();
    started_reader.join().unwrap();
    let _reader = tokio::time::timeout(COMPLETION_WAIT, competing_reader)
        .await
        .expect("reader should proceed after cancelled remove commits")
        .unwrap();

    assert!(!worktree_path.exists());
    assert!(state
        .model
        .lock()
        .unwrap()
        .list_workspaces()
        .iter()
        .all(|workspace| workspace.id != workspace_id));
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.workspace_id != workspace_id));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn existing_worktree_workspace_spawns_every_missing_runtime_surface() {
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let healthy_state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&healthy_state, repo_dir.path().to_path_buf()).unwrap();
    let created = dispatch(
        &healthy_state,
        "worktree.create",
        json!({"name": "topic/missing-surfaces", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let expected_surface_ids = {
        let mut model = model.lock().unwrap();
        let initial_surface_id = model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .next()
            .unwrap()
            .id;
        let second = model
            .split_surface(&initial_surface_id, forktty_core::SplitAxis::Horizontal)
            .unwrap();
        model
            .split_surface(&second.id, forktty_core::SplitAxis::Vertical)
            .unwrap();
        model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>()
    };
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let reopened = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/missing-surfaces", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();

    assert_eq!(reopened["id"], created["id"]);
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(runtime_surface_ids, expected_surface_ids);
}

#[tokio::test]
async fn existing_worktree_partial_spawn_failure_preserves_preexisting_runtime() {
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let healthy_state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&healthy_state, repo_dir.path().to_path_buf()).unwrap();
    let created = dispatch(
        &healthy_state,
        "worktree.create",
        json!({"name": "topic/partial-spawn", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let (preexisting_surface, expected_surface_ids, previous_workspace_id) = {
        let mut model = model.lock().unwrap();
        let preexisting_surface = model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .next()
            .unwrap();
        let second = model
            .split_surface(&preexisting_surface.id, forktty_core::SplitAxis::Horizontal)
            .unwrap();
        model
            .split_surface(&second.id, forktty_core::SplitAxis::Vertical)
            .unwrap();
        let expected_surface_ids = model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>();
        let previous_workspace_id = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id != workspace_id)
            .unwrap()
            .id;
        model
            .select_workspace(forktty_core::WorkspaceSelector::Id(&previous_workspace_id))
            .unwrap();
        (
            preexisting_surface,
            expected_surface_ids,
            previous_workspace_id,
        )
    };
    let backend = Arc::new(FailsNthSpawnBackend::with_initial_surface(
        &preexisting_surface,
        2,
    ));
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/partial-spawn", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("spawn 2 failed"));
    let runtime_surface_ids = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| surface.surface_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        runtime_surface_ids,
        BTreeSet::from([preexisting_surface.id])
    );
    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(previous_workspace_id.as_str())
    );
    assert_eq!(
        model
            .list_surfaces(Some(&workspace_id))
            .into_iter()
            .map(|surface| surface.id)
            .collect::<BTreeSet<_>>(),
        expected_surface_ids
    );
    drop(model);
    assert_eq!(
        worktree::list(repo_dir.path().to_str().unwrap())
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn existing_worktree_workspace_survives_missing_runtime_spawn_failure() {
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let healthy_state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&healthy_state, repo_dir.path().to_path_buf()).unwrap();
    let created = dispatch(
        &healthy_state,
        "worktree.create",
        json!({"name": "topic/existing-workspace", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_path = created["path"].as_str().unwrap().to_string();
    let previous_workspace_id = {
        let mut model = model.lock().unwrap();
        let previous_workspace_id = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id != workspace_id)
            .unwrap()
            .id;
        model
            .select_workspace(forktty_core::WorkspaceSelector::Id(&previous_workspace_id))
            .unwrap();
        previous_workspace_id
    };

    let failing_state = SocketAppState::new(
        model.clone(),
        Arc::new(FailingSpawnBackend),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let error = dispatch(
        &failing_state,
        "worktree.create",
        json!({"name": "topic/existing-workspace", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("spawn failed"));
    assert!(Path::new(&worktree_path).exists());
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
    assert_eq!(model.surface_count(Some(&workspace_id)), 1);
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(previous_workspace_id.as_str())
    );
}

#[tokio::test]
async fn existing_worktree_workspace_restores_active_after_runtime_inventory_failure() {
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let healthy_state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&healthy_state, repo_dir.path().to_path_buf()).unwrap();
    let created = dispatch(
        &healthy_state,
        "worktree.create",
        json!({"name": "topic/existing-inventory", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap();
    let workspace_id = created["id"].as_str().unwrap().to_string();
    let worktree_path = created["path"].as_str().unwrap().to_string();
    let previous_workspace_id = {
        let mut model = model.lock().unwrap();
        let previous_workspace_id = model
            .list_workspaces()
            .into_iter()
            .find(|workspace| workspace.id != workspace_id)
            .unwrap()
            .id;
        model
            .select_workspace(forktty_core::WorkspaceSelector::Id(&previous_workspace_id))
            .unwrap();
        previous_workspace_id
    };
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(FailingSurfaceInventoryBackend),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/existing-inventory", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("surface inventory failed"));
    assert!(Path::new(&worktree_path).exists());
    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(previous_workspace_id.as_str())
    );
}

#[tokio::test]
async fn new_worktree_workspace_rolls_back_when_runtime_inventory_fails() {
    let repo_dir = make_temp_repo();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let bootstrap_state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&bootstrap_state, repo_dir.path().to_path_buf()).unwrap();
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(FailingSurfaceInventoryBackend),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);

    let error = dispatch(
        &state,
        "worktree.create",
        json!({"name": "topic/inventory-failure", "cwd": repo_dir.path()}),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("surface inventory failed"));
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
    let model = model.lock().unwrap();
    assert_eq!(model.list_workspaces().len(), 1);
    assert!(model
        .list_workspaces()
        .iter()
        .all(|workspace| workspace.git_branch != "topic/inventory-failure"));
}

#[tokio::test]
async fn worktree_list_and_status_wait_for_writer_and_share_read_access() {
    const BLOCKED_WAIT: Duration = Duration::from_millis(100);
    const COMPLETION_WAIT: Duration = Duration::from_secs(2);

    let repo_dir = make_temp_repo();
    let (state, _backend) = test_state();
    dispatch(
        &state,
        "workspace.create",
        json!({"name": "repo", "workingDir": repo_dir.path()}),
    )
    .await
    .unwrap();

    let writer = state.worktree_write_guard().await;
    let list_state = state.clone();
    let list_cwd = repo_dir.path().to_path_buf();
    let mut list_task = tokio::spawn(async move {
        dispatch(&list_state, "worktree.list", json!({"cwd": list_cwd})).await
    });
    let status_state = state.clone();
    let status_path = repo_dir.path().to_path_buf();
    let mut status_task = tokio::spawn(async move {
        dispatch(
            &status_state,
            "worktree.status",
            json!({"path": status_path}),
        )
        .await
    });
    assert!(tokio::time::timeout(BLOCKED_WAIT, &mut list_task)
        .await
        .is_err());
    assert!(tokio::time::timeout(BLOCKED_WAIT, &mut status_task)
        .await
        .is_err());
    drop(writer);
    tokio::time::timeout(COMPLETION_WAIT, list_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(COMPLETION_WAIT, status_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let reader = state.worktree_read_guard().await;
    let list_state = state.clone();
    let list_cwd = repo_dir.path().to_path_buf();
    let list_task = tokio::spawn(async move {
        dispatch(&list_state, "worktree.list", json!({"cwd": list_cwd})).await
    });
    let status_state = state.clone();
    let status_path = repo_dir.path().to_path_buf();
    let status_task = tokio::spawn(async move {
        dispatch(
            &status_state,
            "worktree.status",
            json!({"path": status_path}),
        )
        .await
    });
    tokio::time::timeout(COMPLETION_WAIT, list_task)
        .await
        .expect("worktree.list should share worktree read access")
        .unwrap()
        .unwrap();
    tokio::time::timeout(COMPLETION_WAIT, status_task)
        .await
        .expect("worktree.status should share worktree read access")
        .unwrap()
        .unwrap();
    drop(reader);
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
async fn project_action_program_resolution_failure_rolls_back_modeled_surface() {
    let repo_dir = make_temp_repo();
    fs::write(
        repo_dir.path().join("forktty.json"),
        r#"{"actions":[{"id":"missing","label":"Missing","argv":["./missing-tool"],"cwd":"."}]}"#,
    )
    .unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let runtime_dir = super::test_runtime::private_runtime_tempdir("forktty-project-action-");
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(HeadlessTerminalBackend::new()),
        "/bin/sh",
        runtime_dir.path().join("forktty.sock"),
    )
    .with_notification_dispatch(false);
    bootstrap_default_workspace(&state, repo_dir.path().to_path_buf()).unwrap();
    let before = model.lock().unwrap().list_surfaces(None);

    let error = dispatch(
        &state,
        "project.action.run",
        json!({"cwd": repo_dir.path(), "id": "missing"}),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code(), "precondition_failed");
    assert!(error
        .to_string()
        .contains("Cannot resolve project action program"));
    assert_eq!(model.lock().unwrap().list_surfaces(None), before);
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
