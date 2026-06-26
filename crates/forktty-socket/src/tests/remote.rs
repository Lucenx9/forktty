//! Socket remote and SSH workspace regression tests.

use super::*;

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
