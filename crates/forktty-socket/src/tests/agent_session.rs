//! Agent session, health, reclaim, hibernate, and resume socket regression tests.

use super::*;

#[tokio::test]
async fn agent_list_returns_only_surfaces_with_agent_sessions() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));
        (workspace.id, surface_id)
    };
    let _plain = dispatch(
        &state,
        "workspace.create",
        json!({"name": "plain", "workingDir": "/tmp"}),
    )
    .await
    .unwrap();

    let agents = dispatch(&state, "agent.list", json!({})).await.unwrap();

    assert_eq!(agents.as_array().unwrap().len(), 1);
    assert_eq!(agents[0]["workspace_id"], workspace_id);
    assert_eq!(agents[0]["surface_id"], surface_id);
    assert_eq!(agents[0]["agent"], "codex");
    assert_eq!(agents[0]["session_id"], "codex-session-1");
    assert_eq!(agents[0]["source"], "persisted_agent_session");
    let observed_at_ms = agents[0]["observed_at_ms"].as_u64().unwrap();
    assert!(observed_at_ms >= 1_000);
    assert_eq!(agents[0]["age_ms"], observed_at_ms - 1_000);
    assert_eq!(
        agents[0]["lifecycle_evidence"],
        json!({
            "source": "persisted_agent_session",
            "lifecycle": "running",
            "last_activity_ms": 1_000,
            "observed_at_ms": observed_at_ms,
            "age_ms": observed_at_ms - 1_000,
            "status_key": Value::Null,
            "status_value": Value::Null,
            "status_source": Value::Null,
            "status_scope": Value::Null,
            "permission_mode": Value::Null,
        })
    );

    let scoped = dispatch(&state, "agent.list", json!({"workspace_id": workspace_id}))
        .await
        .unwrap();
    assert_eq!(scoped.as_array().unwrap().len(), 1);

    let missing = dispatch(&state, "agent.list", json!({"workspace_name": "missing"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code(), "not_found");
}

#[tokio::test]
async fn agent_health_dispatches_rows_for_persisted_sessions() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &surface_id,
            AgentKind::Custom,
            "custom-session-1",
        ));
        (workspace.id, surface_id)
    };

    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();

    assert_eq!(health.as_array().unwrap().len(), 1);
    assert_eq!(health[0]["workspace_id"], workspace_id);
    assert_eq!(health[0]["surface_id"], surface_id);
    assert_eq!(health[0]["agent"], "custom");
    assert_eq!(health[0]["session_id"], "custom-session-1");
    assert_eq!(health[0]["source"], "persisted_agent_session");
    assert!(health[0]["observed_at_ms"].as_u64().is_some());
    assert!(health[0]["age_ms"].is_null());
    assert_eq!(health[0]["ready"], false);
    assert_eq!(health[0]["reason"], "unsupported_agent");
    assert_eq!(health[0]["argv"], json!([]));
    assert_eq!(
        health[0]["lifecycle_evidence"]["readiness_reason"],
        "unsupported_agent"
    );
    assert_eq!(health[0]["lifecycle_evidence"]["ready"], false);
}

#[test]
fn agent_health_marks_resume_command_ready_when_provider_is_on_path() {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let codex = dir.path().join("codex");
    {
        let mut file = fs::File::create(&codex).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
    }
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));

    let health = agent_health_rows_with_path(&model, None, Some(dir.path().as_os_str()), 1_000);

    assert_eq!(health.len(), 1);
    assert_eq!(health[0]["ready"], true);
    assert_eq!(health[0]["reason"], "ready");
    assert_eq!(health[0]["program"], "codex");
    assert_eq!(health[0]["executable"], codex.to_string_lossy().as_ref());
    assert_eq!(
        health[0]["argv"],
        json!(["codex", "resume", "codex-session-1"])
    );
}

#[test]
#[serial_test::serial]
fn agent_health_uses_codex_session_cwd_fallback_when_not_persisted() {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let codex_home = dir.path().join("codex");
    let sessions_dir = codex_home.join("sessions/2026/06/12");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::write(
        sessions_dir.join("rollout-2026-06-12T15-21-07-codex-session-health-fallback.jsonl"),
        format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "codex-session-health-fallback",
                    "cwd": project.to_string_lossy(),
                }
            })
        ),
    )
    .unwrap();
    let _env = EnvGuard::set("CODEX_HOME", codex_home.to_str().unwrap());
    let path_dir = tempfile::tempdir().unwrap();
    let codex = path_dir.path().join("codex");
    {
        let mut file = fs::File::create(&codex).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
    }
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        AgentKind::Codex,
        "codex-session-health-fallback",
    ));

    let health =
        agent_health_rows_with_path(&model, None, Some(path_dir.path().as_os_str()), 1_000);

    assert_eq!(health.len(), 1);
    assert_eq!(health[0]["ready"], true);
    assert_eq!(health[0]["resume_cwd"], project.to_string_lossy().as_ref());
    assert_eq!(
        health[0]["argv"],
        json!([
            "codex",
            "resume",
            "-C",
            project.to_string_lossy().as_ref(),
            "codex-session-health-fallback"
        ])
    );
}

