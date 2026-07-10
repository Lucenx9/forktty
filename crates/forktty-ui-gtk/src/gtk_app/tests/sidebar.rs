//! Sidebar metadata, workspace badge, and active workspace summary regression tests.

use super::*;

struct IsolatedSidebarUserDirs {
    state_home: PathBuf,
}

impl IsolatedSidebarUserDirs {
    fn session_path(&self) -> PathBuf {
        self.state_home.join("forktty").join("session-v2.json")
    }
}

fn with_isolated_sidebar_user_dirs<T>(f: impl FnOnce(&IsolatedSidebarUserDirs) -> T) -> T {
    let home_dir = tempfile::tempdir().unwrap();
    let home_path = home_dir.path().to_path_buf();
    let state_home = home_path.join("state");
    let data_home = home_path.join("data");
    let config_home = home_path.join("config");
    let cache_home = home_path.join("cache");
    let dirs = IsolatedSidebarUserDirs {
        state_home: state_home.clone(),
    };
    let home = home_path.to_string_lossy().into_owned();
    let state = state_home.to_string_lossy().into_owned();
    let data = data_home.to_string_lossy().into_owned();
    let config = config_home.to_string_lossy().into_owned();
    let cache = cache_home.to_string_lossy().into_owned();

    crate::test_env::with_env(
        &[
            ("HOME", Some(home.as_str())),
            ("XDG_STATE_HOME", Some(state.as_str())),
            ("XDG_DATA_HOME", Some(data.as_str())),
            ("XDG_CONFIG_HOME", Some(config.as_str())),
            ("XDG_CACHE_HOME", Some(cache.as_str())),
        ],
        || f(&dirs),
    )
}

fn sidebar_workspace_order(model: &WorkspaceModel) -> Vec<String> {
    model
        .list_workspaces()
        .into_iter()
        .map(|workspace| workspace.id)
        .collect()
}

#[test]
fn builds_surface_metadata_keys() {
    assert_eq!(surface_status_key("surface-1"), "surface:surface-1:status");
}

#[test]
fn detects_exited_terminal_status_for_sidebar_badge() {
    let status = StatusEntry {
        key: surface_status_key("surface-1"),
        label: "Terminal".to_string(),
        value: "Exited (0)".to_string(),
        color: Some("yellow".to_string()),
    };

    assert!(status_entry_suggests_exited(&status));
    assert!(!status_entry_suggests_error(&status));
}

#[test]
fn closed_terminal_status_blocks_auto_spawn() {
    let status = StatusEntry {
        key: surface_status_key("surface-1"),
        label: "Terminal".to_string(),
        value: "Closed".to_string(),
        color: None,
    };

    assert!(status_entry_suggests_exited(&status));
    assert!(surface_status_blocks_auto_spawn(&[status], "surface-1"));
}

#[test]
fn sidebar_badge_ignores_stale_surface_exit_status() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let stale_exit = StatusEntry {
        key: surface_status_key("surface-missing"),
        label: "Terminal".to_string(),
        value: "Exited (0)".to_string(),
        color: Some("yellow".to_string()),
    };
    let running = StatusEntry {
        key: "agent:codex".to_string(),
        label: "Codex".to_string(),
        value: "Running Bash".to_string(),
        color: Some("blue".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[stale_exit, running], &[], None).unwrap();

    assert_eq!(badge.label, "Working");
    assert_eq!(badge.class_name, "running");
}

#[test]
fn workspace_meta_line_prefers_agent_resume_cwd_for_visible_project() {
    let mut model = WorkspaceModel::new();
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let project = home.join("forktty");
    let workspace = model.create_workspace("main", home.clone());
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "codex-session",
    ));
    assert!(model.set_surface_agent_session_resume_cwd(&surface_id, project));
    let workspace = model.list_workspaces().remove(0);
    let surface = model.surface(&surface_id).unwrap();

    let meta = workspace_meta_line(
        &workspace,
        None,
        Some(surface_effective_project_cwd(surface)),
    );

    assert!(meta.ends_with("forktty"), "{meta}");
    assert!(!meta.ends_with(home.to_string_lossy().as_ref()), "{meta}");
}

