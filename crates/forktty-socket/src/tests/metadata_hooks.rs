//! Metadata hook/session socket method regression tests.

use super::*;

#[test]
fn agent_status_keys_keep_the_exact_alias_contract() {
    assert_eq!(
        agent_kind_from_status_key("agent:claude-code"),
        Some(AgentKind::ClaudeCode)
    );
    assert_eq!(agent_kind_from_status_key("agent: CLAUDE "), None);
}

#[test]
fn nested_agent_hook_lifecycles_match_the_reported_session() {
    assert_eq!(
        agent_session_lifecycle_from_hook("Running", Some("subagent-stop")),
        AgentSessionLifecycle::Running
    );
    assert_eq!(
        agent_session_lifecycle_from_hook("Ready", Some("teammate-idle")),
        AgentSessionLifecycle::Idle
    );
}

#[tokio::test]
async fn hook_session_status_learns_and_reuses_explicit_surface_target() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Ready",
            "hook_session_id": "session-a",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running",
            "hook_session_id": "session-a",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let original_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(original_statuses[0]["value"], "Running");
    assert!(other_statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn hook_session_status_learns_unique_surface_target_from_hook_cwd() {
    let target_dir = tempfile::tempdir().unwrap();
    let other_dir = tempfile::tempdir().unwrap();
    let (state, _backend) = test_state();

    let target = dispatch(
        &state,
        "workspace.create",
        json!({"name": "target", "workingDir": target_dir.path()}),
    )
    .await
    .unwrap();
    let target_workspace_id = target["id"].as_str().unwrap().to_string();
    let target_surface_id = target["focused_surface_id"].as_str().unwrap().to_string();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": other_dir.path()}),
    )
    .await
    .unwrap();
    let other_workspace_id = other["id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "codex-session-cwd",
            "hook_session_cwd": target_dir.path(),
            "hook_event_name": "prompt-submit",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "hook_session_id": "codex-session-cwd",
            "hook_event_name": "stop",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let target_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": target_workspace_id}),
    )
    .await
    .unwrap();
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other_workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(target_statuses[0]["value"], "Ready");
    assert!(other_statuses.as_array().unwrap().is_empty());

    let model = state.model.lock().unwrap();
    let session = model
        .surface(&target_surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(session.agent, AgentKind::Codex);
    assert_eq!(session.session_id, "codex-session-cwd");
    assert_eq!(session.lifecycle, forktty_core::AgentSessionLifecycle::Idle);
}

#[tokio::test]
async fn hook_session_status_rejects_ambiguous_hook_cwd_instead_of_defaulting_active_workspace() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let initial_workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    let other_workspace_id = other["id"].as_str().unwrap().to_string();

    let err = dispatch(
        &state,
        "metadata.set_status",
        json!({
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "codex-session-ambiguous",
            "hook_session_cwd": "/tmp",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "conflict");
    assert!(err.to_string().contains("matches multiple live surfaces"));
    let initial_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": initial_workspace_id}),
    )
    .await
    .unwrap();
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other_workspace_id}),
    )
    .await
    .unwrap();
    assert!(initial_statuses.as_array().unwrap().is_empty());
    assert!(other_statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn hook_status_binds_persisted_sessions_for_all_agent_status_keys() {
    let (state, _backend) = test_state();
    let agents = [
        ("codex", "Codex", AgentKind::Codex),
        ("claude", "Claude", AgentKind::ClaudeCode),
        ("grok", "Grok", AgentKind::Grok),
        ("grok-build", "Grok", AgentKind::Grok),
        ("pi", "Pi", AgentKind::Pi),
        ("agy", "Antigravity", AgentKind::Antigravity),
        ("opencode", "OpenCode", AgentKind::OpenCode),
    ];

    for (index, (key, label, expected_agent)) in agents.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dispatch(
            &state,
            "workspace.create",
            json!({"name": format!("agent-{index}"), "workingDir": dir.path()}),
        )
        .await
        .unwrap();
        let workspace_id = workspace["id"].as_str().unwrap();
        let surface_id = workspace["focused_surface_id"].as_str().unwrap();
        let session_id = format!("{key}-session");

        dispatch(
            &state,
            "metadata.set_status",
            json!({
                "workspace_id": workspace_id,
                "surface_id": surface_id,
                "key": format!("agent:{key}"),
                "label": label,
                "value": "Needs input",
                "hook_session_id": session_id,
                "hook_event_name": "permission-request",
                "hook_event_order": index + 1
            }),
        )
        .await
        .unwrap();

        let model = state.model.lock().unwrap();
        let session = model
            .surface(surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap();
        assert_eq!(&session.agent, expected_agent);
        assert_eq!(session.session_id, session_id);
        assert_eq!(
            session.lifecycle,
            forktty_core::AgentSessionLifecycle::NeedsInput
        );
        drop(model);
    }
}

#[tokio::test]
async fn hook_status_persists_surface_agent_session_binding() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();
    let resume_cwd = tempfile::tempdir().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "hook_session_id": "codex-session-9",
            "hook_session_cwd": resume_cwd.path(),
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    {
        let model = state.model.lock().unwrap();
        let session = model
            .surface(surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap();
        assert_eq!(session.agent, AgentKind::Codex);
        assert_eq!(session.session_id, "codex-session-9");
        assert_eq!(
            session.lifecycle,
            forktty_core::AgentSessionLifecycle::Running
        );
    }

    let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
    assert_eq!(
        agents[0]["resume_cwd"],
        resume_cwd.path().to_string_lossy().as_ref()
    );
    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
    assert_eq!(
        health[0]["resume_cwd"],
        resume_cwd.path().to_string_lossy().as_ref()
    );
    assert_eq!(
        health[0]["argv"],
        json!([
            "codex",
            "resume",
            "-C",
            resume_cwd.path().to_string_lossy().as_ref(),
            "codex-session-9"
        ])
    );
}

#[tokio::test]
async fn hook_status_does_not_steal_surface_from_different_agent_session() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:grok",
            "label": "Grok",
            "value": "Running",
            "hook_session_id": "grok-session-1",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Needs input",
            "hook_session_id": "claude-session-1",
            "hook_event_name": "permission-request",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert_eq!(agents[0]["agent"], "grok");
    assert_eq!(agents[0]["session_id"], "grok-session-1");

    let model = state.model.lock().unwrap();
    let session = model
        .surface(surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(session.agent, AgentKind::Grok);
    assert_eq!(session.session_id, "grok-session-1");
}

#[tokio::test]
async fn hook_status_updates_persisted_agent_session_lifecycle() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Done",
            "hook_session_id": "codex-session-9",
            "hook_event_name": "stop",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
    assert_eq!(agents[0]["lifecycle"], "idle");
    assert!(agents[0]["last_activity_ms"].as_u64().unwrap() > 0);

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ended",
            "hook_session_id": "codex-session-9",
            "hook_event_name": "session-end",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
    assert_eq!(health[0]["lifecycle"], "ended");
    assert!(health[0]["last_activity_ms"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn notification_hook_status_uses_status_value_for_agent_lifecycle() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running",
            "hook_session_id": "claude-session-running",
            "hook_event_name": "notification",
            "hook_event_order": 10
        }),
    )
    .await
    .unwrap();

    let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();
    assert_eq!(agents[0]["lifecycle"], "running");
    assert_eq!(agents[0]["lifecycle_evidence"]["status_value"], "Running");
}

