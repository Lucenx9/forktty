//! Metadata socket method regression tests.

use super::*;

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
