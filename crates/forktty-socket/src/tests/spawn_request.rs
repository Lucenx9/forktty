//! Socket spawn request restoration and validation tests.

use super::*;

#[derive(Debug, Default)]
struct SpawnFailsBackend {
    inner: HeadlessTerminalBackend,
}

impl TerminalBackend for SpawnFailsBackend {
    fn spawn(&self, _request: SpawnRequest) -> Result<(), TerminalError> {
        Err(TerminalError::Backend("runtime spawn failed".to_string()))
    }

    fn send_text(&self, surface_id: &str, text: &str) -> Result<(), TerminalError> {
        self.inner.send_text(surface_id, text)
    }

    fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> Result<(), TerminalError> {
        self.inner.resize(surface_id, cols, rows)
    }

    fn close(&self, surface_id: &str) -> Result<(), TerminalError> {
        self.inner.close(surface_id)
    }

    fn surfaces(&self) -> Result<Vec<TerminalSurfaceState>, TerminalError> {
        self.inner.surfaces()
    }
}

#[test]
fn spawn_terminal_surfaces_respawns_ssh_surfaces() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model,
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let surface = forktty_core::Surface {
        id: "surface-ssh".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "ssh".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Ssh {
            host: "user@example.test".to_string(),
        },
        agent_session: None,
        persisted_scrollback: None,
    };

    spawn_terminal_surfaces(&state, &[surface]).unwrap();

    assert!(backend.spawn_shell("surface-ssh").unwrap().ends_with("ssh"));
    assert_eq!(
        backend.spawn_args("surface-ssh").unwrap(),
        vec!["user@example.test"]
    );
}

#[test]
fn persisted_spawn_planning_failure_records_error_without_spawning_shell() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let backend = Arc::new(HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        backend.clone(),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let surface_id = workspace.focused_surface_id.clone();
        let workspace_id = workspace.id;
        assert!(model.set_surface_agent_session(&surface_id, AgentKind::Custom, "custom-session",));
        let surface = model.surface(&surface_id).unwrap().clone();
        (workspace_id, surface)
    };

    let error = spawn_terminal_surfaces(&state, std::slice::from_ref(&surface)).unwrap_err();

    assert!(error.contains("does not have a safe resume command"));
    assert!(backend.surfaces().unwrap().is_empty());
    let model = model.lock().unwrap();
    let status = model
        .list_status(&workspace_id)
        .into_iter()
        .find(|entry| entry.key == surface_status_key(&surface.id))
        .expect("planning failure should block plain-shell auto-spawn");
    assert_eq!(status.color.as_deref(), Some("red"));
    assert!(status.value.starts_with("Spawn failed:"));
    assert_eq!(
        model
            .list_logs(&workspace_id)
            .iter()
            .filter(|entry| {
                entry.level == forktty_core::LogLevel::Error
                    && entry.message.contains("spawn failed")
            })
            .count(),
        1
    );
    let notifications = model.list_notifications();
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].kind, forktty_core::NotificationKind::Error);
    assert_eq!(
        notifications[0].surface_id.as_deref(),
        Some(surface.id.as_str())
    );
}

#[test]
fn runtime_restore_failure_records_one_complete_spawn_failure() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let state = SocketAppState::new(
        model.clone(),
        Arc::new(SpawnFailsBackend::default()),
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp");
        let surface = model
            .surface(&workspace.focused_surface_id)
            .unwrap()
            .clone();
        (workspace.id, surface)
    };

    let error = spawn_terminal_surfaces(&state, std::slice::from_ref(&surface)).unwrap_err();

    assert!(error.contains("runtime spawn failed"));
    let model = model.lock().unwrap();
    assert_eq!(
        model
            .list_logs(&workspace_id)
            .iter()
            .filter(|entry| entry.message.contains("spawn failed"))
            .count(),
        1
    );
    assert_eq!(model.list_notifications().len(), 1);
    assert!(model
        .list_status(&workspace_id)
        .iter()
        .any(|entry| entry.key == surface_status_key(&surface.id)
            && entry.color.as_deref() == Some("red")));
}

