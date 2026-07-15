//! Context snapshot response and risk flag regression tests.

use super::*;
use forktty_core::{NotificationItem, NotificationKind, TerminalNotificationMetadata};

#[tokio::test]
async fn dispatches_context_snapshot_with_bounded_terminal_tails() {
    let (state, backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Running,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));
        model
            .set_status(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".into()),
            )
            .unwrap();
        (workspace.id, surface_id)
    };
    backend
        .send_text(&surface_id, "one\ntwo\nthree\n")
        .expect("write terminal text");

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 2, "tail_max_bytes": 1024}),
    )
    .await
    .unwrap();

    assert_eq!(result["workspace"]["id"], workspace_id);
    assert_eq!(result["workspace"]["focused_surface_id"], surface_id);
    assert_eq!(result["surfaces"][0]["id"], surface_id);
    assert_eq!(result["status"]["status"][0]["key"], "agent:codex");
    assert_eq!(result["agents"][0]["surface_id"], surface_id);
    assert_eq!(result["agents"][0]["lifecycle"], "running");
    assert_eq!(
        result["agents"][0]["lifecycle_evidence"]["status_key"],
        "agent:codex"
    );
    assert_eq!(
        result["agents"][0]["lifecycle_evidence"]["status_value"],
        "Running"
    );
    assert_eq!(
        result["agent_health"][0]["lifecycle_evidence"]["readiness_reason"],
        result["agent_health"][0]["reason"]
    );
    assert_eq!(
        result["agents"][0]["observed_at_ms"],
        result["status"]["agents"][0]["observed_at_ms"]
    );
    assert_eq!(
        result["agents"][0]["age_ms"],
        result["status"]["agents"][0]["age_ms"]
    );
    assert_eq!(result["terminal_tails"][0]["surface_id"], surface_id);
    assert_eq!(result["terminal_tails"][0]["text"], "two\nthree\n");
    assert_eq!(result["terminal_tails"][0]["untrusted"], true);
    assert_eq!(result["terminal_tails"][0]["truncated"], false);
    assert!(result["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("terminal_text_untrusted")));
}

#[tokio::test]
async fn context_snapshot_exposes_effective_project_cwd_from_resume_cwd() {
    let (state, _) = test_state();
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_string_lossy().to_string();
    {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model
            .set_surface_agent_session_resume_cwd(&surface_id, project_dir.path().to_path_buf()));
    }

    let snapshot = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
        .await
        .unwrap();

    assert_eq!(snapshot["workspace"]["working_dir"], "/tmp");
    assert_eq!(snapshot["workspace"]["effective_project_cwd"], project_cwd);
    assert_eq!(snapshot["surfaces"][0]["cwd"], "/tmp");
    assert_eq!(
        snapshot["surfaces"][0]["effective_project_cwd"],
        project_cwd
    );
    assert_eq!(snapshot["agents"][0]["effective_project_cwd"], project_cwd);
    assert_eq!(
        snapshot["agent_health"][0]["effective_project_cwd"],
        project_cwd
    );
}

#[tokio::test]
async fn context_snapshot_includes_workspace_and_global_notifications() {
    let (state, _) = test_state();
    let (workspace_id, surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };
    let other_workspace_id = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    {
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Needs input",
            "Review the terminal prompt",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            Some(surface_id),
        );
        model.create_notification(
            "Global",
            "Visible in every workspace snapshot",
            NotificationKind::Info,
            None,
            None,
        );
        model.create_notification(
            "Other",
            "Must stay scoped to the other workspace",
            NotificationKind::Error,
            Some(other_workspace_id),
            None,
        );
    }

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    let notifications = snapshot["notifications"].as_array().unwrap();
    assert_eq!(notifications.len(), 2);
    assert!(notifications
        .iter()
        .any(|notification| notification["title"] == "Needs input"));
    assert!(notifications
        .iter()
        .any(|notification| notification["title"] == "Global"));
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("notification_needs_input")));
}