#[test]
fn workspace_meta_line_keeps_workspace_root_without_agent_resume_cwd() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_cwd(&surface_id, PathBuf::from("/tmp/project/subdir")));
    let workspace = model.list_workspaces().remove(0);
    let surface = model.surface(&surface_id).unwrap();

    let meta = workspace_meta_line(&workspace, None, surface_agent_resume_cwd(surface));

    assert!(meta.ends_with("/tmp/project"), "{meta}");
    assert!(!meta.contains("subdir"), "{meta}");
}

#[test]
fn sidebar_snapshot_hides_launch_cwd_title_when_effective_cwd_differs() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id;
        assert!(model.set_surface_title(&surface_id, "/tmp".to_string()));
        assert!(model.set_surface_agent_session(
            &surface_id,
            forktty_core::AgentKind::Codex,
            "codex-session",
        ));
        assert!(
            model.set_surface_agent_session_resume_cwd(&surface_id, PathBuf::from("/tmp/forktty"),)
        );
    }

    let snapshot = sidebar_snapshot(&state);

    assert_eq!(snapshot.active_full_path.as_deref(), Some("/tmp/forktty"));
    assert_eq!(snapshot.active_pane_label, None);
}

#[test]
fn sidebar_snapshot_counts_panes_not_tabs_for_count_badge() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let first = workspace.focused_surface_id.clone();
        model.add_tab(&first).unwrap();
    }

    let snapshot = sidebar_snapshot(&state);

    assert_eq!(snapshot.rows[0].pane_count, 1);
}

#[test]
fn sidebar_metadata_keeps_inactive_tab_surface_status() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let inactive_surface_id = workspace.focused_surface_id.clone();
    let active_surface_id = model.add_tab(&inactive_surface_id).unwrap().id;
    let status_key = surface_status_key(&inactive_surface_id);
    let progress_key = format!("surface:{inactive_surface_id}:download");
    model
        .set_status(
            &workspace_id,
            &status_key,
            "Terminal",
            "Exited (0)",
            Some("yellow".to_string()),
        )
        .unwrap();
    model
        .set_progress(&workspace_id, &progress_key, "Download", 1.0, Some(2.0))
        .unwrap();
    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, active_surface_id);

    let (statuses, progress) = sidebar_visible_metadata(
        &model,
        &workspace,
        &model.list_status(&workspace_id),
        &model.list_progress(&workspace_id),
    );

    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].key, status_key);
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].key, progress_key);
    let badge = workspace_status_badge(&workspace, &statuses, &progress, None).unwrap();
    assert_eq!(badge.label, "Exited");
}

#[test]
fn sidebar_activity_summary_ignores_inactive_agent_metadata() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "codex-session",
    ));
    model
        .set_status(
            &workspace_id,
            "agent:codex",
            "Codex",
            "Running Bash",
            Some("blue".to_string()),
        )
        .unwrap();
    model
        .set_status(
            &workspace_id,
            "agent:opencode",
            "OpenCode",
            "Ready",
            Some("green".to_string()),
        )
        .unwrap();
    model
        .set_status(
            &workspace_id,
            surface_status_key("surface-missing"),
            "Terminal",
            "Closed",
            None,
        )
        .unwrap();
    model
        .set_progress(
            &workspace_id,
            "agent:claude:tokens",
            "Claude input tokens",
            50.0,
            Some(100.0),
        )
        .unwrap();
    let workspace = model.list_workspaces().remove(0);
    let (statuses, progress) = sidebar_visible_metadata(
        &model,
        &workspace,
        &model.list_status(&workspace_id),
        &model.list_progress(&workspace_id),
    );

    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].key, "agent:codex");
    assert!(progress.is_empty());
    assert_eq!(
        format_workspace_activity_summary(&statuses, &progress, None, None),
        "Codex: Working"
    );
}