#[test]
fn spawn_request_revalidates_ssh_host_from_restored_surface() {
    let base = SpawnRequest::for_surface(
        &forktty_core::Surface {
            id: "surface-ssh".to_string(),
            workspace_id: "workspace-1".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: "ssh".to_string(),
            unread: false,
            needs_attention: false,
            kind: forktty_core::SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        },
        "/bin/sh",
        "/tmp/forktty.sock",
    );

    // A valid host is spawned as a single argv element after the ssh binary.
    let valid = spawn_request_for_surface_kind(
        base.clone(),
        &forktty_core::SurfaceKind::Ssh {
            host: "user@example.test".to_string(),
        },
    )
    .expect("valid ssh host should spawn");
    assert_eq!(valid.args, vec!["user@example.test".to_string()]);

    // A tampered/persisted host that would smuggle ssh options must not be
    // spawned, even though it never passed `workspace.create_ssh`.
    for malicious in [
        "-oProxyCommand=touch /tmp/pwned",
        "-l root",
        "host with space",
    ] {
        assert!(
            spawn_request_for_surface_kind(
                base.clone(),
                &forktty_core::SurfaceKind::Ssh {
                    host: malicious.to_string(),
                },
            )
            .is_none(),
            "malicious ssh host {malicious:?} must be rejected on respawn"
        );
    }
}

#[test]
fn spawn_request_resumes_restored_agent_terminal_surface() {
    let surface = forktty_core::Surface {
        id: "surface-agent".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "agent".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Terminal,
        agent_session: Some(forktty_core::AgentSession {
            agent: AgentKind::Codex,
            session_id: "codex-session-1".to_string(),
            resume_cwd: Some(PathBuf::from("/tmp/forktty-project")),
            permission_mode: None,
            lifecycle: AgentSessionLifecycle::Running,
            last_activity_ms: 12_345,
        }),
        persisted_scrollback: None,
    };

    let request = spawn_request_for_surface(
        SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
        &surface,
    )
    .expect("valid persisted metadata should plan")
    .expect("agent terminal surface should spawn");

    assert_eq!(request.shell, "codex");
    assert_eq!(
        request.args,
        vec![
            "resume".to_string(),
            "-C".to_string(),
            "/tmp/forktty-project".to_string(),
            "codex-session-1".to_string(),
        ]
    );
}

#[test]
fn spawn_request_skips_suspended_agent_terminal_surface() {
    let surface = forktty_core::Surface {
        id: "surface-agent".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "agent".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Terminal,
        agent_session: Some(forktty_core::AgentSession {
            agent: AgentKind::Codex,
            session_id: "codex-session-1".to_string(),
            resume_cwd: Some(PathBuf::from("/tmp/forktty-project")),
            permission_mode: None,
            lifecycle: AgentSessionLifecycle::Suspended,
            last_activity_ms: 12_345,
        }),
        persisted_scrollback: None,
    };

    assert!(spawn_request_for_surface(
        SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
        &surface,
    )
    .expect("suspended is an intentional skip")
    .is_none());
}

#[test]
fn spawn_request_reapplies_persisted_bypass_permission_mode() {
    let surface = forktty_core::Surface {
        id: "surface-agent".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "agent".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Terminal,
        agent_session: Some(forktty_core::AgentSession {
            agent: AgentKind::ClaudeCode,
            session_id: "claude-session-1".to_string(),
            resume_cwd: Some(PathBuf::from("/tmp/forktty-project")),
            permission_mode: Some("bypassPermissions".to_string()),
            lifecycle: AgentSessionLifecycle::Running,
            last_activity_ms: 12_345,
        }),
        persisted_scrollback: None,
    };

    let request = spawn_request_for_surface(
        SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
        &surface,
    )
    .expect("valid persisted metadata should plan")
    .expect("agent terminal surface should spawn");

    assert_eq!(request.shell, "claude");
    assert_eq!(
        request.args,
        vec![
            "--dangerously-skip-permissions".to_string(),
            "--resume".to_string(),
            "claude-session-1".to_string()
        ]
    );
    assert_eq!(request.cwd, PathBuf::from("/tmp/forktty-project"));
}

