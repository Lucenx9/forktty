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

#[test]
fn close_worktree_workspace_keeps_model_when_backend_close_fails() {
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_path_buf();
    let fallback_dir = tempfile::tempdir().unwrap();
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let (tx, rx) = mpsc::channel();
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
        let workspace = model.create_worktree_workspace(
            "feature/test",
            &project_cwd,
            "feature/test",
            "feature-test",
        );
        (workspace.id, workspace.focused_surface_id)
    };
    spawn_focused_surface_if_needed(&state).unwrap();
    drop(rx);

    let error =
        close_workspace_by_worktree_name(&state, "feature-test", fallback_dir.path().into())
            .unwrap_err()
            .to_string();

    assert!(error.contains("sending on a closed channel"));
    let model = model.lock().unwrap();
    assert!(model
        .list_workspaces()
        .iter()
        .any(|workspace| workspace.id == workspace_id));
    assert_eq!(model.list_surfaces(Some(&workspace_id)).len(), 1);
    assert!(terminal
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
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

    worktree::remove(repo_dir.path().to_str().unwrap(), &branch_name, false).unwrap();
    let error =
        close_workspace_by_worktree_name(&state, &info.worktree_name, repo_dir.path().into())
            .unwrap_err()
            .to_string();

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
    assert!(worktree::list(repo_dir.path().to_str().unwrap())
        .unwrap()
        .is_empty());
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

    assert_eq!(
        active_workspace_cwd_string(&state).unwrap(),
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