#[test]
fn sidebar_uses_running_session_fallback_for_every_supported_harness() {
    let harnesses = [
        (forktty_core::AgentKind::Codex, "Codex"),
        (forktty_core::AgentKind::ClaudeCode, "Claude"),
        (forktty_core::AgentKind::Pi, "Pi"),
        (forktty_core::AgentKind::OpenCode, "OpenCode"),
        (forktty_core::AgentKind::Antigravity, "Antigravity"),
        (forktty_core::AgentKind::Grok, "Grok"),
    ];

    for (index, (agent, label)) in harnesses.into_iter().enumerate() {
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace(format!("harness-{index}"), "/tmp");
        let workspace_id = workspace.id.clone();
        assert!(model.set_surface_agent_session(
            &workspace.focused_surface_id,
            agent,
            format!("session-{index}"),
        ));
        let workspace = model
            .list_workspaces()
            .into_iter()
            .find(|candidate| candidate.id == workspace_id)
            .unwrap();

        let (statuses, progress) = sidebar_visible_metadata(&model, &workspace, &[], &[]);
        assert_eq!(statuses.len(), 1, "missing {label} lifecycle status");
        assert_eq!(statuses[0].label, label);
        assert_eq!(statuses[0].value, "Running");
        assert!(progress.is_empty());

        let badge = workspace_status_badge(&workspace, &statuses, &progress, None).unwrap();
        assert_eq!(badge.label, "Working", "wrong {label} badge");
        assert_eq!(
            format_workspace_activity_summary(&statuses, &progress, None, None),
            format!("{label}: Working"),
        );
    }
}

#[test]
fn sidebar_lifecycle_fallback_yields_to_primary_hook_status() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    assert!(model.set_surface_agent_session(
        &workspace.focused_surface_id,
        forktty_core::AgentKind::Codex,
        "codex-session",
    ));
    model
        .set_status(
            &workspace_id,
            "agent:codex:permission",
            "Codex mode",
            "full-auto",
            Some("red".to_string()),
        )
        .unwrap();
    let workspace = model.list_workspaces().remove(0);

    let (statuses, _) =
        sidebar_visible_metadata(&model, &workspace, &model.list_status(&workspace_id), &[]);
    assert_eq!(statuses.len(), 2);
    assert!(statuses.iter().any(|status| {
        status.key == "agent:codex" && status.label == "Codex" && status.value == "Running"
    }));

    model
        .set_status(
            &workspace_id,
            "agent:codex",
            "Codex",
            "Running tool",
            Some("blue".to_string()),
        )
        .unwrap();
    let (statuses, _) =
        sidebar_visible_metadata(&model, &workspace, &model.list_status(&workspace_id), &[]);
    assert_eq!(
        statuses
            .iter()
            .filter(|status| status.key == "agent:codex")
            .count(),
        1
    );
    assert!(statuses
        .iter()
        .any(|status| status.key == "agent:codex" && status.value == "Running tool"));
}

#[test]
fn sidebar_lifecycle_fallback_only_badges_actionable_sessions() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "codex-session",
    ));
    let workspace = model.list_workspaces().remove(0);

    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::NeedsInput,
    ));
    let (statuses, progress) = sidebar_visible_metadata(&model, &workspace, &[], &[]);
    assert_eq!(statuses[0].value, "Needs input");
    assert_eq!(
        workspace_status_badge(&workspace, &statuses, &progress, None)
            .unwrap()
            .label,
        "Input"
    );

    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Idle,
    ));
    let (statuses, progress) = sidebar_visible_metadata(&model, &workspace, &[], &[]);
    assert!(statuses.is_empty());
    assert!(workspace_status_badge(&workspace, &statuses, &progress, None).is_none());

    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Ended,
    ));
    let (statuses, progress) = sidebar_visible_metadata(&model, &workspace, &[], &[]);
    assert!(statuses.is_empty());
    assert!(workspace_status_badge(&workspace, &statuses, &progress, None).is_none());
    assert!(model.list_status(&workspace_id).is_empty());
}