#[test]
#[serial_test::serial]
fn spawn_request_uses_codex_session_cwd_fallback_when_not_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    let codex_home = dir.path().join("codex");
    let sessions_dir = codex_home.join("sessions/2026/06/12");
    fs::create_dir_all(&sessions_dir).unwrap();
    fs::write(
        sessions_dir.join("rollout-2026-06-12T15-21-07-codex-session-fallback.jsonl"),
        format!(
            "{}\n",
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "codex-session-fallback",
                    "cwd": project.to_string_lossy(),
                }
            })
        ),
    )
    .unwrap();
    let _env = EnvGuard::set("CODEX_HOME", codex_home.to_str().unwrap());
    let surface = forktty_core::Surface {
        id: "surface-agent".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "agent".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Terminal,
        agent_session: Some(forktty_core::AgentSession {
            agent: AgentKind::Codex,
            session_id: "codex-session-fallback".to_string(),
            resume_cwd: None,
            permission_mode: None,
            lifecycle: AgentSessionLifecycle::Running,
            last_activity_ms: 12_345,
        }),
        persisted_scrollback: None,
    };

    let request = spawn_request_for_surface(
        SpawnRequest::for_surface(&surface, "/bin/sh", "/tmp/forktty.sock"),
        &surface,
    )
    .expect("valid persisted metadata should plan")
    .expect("agent terminal surface should spawn");

    assert_eq!(request.shell, "codex");
    assert_eq!(
        request.args,
        vec![
            "resume".to_string(),
            "-C".to_string(),
            project.to_string_lossy().into_owned(),
            "codex-session-fallback".to_string(),
        ]
    );
}

#[test]
fn persisted_spawn_planning_reports_invalid_agent_metadata_and_skips_browser() {
    let mut surface = forktty_core::Surface {
        id: "surface-agent".to_string(),
        workspace_id: "workspace-1".to_string(),
        cwd: PathBuf::from("/tmp"),
        title: "agent".to_string(),
        unread: false,
        needs_attention: false,
        kind: forktty_core::SurfaceKind::Terminal,
        agent_session: Some(forktty_core::AgentSession {
            agent: AgentKind::Codex,
            session_id: "-invalid".to_string(),
            resume_cwd: None,
            permission_mode: None,
            lifecycle: AgentSessionLifecycle::Running,
            last_activity_ms: 12_345,
        }),
        persisted_scrollback: None,
    };
    let plan = |surface: &forktty_core::Surface| {
        spawn_request_for_surface(
            SpawnRequest::for_surface(surface, "/bin/sh", "/tmp/forktty.sock"),
            surface,
        )
    };

    assert_eq!(
        plan(&surface),
        Err(PersistedSurfaceSpawnError::AgentResume(
            forktty_core::AgentResumeError::InvalidSessionId,
        ))
    );

    for agent in [
        AgentKind::Codex,
        AgentKind::ClaudeCode,
        AgentKind::Antigravity,
        AgentKind::Grok,
        AgentKind::Pi,
        AgentKind::OpenCode,
    ] {
        let session = surface.agent_session.as_mut().unwrap();
        session.agent = agent;
        session.session_id = "valid-session".to_string();
        session.resume_cwd = Some(PathBuf::from("relative/project"));
        assert_eq!(
            plan(&surface),
            Err(PersistedSurfaceSpawnError::AgentResume(
                forktty_core::AgentResumeError::InvalidResumeCwd,
            )),
            "{agent:?} accepted an invalid persisted resume cwd"
        );
    }

    let session = surface.agent_session.as_mut().unwrap();
    session.resume_cwd = None;
    session.agent = AgentKind::Custom;
    assert!(matches!(
        plan(&surface),
        Err(PersistedSurfaceSpawnError::AgentResume(
            forktty_core::AgentResumeError::UnsupportedAgent(AgentKind::Custom)
        ))
    ));

    surface.kind = forktty_core::SurfaceKind::Browser {
        url: "https://example.test".to_string(),
        profile: Default::default(),
    };
    assert!(plan(&surface).unwrap().is_none());
}
