//! Notification method regression tests.

use super::*;
use forktty_core::{NotificationKind, TerminalNotificationMetadata};

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
    let listed = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    assert_eq!(notification, listed[0]);
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
async fn notification_list_pages_newest_first_while_preserving_page_order() {
    let (state, _) = test_state();
    let ids = {
        let mut model = state.model.lock().unwrap();
        (0..205)
            .map(|index| {
                model
                    .create_notification(
                        format!("n{index}"),
                        "body",
                        NotificationKind::Info,
                        None,
                        None,
                    )
                    .id
            })
            .collect::<Vec<_>>()
    };

    let default_page = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    let default_page = default_page.as_array().unwrap();
    assert_eq!(default_page.len(), 200);
    assert_eq!(default_page.first().unwrap()["id"], ids[5]);
    assert_eq!(default_page.last().unwrap()["id"], ids[204]);

    let latest = dispatch(&state, "notification.list", json!({"limit": 3}))
        .await
        .unwrap();
    let latest = latest.as_array().unwrap();
    assert_eq!(latest.len(), 3);
    assert_eq!(latest[0]["id"], ids[202]);
    assert_eq!(latest[2]["id"], ids[204]);

    let previous = dispatch(
        &state,
        "notification.list",
        json!({"limit": 3, "before_id": ids[202]}),
    )
    .await
    .unwrap();
    let previous = previous.as_array().unwrap();
    assert_eq!(previous.len(), 3);
    assert_eq!(previous[0]["id"], ids[199]);
    assert_eq!(previous[2]["id"], ids[201]);

    let max_page = dispatch(&state, "notification.list", json!({"limit": 200}))
        .await
        .unwrap();
    assert_eq!(max_page.as_array().unwrap().len(), 200);
}

#[tokio::test]
async fn maximum_notification_page_fits_the_official_encoded_response_limit() {
    let (state, _) = test_state();
    let max_text = "x".repeat(MAX_METADATA_TEXT_BYTES);
    {
        let mut model = state.model.lock().unwrap();
        for _ in 0..200 {
            model.create_notification(
                max_text.clone(),
                max_text.clone(),
                NotificationKind::Info,
                None,
                None,
            );
        }
    }

    let page = dispatch(&state, "notification.list", json!({}))
        .await
        .unwrap();
    let response = JsonRpcResponse::ok(json!("max-page"), page);
    let encoded = response_encoding::serialize_response(&response).unwrap();

    assert!(encoded.encoded_len() <= protocol_limits::OFFICIAL_SOCKET_RESPONSE_MAX_BYTES);
    let value: serde_json::Value =
        serde_json::from_slice(&encoded.as_bytes()[..encoded.as_bytes().len() - 1]).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(value["result"].as_array().unwrap().len(), 200);
}

#[tokio::test]
async fn notification_list_rejects_invalid_limits_and_nonretained_cursor() {
    let (state, _) = test_state();

    for limit in [json!(0), json!(201), json!(-1), json!(1.5), json!("2")] {
        let error = dispatch(&state, "notification.list", json!({"limit": limit}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
    }

    let dropped_id = {
        let mut model = state.model.lock().unwrap();
        let dropped =
            model.create_notification("dropped", "body", NotificationKind::Info, None, None);
        for index in 0..1_000 {
            model.create_notification(
                format!("retained-{index}"),
                "body",
                NotificationKind::Info,
                None,
                None,
            );
        }
        dropped.id
    };
    let error = dispatch(
        &state,
        "notification.list",
        json!({"before_id": dropped_id}),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code(), "invalid_param");

    for before_id in [json!(""), json!(42)] {
        let error = dispatch(&state, "notification.list", json!({"before_id": before_id}))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
    }
}

#[tokio::test]
async fn notification_socket_projection_omits_icon_bytes_and_preserves_metadata() {
    let (state, _) = test_state();
    let notification_id = {
        let mut model = state.model.lock().unwrap();
        let notification =
            model.create_notification("Terminal", "Metadata", NotificationKind::Custom, None, None);
        model.set_notification_terminal_metadata(
            &notification.id,
            Some(TerminalNotificationMetadata {
                id: "terminal-id".to_string(),
                report_activation: true,
                report_close: true,
                buttons: vec!["Open".to_string()],
                icon_names: vec!["dialog-information".to_string()],
                icon_data: Some(vec![1, 2, 3]),
                icon_cache_id: Some("cache-id".to_string()),
                urgency: Some(2),
                sound_name: Some("bell".to_string()),
                expires_after_ms: Some(5000),
                app_name: Some("ForkTTY test".to_string()),
                notification_types: vec!["system".to_string()],
            }),
        );
        notification.id
    };

    let page = dispatch(&state, "notification.list", json!({"limit": 1}))
        .await
        .unwrap();
    let notification = &page[0];
    assert_eq!(notification["id"], notification_id);
    let metadata = &notification["terminal_metadata"];
    assert!(metadata.get("icon_data").is_none());
    assert_eq!(metadata["id"], "terminal-id");
    assert_eq!(metadata["report_activation"], true);
    assert_eq!(metadata["report_close"], true);
    assert_eq!(metadata["buttons"], json!(["Open"]));
    assert_eq!(metadata["icon_names"], json!(["dialog-information"]));
    assert_eq!(metadata["icon_cache_id"], "cache-id");
    assert_eq!(metadata["urgency"], 2);
    assert_eq!(metadata["sound_name"], "bell");
    assert_eq!(metadata["expires_after_ms"], 5000);
    assert_eq!(metadata["app_name"], "ForkTTY test");
    assert_eq!(metadata["notification_types"], json!(["system"]));
}

#[tokio::test]
async fn notification_list_pages_updated_item_as_newest_with_exclusive_cursor() {
    let (state, _) = test_state();
    let (first_id, second_id) = {
        let mut model = state.model.lock().unwrap();
        let first =
            model.create_notification("first", "old body", NotificationKind::Info, None, None);
        let second =
            model.create_notification("second", "body", NotificationKind::Info, None, None);
        model
            .update_notification(
                &first.id,
                "first updated",
                "new body",
                NotificationKind::Prompt,
            )
            .unwrap();
        (first.id, second.id)
    };

    let newest = dispatch(&state, "notification.list", json!({"limit": 1}))
        .await
        .unwrap();
    assert_eq!(newest[0]["id"], first_id);
    assert_eq!(newest[0]["title"], "first updated");

    let previous = dispatch(
        &state,
        "notification.list",
        json!({"limit": 1, "before_id": first_id}),
    )
    .await
    .unwrap();
    assert_eq!(previous[0]["id"], second_id);
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
