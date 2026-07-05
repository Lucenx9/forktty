//! Notification and feed method regression tests.

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

#[tokio::test]
async fn notification_clear_marks_persisted_approvals_dismissed() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

    dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": workspace_id,
            "kind": "prompt",
            "title": "Permission",
            "body": "Run command?"
        }),
    )
    .await
    .unwrap();

    dispatch(&state, "notification.clear", json!({}))
        .await
        .unwrap();
    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();

    assert_eq!(feed[0]["type"], "approval");
    assert_eq!(feed[0]["approval_state"], "dismissed");
}

#[tokio::test]
async fn notification_clear_marks_persisted_notifications_read() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

    dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": workspace_id,
            "kind": "info",
            "title": "UI smoke",
            "body": "Temporary notification"
        }),
    )
    .await
    .unwrap();

    dispatch(&state, "notification.clear", json!({}))
        .await
        .unwrap();
    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();

    assert_eq!(feed[0]["type"], "notification");
    assert_eq!(feed[0]["title"], "UI smoke");
    assert_eq!(feed[0]["read"], true);
}

#[tokio::test]
async fn context_snapshot_marks_missing_surface_approvals_stale() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let (workspace_id, stale_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        (workspace.id, split.id)
    };
    {
        let prev = current_snapshot(&state.model);
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Permission",
            "Run stale command?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            Some(stale_surface_id.clone()),
        );
        drop(model);
        let next = current_snapshot(&state.model);
        feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
        let mut model = state.model.lock().unwrap();
        model.close_surface(&stale_surface_id).unwrap();
    }

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();
    let feed = snapshot["feed"].as_array().unwrap();

    assert_eq!(feed[0]["surface_id"], stale_surface_id);
    assert_eq!(feed[0]["approval_state"], "stale");
    assert!(!snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("pending_approval")));
}

#[tokio::test]
async fn feed_approval_respond_rejects_stale_surface_approval() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let (workspace_id, stale_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let split = model
            .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
            .unwrap();
        (workspace.id, split.id)
    };
    {
        let prev = current_snapshot(&state.model);
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Permission",
            "Run stale command?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            Some(stale_surface_id.clone()),
        );
        drop(model);
        let next = current_snapshot(&state.model);
        feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
        let mut model = state.model.lock().unwrap();
        model.close_surface(&stale_surface_id).unwrap();
    }

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();
    let approval_id = feed[0]["id"].as_str().unwrap();
    assert_eq!(feed[0]["approval_state"], "stale");

    let err = dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approved"}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("no longer active"));
    assert_eq!(
        forktty_core::FeedStore::open_at(&feed_path)
            .unwrap()
            .list(None, 10)[0]
            .approval_state,
        Some(forktty_core::FeedApprovalState::Stale)
    );
}

#[tokio::test]
async fn dispatches_minimal_feed_list() {
    let (state, _) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        model
            .set_status(&workspace.id, "agent:codex", "Codex", "Running", None)
            .unwrap();
        model
            .set_progress(&workspace.id, "build", "Build", 2.0, Some(4.0))
            .unwrap();
        let note = model.create_notification(
            "Permission",
            "Run command?",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(surface_id.clone()),
        );
        assert!(note.created_at_ms > 0);
        for title in ["One", "Two", "Three"] {
            model.create_notification(
                title,
                "Plain notification",
                NotificationKind::Info,
                Some(workspace.id.clone()),
                None,
            );
        }
        (workspace.id, surface_id)
    };

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 3}),
    )
    .await
    .unwrap();
    let items = feed.as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0]["type"], "approval");
    assert_eq!(items[0]["title"], "Permission");
    assert_eq!(items[0]["body"], "Run command?");
    assert_eq!(items[0]["surface_id"], surface_id);
    assert!(!items.iter().any(|item| item["type"] == "notification"));
    assert!(items
        .iter()
        .any(|item| item["type"] == "status" && item["title"] == "Codex"));
    assert!(items
        .iter()
        .any(|item| item["type"] == "progress" && item["title"] == "Build"));
}

