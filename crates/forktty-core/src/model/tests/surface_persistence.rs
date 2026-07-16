//! Surface persistence and agent-session model regression tests.

use super::*;

#[test]
fn agent_session_makes_terminal_surface_persistable() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();

    assert!(model.to_session_data().surfaces.is_empty());
    assert!(model.set_surface_agent_session(
        &surface_id,
        crate::agents::AgentKind::Codex,
        "  codex-session-1  "
    ));
    assert!(!model.set_surface_agent_session(&surface_id, crate::agents::AgentKind::Codex, "  "));
    assert!(!model.set_surface_agent_session(
        "missing-surface",
        crate::agents::AgentKind::Codex,
        "codex-session-2"
    ));
    assert!(model.set_surface_agent_session_resume_cwd(
        &surface_id,
        std::path::PathBuf::from("/tmp/forktty-project")
    ));
    assert!(model.set_surface_agent_session_permission_mode(&surface_id, "bypassPermissions"));

    let data = model.to_session_data();
    crate::session::validate_session_data(&data).unwrap();
    assert_eq!(data.surfaces.len(), 1);
    assert_eq!(data.surfaces[0].id, surface_id);
    assert_eq!(
        data.surfaces[0].agent_session.as_ref().unwrap().agent,
        crate::agents::AgentKind::Codex
    );
    assert_eq!(
        data.surfaces[0].agent_session.as_ref().unwrap().session_id,
        "codex-session-1"
    );
    assert_eq!(
        data.surfaces[0]
            .agent_session
            .as_ref()
            .unwrap()
            .resume_cwd
            .as_deref(),
        Some(std::path::Path::new("/tmp/forktty-project"))
    );
    assert_eq!(
        data.surfaces[0]
            .agent_session
            .as_ref()
            .unwrap()
            .permission_mode
            .as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(
        data.surfaces[0].agent_session.as_ref().unwrap().lifecycle,
        crate::model::AgentSessionLifecycle::Running
    );
    assert!(model.set_surface_agent_session_last_activity_ms(&surface_id, 42_000));

    let data = model.to_session_data();
    crate::session::validate_session_data(&data).unwrap();
    assert_eq!(
        data.surfaces[0]
            .agent_session
            .as_ref()
            .unwrap()
            .last_activity_ms,
        42_000
    );
    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);
    let restored_session = restored
        .surface(&surface_id)
        .unwrap()
        .agent_session
        .as_ref()
        .unwrap();
    assert_eq!(restored_session.agent, crate::agents::AgentKind::Codex);
    assert_eq!(restored_session.session_id, "codex-session-1");
    assert_eq!(
        restored_session.resume_cwd.as_deref(),
        Some(std::path::Path::new("/tmp/forktty-project"))
    );
    assert_eq!(
        restored_session.permission_mode.as_deref(),
        Some("bypassPermissions")
    );
    assert_eq!(
        restored_session.lifecycle,
        crate::model::AgentSessionLifecycle::Running
    );
    assert_eq!(restored_session.last_activity_ms, 42_000);
    assert!(restored.set_surface_agent_session_lifecycle(
        &surface_id,
        crate::model::AgentSessionLifecycle::Ended
    ));
    assert_eq!(
        restored
            .surface(&surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .lifecycle,
        crate::model::AgentSessionLifecycle::Ended
    );
}

#[test]
fn persisted_scrollback_makes_terminal_surface_persistable() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();

    assert!(model.to_session_data().surfaces.is_empty());
    assert!(model.set_surface_persisted_scrollback(&surface_id, Some("one\ntwo\n".to_string())));

    let data = model.to_session_data();
    crate::session::validate_session_data(&data).unwrap();
    assert_eq!(data.surfaces.len(), 1);
    assert_eq!(
        data.surfaces[0].persisted_scrollback.as_deref(),
        Some("one\ntwo\n")
    );

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);
    assert_eq!(
        restored
            .surface(&surface_id)
            .unwrap()
            .persisted_scrollback
            .as_deref(),
        Some("one\ntwo\n")
    );

    assert!(model.set_surface_persisted_scrollback(&surface_id, None));
    assert!(model.to_session_data().surfaces.is_empty());
}

#[test]
fn terminal_surface_cwd_that_differs_from_workspace_is_persisted() {
    let workspace_dir = tempfile::tempdir().unwrap();
    let live_dir = tempfile::tempdir().unwrap();
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", workspace_dir.path());
    let surface_id = workspace.focused_surface_id;

    assert!(model.set_surface_cwd(&surface_id, live_dir.path().to_path_buf()));

    let data = model.to_session_data();
    crate::session::validate_session_data(&data).unwrap();
    assert_eq!(data.surfaces.len(), 1);
    assert_eq!(data.surfaces[0].cwd, live_dir.path());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);
    assert_eq!(restored.surface(&surface_id).unwrap().cwd, live_dir.path());
}