#[tokio::test]
async fn stale_hook_status_does_not_update_persisted_agent_session_lifecycle() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "codex-session-9",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 100,
            "hook_event_clock": "boottime-ns",
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "hook_session_id": "codex-session-9",
            "hook_event_name": "stop",
            "hook_event_order": 200,
            "hook_event_clock": "boottime-ns",
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "codex-session-9",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 150,
            "hook_event_clock": "boottime-ns",
        }),
    )
    .await
    .unwrap();

    let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
    assert_eq!(summary["status"][0]["value"], "Ready");
    assert_eq!(summary["agents"][0]["lifecycle"], "idle");

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_session_id": "codex-session-10",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 3_000_000_000u64,
            "hook_event_clock": "boottime-ns",
        }),
    )
    .await
    .unwrap();

    let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
    assert_eq!(summary["status"][0]["value"], "Running");
    assert_eq!(summary["agents"][0]["lifecycle"], "running");

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Needs input",
            "hook_session_id": "codex-session-10",
            "hook_event_name": "permission-request",
            "hook_event_order": 2_500_000_000u64,
            "hook_event_clock": "boottime-ns",
        }),
    )
    .await
    .unwrap();

    let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();
    assert_eq!(summary["status"][0]["value"], "Running");
    assert_eq!(summary["agents"][0]["lifecycle"], "running");
}

#[tokio::test]
async fn hook_session_end_clear_marks_persisted_agent_session_ended() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running",
            "hook_session_id": "claude-session-9",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "key": "agent:claude",
            "hook_session_id": "claude-session-9",
            "hook_event_name": "session-end",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
    assert_eq!(health[0]["lifecycle"], "ended");
    assert!(health[0]["last_activity_ms"].as_u64().unwrap() > 0);

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
async fn hook_session_end_cleanup_keeps_learned_target_until_aux_status_clear() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running",
            "hook_session_id": "claude-session-10",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude:permission",
            "label": "Claude permissions",
            "value": "bypassPermissions",
            "hook_session_id": "claude-session-10",
            "hook_event_name": "permission-request",
            "hook_event_order": 150
        }),
    )
    .await
    .unwrap();

    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    let other_workspace_id = other["id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "key": "agent:claude",
            "hook_session_id": "claude-session-10",
            "hook_event_name": "session-end",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "key": "agent:claude:permission",
            "hook_session_id": "claude-session-10",
            "hook_event_name": "session-end",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let health = dispatch(
        &state,
        "agent.health",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(health[0]["lifecycle"], "ended");
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert!(statuses.as_array().unwrap().is_empty());
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other_workspace_id}),
    )
    .await
    .unwrap();
    assert!(other_statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn stale_hook_session_end_clear_does_not_end_newer_running_session() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running",
            "hook_session_id": "claude-session-11",
            "hook_event_name": "prompt-submit",
            "hook_event_clock": "boottime-ns",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "key": "agent:claude",
            "hook_session_id": "claude-session-11",
            "hook_event_name": "session-end",
            "hook_event_clock": "boottime-ns",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();

    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
    assert_eq!(health[0]["lifecycle"], "running");
    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses[0]["value"], "Running");
}