#[tokio::test]
async fn context_snapshot_compacts_feed_trace_by_default_and_expands_on_request() {
    let (state, _) = test_state();
    let workspace_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        model
            .set_status(
                &workspace.id,
                "tool:mcp__forktty__context_snapshot",
                "Tool",
                "Running mcp__forktty__context_snapshot",
                None,
            )
            .unwrap();
        model
            .set_progress(&workspace.id, "tool:review", "Review", 1.0, Some(2.0))
            .unwrap();
        model.create_notification(
            "Permission",
            "Run command?",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(surface_id),
        );
        workspace.id
    };

    let compact = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();
    let compact_feed = compact["feed"].as_array().unwrap();
    assert!(compact_feed.iter().any(|item| item["type"] == "approval"));
    assert!(!compact_feed
        .iter()
        .any(|item| matches!(item["type"].as_str(), Some("status" | "progress"))));

    let expanded = dispatch(
        &state,
        "context.snapshot",
        json!({
            "workspace_id": workspace_id,
            "tail_lines": 0,
            "include_feed_trace": true
        }),
    )
    .await
    .unwrap();
    let expanded_feed = expanded["feed"].as_array().unwrap();
    assert!(expanded_feed.iter().any(|item| item["type"] == "status"));
    assert!(expanded_feed.iter().any(|item| item["type"] == "progress"));
}

#[tokio::test]
async fn feed_list_live_fallback_limits_status_and_progress_to_newest_entries() {
    let (state, _) = test_state();
    let workspace_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        for i in 0..5 {
            model
                .set_status(
                    &workspace.id,
                    format!("status-{i}"),
                    "Status",
                    format!("value-{i}"),
                    None,
                )
                .unwrap();
        }
        workspace.id
    };

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 2}),
    )
    .await
    .unwrap();
    let status_keys = feed
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "status")
        .map(|item| item["key"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(status_keys, vec!["status-3", "status-4"]);

    {
        let mut model = state.model.lock().unwrap();
        assert!(model.clear_status(&workspace_id, None));
        for i in 0..5 {
            model
                .set_progress(
                    &workspace_id,
                    format!("progress-{i}"),
                    "Progress",
                    i as f64,
                    None,
                )
                .unwrap();
        }
    }

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 2}),
    )
    .await
    .unwrap();
    let progress_keys = feed
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "progress")
        .map(|item| item["key"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(progress_keys, vec!["progress-3", "progress-4"]);
}

#[tokio::test]
async fn feed_list_reads_persisted_feed_entries_and_records_approval_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();
    {
        let prev = current_snapshot(&state.model);
        state.model.lock().unwrap().create_notification(
            "Permission",
            "Run command?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            None,
        );
        let next = current_snapshot(&state.model);
        feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
    }

    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();
    let approval_id = feed[0]["id"].as_str().unwrap();
    assert_eq!(feed[0]["type"], "approval");
    assert_eq!(feed[0]["kind"], "prompt");
    assert_eq!(feed[0]["read"], false);
    assert_eq!(feed[0]["approval_state"], "pending");

    let decided = dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approved"}),
    )
    .await
    .unwrap();

    assert_eq!(decided["approval_state"], "approved");
    assert_eq!(
        forktty_core::FeedStore::open_at(&feed_path)
            .unwrap()
            .list(None, 10)[0]
            .approval_state,
        Some(forktty_core::FeedApprovalState::Approved)
    );
}

#[tokio::test]
async fn socket_state_counts_pending_feed_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

    assert_eq!(state.pending_feed_approval_count(), Some(0));
    {
        let prev = current_snapshot(&state.model);
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Permission",
            "Run command?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            None,
        );
        model.create_notification(
            "Apply",
            "Launch workers?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            None,
        );
        model.create_notification(
            "Done",
            "No approval required",
            NotificationKind::Info,
            Some(workspace_id.clone()),
            None,
        );
        drop(model);
        let next = current_snapshot(&state.model);
        feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
    }

    assert_eq!(state.pending_feed_approval_count(), Some(2));
    assert_eq!(
        state.pending_feed_approval_summaries(3),
        Some(vec!["Permission".to_string(), "Apply".to_string()])
    );
    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();
    let approval_id = feed
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["approval_state"] == "pending")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "feed.approval.respond",
        json!({"id": approval_id, "decision": "approved"}),
    )
    .await
    .unwrap();

    assert_eq!(state.pending_feed_approval_count(), Some(1));
    let remaining = state.pending_feed_approval_summaries(3).unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining
        .iter()
        .any(|title| title == "Permission" || title == "Apply"));
}