#[test]
fn agent_health_marks_supported_agent_not_ready_when_provider_is_missing() {
    let empty_path = tempfile::tempdir().unwrap();
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));

    let health =
        agent_health_rows_with_path(&model, None, Some(empty_path.path().as_os_str()), 1_000);

    assert_eq!(health.len(), 1);
    assert_eq!(health[0]["ready"], false);
    assert_eq!(health[0]["reason"], "program_not_found");
    assert_eq!(health[0]["program"], "codex");
    assert_eq!(health[0]["executable"], Value::Null);
    assert_eq!(
        health[0]["argv"],
        json!(["codex", "resume", "codex-session-1"])
    );
}

#[test]
fn agent_reclaim_plan_marks_only_old_idle_ready_sessions_as_candidates() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());

    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let candidate_surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &candidate_surface_id,
        AgentKind::Codex,
        "codex-session-1",
    ));
    assert!(model
        .set_surface_agent_session_lifecycle(&candidate_surface_id, AgentSessionLifecycle::Idle,));
    assert!(model.set_surface_agent_session_last_activity_ms(&candidate_surface_id, 1_000));

    let protected_surface_id = model.add_tab(&candidate_surface_id).unwrap().id;
    assert!(model.set_surface_agent_session(
        &protected_surface_id,
        AgentKind::Codex,
        "codex-session-2",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &protected_surface_id,
        AgentSessionLifecycle::NeedsInput,
    ));
    assert!(model.set_surface_agent_session_last_activity_ms(&protected_surface_id, 500));

    let plan =
        agent_reclaim_plan_with_path(&model, None, Some(dir.path().as_os_str()), 10_000, 5_000);

    assert_eq!(plan["policy"]["now_ms"], 10_000);
    assert_eq!(plan["policy"]["min_idle_ms"], 5_000);
    assert_eq!(plan["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(plan["candidates"][0]["surface_id"], candidate_surface_id);
    assert_eq!(plan["candidates"][0]["idle_ms"], 9_000);
    assert_eq!(plan["candidates"][0]["ready"], true);
    assert_eq!(plan["protected"].as_array().unwrap().len(), 1);
    assert_eq!(plan["protected"][0]["surface_id"], protected_surface_id);
    assert_eq!(plan["protected"][0]["protect_reason"], "needs_input");
}

#[test]
fn agent_reclaim_plan_protects_suspended_sessions() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());

    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1"));
    assert!(
        model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Suspended,)
    );
    assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));

    let plan =
        agent_reclaim_plan_with_path(&model, None, Some(dir.path().as_os_str()), 10_000, 5_000);

    assert!(plan["candidates"].as_array().unwrap().is_empty());
    assert_eq!(plan["protected"][0]["surface_id"], surface_id);
    assert_eq!(plan["protected"][0]["protect_reason"], "suspended");
}