#[test]
fn clearing_agent_session_forgets_metadata_without_closing_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();

    assert!(model.set_surface_agent_session(
        &surface_id,
        crate::agents::AgentKind::Codex,
        "codex-session-1"
    ));
    assert_eq!(model.to_session_data().surfaces.len(), 1);

    let forgotten = model
        .clear_surface_agent_session(&surface_id)
        .expect("agent session should be forgotten");
    assert_eq!(forgotten.session_id, "codex-session-1");
    assert!(model.surface(&surface_id).is_some());
    assert!(model.surface(&surface_id).unwrap().agent_session.is_none());
    assert!(model.to_session_data().surfaces.is_empty());
    assert!(model.clear_surface_agent_session(&surface_id).is_none());
    assert!(model
        .clear_surface_agent_session("missing-surface")
        .is_none());

    assert!(model.set_surface_agent_session(
        &surface_id,
        crate::agents::AgentKind::ClaudeCode,
        "new-session"
    ));
    assert!(!model.restore_surface_agent_session(&surface_id, forgotten.clone()));
    assert_eq!(
        model
            .surface(&surface_id)
            .unwrap()
            .agent_session
            .as_ref()
            .unwrap()
            .session_id,
        "new-session"
    );
    assert_eq!(
        model
            .clear_surface_agent_session(&surface_id)
            .unwrap()
            .session_id,
        "new-session"
    );

    assert!(model.restore_surface_agent_session(&surface_id, forgotten));
    assert!(model.surface(&surface_id).unwrap().agent_session.is_some());
    assert_eq!(model.to_session_data().surfaces.len(), 1);
}

#[test]
fn ending_last_agent_session_removes_agent_status_and_progress() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();

    assert!(model.set_surface_agent_session(
        &surface_id,
        crate::agents::AgentKind::Codex,
        "codex-session-1"
    ));
    model
        .set_status(&workspace.id, "agent:codex", "Codex", "Ready", None)
        .unwrap();
    model
        .set_status(
            &workspace.id,
            "agent:codex:permission",
            "Codex mode",
            "bypassPermissions",
            Some("red".to_string()),
        )
        .unwrap();
    model
        .set_status(&workspace.id, "agent:claude", "Claude", "Ready", None)
        .unwrap();
    model
        .set_progress(
            &workspace.id,
            "agent:codex:tokens",
            "Codex tokens",
            42.0,
            Some(100.0),
        )
        .unwrap();
    model
        .set_progress(&workspace.id, "build", "Build", 1.0, Some(2.0))
        .unwrap();

    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        crate::model::AgentSessionLifecycle::Ended
    ));

    let statuses = model.list_status(&workspace.id);
    assert!(!statuses.iter().any(|status| status.key == "agent:codex"));
    assert!(!statuses
        .iter()
        .any(|status| status.key == "agent:codex:permission"));
    assert!(statuses.iter().any(|status| status.key == "agent:claude"));
    let progress = model.list_progress(&workspace.id);
    assert!(!progress
        .iter()
        .any(|entry| entry.key == "agent:codex:tokens"));
    assert!(progress.iter().any(|entry| entry.key == "build"));
}

#[test]
fn closing_agent_surface_removes_surface_and_agent_metadata() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let surface_id = workspace.focused_surface_id.clone();
    let agent_surface = model.add_tab(&surface_id).expect("add tab");

    assert!(model.set_surface_agent_session(
        &agent_surface.id,
        crate::agents::AgentKind::OpenCode,
        "opencode-session-1"
    ));
    model
        .set_status(&workspace.id, "agent:opencode", "OpenCode", "Ready", None)
        .unwrap();
    model
        .set_status(
            &workspace.id,
            "agent:opencode:permission",
            "OpenCode mode",
            "bypassPermissions",
            Some("red".to_string()),
        )
        .unwrap();
    model
        .set_status(
            &workspace.id,
            format!("surface:{}:status", agent_surface.id),
            "Terminal",
            "Closed",
            None,
        )
        .unwrap();
    model
        .set_progress(
            &workspace.id,
            "agent:opencode:tokens",
            "OpenCode tokens",
            10.0,
            Some(100.0),
        )
        .unwrap();

    let removed = model
        .close_surface(&agent_surface.id)
        .expect("surface removed");
    assert_eq!(removed.id, agent_surface.id);

    let statuses = model.list_status(&workspace.id);
    assert!(!statuses.iter().any(|status| status.key == "agent:opencode"));
    assert!(!statuses
        .iter()
        .any(|status| status.key == "agent:opencode:permission"));
    assert!(!statuses.iter().any(|status| status
        .key
        .starts_with(&format!("surface:{}:", agent_surface.id))));
    assert!(!model
        .list_progress(&workspace.id)
        .iter()
        .any(|entry| entry.key == "agent:opencode:tokens"));
}
