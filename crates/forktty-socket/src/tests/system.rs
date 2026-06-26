//! Socket system method regression tests.

use super::*;

#[tokio::test]
async fn dispatches_system_ping() {
    let (state, _) = test_state();
    assert_eq!(
        dispatch(&state, "system.ping", json!({})).await.unwrap(),
        json!("pong")
    );
}

#[tokio::test]
async fn dispatches_system_top_summary() {
    let (state, backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.mark_surface_unread(&surface_id, true));
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));
        model
            .set_status(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Idle",
                Some("green".into()),
            )
            .unwrap();
        (workspace.id, surface_id)
    };
    backend.resize(&surface_id, 120, 40).unwrap();

    let result = dispatch(&state, "system.top", json!({})).await.unwrap();

    assert_eq!(result["totals"]["workspaces"], 1);
    assert_eq!(result["totals"]["surfaces"], 1);
    assert_eq!(result["totals"]["unread_surfaces"], 1);
    assert_eq!(result["totals"]["agents"], 1);
    assert_eq!(result["workspaces"][0]["id"], workspace_id);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["id"], surface_id);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["focused"], true);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["unread"], true);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["kind"], "terminal");
    assert_eq!(result["workspaces"][0]["surfaces"][0]["shell"], "/bin/sh");
    assert_eq!(result["workspaces"][0]["surfaces"][0]["cols"], 120);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["rows"], 40);
    assert_eq!(result["workspaces"][0]["surfaces"][0]["pid"], Value::Null);
    assert_eq!(
        result["workspaces"][0]["surfaces"][0]["agent"]["session_id"],
        "codex-session-1"
    );
    assert_eq!(
        result["workspaces"][0]["surfaces"][0]["agent"]["lifecycle"],
        "idle"
    );
    assert_eq!(result["workspaces"][0]["status"][0]["key"], "agent:codex");
}

#[tokio::test]
async fn system_identify_reports_target_and_caller_context() {
    let (state, _) = test_state();
    let project_dir = tempfile::tempdir().unwrap();
    let project_cwd = project_dir.path().to_string_lossy().to_string();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model
            .set_surface_agent_session_resume_cwd(&surface_id, project_dir.path().to_path_buf()));
        (workspace.id, surface_id)
    };

    let identify = dispatch(
        &state,
        "system.identify",
        json!({
            "caller_workspace_id": workspace_id,
            "caller_surface_id": surface_id,
        }),
    )
    .await
    .unwrap();

    assert_eq!(identify["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(identify["workspace"]["id"], workspace_id);
    assert_eq!(identify["workspace"]["effective_project_cwd"], project_cwd);
    assert_eq!(identify["surface"]["id"], surface_id);
    assert_eq!(identify["surface"]["effective_project_cwd"], project_cwd);
    assert_eq!(identify["agent"]["agent"], "codex");
    assert_eq!(identify["agent"]["session_id"], "codex-session-1");
    assert_eq!(identify["caller"]["workspace_id"], workspace_id);
    assert_eq!(identify["caller"]["surface_id"], surface_id);
    assert_eq!(identify["caller"]["workspace_known"], true);
    assert_eq!(identify["caller"]["surface_known"], true);
    assert_eq!(identify["caller"]["matches_workspace"], true);
    assert_eq!(identify["caller"]["matches_surface"], true);
}

#[tokio::test]
async fn system_identify_defaults_to_known_caller_surface() {
    let (state, _) = test_state();
    let caller_surface_id = {
        let model = state.model.lock().unwrap();
        model.active_workspace().unwrap().focused_surface_id.clone()
    };
    let focused = dispatch(
        &state,
        "pane.new_tab",
        json!({"surface_id": caller_surface_id}),
    )
    .await
    .unwrap();
    let focused_surface_id = focused["id"].as_str().unwrap().to_string();
    assert_ne!(caller_surface_id, focused_surface_id);

    let identify = dispatch(
        &state,
        "system.identify",
        json!({"caller_surface_id": caller_surface_id}),
    )
    .await
    .unwrap();

    assert_eq!(identify["target_source"], "caller_surface");
    assert_eq!(identify["surface"]["id"], caller_surface_id);
    assert_eq!(identify["caller"]["surface_known"], true);
    assert_eq!(identify["caller"]["matches_surface"], true);
}

#[tokio::test]
async fn system_identify_degrades_stale_caller_surface_to_active_focus() {
    let (state, _) = test_state();
    let (workspace_id, focused_surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };

    let identify = dispatch(
        &state,
        "system.identify",
        json!({
            "caller_workspace_id": workspace_id,
            "caller_surface_id": "surface-missing"
        }),
    )
    .await
    .unwrap();

    assert_eq!(identify["target_source"], "active_workspace");
    assert_eq!(identify["workspace"]["id"], workspace_id);
    assert_eq!(identify["surface"]["id"], focused_surface_id);
    assert_eq!(identify["caller"]["workspace_known"], true);
    assert_eq!(identify["caller"]["surface_known"], false);
    assert_eq!(identify["caller"]["matches_workspace"], true);
    assert_eq!(identify["caller"]["matches_surface"], false);
}
