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

#[tokio::test]
async fn metadata_commands_reject_invalid_workspace_selectors() {
    let (state, _backend) = test_state();

    for workspace_id in [json!(""), json!(42)] {
        let error = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter workspace_id"));
    }

    let statuses = dispatch(&state, "metadata.list_status", json!({}))
        .await
        .unwrap();
    assert!(statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_log_rejects_invalid_level() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for level in [json!("verbose"), json!(""), json!(42)] {
        let error = dispatch(
            &state,
            "metadata.log",
            json!({
                "workspace_id": workspace_id,
                "level": level,
                "message": "waiting"
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter level"));
    }

    let logs = dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert!(logs.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_commands_reject_oversized_payload_fields() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let oversized = "x".repeat(MAX_METADATA_TEXT_BYTES + 1);

    for (method, params, expected_field) in [
        (
            "metadata.set_status",
            json!({"workspace_id": workspace_id, "key": oversized, "label": "Codex", "value": "Running"}),
            "key",
        ),
        (
            "metadata.set_progress",
            json!({"workspace_id": workspace_id, "key": "build", "label": oversized, "value": 1}),
            "label",
        ),
        (
            "metadata.log",
            json!({"workspace_id": workspace_id, "level": "info", "message": oversized}),
            "message",
        ),
        (
            "notification.create",
            json!({"workspace_id": workspace_id, "title": oversized, "body": "body"}),
            "title",
        ),
        (
            "notification.create",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "hook_session_id": oversized,
                "title": "Prompt",
                "body": "body"
            }),
            "hook_session_id",
        ),
    ] {
        let error = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(error.code(), "payload_too_large");
        assert!(error.to_string().contains(expected_field));
    }
}

#[tokio::test]
async fn metadata_clear_rejects_invalid_keys_without_clearing_all() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_progress",
        json!({
            "workspace_id": workspace_id,
            "key": "build",
            "label": "Build",
            "value": 1
        }),
    )
    .await
    .unwrap();

    for key in [json!(""), json!(42)] {
        let status_error = dispatch(
            &state,
            "metadata.clear_status",
            json!({"workspace_id": workspace_id, "key": key.clone()}),
        )
        .await
        .unwrap_err();
        let progress_error = dispatch(
            &state,
            "metadata.clear_progress",
            json!({"workspace_id": workspace_id, "key": key}),
        )
        .await
        .unwrap_err();

        assert_eq!(status_error.code(), "invalid_param");
        assert_eq!(progress_error.code(), "invalid_param");
        assert!(status_error.to_string().contains("Invalid parameter key"));
        assert!(progress_error.to_string().contains("Invalid parameter key"));
    }

    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let progress = dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses.as_array().unwrap().len(), 1);
    assert_eq!(progress.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn metadata_set_trims_keys_before_storage() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    let status = dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": " agent:codex ",
            "label": " Codex ",
            "value": " Running ",
            "color": " green "
        }),
    )
    .await
    .unwrap();
    let progress = dispatch(
        &state,
        "metadata.set_progress",
        json!({
            "workspace_id": workspace_id,
            "key": " build ",
            "label": " Build ",
            "value": 1
        }),
    )
    .await
    .unwrap();

    assert_eq!(status["key"], "agent:codex");
    assert_eq!(status["label"], "Codex");
    assert_eq!(status["value"], "Running");
    assert_eq!(status["color"], "green");
    assert_eq!(progress["key"], "build");
    assert_eq!(progress["label"], "Build");

    dispatch(
        &state,
        "metadata.clear_status",
        json!({"workspace_id": workspace_id, "key": "agent:codex"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.clear_progress",
        json!({"workspace_id": workspace_id, "key": "build"}),
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
    let progress = dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert!(statuses.as_array().unwrap().is_empty());
    assert!(progress.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_set_status_rejects_invalid_required_fields() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for (method, params, message) in [
        (
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": 42,
                "label": "Codex",
                "value": "Running"
            }),
            "Invalid parameter key",
        ),
        (
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "",
                "value": "Running"
            }),
            "Invalid parameter label",
        ),
        (
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": 42
            }),
            "Invalid parameter value",
        ),
    ] {
        let error = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains(message));
    }

    assert!(dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn metadata_set_progress_rejects_invalid_required_fields() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for (method, params, message) in [
        (
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": "",
                "label": "Build",
                "value": 1
            }),
            "Invalid parameter key",
        ),
        (
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": "build",
                "label": 42,
                "value": 1
            }),
            "Invalid parameter label",
        ),
        (
            "metadata.set_progress",
            json!({
                "workspace_id": workspace_id,
                "key": "build",
                "label": "Build",
                "value": "1"
            }),
            "Invalid parameter value",
        ),
    ] {
        let error = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains(message));
    }

    assert!(dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn metadata_log_rejects_invalid_required_fields() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for (method, params, message) in [
        (
            "metadata.log",
            json!({
                "workspace_id": workspace_id,
                "message": ""
            }),
            "Invalid parameter message",
        ),
        (
            "metadata.log",
            json!({
                "workspace_id": workspace_id,
                "message": 42
            }),
            "Invalid parameter message",
        ),
    ] {
        let error = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains(message));
    }

    assert!(dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn metadata_set_progress_rejects_missing_required_fields() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    let error = dispatch(
        &state,
        "metadata.set_progress",
        json!({
            "workspace_id": workspace_id,
            "key": "build",
            "label": "Build"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "missing_param");
    assert!(error.to_string().contains("value"));

    assert!(dispatch(
        &state,
        "metadata.list_progress",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap()
    .as_array()
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn metadata_set_status_rejects_invalid_colors() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for color in [
        json!("purple"),
        json!(""),
        json!(42),
        json!("#"),
        json!("#12"),
        json!("#nothex"),
    ] {
        let error = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": "agent:codex",
                "label": "Codex",
                "value": "Running",
                "color": color
            }),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter color"));
    }

    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert!(statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_set_status_accepts_hex_colors() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();

    for color in ["#abc", "#abcd", "#a1B2c3", "#a1B2c3D4"] {
        let status = dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "key": format!("agent:codex:{color}"),
                "label": "Codex",
                "value": "Running",
                "color": color
            }),
        )
        .await
        .unwrap();

        assert_eq!(status["color"], color);
    }
}