#[test]
fn stale_surface_notification_does_not_target_workspace() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let surface_id = workspace.focused_surface_id.clone();
    let notification = model.create_notification(
        "Continue?",
        "",
        NotificationKind::Prompt,
        Some(workspace_id.clone()),
        Some(surface_id.clone()),
    );

    assert!(model.close_surface(&surface_id).is_some());

    assert!(!notification_targets_workspace(
        &model,
        &notification,
        &workspace_id
    ));
}

#[test]
fn move_active_workspace_relative_reorders_active_workspace() {
    with_isolated_sidebar_user_dirs(|dirs| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (first_id, second_id, third_id) = {
            let mut model = model.lock().unwrap();
            let first_id = model.create_workspace("first", "/tmp/first").id;
            let second_id = model.create_workspace("second", "/tmp/second").id;
            let third_id = model.create_workspace("third", "/tmp/third").id;
            model.select_workspace(WorkspaceSelector::Id(&second_id));
            (first_id, second_id, third_id)
        };

        assert!(move_active_workspace_relative(&state, -1));
        {
            let model = model.lock().unwrap();
            assert_eq!(
                sidebar_workspace_order(&model),
                vec![second_id.clone(), first_id.clone(), third_id.clone()]
            );
            assert_eq!(
                model.active_workspace_id().as_deref(),
                Some(second_id.as_str())
            );
        }
        assert!(dirs.session_path().exists());

        assert!(move_active_workspace_relative(&state, 1));
        let model = model.lock().unwrap();
        assert_eq!(
            sidebar_workspace_order(&model),
            vec![first_id, second_id.clone(), third_id]
        );
        assert_eq!(
            model.active_workspace_id().as_deref(),
            Some(second_id.as_str())
        );
    });
}

#[test]
fn move_active_workspace_relative_noops_at_edges() {
    with_isolated_sidebar_user_dirs(|_| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (first_id, second_id) = {
            let mut model = model.lock().unwrap();
            let first_id = model.create_workspace("first", "/tmp/first").id;
            let second_id = model.create_workspace("second", "/tmp/second").id;
            (first_id, second_id)
        };

        assert!(!move_active_workspace_relative(&state, 1));
        {
            let model = model.lock().unwrap();
            assert_eq!(
                sidebar_workspace_order(&model),
                vec![first_id.clone(), second_id.clone()]
            );
        }

        {
            let mut model = model.lock().unwrap();
            model.select_workspace(WorkspaceSelector::Id(&first_id));
        }
        assert!(!move_active_workspace_relative(&state, -1));
        let model = model.lock().unwrap();
        assert_eq!(sidebar_workspace_order(&model), vec![first_id, second_id]);
    });
}

#[test]
fn active_workspace_can_move_relative_tracks_edges() {
    with_isolated_sidebar_user_dirs(|_| {
        let model = Arc::new(Mutex::new(WorkspaceModel::new()));
        let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
        let state = SocketAppState::new(
            model.clone(),
            terminal,
            "/bin/sh",
            PathBuf::from("/tmp/forktty.sock"),
        )
        .with_notification_dispatch(false);
        let (first_id, second_id) = {
            let mut model = model.lock().unwrap();
            let first_id = model.create_workspace("first", "/tmp/first").id;
            let second_id = model.create_workspace("second", "/tmp/second").id;
            model.select_workspace(WorkspaceSelector::Id(&second_id));
            (first_id, second_id)
        };

        assert!(active_workspace_can_move_relative(&state, -1));
        assert!(!active_workspace_can_move_relative(&state, 1));

        {
            let mut model = model.lock().unwrap();
            model.select_workspace(WorkspaceSelector::Id(&first_id));
        }
        assert!(!active_workspace_can_move_relative(&state, -1));
        assert!(active_workspace_can_move_relative(&state, 1));

        {
            let mut model = model.lock().unwrap();
            model.select_workspace(WorkspaceSelector::Id(&second_id));
            assert!(model
                .close_workspace(WorkspaceSelector::Id(&first_id))
                .is_some());
        }
        assert!(!active_workspace_can_move_relative(&state, -1));
        assert!(!active_workspace_can_move_relative(&state, 1));
    });
}

