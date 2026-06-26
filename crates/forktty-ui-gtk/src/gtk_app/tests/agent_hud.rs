//! Agent HUD row ordering, loop badge, action, and styling regression tests.

use super::*;

#[test]
fn agent_hud_snapshot_prioritizes_attention_and_formats_rows() {
    let mut model = WorkspaceModel::new();
    let main = model.create_workspace("main", "/tmp/project");
    let codex_surface = main.focused_surface_id.clone();
    assert!(model.set_surface_title(&codex_surface, "Codex session".to_string()));
    assert!(model.set_surface_agent_session(
        &codex_surface,
        forktty_core::AgentKind::Codex,
        "019ebd1f-870e-7053-9765-11facbd295d2",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &codex_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));
    assert!(model.set_surface_agent_session_last_activity_ms(&codex_surface, 1_700_000_000_000,));
    // Idle Codex has produced output the user has not looked at yet.
    assert!(model.mark_surface_unread(&codex_surface, true));

    let review = model.create_workspace("review", "/tmp/review");
    let claude_surface = review.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &claude_surface,
        forktty_core::AgentKind::ClaudeCode,
        "a5643754-0a80-45bd-b591-c402dfdf16e1",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &claude_surface,
        forktty_core::AgentSessionLifecycle::NeedsInput,
    ));
    assert!(model.set_surface_agent_session_permission_mode(&claude_surface, "bypassPermissions"));
    assert!(model.set_surface_agent_session_last_activity_ms(&claude_surface, 1_700_000_120_000,));
    // The waiting agent's latest prompt notification becomes its attention
    // hint; prompt notifications on non-waiting agents must not surface.
    model.create_notification(
        "Claude needs input",
        "An older request",
        NotificationKind::Prompt,
        Some(review.id.clone()),
        Some(claude_surface.clone()),
    );
    model.create_notification(
        "Claude needs input",
        "Claude needs your permission to use Bash",
        NotificationKind::Prompt,
        Some(review.id.clone()),
        Some(claude_surface.clone()),
    );
    model.create_notification(
        "Codex prompt",
        "Stale codex prompt",
        NotificationKind::Prompt,
        None,
        Some(codex_surface.clone()),
    );

    let rows = agent_hud_rows(&model, 1_700_000_300_000);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].agent_label, "Claude");
    assert_eq!(rows[0].workspace_name, "review");
    assert!(rows[0].current);
    assert_eq!(rows[0].section_label, "Needs you");
    assert_eq!(rows[0].lifecycle_label, "Needs input");
    assert!(rows[0].needs_input);
    assert_eq!(
        rows[0].permission_mode.as_deref(),
        Some("bypassPermissions")
    );
    assert!(rows[0].permission_mode_risky);
    assert_eq!(rows[0].session_short, "a5643754");
    assert_eq!(rows[0].last_activity_label, "3m ago");
    assert!(rows[0].can_resume);
    assert_eq!(
        rows[0].attention_hint.as_deref(),
        Some("Claude needs your permission to use Bash")
    );

    assert_eq!(rows[1].agent_label, "Codex");
    assert!(!rows[1].current);
    assert_eq!(rows[1].section_label, "Idle");
    assert_eq!(rows[1].surface_title, "Codex session");
    assert_eq!(rows[1].lifecycle_label, "Idle");
    assert_eq!(rows[1].last_activity_label, "5m ago");
    // Idle: the stale prompt notification must not become a hint.
    assert_eq!(rows[1].attention_hint, None);
    // The unread output flag rides through to the row.
    assert!(rows[1].unread);
}

#[test]
fn agent_hud_ended_rows_are_compact_done_rows() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "019ebd1f-870e-7053-9765-11facbd295d2",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Ended,
    ));

    let rows = agent_hud_rows(&model, 1_700_000_300_000);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].section_label, "Done");
    assert_eq!(rows[0].lifecycle_label, "Done");
    assert!(rows[0].compact);
    assert!(rows[0].can_resume);
}

#[test]
fn agent_hud_risky_permission_modes_use_pills_outside_needs_input() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::ClaudeCode,
        "a5643754-0a80-45bd-b591-c402dfdf16e1",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Running,
    ));
    assert!(model.set_surface_agent_session_permission_mode(&surface_id, "bypassPermissions"));

    let rows = agent_hud_rows(&model, 1_700_000_300_000);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].lifecycle_label, "Working");
    assert!(!rows[0].needs_input);
    assert!(rows[0].permission_mode_risky);
    assert_eq!(
        agent_permission_pill_label(&rows[0]).as_deref(),
        Some("Bypass")
    );
    assert!(!agent_meta_line(&rows[0]).contains("mode bypassPermissions"));
}

