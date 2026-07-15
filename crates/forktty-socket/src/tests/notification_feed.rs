//! Notification method regression tests.

use super::*;

#[tokio::test]
async fn dispatches_notification_methods() {
    let (state, _) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    let notification = dispatch(
        &state,
        "notification.create",
        json!({"title": "Prompt", "body": "Ready", "surface_id": surface_id}),
    )
    .await
    .unwrap();
    assert_eq!(notification["title"], "Prompt");
    assert_eq!(notification["workspace_id"], workspaces[0]["id"]);
    dispatch(&state, "notification.clear", json!({}))
        .await
        .unwrap();
    assert!(dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn notification_create_rejects_stale_targets() {
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

    let stale_workspace = dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": "workspace-missing",
            "title": "Prompt",
            "body": "stale workspace"
        }),
    )
    .await
    .unwrap_err();
    let stale_surface = dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": workspace_id,
            "surface_id": stale_surface_id,
            "title": "Prompt",
            "body": "stale surface"
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(stale_workspace.code(), "not_found");
    assert_eq!(stale_surface.code(), "not_found");
    let notifications = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    assert!(notifications.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notification_create_rejects_invalid_kind() {
    let (state, _backend) = test_state();

    for kind in [json!("promtp"), json!(""), json!(42)] {
        let error = dispatch(
            &state,
            "notification.create",
            json!({"title": "Prompt", "kind": kind}),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter kind"));
    }
    let notifications = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    assert!(notifications.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notification_create_rejects_invalid_text_fields() {
    let (state, _backend) = test_state();

    for title in [json!(""), json!(" \n "), json!(42)] {
        let error = dispatch(&state, "notification.create", json!({"title": title}))
            .await
            .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter title"));
    }

    let error = dispatch(&state, "notification.create", json!({"body": 42}))
        .await
        .unwrap_err();
    assert_eq!(error.code(), "invalid_param");
    assert!(error.to_string().contains("Invalid parameter body"));

    let notifications = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    assert!(notifications.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notification_create_rejects_invalid_surface_targets() {
    let (state, _backend) = test_state();

    for surface_id in [json!(""), json!(42)] {
        let error = dispatch(
            &state,
            "notification.create",
            json!({"title": "Prompt", "surface_id": surface_id}),
        )
        .await
        .unwrap_err();

        assert_eq!(error.code(), "invalid_param");
        assert!(error.to_string().contains("Invalid parameter surface_id"));
    }

    let notifications = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    assert!(notifications.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn notification_create_respects_workspace_selectors() {
    let (state, _backend) = test_state();
    let created = dispatch(
        &state,
        "workspace.create",
        json!({"name": "target", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();

    let notification = dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_name": " target ",
            "title": "Targeted",
            "body": "by workspace name"
        }),
    )
    .await
    .unwrap();

    assert_eq!(notification["workspace_id"], created["id"]);
    assert!(notification["surface_id"].is_null());
}
