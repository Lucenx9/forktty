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