#[test]
fn agent_hud_safe_permission_modes_remain_meta_outside_needs_input() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::ClaudeCode,
        "a5643754-0a80-45bd-b591-c402dfdf16e1",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Running,
    ));
    assert!(model.set_surface_agent_session_permission_mode(&surface_id, "dontAsk"));

    let rows = agent_hud_rows(&model, 1_700_000_300_000);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].lifecycle_label, "Working");
    assert!(!rows[0].permission_mode_risky);
    assert_eq!(agent_permission_pill_label(&rows[0]), None);
    assert!(agent_meta_line(&rows[0]).contains("mode dontAsk"));
}

#[test]
fn agent_hud_row_carries_loop_badge_for_matching_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        "019ebd1f-870e-7053-9765-11facbd295d2",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &surface_id,
        forktty_core::AgentSessionLifecycle::Running,
    ));
    let workflow = forktty_core::WorkflowState {
        id: "loop-1".to_string(),
        workspace_id: Some(workspace.id.clone()),
        surface_id: Some(surface_id.clone()),
        agent: Some("codex".to_string()),
        session_id: Some("019ebd1f-870e-7053-9765-11facbd295d2".to_string()),
        mode: "closed-loop".to_string(),
        status: "running".to_string(),
        goal: None,
        memory: None,
        loop_recipe: Some("review-fix-verify".to_string()),
        loop_stage: Some("verify".to_string()),
        loop_iteration: Some(2),
        loop_max_iterations: Some(3),
        loop_stop_reason: None,
        loop_updated_at_ms: Some(1_700_000_250_000),
        loop_gates: vec![
            forktty_core::WorkflowLoopGate {
                id: "test".to_string(),
                kind: "command".to_string(),
                label: "Tests".to_string(),
                status: "passed".to_string(),
                summary: None,
                updated_at_ms: 1_700_000_240_000,
            },
            forktty_core::WorkflowLoopGate {
                id: "review".to_string(),
                kind: "review".to_string(),
                label: "Review".to_string(),
                status: "running".to_string(),
                summary: None,
                updated_at_ms: 1_700_000_245_000,
            },
        ],
        created_at_ms: 1_700_000_100_000,
        updated_at_ms: 1_700_000_250_000,
        plan: Vec::new(),
        evidence: Vec::new(),
    };

    let loop_badges = agent_hud_loop_badges_from_workflows(&[workflow]);
    let rows = agent_hud_rows_with_loops(&model, &loop_badges, 1_700_000_300_000);

    assert_eq!(rows.len(), 1);
    let badge = rows[0].loop_badge.as_ref().unwrap();
    assert_eq!(badge.label, "Loop verify 2/3");
    assert_eq!(badge.class_name, "running");
    assert!(badge.tooltip.contains("review-fix-verify"));
    assert!(badge.tooltip.contains("2 gates"));
    assert!(badge.tooltip.contains("1 passed"));
    assert!(badge.tooltip.contains("1 running"));
}

#[test]
fn agent_hud_loop_badges_pick_latest_workflow_per_surface() {
    let mut older = forktty_core::WorkflowState {
        id: "older".to_string(),
        workspace_id: Some("workspace-1".to_string()),
        surface_id: Some("surface-1".to_string()),
        agent: Some("codex".to_string()),
        session_id: Some("session-1".to_string()),
        mode: "closed-loop".to_string(),
        status: "running".to_string(),
        goal: None,
        memory: None,
        loop_recipe: Some("old-loop".to_string()),
        loop_stage: Some("execute".to_string()),
        loop_iteration: Some(1),
        loop_max_iterations: Some(3),
        loop_stop_reason: None,
        loop_updated_at_ms: Some(1_700_000_100_000),
        loop_gates: Vec::new(),
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_100_000,
        plan: Vec::new(),
        evidence: Vec::new(),
    };
    let mut newer = older.clone();
    newer.id = "newer".to_string();
    newer.loop_recipe = Some("new-loop".to_string());
    newer.loop_stage = Some("review".to_string());
    newer.loop_iteration = Some(2);
    newer.loop_updated_at_ms = Some(1_700_000_200_000);
    older.updated_at_ms = 1_700_000_100_000;

    let loop_badges = agent_hud_loop_badges_from_workflows(&[older, newer]);
    let badge = loop_badges.by_surface.get("surface-1").unwrap();

    assert_eq!(badge.label, "Loop review 2/3");
    assert!(badge.tooltip.contains("new-loop"));
}

