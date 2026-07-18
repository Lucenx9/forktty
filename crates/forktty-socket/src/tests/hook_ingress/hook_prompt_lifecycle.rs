//! Prompt correlation, resolution, and cleanup regressions at hook ingress.

use super::*;

#[tokio::test]
async fn accepted_permission_reply_resolves_only_correlated_prompt_and_closes_desktop_id() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);

    let mut first = hook_target(
        &workspace_id,
        &surface_id,
        "prompt-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    first["title"] = json!("First permission");
    first["body"] = json!("Approve first?");
    first["kind"] = json!("prompt");
    first["hook_agent"] = json!("opencode");
    first["hook_prompt_kind"] = json!("permission");
    first["hook_prompt_id"] = json!("opencode/session/permission/id:first");
    first["hook_correlation_id"] = json!("id:first");
    let first = dispatch(&state, "notification.create", first)
        .await
        .unwrap();
    assert!(first.get("hook_prompt").is_none());

    let mut second = hook_target(
        &workspace_id,
        &surface_id,
        "prompt-session",
        "permission-request",
        Some("boottime-ns"),
        Some(101),
    );
    second["title"] = json!("Second permission");
    second["body"] = json!("Approve second?");
    second["kind"] = json!("prompt");
    second["hook_agent"] = json!("opencode");
    second["hook_prompt_kind"] = json!("permission");
    second["hook_prompt_id"] = json!("opencode/session/permission/id:second");
    second["hook_correlation_id"] = json!("id:second");
    let second = dispatch(&state, "notification.create", second)
        .await
        .unwrap();

    let mut reply = hook_target(
        &workspace_id,
        &surface_id,
        "prompt-session",
        "permission-replied",
        Some("boottime-ns"),
        Some(102),
    );
    reply["level"] = json!("info");
    reply["message"] = json!("Permission resolved");
    reply["hook_agent"] = json!("opencode");
    reply["hook_prompt_kind"] = json!("permission");
    reply["hook_prompt_id"] = json!("opencode/session/permission/id:first");
    reply["hook_correlation_id"] = json!("id:first");
    dispatch(&state, "metadata.log", reply).await.unwrap();

    let remaining = state.model.lock().unwrap().list_notifications();
    assert_eq!(remaining.len(), 2);
    assert!(
        remaining
            .iter()
            .find(|notification| notification.id == first["id"])
            .unwrap()
            .read
    );
    assert!(
        !remaining
            .iter()
            .find(|notification| notification.id == second["id"])
            .unwrap()
            .read
    );
    assert!(
        state
            .model
            .lock()
            .unwrap()
            .surface(&surface_id)
            .unwrap()
            .unread
    );
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [first["id"].as_str().unwrap()]
    );
}

#[tokio::test]
async fn permission_reply_with_only_surface_id_uses_its_inferred_workspace() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);

    let mut prompt = hook_target(
        &workspace_id,
        &surface_id,
        "surface-only-resolution",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt["title"] = json!("Permission");
    prompt["body"] = json!("Approve?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("opencode");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("opencode/session/permission/id:surface-only");
    prompt["hook_correlation_id"] = json!("id:surface-only");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();

    let mut reply = hook_target(
        &workspace_id,
        &surface_id,
        "surface-only-resolution",
        "permission-replied",
        Some("boottime-ns"),
        Some(101),
    );
    reply.as_object_mut().unwrap().remove("workspace_id");
    reply["level"] = json!("info");
    reply["message"] = json!("Permission resolved");
    reply["hook_agent"] = json!("opencode");
    reply["hook_prompt_kind"] = json!("permission");
    reply["hook_prompt_id"] = json!("opencode/session/permission/id:surface-only");
    reply["hook_correlation_id"] = json!("id:surface-only");

    dispatch(&state, "metadata.log", reply).await.unwrap();

    let notification = state
        .model
        .lock()
        .unwrap()
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == prompt["id"])
        .unwrap();
    assert!(notification.read);
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [prompt["id"].as_str().unwrap()]
    );
}