#[tokio::test]
async fn context_snapshot_bounds_notifications_without_hiding_prompt_risk() {
    let (state, _) = test_state();
    let (workspace_id, surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };
    {
        let mut model = state.model.lock().unwrap();
        model.create_notification(
            "Old prompt",
            "Still contributes to risk flags",
            NotificationKind::Prompt,
            Some(workspace_id.clone()),
            Some(surface_id.clone()),
        );
        for index in 0..=MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS {
            let notification = model.create_notification(
                format!("Info {index}"),
                "Recent notification",
                NotificationKind::Info,
                Some(workspace_id.clone()),
                Some(surface_id.clone()),
            );
            if index == MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS {
                model.set_notification_terminal_metadata(
                    &notification.id,
                    Some(TerminalNotificationMetadata {
                        id: "terminal-notification".to_string(),
                        report_activation: true,
                        report_close: true,
                        buttons: vec!["Open".to_string()],
                        icon_names: vec!["dialog-information".to_string()],
                        icon_data: Some(vec![7; 64 * 1024]),
                        icon_cache_id: Some("cached-icon".to_string()),
                        urgency: Some(1),
                        sound_name: None,
                        expires_after_ms: None,
                        app_name: Some("test".to_string()),
                        notification_types: vec!["system".to_string()],
                    }),
                );
            }
        }
    }

    let snapshot = dispatch(
        &state,
        "context.snapshot",
        json!({"workspace_id": workspace_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    let notifications = snapshot["notifications"].as_array().unwrap();
    assert_eq!(notifications.len(), MAX_CONTEXT_SNAPSHOT_NOTIFICATIONS);
    assert!(notifications
        .iter()
        .all(|notification| notification["title"] != "Old prompt"));
    assert!(notifications.last().unwrap()["terminal_metadata"]
        .get("icon_data")
        .is_none());
    assert_eq!(
        notifications.last().unwrap()["terminal_metadata"]["icon_cache_id"],
        "cached-icon"
    );
    assert!(snapshot["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("notification_needs_input")));
}

#[tokio::test]
async fn context_snapshot_limits_terminal_tail_surface_count() {
    let surface_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_SURFACES;
    let (state, backend) = test_state();
    let mut surface_ids = Vec::new();
    let mut focused_surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    surface_ids.push(focused_surface_id.clone());
    for _ in 0..surface_limit {
        let created = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": focused_surface_id}),
        )
        .await
        .unwrap();
        focused_surface_id = created["id"].as_str().unwrap().to_string();
        surface_ids.push(focused_surface_id.clone());
    }
    for (index, surface_id) in surface_ids.iter().enumerate() {
        backend
            .send_text(surface_id, &format!("surface-{index}\n"))
            .expect("write terminal text");
    }

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 1, "tail_max_bytes": 1024}),
    )
    .await
    .unwrap();

    assert_eq!(
        result["terminal_tails"].as_array().unwrap().len(),
        surface_limit
    );
    let errors = result["terminal_tail_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["skipped_surfaces"], 1);
    assert!(errors[0]["error"]
        .as_str()
        .unwrap()
        .contains("surface limit"));
}

#[tokio::test]
async fn context_snapshot_limits_aggregate_terminal_tail_bytes() {
    let aggregate_byte_limit = MAX_CONTEXT_SNAPSHOT_TERMINAL_TAIL_BYTES;
    let (state, backend) = test_state();
    let mut surface_ids = Vec::new();
    let mut focused_surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    surface_ids.push(focused_surface_id.clone());
    for _ in 0..2 {
        let created = dispatch(
            &state,
            "pane.new_tab",
            json!({"surface_id": focused_surface_id}),
        )
        .await
        .unwrap();
        focused_surface_id = created["id"].as_str().unwrap().to_string();
        surface_ids.push(focused_surface_id.clone());
    }
    let large_tail = format!("{}\n", "x".repeat(MAX_TERMINAL_TEXT_BYTES));
    for surface_id in &surface_ids {
        backend
            .send_text(surface_id, &large_tail)
            .expect("write terminal text");
    }

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"tail_lines": 1, "tail_max_bytes": MAX_TERMINAL_TEXT_BYTES}),
    )
    .await
    .unwrap();

    let total_tail_bytes: usize = result["terminal_tails"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tail| tail["text"].as_str().unwrap().len())
        .sum();
    assert!(total_tail_bytes <= aggregate_byte_limit);
    assert!(result["terminal_tails"].as_array().unwrap().len() < surface_ids.len());
    let errors = result["terminal_tail_errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["skipped_surfaces"], 1);
    assert!(errors[0]["error"].as_str().unwrap().contains("byte limit"));
}

