//! Socket spawn request restoration and validation tests.

use super::*;

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

#[serial_test::serial]
#[test]
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