#[tokio::test]
async fn workspace_only_reply_does_not_inherit_the_sessions_cached_surface() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);

    let mut seed = hook_target(
        &workspace_id,
        &surface_id,
        "workspace-only-resolution",
        "post-tool",
        Some("boottime-ns"),
        Some(99),
    );
    seed["level"] = json!("info");
    seed["message"] = json!("Seed the session surface target");
    dispatch(&state, "metadata.log", seed).await.unwrap();

    let mut prompt = hook_target(
        &workspace_id,
        &surface_id,
        "workspace-only-resolution",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt.as_object_mut().unwrap().remove("surface_id");
    prompt["title"] = json!("Workspace permission");
    prompt["body"] = json!("Approve for this workspace?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("opencode");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("opencode/session/permission/id:workspace-only");
    prompt["hook_correlation_id"] = json!("id:workspace-only");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();

    let mut reply = hook_target(
        &workspace_id,
        &surface_id,
        "workspace-only-resolution",
        "permission-replied",
        Some("boottime-ns"),
        Some(101),
    );
    reply.as_object_mut().unwrap().remove("surface_id");
    reply["level"] = json!("info");
    reply["message"] = json!("Workspace permission resolved");
    reply["hook_agent"] = json!("opencode");
    reply["hook_prompt_kind"] = json!("permission");
    reply["hook_prompt_id"] = json!("opencode/session/permission/id:workspace-only");
    reply["hook_correlation_id"] = json!("id:workspace-only");

    dispatch(&state, "metadata.log", reply).await.unwrap();

    let notification = state
        .model
        .lock()
        .unwrap()
        .list_notifications()
        .into_iter()
        .find(|notification| notification.id == prompt["id"])
        .unwrap();
    assert!(notification.read);
}

#[tokio::test]
async fn notification_create_rejects_partial_hook_prompt_metadata() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    let error = dispatch(
        &state,
        "notification.create",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "title": "Permission",
            "body": "Approve?",
            "kind": "prompt",
            "hook_agent": "claude",
            "hook_session_id": "partial-session",
            "hook_event_name": "permission-request",
            "hook_event_clock": "boottime-ns",
            "hook_event_order": 100,
            "hook_prompt_kind": "permission",
        }),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("hook_prompt_id"));
    assert!(state.model.lock().unwrap().list_notifications().is_empty());
}

#[tokio::test]
async fn accepted_permission_denied_resolves_correlated_prompt() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);
    let mut prompt = hook_target(
        &workspace_id,
        &surface_id,
        "denied-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt["title"] = json!("Permission");
    prompt["body"] = json!("Approve?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("claude");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("claude/denied-session/permission/id:denied");
    prompt["hook_correlation_id"] = json!("id:denied");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();

    let mut denied = hook_target(
        &workspace_id,
        &surface_id,
        "denied-session",
        "permission-denied",
        Some("boottime-ns"),
        Some(101),
    );
    denied["level"] = json!("warn");
    denied["message"] = json!("Permission denied");
    denied["hook_agent"] = json!("claude");
    denied["hook_prompt_kind"] = json!("permission");
    denied["hook_correlation_id"] = json!("id:denied");
    dispatch(&state, "metadata.log", denied).await.unwrap();

    let history = state.model.lock().unwrap().list_notifications();
    assert_eq!(history.len(), 1);
    assert!(history[0].read);
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [prompt["id"].as_str().unwrap()]
    );
}