#[tokio::test]
async fn context_snapshot_surface_id_selects_inactive_workspace() {
    let (state, _) = test_state();
    let review_dir = tempfile::tempdir().unwrap();
    let first = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let first_workspace_id = first[0]["id"].as_str().unwrap().to_string();
    let second = dispatch(
        &state,
        "workspace.create",
        json!({"name": "review", "workingDir": review_dir.path()}),
    )
    .await
    .unwrap();
    let second_workspace_id = second["id"].as_str().unwrap().to_string();
    let second_surface_id = second["focused_surface_id"].as_str().unwrap().to_string();
    dispatch(
        &state,
        "workspace.select",
        json!({"workspace_id": first_workspace_id}),
    )
    .await
    .unwrap();

    let result = dispatch(
        &state,
        "context.snapshot",
        json!({"surface_id": second_surface_id, "tail_lines": 0}),
    )
    .await
    .unwrap();

    assert_eq!(result["workspace"]["id"], second_workspace_id);
    assert_eq!(result["workspace"]["focused_surface_id"], second_surface_id);
    assert_eq!(result["surfaces"][0]["id"], second_surface_id);
}

#[tokio::test]
async fn context_snapshot_review_gap_rejects_invalid_tail_bounds() {
    let (state, _) = test_state();
    for (params, expected_message) in [
        (
            json!({"tail_lines": MAX_CAPTURE_TAIL_LINES + 1}),
            "Invalid parameter tail_lines",
        ),
        (
            json!({"tail_max_bytes": 0}),
            "Invalid parameter tail_max_bytes",
        ),
        (
            json!({"tail_max_bytes": MAX_TERMINAL_TEXT_BYTES + 1}),
            "Invalid parameter tail_max_bytes",
        ),
    ] {
        let error = dispatch(&state, "context.snapshot", params)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "invalid_param");
        assert!(
            error.to_string().contains(expected_message),
            "expected {expected_message:?}, got {error}"
        );
    }
}

#[tokio::test]
async fn context_snapshot_review_gap_tail_lines_zero_skips_terminal_tails() {
    let (state, backend) = test_state();
    let surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    backend
        .send_text(&surface_id, "one\ntwo\nthree\n")
        .expect("write terminal text");

    let result = dispatch(&state, "context.snapshot", json!({"tail_lines": 0}))
        .await
        .unwrap();

    assert!(result["terminal_tails"].as_array().unwrap().is_empty());
    assert!(result["terminal_tail_errors"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!result["risk_flags"]
        .as_array()
        .unwrap()
        .contains(&json!("terminal_text_untrusted")));
}

#[test]
fn context_snapshot_risk_flags_cover_terminal_remote_and_permission_signals() {
    let truncated_tail = json!(forktty_terminal::TerminalTextSnapshot::from_text(
        "surface-1",
        "alpha\nbeta\n",
        80,
        24,
        TerminalTextCapture::Tail { lines: 2 },
        4,
    ));
    let status = json!({
        "status": [{
            "key": "agent:codex:permission",
            "value": "bypassPermissions",
        }],
    });
    let agent_health = [json!({"permission_mode": "bypassPermissions"})];
    let remotes = [json!({"surface_id": "surface-ssh", "kind": "ssh"})];
    let terminal_tails = [truncated_tail];
    let terminal_tail_errors = [json!({"surface_id": "surface-missing", "error": "not ready"})];
    let notifications = [NotificationItem {
        id: "notification-1".to_string(),
        title: "Needs input".to_string(),
        body: "Review the prompt".to_string(),
        kind: NotificationKind::Prompt,
        created_at_ms: 1,
        read: false,
        workspace_id: Some("workspace-1".to_string()),
        surface_id: Some("surface-1".to_string()),
        terminal_metadata: None,
    }];

    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        notifications: &notifications,
        remotes: &remotes,
        terminal_tails: &terminal_tails,
        terminal_tail_errors: &terminal_tail_errors,
    });

    assert_eq!(
        flags,
        vec![
            "terminal_text_untrusted",
            "terminal_tail_truncated",
            "terminal_tail_unavailable",
            "remote_surface",
            "notification_needs_input",
            "permission_bypass",
        ]
    );
}

#[test]
fn context_snapshot_permission_bypass_reads_agent_health() {
    let status = json!({"status": []});
    let agent_health = [json!({"permission_mode": "bypassPermissions"})];
    let flags = context_snapshot_risk_flags(ContextSnapshotRiskInputs {
        status: &status,
        agent_health: &agent_health,
        notifications: &[],
        remotes: &[],
        terminal_tails: &[],
        terminal_tail_errors: &[],
    });

    assert_eq!(flags, vec!["permission_bypass"]);
}