#[test]
fn agent_hud_loop_badges_fall_back_to_matching_session_binding() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let surface_id = workspace.focused_surface_id.clone();
    let session_id = "019ebd1f-870e-7053-9765-11facbd295d2";
    assert!(model.set_surface_agent_session(
        &surface_id,
        forktty_core::AgentKind::Codex,
        session_id,
    ));
    let workflow = forktty_core::WorkflowState {
        id: "session-loop".to_string(),
        workspace_id: Some(workspace.id.clone()),
        surface_id: None,
        agent: Some("codex".to_string()),
        session_id: Some(session_id.to_string()),
        mode: "closed-loop".to_string(),
        status: "running".to_string(),
        goal: None,
        memory: None,
        loop_recipe: Some("review-fix-verify".to_string()),
        loop_stage: Some("execute".to_string()),
        loop_iteration: Some(1),
        loop_max_iterations: Some(3),
        loop_stop_reason: None,
        loop_updated_at_ms: Some(1_700_000_250_000),
        loop_gates: Vec::new(),
        created_at_ms: 1_700_000_100_000,
        updated_at_ms: 1_700_000_250_000,
        plan: Vec::new(),
        evidence: Vec::new(),
    };

    let loop_badges = agent_hud_loop_badges_from_workflows(&[workflow]);
    let rows = agent_hud_rows_with_loops(&model, &loop_badges, 1_700_000_300_000);

    assert_eq!(
        rows[0]
            .loop_badge
            .as_ref()
            .map(|badge| badge.label.as_str()),
        Some("Loop execute 1/3")
    );
}

#[test]
fn agent_hud_loop_badges_classify_attention_and_done_states() {
    let warning = forktty_core::WorkflowState {
        id: "warning".to_string(),
        workspace_id: Some("workspace-1".to_string()),
        surface_id: Some("surface-1".to_string()),
        agent: Some("codex".to_string()),
        session_id: Some("session-1".to_string()),
        mode: "closed-loop".to_string(),
        status: "running".to_string(),
        goal: None,
        memory: None,
        loop_recipe: Some("loop".to_string()),
        loop_stage: Some("needs_human".to_string()),
        loop_iteration: Some(1),
        loop_max_iterations: Some(3),
        loop_stop_reason: None,
        loop_updated_at_ms: Some(1_700_000_100_000),
        loop_gates: Vec::new(),
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_100_000,
        plan: Vec::new(),
        evidence: Vec::new(),
    };
    let mut done = warning.clone();
    done.id = "done".to_string();
    done.surface_id = Some("surface-2".to_string());
    done.status = "done".to_string();
    done.loop_stage = Some("verify".to_string());
    done.loop_stop_reason = Some("passed".to_string());
    done.loop_updated_at_ms = Some(1_700_000_200_000);

    let loop_badges = agent_hud_loop_badges_from_workflows(&[warning, done]);

    assert_eq!(
        loop_badges
            .by_surface
            .get("surface-1")
            .map(|badge| badge.class_name),
        Some("warning")
    );
    assert_eq!(
        loop_badges
            .by_surface
            .get("surface-2")
            .map(|badge| badge.class_name),
        Some("done")
    );
}

#[test]
fn agent_surface_for_row_index_ignores_section_headers() {
    let row_targets = vec![
        None,
        Some("surface-1".to_string()),
        None,
        Some("surface-2".to_string()),
    ];

    assert_eq!(agent_surface_for_row_index(0, &row_targets), None);
    assert_eq!(
        agent_surface_for_row_index(1, &row_targets),
        Some("surface-1")
    );
    assert_eq!(agent_surface_for_row_index(2, &row_targets), None);
    assert_eq!(
        agent_surface_for_row_index(3, &row_targets),
        Some("surface-2")
    );
    assert_eq!(agent_surface_for_row_index(-1, &row_targets), None);
    assert_eq!(agent_surface_for_row_index(4, &row_targets), None);
}