#[tokio::test]
#[serial_test::serial]
async fn agent_hibernate_marks_idle_ready_session_suspended_and_closes_backend() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let (state, backend) = test_state();
    let surface_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let workspace_id = workspace.id.clone();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
        assert!(model
            .set_status(&workspace_id, "agent:codex", "Codex", "Ready", None)
            .is_some());
        assert!(model
            .set_progress(
                &workspace_id,
                "agent:codex:tokens",
                "Codex tokens",
                42.0,
                Some(100.0),
            )
            .is_some());
        surface_id
    };

    let hibernated = dispatch(
        &state,
        "agent.hibernate",
        json!({"surface_id": surface_id, "min_idle_ms": 0}),
    )
    .await
    .unwrap();

    assert_eq!(hibernated["surface"]["id"], surface_id);
    assert_eq!(hibernated["agent"], "codex");
    assert_eq!(hibernated["session_id"], "codex-session-1");
    assert_eq!(hibernated["lifecycle"], "suspended");
    assert_eq!(
        hibernated["argv"],
        json!(["codex", "resume", "codex-session-1"])
    );
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .all(|surface| surface.surface_id != surface_id));

    let model = state.model.lock().unwrap();
    let surface = model.surface(&surface_id).unwrap();
    assert_eq!(
        surface.agent_session.as_ref().unwrap().lifecycle,
        AgentSessionLifecycle::Suspended
    );
    assert_eq!(model.list_status(&surface.workspace_id).len(), 1);
    assert_eq!(
        model.list_status(&surface.workspace_id)[0].key,
        surface_status_key(&surface_id)
    );
    assert_eq!(
        model.list_status(&surface.workspace_id)[0].value,
        "Suspended"
    );
    assert!(model.list_progress(&surface.workspace_id).is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn agent_hibernate_close_failure_rolls_back_visible_state() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(FailingCloseBackend::default());
    let mut state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    state.workflow_store_path = None;
    bootstrap_default_workspace(&state, PathBuf::from("/tmp")).unwrap();
    let surface_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let workspace_id = workspace.id.clone();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(
            model.set_surface_agent_session_lifecycle(&surface_id, AgentSessionLifecycle::Idle,)
        );
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
        assert!(model.mark_surface_unread(&surface_id, true));
        assert!(model
            .set_status(
                &workspace_id,
                surface_status_key(&surface_id),
                "Agent",
                "Running",
                Some("green".to_string()),
            )
            .is_some());
        assert!(model
            .set_status(&workspace_id, "agent:codex", "Codex", "Ready", None)
            .is_some());
        assert!(model
            .set_progress(
                &workspace_id,
                "agent:codex:tokens",
                "Codex tokens",
                42.0,
                Some(100.0),
            )
            .is_some());
        surface_id
    };

    let err = dispatch(
        &state,
        "agent.hibernate",
        json!({"surface_id": surface_id, "min_idle_ms": 0}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "error");
    assert!(err.to_string().contains("close failed"));
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));

    let model = state.model.lock().unwrap();
    let surface = model.surface(&surface_id).unwrap();
    assert_eq!(
        surface.agent_session.as_ref().unwrap().lifecycle,
        AgentSessionLifecycle::Idle
    );
    assert_eq!(surface.agent_session.as_ref().unwrap().last_activity_ms, 1);
    assert!(surface.unread);
    let statuses = model.list_status(&surface.workspace_id);
    assert_eq!(statuses.len(), 2);
    assert!(statuses.iter().any(|status| {
        status.key == surface_status_key(&surface_id)
            && status.value == "Running"
            && status.color.as_deref() == Some("green")
    }));
    assert!(statuses
        .iter()
        .any(|status| status.key == "agent:codex" && status.value == "Ready"));
    let progress = model.list_progress(&surface.workspace_id);
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].key, "agent:codex:tokens");
}

#[tokio::test]
#[serial_test::serial]
async fn agent_hibernate_rejects_running_session_without_closing_backend() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let (state, backend) = test_state();
    let surface_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1));
        surface_id
    };

    let err = dispatch(&state, "agent.hibernate", json!({"surface_id": surface_id}))
        .await
        .unwrap_err();

    assert_eq!(err.code(), "precondition_failed");
    assert!(err.to_string().contains("Only idle"));
    assert!(backend
        .surfaces()
        .unwrap()
        .iter()
        .any(|surface| surface.surface_id == surface_id));
}