#[tokio::test]
async fn ignored_stale_permission_reply_keeps_prompt_and_desktop_notification() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);
    let mut prompt = hook_target(
        &workspace_id,
        &surface_id,
        "stale-result-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt["title"] = json!("Permission");
    prompt["body"] = json!("Approve?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("opencode");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("opencode/session/permission/id:stale");
    prompt["hook_correlation_id"] = json!("id:stale");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();
    set_agent_status(
        &state,
        &workspace_id,
        &surface_id,
        "stale-result-session",
        "prompt-submit",
        Some("boottime-ns"),
        Some(200),
        "Running",
        None,
    )
    .await;

    let mut stale_reply = hook_target(
        &workspace_id,
        &surface_id,
        "stale-result-session",
        "permission-replied",
        Some("boottime-ns"),
        Some(150),
    );
    stale_reply["level"] = json!("info");
    stale_reply["message"] = json!("Stale permission result");
    stale_reply["hook_agent"] = json!("opencode");
    stale_reply["hook_prompt_kind"] = json!("permission");
    stale_reply["hook_correlation_id"] = json!("id:stale");

    assert_eq!(
        dispatch(&state, "metadata.log", stale_reply).await.unwrap(),
        json!({"ignored": true})
    );
    assert_eq!(
        state.model.lock().unwrap().list_notifications()[0].id,
        prompt["id"]
    );
    assert!(closed_ids.lock().unwrap().is_empty());
}

#[tokio::test]
async fn surface_close_evicts_hook_prompt_correlation_and_closes_desktop_notification() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);
    let split = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let split_id = split["id"].as_str().unwrap();
    let mut prompt = hook_target(
        &workspace_id,
        split_id,
        "removed-target-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt["title"] = json!("Permission");
    prompt["body"] = json!("Approve?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("opencode");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("opencode/session/permission/id:removed");
    prompt["hook_correlation_id"] = json!("id:removed");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();

    dispatch(&state, "surface.close", json!({"surface_id": split_id}))
        .await
        .unwrap();

    let retained = state.model.lock().unwrap().list_notifications()[0].clone();
    assert_eq!(retained.id, prompt["id"]);
    assert!(retained.read);
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [prompt["id"].as_str().unwrap()]
    );
}

#[tokio::test]
async fn hook_session_target_remap_evicts_only_that_sessions_old_target_prompts() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, old_surface_id) = target(&state);
    let split = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": old_surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let new_surface_id = split["id"].as_str().unwrap();

    let mut moved_prompt = hook_target(
        &workspace_id,
        &old_surface_id,
        "moved-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    moved_prompt["title"] = json!("Moved permission");
    moved_prompt["body"] = json!("Approve moved session?");
    moved_prompt["kind"] = json!("prompt");
    moved_prompt["hook_agent"] = json!("claude");
    moved_prompt["hook_prompt_kind"] = json!("permission");
    moved_prompt["hook_prompt_id"] = json!("claude/moved-session/permission/100");
    let moved_prompt = dispatch(&state, "notification.create", moved_prompt)
        .await
        .unwrap();

    let mut retained_prompt = hook_target(
        &workspace_id,
        &old_surface_id,
        "retained-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    retained_prompt["title"] = json!("Retained permission");
    retained_prompt["body"] = json!("Approve retained session?");
    retained_prompt["kind"] = json!("prompt");
    retained_prompt["hook_agent"] = json!("claude");
    retained_prompt["hook_prompt_kind"] = json!("permission");
    retained_prompt["hook_prompt_id"] = json!("claude/retained-session/permission/100");
    let retained_prompt = dispatch(&state, "notification.create", retained_prompt)
        .await
        .unwrap();

    let mut remap = hook_target(
        &workspace_id,
        new_surface_id,
        "moved-session",
        "tool-start",
        Some("boottime-ns"),
        Some(101),
    );
    remap["level"] = json!("info");
    remap["message"] = json!("Session moved");
    remap["hook_agent"] = json!("claude");
    dispatch(&state, "metadata.log", remap).await.unwrap();

    let notifications = state.model.lock().unwrap().list_notifications();
    let moved = notifications
        .iter()
        .find(|notification| notification.id == moved_prompt["id"])
        .unwrap();
    let retained = notifications
        .iter()
        .find(|notification| notification.id == retained_prompt["id"])
        .unwrap();
    assert!(moved.read);
    assert!(!retained.read);
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [moved_prompt["id"].as_str().unwrap()]
    );
}