#[test]
fn agent_hud_floats_unread_within_lifecycle_group() {
    let mut model = WorkspaceModel::new();
    // Two idle agents in the same lifecycle group; only the second is unread,
    // yet "alpha" would sort first by workspace name. Unread must win.
    let seen = model.create_workspace("alpha", "/tmp/alpha");
    let seen_surface = seen.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &seen_surface,
        forktty_core::AgentKind::Codex,
        "11111111-0000-0000-0000-000000000000",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &seen_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));

    let unseen = model.create_workspace("zeta", "/tmp/zeta");
    let unseen_surface = unseen.focused_surface_id.clone();
    assert!(model.set_surface_agent_session(
        &unseen_surface,
        forktty_core::AgentKind::Codex,
        "22222222-0000-0000-0000-000000000000",
    ));
    assert!(model.set_surface_agent_session_lifecycle(
        &unseen_surface,
        forktty_core::AgentSessionLifecycle::Idle,
    ));
    assert!(model.mark_surface_unread(&unseen_surface, true));

    let rows = agent_hud_rows(&model, 1_700_000_300_000);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].workspace_name, "zeta");
    assert!(rows[0].unread);
    assert_eq!(rows[1].workspace_name, "alpha");
    assert!(!rows[1].unread);
}

#[test]
fn agent_reply_payload_preserves_significant_spaces() {
    assert_eq!(
        agent_reply_payload("  /review  "),
        Some("  /review  \r".to_string())
    );
    assert_eq!(agent_reply_payload("   "), None);
}

#[test]
fn agent_hud_tail_picks_last_nonempty_line() {
    assert_eq!(
        last_nonempty_line("running tests\n3 passed   \n\n  \n").as_deref(),
        Some("3 passed")
    );
    // Leading whitespace is kept (indentation is meaningful in output),
    // trailing whitespace is trimmed.
    assert_eq!(
        last_nonempty_line("a\n  indented tail  \n").as_deref(),
        Some("  indented tail")
    );
    assert_eq!(last_nonempty_line(""), None);
    assert_eq!(last_nonempty_line("\n   \n\t\n"), None);
}

#[test]
fn agent_hud_truncated_tail_keeps_full_tooltip() {
    let source = include_str!("../agents_panel.rs");

    assert!(source.contains(
        "if label.label() != text {\n                label.set_label(text);\n                if text.is_empty() {\n                    label.set_tooltip_text(None);\n                } else {\n                    label.set_tooltip_text(Some(text));\n                }\n            }"
    ));
    assert!(source.contains("label.set_tooltip_text(None);"));
}

#[test]
fn agent_hud_suspended_lifecycle_has_css_pill() {
    let source = include_str!("../../style.css");

    assert!(source.contains(".agent-lifecycle.suspended"));
}

#[test]
fn agent_hud_css_keeps_chips_quiet_and_tail_unboxed() {
    let source = include_str!("../../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    for selector in [
        ".agent-lifecycle {",
        ".agent-current,\n.agent-permission,\n.agent-loop {",
    ] {
        let css = block(selector);
        assert!(!css.contains("text-transform: uppercase;"));
        assert!(!css.contains("font-weight: 700;"));
    }

    let tail = block(".agent-tail {");
    assert!(!tail.contains("background:"));
    assert!(!tail.contains("border:"));
}

#[test]
fn agent_hud_css_preserves_attention_tints_on_hover_and_focus() {
    let source = include_str!("../../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(
        block(".agent-list row:hover .agent-row.needs-input {").contains("background: #1e1a17;")
    );
    assert!(block(".agent-list row:hover .agent-row.current {").contains("background: #1c1b18;"));
    assert!(
        block(".agent-list row:focus-visible .agent-row.needs-input {")
            .contains("inset 3px 0 0 alpha(#e88745")
    );
}

#[test]
fn agent_reply_placeholder_uses_ascii_ellipsis() {
    let source = include_str!("../agents_panel.rs");

    assert!(source.contains(".placeholder_text(\"Reply and press Enter...\")"));
    assert!(!source.contains("Reply and press Enter…"));
}

#[test]
fn agent_hud_scrollbar_does_not_overlay_row_actions() {
    let source = include_str!("../agents_panel.rs");

    assert!(source.contains(".overlay_scrolling(false)"));
}