#[tokio::test]
#[serial_test::serial]
async fn agent_reclaim_hibernates_only_plan_candidates() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_codex(dir.path());
    let _path = EnvGuard::set("PATH", dir.path().to_str().unwrap());
    let (state, _backend) = test_state();
    let (candidate_surface_id, protected_surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let candidate_surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &candidate_surface_id,
            AgentKind::Codex,
            "codex-session-1",
        ));
        assert!(model.set_surface_agent_session_lifecycle(
            &candidate_surface_id,
            AgentSessionLifecycle::Idle,
        ));
        assert!(model.set_surface_agent_session_last_activity_ms(&candidate_surface_id, 1));

        let protected_surface_id = model.add_tab(&candidate_surface_id).unwrap().id;
        assert!(model.set_surface_agent_session(
            &protected_surface_id,
            AgentKind::Codex,
            "codex-session-2",
        ));
        assert!(model.set_surface_agent_session_lifecycle(
            &protected_surface_id,
            AgentSessionLifecycle::Running,
        ));
        assert!(model.set_surface_agent_session_last_activity_ms(&protected_surface_id, 1));
        (candidate_surface_id, protected_surface_id)
    };

    let reclaimed = dispatch(
        &state,
        "agent.reclaim",
        json!({"min_idle_ms": 0, "limit": 5}),
    )
    .await
    .unwrap();

    assert_eq!(reclaimed["hibernated"].as_array().unwrap().len(), 1);
    assert_eq!(
        reclaimed["hibernated"][0]["surface"]["id"],
        candidate_surface_id
    );
    assert_eq!(reclaimed["failed"].as_array().unwrap().len(), 0);
    assert_eq!(
        reclaimed["protected"][0]["surface_id"],
        protected_surface_id
    );
    assert_eq!(reclaimed["protected"][0]["protect_reason"], "running");

    let model = state.model.lock().unwrap();
    assert_eq!(
        model
            .surface(&candidate_surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .lifecycle,
        AgentSessionLifecycle::Suspended
    );
    assert_eq!(
        model
            .surface(&protected_surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .lifecycle,
        AgentSessionLifecycle::Running
    );
}

#[tokio::test]
async fn status_summary_includes_workspace_agents_status_and_progress() {
    let (state, _backend) = test_state();
    let (workspace_id, surface_id) = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Codex, "codex-session-1",));
        assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 1_000));
        model
            .set_status(
                &workspace.id,
                "agent:codex",
                "Codex",
                "Running",
                Some("blue".into()),
            )
            .unwrap();
        model
            .set_progress(&workspace.id, "build", "Build", 2.0, Some(4.0))
            .unwrap();
        (workspace.id, surface_id)
    };

    let summary = dispatch(&state, "status.summary", json!({})).await.unwrap();

    assert_eq!(summary["workspace"]["id"], workspace_id);
    assert_eq!(summary["workspace"]["focused_surface_id"], surface_id);
    assert_eq!(summary["agents"][0]["agent"], "codex");
    assert_eq!(summary["agents"][0]["session_id"], "codex-session-1");
    assert_eq!(summary["agents"][0]["source"], "persisted_agent_session");
    assert!(summary["agents"][0]["age_ms"].as_u64().is_some());
    assert_eq!(
        summary["agents"][0]["lifecycle_evidence"]["status_key"],
        "agent:codex"
    );
    assert_eq!(
        summary["agents"][0]["lifecycle_evidence"]["status_value"],
        "Running"
    );
    assert_eq!(
        summary["agents"][0]["lifecycle_evidence"]["status_source"],
        "model"
    );
    assert_eq!(
        summary["agents"][0]["lifecycle_evidence"]["status_scope"],
        "workspace_provider"
    );
    assert_eq!(summary["status"][0]["key"], "agent:codex");
    assert_eq!(summary["status"][0]["value"], "Running");
    assert_eq!(summary["status"][0]["source"], "model");
    assert_eq!(summary["progress"][0]["key"], "build");
    assert_eq!(summary["progress"][0]["value"], 2.0);
    assert_eq!(summary["progress"][0]["source"], "model");

    let scoped = dispatch(
        &state,
        "status.summary",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    assert_eq!(scoped["workspace"]["id"], summary["workspace"]["id"]);
}

#[tokio::test]
async fn agent_resume_opens_new_tab_with_provider_resume_argv() {
    let (state, backend) = test_state();
    let source_surface_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let source_surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &source_surface_id,
            AgentKind::Codex,
            "codex-session-1",
        ));
        source_surface_id
    };

    let resumed = dispatch(
        &state,
        "agent.resume",
        json!({"surface_id": source_surface_id}),
    )
    .await
    .unwrap();

    let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
    assert_ne!(resumed_surface_id, source_surface_id);
    assert_eq!(resumed["agent"], "codex");
    assert_eq!(resumed["session_id"], "codex-session-1");
    assert_eq!(
        resumed["argv"],
        json!(["codex", "resume", "codex-session-1"])
    );
    assert_eq!(backend.spawn_shell(resumed_surface_id).unwrap(), "codex");
    assert_eq!(
        backend.spawn_args(resumed_surface_id).unwrap(),
        vec!["resume", "codex-session-1"]
    );

    let model = state.model.lock().unwrap();
    let persisted = model
        .surface(resumed_surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(persisted.agent, AgentKind::Codex);
    assert_eq!(persisted.session_id, "codex-session-1");
}