#[tokio::test]
async fn accepted_session_end_cleanup_evicts_prompts_before_auxiliary_status_clears() {
    let (state, _backend) = test_state();
    let closed_ids = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_ids = closed_ids.clone();
    let state = state.with_desktop_notification_closer(move |notification_id| {
        observed_ids
            .lock()
            .unwrap()
            .push(notification_id.to_string());
    });
    let (workspace_id, surface_id) = target(&state);
    let mut prompt = hook_target(
        &workspace_id,
        &surface_id,
        "cleanup-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    prompt["title"] = json!("Permission");
    prompt["body"] = json!("Approve?");
    prompt["kind"] = json!("prompt");
    prompt["hook_agent"] = json!("claude");
    prompt["hook_prompt_kind"] = json!("permission");
    prompt["hook_prompt_id"] = json!("claude/cleanup-session/permission/100");
    let prompt = dispatch(&state, "notification.create", prompt)
        .await
        .unwrap();
    let mut other_provider_prompt = hook_target(
        &workspace_id,
        &surface_id,
        "cleanup-session",
        "permission-request",
        Some("boottime-ns"),
        Some(100),
    );
    other_provider_prompt["title"] = json!("OpenCode permission");
    other_provider_prompt["body"] = json!("Keep open");
    other_provider_prompt["kind"] = json!("prompt");
    other_provider_prompt["hook_agent"] = json!("opencode");
    other_provider_prompt["hook_prompt_kind"] = json!("permission");
    other_provider_prompt["hook_prompt_id"] = json!("opencode/cleanup-session/permission/100");
    let other_provider_prompt = dispatch(&state, "notification.create", other_provider_prompt)
        .await
        .unwrap();

    let mut session_end = hook_target(
        &workspace_id,
        &surface_id,
        "cleanup-session",
        "session-end",
        Some("boottime-ns"),
        Some(101),
    );
    session_end["key"] = json!("agent:claude");
    session_end["hook_agent"] = json!("claude");
    dispatch(&state, "metadata.clear_status", session_end)
        .await
        .unwrap();

    let notifications = state.model.lock().unwrap().list_notifications();
    assert!(
        notifications
            .iter()
            .find(|item| item.id == prompt["id"])
            .unwrap()
            .read
    );
    assert!(
        !notifications
            .iter()
            .find(|item| item.id == other_provider_prompt["id"])
            .unwrap()
            .read
    );
    assert_eq!(
        closed_ids.lock().unwrap().as_slice(),
        [prompt["id"].as_str().unwrap()]
    );

    let auxiliary_clear = json!({
        "hook_session_id": "cleanup-session",
        "hook_event_name": "session-end",
        "hook_event_clock": "boottime-ns",
        "hook_event_order": 102,
        "hook_agent": "claude",
        "key": "agent-session:claude",
    });
    assert_ne!(
        dispatch(&state, "metadata.clear_status", auxiliary_clear)
            .await
            .unwrap(),
        json!({"ignored": true})
    );
}

#[tokio::test]
async fn malformed_prompt_resolution_metadata_does_not_mutate_logs() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = target(&state);
    let mut malformed = hook_target(
        &workspace_id,
        &surface_id,
        "malformed-resolution",
        "permission-replied",
        Some("boottime-ns"),
        Some(100),
    );
    malformed["level"] = json!("info");
    malformed["message"] = json!("must not be appended");
    malformed["hook_agent"] = json!("claude");
    malformed["hook_prompt_kind"] = json!("elicitation");

    assert!(dispatch(&state, "metadata.log", malformed.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("hook_prompt_kind"));
    assert!(dispatch(&state, "metadata.log", malformed)
        .await
        .unwrap_err()
        .to_string()
        .contains("hook_prompt_kind"));

    let logs = dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert!(logs.as_array().unwrap().is_empty());
}