#[tokio::test]
async fn hook_session_status_explicit_target_beats_learned_target() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Ready",
            "hook_session_id": "session-b",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    let other_workspace_id = other["id"].as_str().unwrap().to_string();
    let other_surface_id = other["focused_surface_id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": other_workspace_id,
            "surface_id": other_surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Running elsewhere",
            "hook_session_id": "session-b",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let original_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other_workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(original_statuses[0]["value"], "Ready");
    assert_eq!(other_statuses[0]["value"], "Running elsewhere");
}

#[tokio::test]
async fn hook_session_status_surface_close_clears_status_and_invalidates_learned_target() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap().to_string();
    let surface_id = workspaces[0]["focused_surface_id"]
        .as_str()
        .unwrap()
        .to_string();
    let split = dispatch(
        &state,
        "surface.split",
        json!({"surface_id": surface_id, "axis": "vertical"}),
    )
    .await
    .unwrap();
    let mapped_surface_id = split["id"].as_str().unwrap().to_string();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": mapped_surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Ready",
            "hook_session_id": "session-c",
            "hook_event_name": "session-start",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();
    let other = dispatch(
        &state,
        "workspace.create",
        json!({"name": "other", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "surface.close",
        json!({"surface_id": mapped_surface_id}),
    )
    .await
    .unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "key": "agent:claude",
            "label": "Claude",
            "value": "Untargeted after close",
            "hook_session_id": "session-c",
            "hook_event_name": "prompt-submit",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();

    let original_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let other_statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": other["id"]}),
    )
    .await
    .unwrap();
    assert_eq!(original_statuses, json!([]));
    assert_eq!(other_statuses[0]["value"], "Untargeted after close");
}

#[tokio::test]
async fn metadata_hooks_reject_cleanup_after_target_surface_closes() {
    let (state, _backend) = test_state();
    let workspaces = dispatch(&state, "workspace.list", json!({})).await.unwrap();
    let workspace_id = workspaces[0]["id"].as_str().unwrap();
    let surface_id = workspaces[0]["focused_surface_id"].as_str().unwrap();

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_event_name": "prompt-submit",
            "hook_event_clock": "monotonic-ns",
            "hook_event_order": 100,
            "hook_turn_id": "prompt:one"
        }),
    )
    .await
    .unwrap();
    {
        let mut model = state.model.lock().unwrap();
        model.close_surface(surface_id).unwrap();
    }

    let log_error = dispatch(
        &state,
        "metadata.log",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "level": "info",
            "message": "Codex session ended"
        }),
    )
    .await
    .unwrap_err();
    let clear_error = dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": surface_id,
            "key": "agent:codex",
            "hook_event_name": "session-end",
            "hook_event_clock": "monotonic-ns",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap_err();

    assert_eq!(log_error.code(), "not_found");
    assert_eq!(clear_error.code(), "not_found");

    let statuses = dispatch(
        &state,
        "metadata.list_status",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let logs = dispatch(
        &state,
        "metadata.list_logs",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(statuses.as_array().unwrap().len(), 1);
    assert!(logs.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ordered_metadata_status_ignores_stale_hook_events() {
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
            "value": "Running",
            "hook_event_order": 100
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "hook_event_order": 200
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_event_order": 100
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
    assert_eq!(statuses[0]["value"], "Ready");

    dispatch(
        &state,
        "metadata.clear_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "hook_event_order": "300"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_event_order": 250
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
    assert!(statuses.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn metadata_hook_state_ignores_late_prompt_submit_for_same_turn() {
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
            "value": "Running",
            "hook_event_name": "prompt-submit",
            "hook_event_clock": "monotonic-ns",
            "hook_event_order": "100",
            "hook_turn_id": "prompt:one"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "hook_event_name": "stop",
            "hook_event_clock": "monotonic-ns",
            "hook_event_order": "200"
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Running",
            "hook_event_name": "prompt-submit",
            "hook_event_clock": "monotonic-ns",
            "hook_event_order": "300",
            "hook_turn_id": "prompt:one"
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
    assert_eq!(statuses[0]["value"], "Ready");
}