#[test]
fn agent_hud_loop_badge_has_quiet_warning_css() {
    let source = include_str!("../../style.css");
    let block = |selector: &str| {
        source
            .split(selector)
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap_or_else(|| panic!("missing CSS block {selector}"))
    };

    assert!(block(".agent-current,\n.agent-permission,\n.agent-loop {")
        .contains("border-radius: 999px;"));
    assert!(!block(".agent-current,\n.agent-permission,\n.agent-loop {")
        .contains("text-transform: uppercase;"));
    assert!(source.contains(".agent-loop {\n  color: #9aa0a6;\n  background: #242424;\n}"));
    assert!(block(".agent-loop.warning {").contains("background: #2b1d1d;"));
}

#[test]
fn embedded_agent_hud_tail_generation_advances_without_content_generation() {
    assert_eq!(embedded_agent_tail_generation(None), 0);
    let known = (41, Some("old tail".to_string()));
    assert_eq!(embedded_agent_tail_generation(Some(&known)), 42);
    let maxed = (u64::MAX, Some("old tail".to_string()));
    assert_eq!(embedded_agent_tail_generation(Some(&maxed)), u64::MAX);
}

#[test]
fn agent_hud_focuses_existing_agent_surface() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (first_surface, second_surface) = {
        let mut model = model.lock().unwrap();
        let first = model.create_workspace("main", "/tmp/main");
        let first_surface = first.focused_surface_id.clone();
        let second = model.create_workspace("review", "/tmp/review");
        let second_surface = second.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &second_surface,
            forktty_core::AgentKind::ClaudeCode,
            "claude-session",
        ));
        model
            .select_workspace(WorkspaceSelector::Id(&first.id))
            .unwrap();
        (first_surface, second_surface)
    };

    assert!(open_agent_surface(&state, &second_surface, None));

    let model = model.lock().unwrap();
    assert_eq!(
        model.active_workspace().unwrap().focused_surface_id,
        second_surface
    );
    assert_ne!(
        model.active_workspace().unwrap().focused_surface_id,
        first_surface
    );
}

#[test]
fn agent_hud_forget_removes_and_can_restore_tracked_agent() {
    let model = Arc::new(Mutex::new(WorkspaceModel::new()));
    let terminal = Arc::new(forktty_terminal::HeadlessTerminalBackend::new());
    let state = SocketAppState::new(
        model.clone(),
        terminal,
        "/bin/sh",
        PathBuf::from("/tmp/forktty.sock"),
    )
    .with_notification_dispatch(false);
    let (workspace_id, surface_id) = {
        let mut model = model.lock().unwrap();
        let workspace = model.create_workspace("main", "/tmp/main");
        let workspace_id = workspace.id.clone();
        let surface_id = workspace.focused_surface_id.clone();
        assert!(model.set_surface_agent_session(
            &surface_id,
            forktty_core::AgentKind::Codex,
            "codex-session",
        ));
        model
            .set_status(&workspace_id, "agent:codex", "Codex", "Ready", None)
            .unwrap();
        model
            .set_status(
                &workspace_id,
                "agent:codex:permission",
                "Codex mode",
                "bypassPermissions",
                Some("red".to_string()),
            )
            .unwrap();
        model
            .set_progress(
                &workspace_id,
                "agent:codex:tokens",
                "Codex tokens",
                42.0,
                Some(100.0),
            )
            .unwrap();
        (workspace_id, surface_id)
    };

    let forgotten = forget_agent_surface(&state, &surface_id).expect("agent session forgotten");
    {
        let model = model.lock().unwrap();
        assert!(model.surface(&surface_id).is_some());
        assert!(agent_hud_rows(&model, 1_700_000_300_000).is_empty());
        assert!(model.list_status(&workspace_id).is_empty());
        assert!(model.list_progress(&workspace_id).is_empty());
    }

    assert!(restore_forgotten_agent_surface(
        &state,
        &surface_id,
        forgotten
    ));
    let model = model.lock().unwrap();
    let rows = agent_hud_rows(&model, 1_700_000_300_000);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_label, "Codex");
    assert_eq!(model.list_status(&workspace_id).len(), 2);
    assert!(model
        .list_status(&workspace_id)
        .iter()
        .any(|entry| entry.key == "agent:codex" && entry.value == "Ready"));
    assert_eq!(model.list_progress(&workspace_id).len(), 1);
    assert_eq!(
        model.list_progress(&workspace_id)[0].key,
        "agent:codex:tokens"
    );
}