#[tokio::test]
async fn socket_state_lists_and_decides_pending_feed_approvals() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

    assert_eq!(state.pending_feed_approvals(3), Some(Vec::new()));
    assert!(state
        .decide_feed_approval("missing", forktty_core::FeedApprovalState::Denied)
        .is_err());

    {
        let prev = current_snapshot(&state.model);
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Permission",
            "Run command?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            None,
        );
        model.create_notification(
            "Apply",
            "Launch workers?",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            None,
        );
        drop(model);
        let next = current_snapshot(&state.model);
        feed_events::record_feed_events(&state, &events::diff(&prev, &next)).unwrap();
    }

    let approvals = state.pending_feed_approvals(3).unwrap();
    assert_eq!(approvals.len(), 2);
    assert!(approvals
        .iter()
        .all(|approval| !approval.id.is_empty() && approval.created_at_ms > 0));
    let denied = approvals
        .iter()
        .find(|approval| approval.title == "Apply")
        .unwrap();

    state
        .decide_feed_approval(&denied.id, forktty_core::FeedApprovalState::Denied)
        .unwrap();

    let remaining = state.pending_feed_approvals(3).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].title, "Permission");
    // Deciding twice must fail instead of silently rewriting the decision.
    assert!(state
        .decide_feed_approval(&denied.id, forktty_core::FeedApprovalState::Approved)
        .is_err());
    assert_eq!(
        forktty_core::FeedStore::open_at(&feed_path)
            .unwrap()
            .list(None, 10)
            .iter()
            .find(|entry| entry.id == denied.id)
            .unwrap()
            .approval_state,
        Some(forktty_core::FeedApprovalState::Denied)
    );
}

#[tokio::test]
async fn feed_list_sees_socket_created_notification_without_waiting_for_event_tick() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (state, _) = test_state();
    let state = state.with_feed_store_path(&feed_path).unwrap();
    let workspace_id = state.model.lock().unwrap().active_workspace_id().unwrap();

    dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": workspace_id,
            "kind": "prompt",
            "title": "Permission",
            "body": "Run command?"
        }),
    )
    .await
    .unwrap();
    let feed = dispatch(
        &state,
        "feed.list",
        json!({"workspace_id": workspace_id, "limit": 10}),
    )
    .await
    .unwrap();

    assert_eq!(feed[0]["type"], "approval");
    assert_eq!(feed[0]["title"], "Permission");
    assert_eq!(feed[0]["approval_state"], "pending");
}

#[tokio::test]
async fn feed_notification_approval_ids_do_not_collide_after_model_restart() {
    let dir = tempfile::tempdir().unwrap();
    let feed_path = dir.path().join("feed.json");
    let (first_state, _) = test_state();
    let first_state = first_state.with_feed_store_path(&feed_path).unwrap();
    let first_workspace_id = first_state
        .model
        .lock()
        .unwrap()
        .active_workspace_id()
        .unwrap();

    dispatch(
        &first_state,
        "notification.create",
        json!({
            "workspace_id": first_workspace_id,
            "kind": "prompt",
            "title": "Old prompt",
            "body": "Run old command?"
        }),
    )
    .await
    .unwrap();
    let old_feed = dispatch(
        &first_state,
        "feed.list",
        json!({"workspace_id": first_workspace_id, "limit": 10}),
    )
    .await
    .unwrap();
    let old_approval_id = old_feed[0]["id"].as_str().unwrap();
    dispatch(
        &first_state,
        "feed.approval.respond",
        json!({"id": old_approval_id, "decision": "approved"}),
    )
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let (second_state, _) = test_state();
    let second_state = second_state.with_feed_store_path(&feed_path).unwrap();
    let second_workspace_id = second_state
        .model
        .lock()
        .unwrap()
        .active_workspace_id()
        .unwrap();
    dispatch(
        &second_state,
        "notification.create",
        json!({
            "workspace_id": second_workspace_id,
            "kind": "prompt",
            "title": "New prompt",
            "body": "Run new command?"
        }),
    )
    .await
    .unwrap();
    let feed = dispatch(
        &second_state,
        "feed.list",
        json!({"workspace_id": second_workspace_id, "limit": 10}),
    )
    .await
    .unwrap();

    assert_eq!(feed[0]["title"], "New prompt");
    assert_eq!(feed[0]["approval_state"], "pending");
    assert_ne!(feed[0]["id"], old_approval_id);
}