#[tokio::test]
async fn agent_resume_opens_claude_tab_from_persisted_session_cwd() {
    let (state, backend) = test_state();
    let resume_cwd = tempfile::tempdir().unwrap();
    let source_surface_id = {
        let mut model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        let source_surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &source_surface_id,
            AgentKind::ClaudeCode,
            "claude-session-1",
        ));
        assert!(model.set_surface_agent_session_resume_cwd(
            &source_surface_id,
            resume_cwd.path().to_path_buf()
        ));
        source_surface_id
    };

    let resumed = dispatch(
        &state,
        "agent.resume",
        json!({"surface_id": source_surface_id}),
    )
    .await
    .unwrap();

    let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
    assert_ne!(resumed_surface_id, source_surface_id);
    assert_eq!(resumed["agent"], "claude_code");
    assert_eq!(resumed["session_id"], "claude-session-1");
    assert_eq!(
        resumed["argv"],
        json!(["claude", "--resume", "claude-session-1"])
    );
    assert_eq!(backend.spawn_shell(resumed_surface_id).unwrap(), "claude");
    assert_eq!(
        backend.spawn_args(resumed_surface_id).unwrap(),
        vec!["--resume", "claude-session-1"]
    );
    let spawned = backend
        .surfaces()
        .unwrap()
        .into_iter()
        .find(|surface| surface.surface_id == resumed_surface_id)
        .unwrap();
    assert_eq!(spawned.cwd, resume_cwd.path());

    let model = state.model.lock().unwrap();
    let persisted = model
        .surface(resumed_surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(persisted.resume_cwd.as_deref(), Some(resume_cwd.path()));
}

#[tokio::test]
async fn hook_permission_mode_reapplies_claude_bypass_resume_argv() {
    let (state, backend) = test_state();
    let (workspace_id, source_surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": source_surface_id,
            "key": "agent:claude",
            "label": "Claude",
            "value": "Ready",
            "color": "green",
            "hook_session_id": "claude-session-1",
            "hook_session_cwd": "/tmp",
            "hook_event_name": "session-start",
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": source_surface_id,
            "key": "agent:claude:permission",
            "label": "Claude mode",
            "value": "bypassPermissions",
            "color": "red",
            "hook_session_id": "claude-session-1",
            "hook_event_name": "session-start",
        }),
    )
    .await
    .unwrap();

    let health = dispatch(&state, "agent.health", json!({})).await.unwrap();
    assert_eq!(
        health[0]["argv"],
        json!([
            "claude",
            "--dangerously-skip-permissions",
            "--resume",
            "claude-session-1"
        ])
    );

    let resumed = dispatch(
        &state,
        "agent.resume",
        json!({"surface_id": source_surface_id}),
    )
    .await
    .unwrap();

    let resumed_surface_id = resumed["surface"]["id"].as_str().unwrap();
    assert_eq!(
        resumed["argv"],
        json!([
            "claude",
            "--dangerously-skip-permissions",
            "--resume",
            "claude-session-1"
        ])
    );
    assert_eq!(
        backend.spawn_args(resumed_surface_id).unwrap(),
        vec![
            "--dangerously-skip-permissions",
            "--resume",
            "claude-session-1"
        ]
    );
}

#[tokio::test]
async fn hook_permission_mode_reapplies_codex_bypass_resume_argv() {
    let (state, _backend) = test_state();
    let (workspace_id, source_surface_id) = {
        let model = state.model.lock().unwrap();
        let workspace = model.active_workspace().unwrap();
        (workspace.id.clone(), workspace.focused_surface_id.clone())
    };

    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": source_surface_id,
            "key": "agent:codex",
            "label": "Codex",
            "value": "Ready",
            "color": "green",
            "hook_session_id": "codex-session-1",
            "hook_session_cwd": "/tmp",
            "hook_event_name": "session-start",
        }),
    )
    .await
    .unwrap();
    dispatch(
        &state,
        "metadata.set_status",
        json!({
            "workspace_id": workspace_id,
            "surface_id": source_surface_id,
            "key": "agent:codex:permission",
            "label": "Codex mode",
            "value": "bypassPermissions",
            "color": "red",
            "hook_session_id": "codex-session-1",
            "hook_event_name": "session-start",
        }),
    )
    .await
    .unwrap();

    let resumed = dispatch(
        &state,
        "agent.resume",
        json!({"surface_id": source_surface_id}),
    )
    .await
    .unwrap();

    assert_eq!(
        resumed["argv"],
        json!([
            "codex",
            "--dangerously-bypass-approvals-and-sandbox",
            "resume",
            "-C",
            "/tmp",
            "codex-session-1"
        ])
    );
}