// Regression: a workspace running Claude in bypassPermissions stayed badged
// "ERROR" forever -- the red permission-mode pill (deliberate, it flags the
// risky mode) was read by the badge heuristics as a workspace failure. Mode
// indicators must not drive the Error/Exited badge.
#[test]
fn sidebar_badge_ignores_permission_mode_pills() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let agent_running = StatusEntry {
        key: "agent:claude".to_string(),
        label: "Claude".to_string(),
        value: "Running Bash".to_string(),
        color: Some("blue".to_string()),
    };
    let bypass_mode = StatusEntry {
        key: "agent:claude:permission".to_string(),
        label: "Claude mode".to_string(),
        value: "bypassPermissions".to_string(),
        color: Some("red".to_string()),
    };
    let codex_yolo_mode = StatusEntry {
        key: "agent:codex:permission".to_string(),
        label: "Codex mode".to_string(),
        value: "bypassPermissions".to_string(),
        color: Some("red".to_string()),
    };
    let accept_edits_mode = StatusEntry {
        key: "agent:codex:permission".to_string(),
        label: "Codex mode".to_string(),
        value: "acceptEdits".to_string(),
        color: Some("yellow".to_string()),
    };

    let badge = workspace_status_badge(
        &workspace,
        &[
            agent_running,
            bypass_mode,
            codex_yolo_mode,
            accept_edits_mode,
        ],
        &[],
        None,
    )
    .unwrap();

    assert_eq!(badge.label, "Working");
    assert_eq!(badge.class_name, "running");
}

#[test]
fn sidebar_badge_keeps_non_agent_permission_status_errors() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: "deploy:permission".to_string(),
        label: "Deploy".to_string(),
        value: "Permission failed".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], None).unwrap();

    assert_eq!(badge.label, "Error");
    assert_eq!(badge.class_name, "error");
}

#[test]
fn sidebar_badge_keeps_error_ahead_of_info_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_notification(
        "Heads up",
        "Background task finished",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], Some(&notification)).unwrap();

    assert_eq!(badge.label, "Error");
    assert_eq!(badge.class_name, "error");
}

#[test]
fn sidebar_badge_keeps_prompt_ahead_of_error_status() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_notification(
        "Continue?",
        "",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "Spawn failed: /bin/missing".to_string(),
        color: Some("red".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], Some(&notification)).unwrap();

    assert_eq!(badge.label, "Input");
    assert_eq!(badge.class_name, "needs-input");
}

#[test]
fn sidebar_badge_highlights_status_needs_input_without_notification() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let status = StatusEntry {
        key: "agent:codex".to_string(),
        label: "Codex".to_string(),
        value: "needs_input".to_string(),
        color: Some("yellow".to_string()),
    };

    let badge = workspace_status_badge(&workspace, &[status], &[], None).unwrap();

    assert_eq!(badge.label, "Input");
    assert_eq!(badge.class_name, "needs-input");
}

#[test]
fn sidebar_badge_distinguishes_starting_and_surface_missing_status() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("main", "/tmp");
    let workspace = model.list_workspaces().remove(0);
    let starting = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "Starting".to_string(),
        color: Some("blue".to_string()),
    };
    let missing = StatusEntry {
        key: surface_status_key(&workspace.focused_surface_id),
        label: "Terminal".to_string(),
        value: "surface_missing".to_string(),
        color: None,
    };

    let badge =
        workspace_status_badge(&workspace, std::slice::from_ref(&starting), &[], None).unwrap();
    assert_eq!(badge.label, "Starting");
    assert_eq!(badge.class_name, "working");

    let badge = workspace_status_badge(&workspace, &[missing], &[], None).unwrap();
    assert_eq!(badge.label, "Missing");
    assert_eq!(badge.class_name, "exited");
}
