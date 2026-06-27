//! Workspace model regression tests.

use super::*;

#[test]
fn new_workspace_has_one_surface_and_leaf() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    assert_eq!(workspace.id, "workspace-1");
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 1);
    assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
}

#[test]
fn move_workspace_reorders_without_changing_active_workspace() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("one", "/tmp/one");
    let second = model.create_workspace("two", "/tmp/two");
    let third = model.create_workspace("three", "/tmp/three");

    assert_eq!(model.active_workspace_id(), Some(third.id.clone()));
    assert!(model.move_workspace(&third.id, &first.id, MovePosition::Before));
    assert_eq!(
        model
            .list_workspaces()
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        vec![third.id.as_str(), first.id.as_str(), second.id.as_str()]
    );
    assert_eq!(model.active_workspace_id(), Some(third.id.clone()));
    assert!(model.move_workspace(&third.id, &second.id, MovePosition::After));
    assert_eq!(
        model
            .list_workspaces()
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        vec![first.id.as_str(), second.id.as_str(), third.id.as_str()]
    );
    assert!(!model.move_workspace(&second.id, &first.id, MovePosition::After));
    assert_eq!(
        model
            .list_workspaces()
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        vec![first.id.as_str(), second.id.as_str(), third.id.as_str()]
    );
    assert!(!model.move_workspace(&third.id, &third.id, MovePosition::Before));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn set_listening_ports_sorts_dedupes_and_reports_change() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    assert!(model.set_listening_ports(&workspace.id, vec![8080, 3000, 3000]));
    assert_eq!(
        model.workspaces[&workspace.id].listening_ports,
        vec![3000, 8080]
    );
    // Same set in a different order is not a change.
    assert!(!model.set_listening_ports(&workspace.id, vec![8080, 3000]));
    // Unknown workspace is a no-op.
    assert!(!model.set_listening_ports("workspace-404", vec![1234]));
}

#[test]
fn set_pr_reports_change_only_on_difference() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let pr = crate::pr::PrInfo {
        number: 42,
        state: crate::pr::PrState::Open,
        url: "u".to_string(),
    };

    assert!(model.set_pr(&workspace.id, Some(pr.clone())));
    assert!(!model.set_pr(&workspace.id, Some(pr)));
    assert!(model.set_pr(&workspace.id, None));
    assert!(!model.set_pr("workspace-404", None));
}

#[test]
fn session_data_drops_runtime_sidebar_hints() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.set_listening_ports(&workspace.id, vec![8080]);
    model.set_pr(
        &workspace.id,
        Some(crate::pr::PrInfo {
            number: 42,
            state: crate::pr::PrState::Open,
            url: "https://github.com/o/r/pull/42".to_string(),
        }),
    );

    let data = model.to_session_data();

    assert!(data.workspaces[0].listening_ports.is_empty());
    assert!(data.workspaces[0].pr.is_none());
    let json = serde_json::to_string(&data).unwrap();
    assert!(!json.contains("listening_ports"));
    assert!(!json.contains("\"pr\""));
}

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

#[test]
fn split_surface_adds_second_surface_and_focuses_it() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let new_surface = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, new_surface.id);
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
    assert!(matches!(workspace.pane_tree, PaneNode::Split { .. }));
}

#[test]
fn split_surface_does_not_leak_id_when_source_is_not_a_leaf_in_workspace() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    // Detach the second surface from the pane tree but keep it in the
    // surface map — emulating a corrupted in-memory state.
    let first = workspace.focused_surface_id;
    model.workspaces.get_mut(&workspace.id).unwrap().pane_tree =
        PaneNode::single_leaf(first.clone());
    let before = model.next_surface;

    assert!(model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .is_none());
    assert_eq!(
        model.next_surface, before,
        "failed split must not advance the surface id counter"
    );
}

#[test]
fn split_surface_refuses_split_deeper_than_session_limit() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let mut newest = workspace.focused_surface_id;
    let axes = [SplitAxis::Horizontal, SplitAxis::Vertical];
    // Alternating axes deepen the tree by one Split per split: step `k`
    // creates a Split at depth `k`, valid up to MAX_SESSION_SPLIT_DEPTH.
    for step in 0..=MAX_SESSION_SPLIT_DEPTH {
        let new_surface = model.split_surface(&newest, axes[step % 2]).unwrap();
        // Every successful split must still produce a saveable session.
        crate::session::validate_session_data(&model.to_session_data()).unwrap();
        newest = new_surface.id;
    }

    // The next alternating split would create a Split at depth
    // MAX_SESSION_SPLIT_DEPTH + 1, which session validation rejects —
    // it must fail cleanly instead of breaking every autosave.
    let before = model.next_surface;
    assert!(model
        .split_surface(&newest, axes[(MAX_SESSION_SPLIT_DEPTH + 1) % 2])
        .is_none());
    assert_eq!(
        model.next_surface, before,
        "refused split must not advance the surface id counter"
    );
    crate::session::validate_session_data(&model.to_session_data()).unwrap();

    // A same-axis split of the deepest leaf inserts a sibling without
    // deepening the tree, so it is still allowed at the limit.
    assert!(model
        .split_surface(&newest, axes[MAX_SESSION_SPLIT_DEPTH % 2])
        .is_some());
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn focus_surface_rejects_surface_outside_workspace_pane_tree() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let first = workspace.focused_surface_id;
    model.workspaces.get_mut(&workspace.id).unwrap().pane_tree = PaneNode::single_leaf(first);

    assert!(!model.focus_surface(&second.id));
}

#[test]
fn repair_session_invariants_restores_missing_focus() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let first = workspace.focused_surface_id;
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        workspace.pane_tree = PaneNode::single_leaf(first.clone());
        workspace.focused_surface_id = second.id.clone();
    }

    assert!(model.repair_session_invariants());
    let repaired = model.list_workspaces().remove(0);
    assert_eq!(repaired.focused_surface_id, first);
    assert!(model.surface(&second.id).is_none());
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_advances_id_counters_for_repaired_leaves() {
    let mut model = WorkspaceModel::new();
    let workspace = Workspace {
        id: "workspace-1".to_string(),
        name: "main".to_string(),
        active: true,
        working_dir: PathBuf::from("/tmp"),
        git_branch: String::new(),
        worktree_dir: None,
        worktree_name: None,
        pane_tree: PaneNode::single_leaf("surface-1".to_string()),
        focused_surface_id: "surface-1".to_string(),
        needs_attention: false,
        listening_ports: Vec::new(),
        pr: None,
    };
    model.workspace_order.push(workspace.id.clone());
    model.workspaces.insert(workspace.id.clone(), workspace);

    assert!(model.repair_session_invariants());
    let split = model
        .split_surface("surface-1", SplitAxis::Horizontal)
        .unwrap();

    assert_eq!(split.id, "surface-2");
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn restore_session_clears_stale_workspace_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Prompt",
        "Needs input",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );
    assert!(model.list_workspaces()[0].needs_attention);
    let data = model.to_session_data();

    let mut fresh = WorkspaceModel::new();
    fresh.restore_session(data);

    // Notifications are not persisted, so the restored workspace must not
    // keep the saved attention badge.
    assert!(!fresh.list_workspaces()[0].needs_attention);
    assert!(
        !fresh
            .surface(&fresh.list_workspaces()[0].focused_surface_id)
            .unwrap()
            .unread
    );
}

#[test]
fn repeated_same_axis_splits_are_siblings() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .unwrap();

    let workspace = model.list_workspaces().remove(0);

    let PaneNode::Split {
        axis,
        children,
        sizes,
    } = workspace.pane_tree
    else {
        panic!("expected split pane tree");
    };
    assert_eq!(axis, SplitAxis::Horizontal);
    assert_eq!(children.len(), 3);
    assert_eq!(sizes.len(), 3);
    assert_eq!(workspace.focused_surface_id, third.id);
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 3);
}

#[test]
fn closing_split_surface_rebalances_sizes() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let _third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .unwrap();

    model.close_surface(&second.id).unwrap();
    let workspace = model.list_workspaces().remove(0);

    let PaneNode::Split {
        children, sizes, ..
    } = workspace.pane_tree
    else {
        panic!("expected split pane tree");
    };
    assert_eq!(children.len(), 2);
    assert_eq!(sizes, vec![0.5, 0.5]);
}

#[test]
fn closing_focused_middle_pane_focuses_adjacent_sibling() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first = workspace.focused_surface_id.clone();
    let second = model.split_surface(&first, SplitAxis::Horizontal).unwrap();
    let third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .unwrap();
    assert!(model.focus_surface(&second.id));

    model.close_surface(&second.id).unwrap();

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(
        workspace.focused_surface_id, first,
        "focus should move to the adjacent sibling, not stay on a removed pane"
    );
    assert!(model.surface(&third.id).is_some());
}

#[test]
fn closing_unfocused_pane_keeps_focus() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first = workspace.focused_surface_id.clone();
    let second = model.split_surface(&first, SplitAxis::Horizontal).unwrap();
    let third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .unwrap();
    assert!(model.focus_surface(&third.id));

    model.close_surface(&second.id).unwrap();

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(
        workspace.focused_surface_id, third.id,
        "closing a background pane must not steal focus"
    );
}

#[test]
fn root_surface_close_can_use_prepared_replacement() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/project");
    let old_surface_id = workspace.focused_surface_id.clone();
    let replacement = model
        .prepare_root_surface_replacement(&old_surface_id)
        .unwrap();

    let removed = model.close_surface_with_replacement(&old_surface_id, Some(replacement.clone()));

    assert_eq!(removed.unwrap().id, old_surface_id);
    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, replacement.id);
    assert_eq!(model.list_surfaces(Some(&workspace.id)), vec![replacement]);
}

#[test]
fn update_split_partition_ratio_persists_drag() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .unwrap();

    let left = vec![workspace.focused_surface_id.clone(), second.id.clone()];
    let right = vec![third.id.clone()];
    assert!(model.update_split_partition_ratio(&workspace.id, &left, &right, 0.8));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
        panic!("expected split pane tree");
    };
    let total: f64 = sizes.iter().sum();
    let left_sum: f64 = sizes[..2].iter().sum();
    assert!((total - 1.0).abs() < 1e-6);
    assert!((left_sum / total - 0.8).abs() < 1e-6);
    assert!((sizes[0] - sizes[1]).abs() < 1e-6);
}

#[test]
fn update_split_partition_ratio_clamps_out_of_range_input() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();

    let left = vec![workspace.focused_surface_id.clone()];
    let right = vec![second.id.clone()];
    // Caller passes a wildly out-of-range value (negative). The model must
    // clamp into the valid (0.01, 0.99) band rather than corrupt sizes.
    assert!(model.update_split_partition_ratio(&workspace.id, &left, &right, -5.0));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
        panic!("expected split pane tree");
    };
    let total: f64 = sizes.iter().sum();
    assert!((total - 1.0).abs() < 1e-6);
    assert!(sizes.iter().all(|size| size.is_finite() && *size > 0.0));
    assert!(sizes[0] < sizes[1]);
    // Validation requires every size to be strictly positive and finite.
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn update_split_partition_ratio_rejects_non_finite_input() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();

    let left = vec![workspace.focused_surface_id.clone()];
    let right = vec![second.id.clone()];
    assert!(!model.update_split_partition_ratio(&workspace.id, &left, &right, f64::NAN));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
        panic!("expected split pane tree");
    };
    assert_eq!(sizes, vec![0.5, 0.5]);
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn update_split_partition_ratio_rejects_unknown_partition() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();

    assert!(!model.update_split_partition_ratio(
        &workspace.id,
        std::slice::from_ref(&second.id),
        &["bogus".into()],
        0.5,
    ));
}

#[test]
fn can_select_and_close_workspaces() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("main", "/tmp/main");
    let second = model.create_workspace("feature", "/tmp/feature");

    assert!(model.list_workspaces()[1].active);
    model
        .select_workspace(WorkspaceSelector::Id(&first.id))
        .unwrap();
    assert!(model.list_workspaces()[0].active);

    let removed = model
        .close_workspace(WorkspaceSelector::Name(&second.name))
        .unwrap();
    assert_eq!(removed.id, second.id);
    assert_eq!(model.list_workspaces().len(), 1);
}

#[test]
fn can_rename_workspace() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp/main");

    let renamed = model
        .rename_workspace(WorkspaceSelector::Id(&workspace.id), "prod")
        .unwrap();

    assert_eq!(renamed.name, "prod");
    assert_eq!(model.list_workspaces()[0].name, "prod");
}

#[test]
fn can_close_a_split_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let new_surface = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();

    model.close_surface(&new_surface.id).unwrap();

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 1);
    assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
}

#[test]
fn can_close_surface_nested_after_unrelated_split() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let third = model
        .split_surface(&second.id, SplitAxis::Vertical)
        .unwrap();
    let fourth = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Vertical)
        .unwrap();

    let removed = model.close_surface(&third.id).unwrap();

    assert_eq!(removed.id, third.id);
    assert!(model.surface(&third.id).is_none());
    assert!(model.surface(&fourth.id).is_some());
    let workspace = model.list_workspaces().remove(0);
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 3);
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn worktree_workspace_keeps_branch_and_worktree_metadata() {
    let mut model = WorkspaceModel::new();
    let workspace =
        model.create_worktree_workspace("feature", "/tmp/feature", "feature", "feature");

    assert_eq!(workspace.git_branch, "feature");
    assert_eq!(workspace.worktree_name.as_deref(), Some("feature"));
    assert_eq!(workspace.worktree_dir, Some(PathBuf::from("/tmp/feature")));
}

#[test]
fn notification_marks_surface_unread() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Prompt",
        "Needs input",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    let surface = model.surface(&workspace.focused_surface_id).unwrap();
    assert!(surface.unread);
    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn notifications_are_capped_dropping_oldest() {
    let mut model = WorkspaceModel::new();
    let total = MAX_NOTIFICATIONS + 5;
    for index in 0..total {
        model.create_notification(
            format!("n{index}"),
            "body",
            NotificationKind::Info,
            None,
            None,
        );
    }

    let notifications = model.list_notifications();
    assert_eq!(notifications.len(), MAX_NOTIFICATIONS);
    // The five oldest were dropped; the newest is retained.
    assert_eq!(notifications.first().unwrap().title, "n5");
    assert_eq!(
        notifications.last().unwrap().title,
        format!("n{}", total - 1)
    );
}

#[test]
fn clear_notifications_resets_attention_state() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    model.clear_notifications();

    assert!(model.list_notifications().is_empty());
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn mark_notifications_read_keeps_items_and_clears_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    model.mark_notifications_read();

    let notifications = model.list_notifications();
    assert_eq!(notifications.len(), 1);
    assert!(notifications[0].read);
    assert_eq!(model.unread_notification_count(), 0);
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn dismiss_notification_keeps_attention_when_unread_target_remains() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first = model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );
    model.create_notification(
        "Prompt",
        "Still ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    assert!(model.dismiss_notification(&first.id));

    assert_eq!(model.list_notifications().len(), 1);
    assert_eq!(model.unread_notification_count(), 1);
    assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn close_surface_clears_workspace_attention_when_unread_pane_is_removed() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let split = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    model.create_notification(
        "Prompt",
        "Needs input",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(split.id.clone()),
    );
    assert!(model.list_workspaces()[0].needs_attention);

    model.close_surface(&split.id).unwrap();

    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn close_surface_preserves_workspace_only_notification_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let split = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    model.create_notification(
        "Workspace",
        "Needs input",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );

    model.close_surface(&split.id).unwrap();

    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn closed_surface_notification_does_not_revive_workspace_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let split = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    model.create_notification(
        "Prompt",
        "Needs input",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(split.id.clone()),
    );

    model.close_surface(&split.id).unwrap();
    assert!(!model.list_workspaces()[0].needs_attention);

    let focused_surface_id = model.list_workspaces()[0].focused_surface_id.clone();
    assert!(model.mark_surface_unread(&focused_surface_id, false));

    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn can_update_surface_title() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    assert!(model.set_surface_title(&workspace.focused_surface_id, "build"));

    assert_eq!(
        model.surface(&workspace.focused_surface_id).unwrap().title,
        "build"
    );
}

#[test]
fn can_set_list_and_clear_workspace_status() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    let status = model
        .set_status(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
        )
        .unwrap();

    assert_eq!(status.value, "Running");
    assert_eq!(model.list_status(&workspace.id), vec![status]);

    model
        .set_status(&workspace.id, "agent:codex", "Codex", "Ready", None)
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

    assert!(model.clear_status(&workspace.id, Some("agent:codex")));
    assert!(model.list_status(&workspace.id).is_empty());
}

#[test]
fn status_and_progress_entries_are_capped_per_workspace() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    // Each distinct key adds a new entry; posting many more keys than the
    // cap must not grow the maps without bound.
    let overflow = MAX_STATUS_ENTRIES + 10;
    for i in 0..overflow {
        model
            .set_status(&workspace.id, format!("key-{i}"), "Label", "Value", None)
            .unwrap();
        model
            .set_progress(&workspace.id, format!("key-{i}"), "Label", i as f64, None)
            .unwrap();
    }

    assert_eq!(model.list_status(&workspace.id).len(), MAX_STATUS_ENTRIES);
    assert_eq!(
        model.list_progress(&workspace.id).len(),
        MAX_PROGRESS_ENTRIES
    );
    // The oldest keys are evicted; the newest is retained.
    let statuses = model.list_status(&workspace.id);
    assert!(statuses.iter().all(|entry| entry.key != "key-0"));
    assert!(statuses
        .iter()
        .any(|entry| entry.key == format!("key-{}", overflow - 1)));
    let progress = model.list_progress(&workspace.id);
    assert!(progress.iter().all(|entry| entry.key != "key-0"));
    assert!(progress
        .iter()
        .any(|entry| entry.key == format!("key-{}", overflow - 1)));
    // Updating an existing key never grows past the cap.
    model
        .set_status(
            &workspace.id,
            format!("key-{}", overflow - 1),
            "Label",
            "Updated",
            None,
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id).len(), MAX_STATUS_ENTRIES);
    model
        .set_progress(
            &workspace.id,
            format!("key-{}", overflow - 1),
            "Label",
            1.0,
            None,
        )
        .unwrap();
    assert_eq!(
        model.list_progress(&workspace.id).len(),
        MAX_PROGRESS_ENTRIES
    );
}

#[test]
fn status_and_progress_limited_return_newest_entries() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    for i in 0..5 {
        model
            .set_status(
                &workspace.id,
                format!("status-{i}"),
                "Status",
                "Value",
                None,
            )
            .unwrap();
        model
            .set_progress(
                &workspace.id,
                format!("progress-{i}"),
                "Progress",
                i as f64,
                None,
            )
            .unwrap();
    }

    let statuses = model
        .list_status_limited(&workspace.id, 2)
        .into_iter()
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    assert_eq!(statuses, vec!["status-3", "status-4"]);

    let progress = model
        .list_progress_limited(&workspace.id, 2)
        .into_iter()
        .map(|entry| entry.key)
        .collect::<Vec<_>>();
    assert_eq!(progress, vec!["progress-3", "progress-4"]);
}

#[test]
fn ordered_status_updates_ignore_stale_hook_events() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    model
        .set_status_ordered(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(10),
        )
        .unwrap();
    model
        .set_status_ordered(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Ready",
            Some("green".to_string()),
            Some(20),
        )
        .unwrap();
    model
        .set_status_ordered(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(10),
        )
        .unwrap();

    assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

    assert!(model.clear_status_ordered(&workspace.id, Some("agent:codex"), Some(30)));
    assert!(model.list_status(&workspace.id).is_empty());

    model
        .set_status_ordered(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(20),
        )
        .unwrap();
    assert!(
        model.list_status(&workspace.id).is_empty(),
        "stale prompt-submit must not revive a status cleared by a newer session-end",
    );

    model
        .set_status_ordered(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(40),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
}

#[test]
fn hook_status_state_ignores_late_prompt_for_completed_turn() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(10),
                clock: Some("monotonic-ns".to_string()),
                turn_id: Some("prompt:one".to_string()),
            }),
        )
        .unwrap();
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Ready",
            Some("green".to_string()),
            Some(StatusHookMetadata {
                event: "stop".to_string(),
                order: Some(20),
                clock: Some("monotonic-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(30),
                clock: Some("monotonic-ns".to_string()),
                turn_id: Some("prompt:one".to_string()),
            }),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(40),
                clock: Some("monotonic-ns".to_string()),
                turn_id: Some("prompt:two".to_string()),
            }),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
}

#[test]
fn hook_status_state_briefly_guards_prompt_after_terminal_without_turn_id() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Ready",
            Some("green".to_string()),
            Some(StatusHookMetadata {
                event: "stop".to_string(),
                order: Some(20),
                clock: Some("monotonic-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(20 + HOOK_TERMINAL_PROMPT_GUARD_NS),
                clock: Some("monotonic-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(21 + HOOK_TERMINAL_PROMPT_GUARD_NS),
                clock: Some("monotonic-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
}

#[test]
fn hook_status_orders_only_compare_within_the_same_clock() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let metadata = |order: u128, clock: &str| StatusHookMetadata {
        event: "session-start".to_string(),
        order: Some(order),
        clock: Some(clock.to_string()),
        turn_id: None,
    };

    // A huge wall-clock order stored before an upgrade must not drown out
    // smaller boottime orders sent afterwards: mismatched clocks are not
    // comparable, so the incoming update is accepted.
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Ready",
            None,
            Some(metadata(1_700_000_000_000_000_000, "monotonic-ns")),
        )
        .unwrap();
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            None,
            Some(metadata(100, "boottime-ns")),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Running");

    // Once both sides use the boottime clock, stale orders drop again.
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Stale",
            None,
            Some(metadata(50, "boottime-ns")),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Running");
}

#[test]
fn hook_status_guard_applies_to_matching_boottime_clocks() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Ready",
            Some("green".to_string()),
            Some(StatusHookMetadata {
                event: "stop".to_string(),
                order: Some(20),
                clock: Some("boottime-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();
    model
        .set_status_with_hook_metadata(
            &workspace.id,
            "agent:codex",
            "Codex",
            "Running",
            Some("blue".to_string()),
            Some(StatusHookMetadata {
                event: "prompt-submit".to_string(),
                order: Some(20 + HOOK_TERMINAL_PROMPT_GUARD_NS),
                clock: Some("boottime-ns".to_string()),
                turn_id: None,
            }),
        )
        .unwrap();
    assert_eq!(model.list_status(&workspace.id)[0].value, "Ready");
}

#[test]
fn can_set_clear_progress_and_append_logs() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");

    let progress = model
        .set_progress(&workspace.id, "task", "Task", 150.0, Some(100.0))
        .unwrap();
    assert_eq!(progress.value, 100.0);
    assert_eq!(model.list_progress(&workspace.id), vec![progress]);

    let log = model
        .append_log(&workspace.id, LogLevel::Warn, "waiting for input")
        .unwrap();
    assert_eq!(log.level, LogLevel::Warn);
    assert_eq!(
        model.list_logs(&workspace.id)[0].message,
        "waiting for input"
    );

    assert!(model.clear_progress(&workspace.id, Some("task")));
    assert!(model.list_progress(&workspace.id).is_empty());
    assert!(model.clear_logs(&workspace.id));
    assert!(model.list_logs(&workspace.id).is_empty());
}

#[test]
fn restore_session_collapses_multiple_active_flags_to_active_workspace_id() {
    let mut source = WorkspaceModel::new();
    source.create_workspace("first", "/tmp/a");
    let second = source.create_workspace("second", "/tmp/b");
    let mut data = source.to_session_data();
    // Two workspaces flagged active simultaneously must be reduced to a
    // single active by restore_session, following active_workspace_id.
    for workspace in &mut data.workspaces {
        workspace.active = true;
    }
    data.active_workspace_id = Some(second.id.clone());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let actives: Vec<_> = restored
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace.active)
        .map(|workspace| workspace.id)
        .collect();
    assert_eq!(actives, vec![second.id]);
}

#[test]
fn restore_session_assigns_first_workspace_active_when_id_is_missing() {
    let mut source = WorkspaceModel::new();
    source.create_workspace("first", "/tmp/a");
    source.create_workspace("second", "/tmp/b");
    let mut data = source.to_session_data();
    for workspace in &mut data.workspaces {
        workspace.active = false;
    }
    data.active_workspace_id = None;

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let workspaces = restored.list_workspaces();
    assert!(workspaces[0].active);
    assert!(!workspaces[1].active);
}

#[test]
fn restore_session_dedups_workspace_order_on_duplicate_ids() {
    let mut source = WorkspaceModel::new();
    source.create_workspace("first", "/tmp/a");
    source.create_workspace("second", "/tmp/b");
    let mut data = source.to_session_data();
    // Force the two workspaces to share an id. `workspaces` is a map so it
    // collapses to one entry; `workspace_order` must not list it twice.
    let shared_id = data.workspaces[0].id.clone();
    data.workspaces[1].id = shared_id.clone();
    data.workspaces[1].pane_tree =
        PaneNode::single_leaf(data.workspaces[1].focused_surface_id.clone());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let workspaces = restored.list_workspaces();
    let occurrences = workspaces
        .iter()
        .filter(|workspace| workspace.id == shared_id)
        .count();
    assert_eq!(occurrences, 1);
    crate::session::validate_session_data(&restored.to_session_data()).unwrap();
}

#[test]
fn restore_session_repairs_focused_surface_id_pointing_outside_pane_tree() {
    let mut source = WorkspaceModel::new();
    let workspace = source.create_workspace("main", "/tmp");
    let mut data = source.to_session_data();
    data.workspaces[0].focused_surface_id = "surface-99".to_string();

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let restored_workspace = &restored.list_workspaces()[0];
    // The focus is normalised to a leaf that actually exists in the pane
    // tree, not the bogus id we forged.
    assert_ne!(restored_workspace.focused_surface_id, "surface-99");
    assert_eq!(
        restored_workspace.focused_surface_id,
        workspace.focused_surface_id
    );
    assert!(restored
        .surface(&restored_workspace.focused_surface_id)
        .is_some());
}

#[test]
fn restore_session_repairs_duplicate_leaf_ids() {
    let mut source = WorkspaceModel::new();
    let first = source.create_workspace("first", "/tmp/a");
    let second = source.create_workspace("second", "/tmp/b");
    let mut data = source.to_session_data();
    data.workspaces[1].pane_tree = PaneNode::single_leaf(first.focused_surface_id.clone());
    data.workspaces[1].focused_surface_id = first.focused_surface_id.clone();

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let workspaces = restored.list_workspaces();
    let first_leaves = leaf_surface_ids(&workspaces[0].pane_tree);
    let second_leaves = leaf_surface_ids(&workspaces[1].pane_tree);
    assert_eq!(first_leaves.len(), 1);
    assert_eq!(second_leaves.len(), 1);
    assert_ne!(first_leaves[0], second_leaves[0]);
    assert_eq!(
        restored.surface(&first_leaves[0]).unwrap().workspace_id,
        first.id
    );
    assert_eq!(
        restored.surface(&second_leaves[0]).unwrap().workspace_id,
        second.id
    );
    crate::session::validate_session_data(&restored.to_session_data()).unwrap();
}

#[test]
fn close_workspace_keeps_single_active_workspace_invariant() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/a");
    let second = model.create_workspace("second", "/tmp/b");
    let third = model.create_workspace("third", "/tmp/c");
    // Third is active because create_workspace always activates the new one.
    assert_eq!(
        model.active_workspace_id().as_deref(),
        Some(third.id.as_str())
    );

    model
        .close_workspace(WorkspaceSelector::Id(&third.id))
        .unwrap();

    let actives: Vec<_> = model
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace.active)
        .map(|workspace| workspace.id)
        .collect();
    assert_eq!(actives.len(), 1);
    // Falls back to the first workspace in insertion order.
    assert_eq!(actives[0], first.id);
    let _ = second;
}

#[test]
fn active_workspace_returns_active_or_first_workspace() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/a");
    let second = model.create_workspace("second", "/tmp/b");

    assert_eq!(
        model.active_workspace().map(|workspace| workspace.id),
        Some(second.id.clone())
    );

    model.workspaces.get_mut(&first.id).unwrap().active = false;
    model.workspaces.get_mut(&second.id).unwrap().active = false;

    assert_eq!(
        model.active_workspace().map(|workspace| workspace.id),
        Some(first.id)
    );
}

#[test]
fn dismissing_only_notification_for_surface_clears_workspace_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );
    assert!(model.list_workspaces()[0].needs_attention);

    assert!(model.dismiss_notification(&notification.id));

    assert!(!model.list_workspaces()[0].needs_attention);
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
}

#[test]
fn clearing_surface_unread_preserves_workspace_only_notification_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    model.create_notification(
        "Workspace",
        "Needs input",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );

    assert!(model.mark_surface_unread(&workspace.focused_surface_id, false));
    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn workspace_notification_marks_workspace_attention_without_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let _ = model.create_notification(
        "Workspace",
        "Needs input",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );

    assert!(model.list_workspaces()[0].needs_attention);
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);

    model.mark_notifications_read();

    assert!(!model.list_workspaces()[0].needs_attention);

    let notification = model.create_notification(
        "Workspace",
        "Needs input again",
        NotificationKind::Info,
        Some(workspace.id.clone()),
        None,
    );
    assert!(model.list_workspaces()[0].needs_attention);

    assert!(model.dismiss_notification(&notification.id));

    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn workspace_id_for_resolves_each_selector_variant() {
    let mut model = WorkspaceModel::new();
    let main = model.create_workspace("main", "/tmp/main");
    let feature =
        model.create_worktree_workspace("feature", "/tmp/feature", "feature", "feature-wt");

    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::Id(&main.id)),
        Some(main.id.clone())
    );
    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::Name("feature")),
        Some(feature.id.clone())
    );
    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::WorktreeName("feature-wt")),
        Some(feature.id)
    );
    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::Id("workspace-missing")),
        None
    );
    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::Name("missing")),
        None
    );
    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::WorktreeName("missing")),
        None
    );
}

#[test]
fn workspace_id_for_prefers_first_workspace_in_list_order_for_duplicate_names() {
    let mut model = WorkspaceModel::new();
    model.create_workspace("seed", "/tmp/seed");
    let first_dup = model.create_workspace("dup", "/tmp/dup-1");
    for idx in 3..=9 {
        model.create_workspace(format!("w{idx}"), format!("/tmp/w{idx}"));
    }
    let second_dup = model.create_workspace("dup", "/tmp/dup-2");

    assert_eq!(
        model.workspace_id_for(WorkspaceSelector::Name("dup")),
        Some(first_dup.id),
    );
    assert_ne!(
        model.workspace_id_for(WorkspaceSelector::Name("dup")),
        Some(second_dup.id),
    );
}

#[test]
fn auto_named_workspace_name_matches_allocated_id_after_closed_gaps() {
    let mut model = WorkspaceModel::new();
    let first = model.create_auto_named_workspace("/tmp/one");
    let second = model.create_auto_named_workspace("/tmp/two");
    let third = model.create_auto_named_workspace("/tmp/three");

    assert_eq!(first.name, first.id);
    assert_eq!(second.name, second.id);
    assert_eq!(third.name, third.id);

    model.close_workspace(WorkspaceSelector::Id(&first.id));
    model.close_workspace(WorkspaceSelector::Id(&second.id));

    let fourth = model.create_auto_named_workspace("/tmp/four");
    assert_eq!(fourth.id, "workspace-4");
    assert_eq!(fourth.name, "workspace-4");

    let ssh = model.create_auto_named_ssh_workspace("/tmp/ssh", "server.local".to_string());
    assert_eq!(ssh.id, "workspace-5");
    assert_eq!(ssh.name, "workspace-5");
}

#[test]
fn next_surface_id_skips_collisions_with_non_numeric_ids() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    // Inject a surface keyed with the next monotonic id we would
    // otherwise hand out, emulating a restore from a session that
    // pre-allocated that name through some external route.
    let blocker_id = format!("surface-{}", model.next_surface + 1);
    model.surfaces.insert(
        blocker_id.clone(),
        Surface {
            id: blocker_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: PathBuf::from("/tmp"),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        },
    );

    let new_surface = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    assert_ne!(new_surface.id, blocker_id);
}

#[test]
fn next_ids_wrap_after_restored_max_numeric_suffixes() {
    let max = u64::MAX;
    let workspace_id = format!("workspace-{max}");
    let surface_id = format!("surface-{max}");
    let mut model = WorkspaceModel::new();
    model.restore_session(SessionData {
        version: SESSION_FORMAT_VERSION,
        workspaces: vec![Workspace {
            id: workspace_id,
            name: String::from("max ids"),
            active: true,
            working_dir: PathBuf::from("/tmp"),
            git_branch: String::new(),
            worktree_dir: None,
            worktree_name: None,
            pane_tree: PaneNode::single_leaf(surface_id.clone()),
            focused_surface_id: surface_id.clone(),
            needs_attention: false,
            listening_ports: Vec::new(),
            pr: None,
        }],
        active_workspace_id: None,
        surfaces: Vec::new(),
    });

    let tab = model.add_tab(&surface_id).unwrap();
    assert_eq!(tab.id, "surface-1");

    let workspace = model.create_workspace("fresh", "/tmp/fresh");
    assert_eq!(workspace.id, "workspace-1");
}

#[test]
fn repair_session_invariants_collapses_single_child_splits() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let leaf_id = workspace.focused_surface_id.clone();
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        workspace.pane_tree = PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: vec![PaneNode::single_leaf(leaf_id.clone())],
            sizes: vec![1.0],
        };
    }

    assert!(model.repair_session_invariants());

    let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
    assert!(matches!(
        repaired.pane_tree,
        PaneNode::Leaf { ref tabs, .. } if tabs.len() == 1 && tabs[0] == leaf_id
    ));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_rebalances_non_finite_split_sizes() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let split = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        if let PaneNode::Split { sizes, .. } = &mut workspace.pane_tree {
            sizes[0] = f64::NAN;
            sizes[1] = -1.0;
        } else {
            panic!("expected split pane tree");
        }
    }

    assert!(model.repair_session_invariants());

    let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
    match repaired.pane_tree {
        PaneNode::Split { sizes, .. } => {
            assert!(sizes.iter().all(|s| s.is_finite() && *s > 0.0));
        }
        _ => panic!("expected split"),
    }
    // Ensure the split surface is still reachable.
    assert!(model.surface(&split.id).is_some());
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_renames_duplicate_leaf_ids_across_workspaces() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/a");
    let second = model.create_workspace("second", "/tmp/b");
    // Force both workspaces to reference the same leaf surface id.
    let shared_id = first.focused_surface_id.clone();
    {
        let workspace = model.workspaces.get_mut(&second.id).unwrap();
        workspace.pane_tree = PaneNode::single_leaf(shared_id.clone());
        workspace.focused_surface_id = shared_id.clone();
    }
    model.surfaces.insert(
        shared_id.clone(),
        Surface {
            id: shared_id.clone(),
            workspace_id: second.id.clone(),
            cwd: PathBuf::from("/tmp/b"),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        },
    );

    assert!(model.repair_session_invariants());

    let first_after = model.workspaces.get(&first.id).unwrap().clone();
    let second_after = model.workspaces.get(&second.id).unwrap().clone();
    let first_leaves = leaf_surface_ids(&first_after.pane_tree);
    let second_leaves = leaf_surface_ids(&second_after.pane_tree);
    assert_eq!(first_leaves.len(), 1);
    assert_eq!(second_leaves.len(), 1);
    assert_ne!(
        first_leaves[0], second_leaves[0],
        "duplicate leaf ids must be split between workspaces"
    );
    assert_eq!(first_after.focused_surface_id, first_leaves[0]);
    assert_eq!(second_after.focused_surface_id, second_leaves[0]);
    // Both surfaces should now resolve back to their owning workspace.
    assert_eq!(
        model.surface(&first_leaves[0]).unwrap().workspace_id,
        first.id
    );
    assert_eq!(
        model.surface(&second_leaves[0]).unwrap().workspace_id,
        second.id
    );
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_renames_duplicate_leaf_ids_within_workspace() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let shared_id = workspace.focused_surface_id.clone();
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        workspace.pane_tree = PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: vec![
                PaneNode::single_leaf(shared_id.clone()),
                PaneNode::single_leaf(shared_id.clone()),
            ],
            sizes: vec![0.5, 0.5],
        };
    }

    assert!(model.repair_session_invariants());

    let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
    let leaves = leaf_surface_ids(&repaired.pane_tree);
    assert_eq!(leaves.len(), 2);
    assert_ne!(
        leaves[0], leaves[1],
        "duplicate leaf ids must be split within a workspace"
    );
    assert_eq!(repaired.focused_surface_id, leaves[0]);
    assert_eq!(
        model.surface(&leaves[0]).unwrap().workspace_id,
        workspace.id
    );
    assert_eq!(
        model.surface(&leaves[1]).unwrap().workspace_id,
        workspace.id
    );
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_replaces_leafless_pane_tree() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let old_surface_id = workspace.focused_surface_id.clone();
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        workspace.pane_tree = PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: Vec::new(),
            sizes: Vec::new(),
        };
        workspace.focused_surface_id = "missing-surface".to_string();
    }

    assert!(model.repair_session_invariants());

    let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
    let leaves = leaf_surface_ids(&repaired.pane_tree);
    assert_eq!(leaves.len(), 1);
    assert_eq!(repaired.focused_surface_id, leaves[0]);
    assert_ne!(leaves[0], old_surface_id);
    assert!(model.surface(&old_surface_id).is_none());
    assert_eq!(
        model.surface(&leaves[0]).unwrap().workspace_id,
        workspace.id
    );
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_prunes_nested_leafless_splits() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let leaf_id = workspace.focused_surface_id.clone();
    {
        let workspace = model.workspaces.get_mut(&workspace.id).unwrap();
        workspace.pane_tree = PaneNode::Split {
            axis: SplitAxis::Horizontal,
            children: vec![
                PaneNode::Split {
                    axis: SplitAxis::Vertical,
                    children: Vec::new(),
                    sizes: Vec::new(),
                },
                PaneNode::single_leaf(leaf_id.clone()),
            ],
            sizes: vec![0.5, 0.5],
        };
    }

    assert!(model.repair_session_invariants());

    let repaired = model.workspaces.get(&workspace.id).unwrap().clone();
    assert!(matches!(
        repaired.pane_tree,
        PaneNode::Leaf { ref tabs, .. } if tabs.len() == 1 && tabs[0] == leaf_id
    ));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn repair_session_invariants_drops_orphan_surfaces() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    // Inject an orphan surface that no pane leaf references.
    let orphan_id = "surface-orphan".to_string();
    model.surfaces.insert(
        orphan_id.clone(),
        Surface {
            id: orphan_id.clone(),
            workspace_id: workspace.id.clone(),
            cwd: PathBuf::from("/tmp"),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        },
    );
    // Also inject a surface tied to a no-longer-present workspace.
    let dangling_id = "surface-dangling".to_string();
    model.surfaces.insert(
        dangling_id.clone(),
        Surface {
            id: dangling_id.clone(),
            workspace_id: "workspace-missing".to_string(),
            cwd: PathBuf::from("/tmp"),
            title: String::from("shell"),
            unread: false,
            needs_attention: false,
            kind: SurfaceKind::Terminal,
            agent_session: None,
            persisted_scrollback: None,
        },
    );

    assert!(model.repair_session_invariants());

    assert!(model.surface(&orphan_id).is_none());
    assert!(model.surface(&dangling_id).is_none());
    // The original workspace surface must remain reachable.
    assert!(model.surface(&workspace.focused_surface_id).is_some());
}

#[test]
fn can_restore_model_from_session_data() {
    let mut source = WorkspaceModel::new();
    let workspace = source.create_workspace("main", "/tmp");
    let split = source
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let mut session = source.to_session_data();
    session.active_workspace_id = Some("missing-workspace".to_string());

    let mut restored = WorkspaceModel::new();
    restored.restore_session(session);

    assert_eq!(restored.list_workspaces().len(), 1);
    assert!(restored.list_workspaces()[0].active);
    assert_eq!(restored.list_surfaces(None).len(), 2);
    assert!(restored.surface(&split.id).is_some());
    assert_eq!(restored.to_session_data().workspaces[0].name, "main");
}

#[test]
fn surface_without_kind_field_deserializes_as_terminal() {
    // Sessions persisted before SurfaceKind existed have no `kind` key.
    let json = r#"{
        "id": "s1",
        "workspace_id": "w1",
        "cwd": "/tmp",
        "title": "shell",
        "unread": false,
        "needs_attention": false
    }"#;
    let surface: Surface = serde_json::from_str(json).unwrap();
    assert_eq!(surface.kind, SurfaceKind::Terminal);
}

#[test]
fn open_browser_adds_browser_surface_splits_and_focuses() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let first = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();

    let browser = model
        .open_browser(
            &ws.id,
            "https://example.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .expect("browser surface created");

    assert_eq!(
        browser.kind,
        SurfaceKind::Browser {
            url: "https://example.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
    assert_eq!(browser.title, "example.com");
    assert_eq!(model.workspaces[&ws.id].focused_surface_id, browser.id);
    let leaves = leaf_surface_ids(&model.workspaces[&ws.id].pane_tree);
    assert!(leaves.contains(&first));
    assert!(leaves.contains(&browser.id));
}

#[test]
fn open_browser_records_the_requested_profile() {
    use crate::profile::ProfileId;
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));

    let custom = ProfileId::new();
    let surface = model
        .open_browser(&ws.id, "https://example.com", custom, SplitAxis::Horizontal)
        .expect("opens");
    match surface.kind {
        SurfaceKind::Browser { url, profile } => {
            assert_eq!(url, "https://example.com");
            assert_eq!(profile, custom);
        }
        _ => panic!("expected a browser surface"),
    }
}

#[test]
fn legacy_browser_surface_without_profile_loads_as_default() {
    use crate::profile::ProfileId;
    let json = r#"{"type":"browser","url":"https://example.com"}"#;
    let kind: SurfaceKind = serde_json::from_str(json).unwrap();
    match kind {
        SurfaceKind::Browser { url, profile } => {
            assert_eq!(url, "https://example.com");
            assert_eq!(profile, ProfileId::default());
        }
        _ => panic!("expected browser"),
    }
}

#[test]
fn set_surface_url_updates_only_browser_surfaces() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let terminal = first_leaf_surface_id(&model.workspaces[&ws.id].pane_tree).unwrap();
    let browser = model
        .open_browser(
            &ws.id,
            "https://a.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();

    assert!(model.set_surface_url(&browser.id, "https://b.com"));
    assert!(model.set_surface_url(&browser.id, "https://b.com"));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser {
            url: "https://b.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
    // title also refreshes
    assert_eq!(model.surface(&browser.id).unwrap().title, "b.com");
    // terminal + missing rejected
    assert!(!model.set_surface_url(&terminal, "https://b.com"));
    assert!(!model.set_surface_url("nope", "https://b.com"));
}

#[test]
fn set_surface_url_rejects_overlong_browser_urls() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let browser = model
        .open_browser(
            &ws.id,
            "https://a.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();
    let overlong = format!("https://{}", "a".repeat(MAX_BROWSER_URL_BYTES));

    assert!(!model.set_surface_url(&browser.id, &overlong));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser {
            url: "https://a.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
}

#[test]
fn set_surface_url_preserves_committed_non_hierarchical_urls() {
    let mut model = WorkspaceModel::default();
    let ws = model.create_workspace("w", PathBuf::from("/tmp"));
    let browser = model
        .open_browser(
            &ws.id,
            "https://a.com",
            crate::profile::ProfileId::default(),
            SplitAxis::Horizontal,
        )
        .unwrap();

    assert!(model.set_surface_url(&browser.id, "about:blank"));
    assert_eq!(
        model.surface(&browser.id).unwrap().kind,
        SurfaceKind::Browser {
            url: "about:blank".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
    assert_eq!(model.surface(&browser.id).unwrap().title, "browser");
}

#[test]
fn browser_url_validation_applies_default_scheme_before_limit() {
    let fits = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len());
    assert_eq!(
        validated_browser_url(&fits),
        Some(format!("https://{fits}"))
    );

    let overlong = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len() + 1);
    assert_eq!(validated_browser_url(&overlong), None);
}

#[test]
fn browser_title_for_extracts_host_and_falls_back() {
    assert_eq!(browser_title_for("https://example.com"), "example.com");
    assert_eq!(
        browser_title_for("https://example.com/path?q=1#frag"),
        "example.com"
    );
    assert_eq!(
        browser_title_for("http://user:pass@example.com/"),
        "example.com"
    );
    assert_eq!(browser_title_for("about:blank"), "browser");
    assert_eq!(browser_title_for("data:text/html,hi"), "browser");
    assert_eq!(browser_title_for("https://"), "browser");
}

#[test]
fn has_uri_scheme_detects_only_leading_scheme() {
    assert!(!has_uri_scheme("example.com"));
    assert!(has_uri_scheme("https://x"));
    // A `://` inside the query must not be mistaken for a scheme.
    assert!(!has_uri_scheme("example.com/?next=https://x"));
    assert!(has_uri_scheme("ftp://h"));
    assert!(has_uri_scheme("custom+scheme.1-2://h"));
    // Empty scheme, non-alpha leading char, and no `://` are all rejected.
    assert!(!has_uri_scheme("://x"));
    assert!(!has_uri_scheme("1http://x"));
    assert!(!has_uri_scheme("noscheme"));
}

#[test]
fn normalize_browser_url_trims_and_defaults_to_https() {
    assert_eq!(
        normalize_browser_url(" example.com/path "),
        Some("https://example.com/path".to_string())
    );
    assert_eq!(
        normalize_browser_url("https://example.com"),
        Some("https://example.com".to_string())
    );
    assert_eq!(
        normalize_browser_url("custom+scheme.1-2://host"),
        Some("custom+scheme.1-2://host".to_string())
    );
    assert_eq!(normalize_browser_url(" \t\n "), None);
}

#[test]
fn create_ssh_workspace_produces_ssh_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());

    assert_eq!(workspace.name, "remote");
    let surfaces = model.list_surfaces(Some(&workspace.id));
    assert_eq!(surfaces.len(), 1);
    let surface = &surfaces[0];
    assert_eq!(
        surface.kind,
        SurfaceKind::Ssh {
            host: "user@example.com".to_string()
        }
    );
    assert_eq!(surface.title, "ssh:user@example.com");
}

#[test]
fn open_ssh_splits_into_ssh_surface() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let new_surface = model
        .open_ssh(
            &workspace.id,
            "server.local".to_string(),
            SplitAxis::Horizontal,
        )
        .expect("open_ssh succeeds");

    assert_eq!(
        new_surface.kind,
        SurfaceKind::Ssh {
            host: "server.local".to_string()
        }
    );
    assert_eq!(new_surface.title, "ssh:server.local");
    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, new_surface.id);
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
}

#[test]
fn ssh_workspace_survives_session_round_trip() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_ssh_workspace("remote", "/tmp", "user@example.com".to_string());
    let ssh_id = workspace.focused_surface_id.clone();

    let data = model.to_session_data();
    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let surface = restored.surface(&ssh_id).expect("ssh surface restored");
    assert_eq!(
        surface.kind,
        SurfaceKind::Ssh {
            host: "user@example.com".to_string()
        }
    );
}

#[test]
fn restore_session_preserves_browser_surface_kind() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("ws", "/tmp");
    model.open_browser(
        &workspace.id,
        "https://example.com",
        crate::profile::ProfileId::default(),
        SplitAxis::Vertical,
    );
    let browser_id = model
        .list_surfaces(Some(&workspace.id))
        .into_iter()
        .find(|s| matches!(s.kind, SurfaceKind::Browser { .. }))
        .map(|s| s.id)
        .expect("browser surface present");

    let data = model.to_session_data();
    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);

    let surface = restored.surface(&browser_id).expect("surface restored");
    assert_eq!(
        surface.kind,
        SurfaceKind::Browser {
            url: "https://example.com".to_string(),
            profile: crate::profile::ProfileId::default(),
        }
    );
}

// ── per-pane tab group tests ───────────────────────────────────────────────

#[test]
fn add_tab_creates_new_surface_in_same_leaf_and_focuses_it() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();

    let new_surface = model.add_tab(&first_id).expect("add_tab succeeds");

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, new_surface.id);
    // Both surfaces exist.
    assert!(model.surface(&first_id).is_some());
    assert!(model.surface(&new_surface.id).is_some());
    // The pane tree is still a single leaf with 2 tabs.
    let tabs = workspace.pane_tree.leaf_tabs().expect("root is leaf");
    assert_eq!(tabs.len(), 2);
    assert!(tabs.contains(&first_id));
    assert!(tabs.contains(&new_surface.id));
    // Active points to the newly added tab.
    assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&new_surface.id));
    // Surface count should be 2.
    assert_eq!(model.list_surfaces(Some(&workspace.id)).len(), 2);
}

#[test]
fn select_tab_switches_active_and_focus() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    // Now tab2 is active. Switch back to first.
    assert!(model.select_tab(&first_id));

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, first_id);
    assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&first_id));
    // tab2 is still present.
    assert!(model.surface(&tab2.id).is_some());
}

#[test]
fn move_tab_reorders_within_leaf_and_preserves_active_tab() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    let tab3 = model.add_tab(&first_id).expect("add_tab");
    assert!(model.select_tab(&tab2.id));

    assert!(model.move_tab(&tab3.id, &first_id, MovePosition::Before));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Leaf { tabs, active } = workspace.pane_tree else {
        panic!("expected a tab leaf");
    };
    assert_eq!(
        tabs,
        vec![tab3.id.clone(), first_id.clone(), tab2.id.clone()]
    );
    assert_eq!(tabs[active], tab2.id);
    assert_eq!(workspace.focused_surface_id, tab2.id);
    assert!(!model.move_tab(&first_id, &tab3.id, MovePosition::After));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn move_tab_between_panes_when_source_leaf_keeps_a_tab() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    let second = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    assert!(model.select_tab(&tab2.id));

    assert!(model.move_tab(&tab2.id, &second.id, MovePosition::After));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(
        children[0].leaf_tabs().unwrap(),
        std::slice::from_ref(&first_id)
    );
    assert_eq!(
        children[1].leaf_tabs().unwrap(),
        &[second.id.clone(), tab2.id.clone()]
    );
    assert_eq!(children[1].leaf_active_id(), Some(&tab2.id));
    assert_eq!(workspace.focused_surface_id, tab2.id);
    assert!(!model.move_tab(&first_id, &second.id, MovePosition::Before));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn move_inactive_tab_between_panes_preserves_focused_tab() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    let second = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    assert!(model.select_tab(&first_id));

    assert!(model.move_tab(&tab2.id, &second.id, MovePosition::Before));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(
        children[0].leaf_tabs().unwrap(),
        std::slice::from_ref(&first_id)
    );
    assert_eq!(
        children[1].leaf_tabs().unwrap(),
        &[tab2.id.clone(), second.id.clone()]
    );
    assert_eq!(children[1].leaf_active_id(), Some(&second.id));
    assert_eq!(workspace.focused_surface_id, first_id);
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn move_tab_rejects_cross_workspace_missing_and_last_tab_sources() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    let second = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    let other_workspace_surface_id = model
        .create_workspace("other", "/tmp/other")
        .focused_surface_id;

    assert!(!model.move_tab(&tab2.id, &other_workspace_surface_id, MovePosition::After));
    assert!(!model.move_tab("missing-surface", &second.id, MovePosition::After));
    assert!(!model.move_tab(&tab2.id, "missing-surface", MovePosition::After));
    assert!(!model.move_tab(&second.id, &first_id, MovePosition::Before));

    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .expect("workspace still present");
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(
        children[0].leaf_tabs().unwrap(),
        &[first_id.clone(), tab2.id.clone()]
    );
    assert_eq!(
        children[1].leaf_tabs().unwrap(),
        std::slice::from_ref(&second.id)
    );
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn swap_panes_exchanges_leaf_positions_without_changing_focus() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let second = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    let third = model
        .split_surface(&second.id, SplitAxis::Horizontal)
        .expect("split succeeds");
    assert!(model.focus_surface(&second.id));

    assert!(model.swap_panes(&first_id, &third.id));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(children[0].leaf_active_id(), Some(&third.id));
    assert_eq!(children[2].leaf_active_id(), Some(&first_id));
    assert_eq!(workspace.focused_surface_id, second.id);
    assert!(!model.swap_panes(&second.id, &second.id));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn swap_panes_rejects_cross_workspace_and_missing_surfaces() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let workspace_id = workspace.id.clone();
    let first_id = workspace.focused_surface_id.clone();
    let second = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    let other_workspace_surface_id = model
        .create_workspace("other", "/tmp/other")
        .focused_surface_id;

    assert!(!model.swap_panes(&first_id, "missing-surface"));
    assert!(!model.swap_panes("missing-surface", &first_id));
    assert!(!model.swap_panes(&first_id, &other_workspace_surface_id));

    let workspace = model
        .list_workspaces()
        .into_iter()
        .find(|workspace| workspace.id == workspace_id)
        .expect("workspace still present");
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(children[0].leaf_active_id(), Some(&first_id));
    assert_eq!(children[1].leaf_active_id(), Some(&second.id));
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn close_tab_non_last_removes_only_from_leaf_without_collapse() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");

    // Close the second tab — the leaf must remain (still has first_id).
    let removed = model.close_surface(&tab2.id).expect("close succeeds");
    assert_eq!(removed.id, tab2.id);

    let workspace = model.list_workspaces().remove(0);
    // The pane tree must still be a leaf.
    assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
    // The leaf must now have exactly one tab.
    let tabs = workspace.pane_tree.leaf_tabs().expect("leaf");
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0], first_id);
    // Focus reverted to the remaining tab.
    assert_eq!(workspace.focused_surface_id, first_id);
    // tab2 is gone from the surfaces map.
    assert!(model.surface(&tab2.id).is_none());
}

#[test]
fn close_focused_tab_in_later_pane_keeps_focus_in_that_pane() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let split_surface = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    let second_tab = model.add_tab(&split_surface.id).expect("add_tab");

    let removed = model.close_surface(&second_tab.id).expect("close succeeds");
    assert_eq!(removed.id, second_tab.id);

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, split_surface.id);
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(children[1].leaf_active_id(), Some(&split_surface.id));
}

#[test]
fn close_background_pane_tab_preserves_current_focus() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let focused_pane = model
        .split_surface(&first_id, SplitAxis::Horizontal)
        .expect("split succeeds");
    let background_tab = model.add_tab(&first_id).expect("add_tab");
    assert!(model.focus_surface(&focused_pane.id));

    let removed = model
        .close_surface(&background_tab.id)
        .expect("close succeeds");
    assert_eq!(removed.id, background_tab.id);

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, focused_pane.id);
    let PaneNode::Split { children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(children[0].leaf_active_id(), Some(&first_id));
}

#[test]
fn close_last_tab_collapses_leaf_and_replaces_with_new_terminal() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let only_id = workspace.focused_surface_id.clone();

    let removed = model.close_surface(&only_id).expect("close succeeds");
    assert_eq!(removed.id, only_id);

    let workspace = model.list_workspaces().remove(0);
    // A replacement leaf was created.
    assert!(matches!(workspace.pane_tree, PaneNode::Leaf { .. }));
    let new_id = workspace.focused_surface_id.clone();
    assert_ne!(new_id, only_id);
    assert!(model.surface(&new_id).is_some());
    assert!(model.surface(&only_id).is_none());
}

#[test]
fn split_surface_on_multi_tab_leaf_preserves_all_tabs_in_original_pane() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let tab2 = model.add_tab(&first_id).expect("add_tab");

    // Now split the pane (the active tab is tab2).
    let new_split = model
        .split_surface(&tab2.id, SplitAxis::Horizontal)
        .expect("split succeeds");

    let workspace = model.list_workspaces().remove(0);
    // The pane tree must now be a split with 2 leaves.
    let PaneNode::Split { ref children, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    assert_eq!(children.len(), 2);
    // The first child keeps BOTH original tabs.
    let orig_tabs = children[0].leaf_tabs().expect("first child is leaf");
    assert_eq!(orig_tabs.len(), 2);
    assert!(orig_tabs.contains(&first_id));
    assert!(orig_tabs.contains(&tab2.id));
    // The second child is the new split surface.
    let new_tabs = children[1].leaf_tabs().expect("second child is leaf");
    assert_eq!(new_tabs, std::slice::from_ref(&new_split.id));
    // Focus is on the new split surface.
    assert_eq!(workspace.focused_surface_id, new_split.id);
}

#[test]
fn update_split_partition_ratio_works_with_multi_tab_leaf() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    // Add a second tab to the initial leaf.
    let tab2 = model.add_tab(&first_id).expect("add_tab");
    // Split on tab2 to get two panes.
    let right_pane = model
        .split_surface(&tab2.id, SplitAxis::Horizontal)
        .expect("split");

    // The left leaf now has [first_id, tab2.id]; right leaf has [right_pane.id].
    let left_leaves = vec![first_id.clone(), tab2.id.clone()];
    let right_leaves = vec![right_pane.id.clone()];
    assert!(model.update_split_partition_ratio(&workspace.id, &left_leaves, &right_leaves, 0.7,));

    let workspace = model.list_workspaces().remove(0);
    let PaneNode::Split { sizes, .. } = workspace.pane_tree else {
        panic!("expected split");
    };
    let total: f64 = sizes.iter().sum();
    assert!((total - 1.0).abs() < 1e-6);
    assert!((sizes[0] / total - 0.7).abs() < 1e-6);
}

#[test]
fn focus_surface_on_non_active_tab_updates_leaf_active() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    let _tab2 = model.add_tab(&first_id).expect("add_tab");
    // tab2 is now active. Focus back to first.
    assert!(model.focus_surface(&first_id));

    let workspace = model.list_workspaces().remove(0);
    assert_eq!(workspace.focused_surface_id, first_id);
    assert_eq!(workspace.pane_tree.leaf_active_id(), Some(&first_id));
}

#[test]
fn repair_session_invariants_clamps_out_of_range_active_index() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let first_id = workspace.focused_surface_id.clone();
    {
        let ws = model.workspaces.get_mut(&workspace.id).unwrap();
        // Force an invalid active index.
        ws.pane_tree = PaneNode::Leaf {
            tabs: vec![first_id.clone()],
            active: 99,
        };
    }

    assert!(model.repair_session_invariants());

    let ws = model.workspaces.get(&workspace.id).unwrap();
    let PaneNode::Leaf { active, .. } = ws.pane_tree else {
        panic!("expected leaf");
    };
    assert_eq!(active, 0);
    crate::session::validate_session_data(&model.to_session_data()).unwrap();
}

#[test]
fn single_leaf_helper_creates_leaf_with_one_tab() {
    let leaf = PaneNode::single_leaf("surface-42".to_string());
    let tabs = leaf.leaf_tabs().expect("leaf");
    assert_eq!(tabs, &["surface-42"]);
    assert_eq!(leaf.leaf_active_id(), Some(&"surface-42".to_string()));
}

fn assert_workspace_model_invariants(model: &WorkspaceModel) {
    use std::collections::BTreeSet;

    let workspaces = model.list_workspaces();
    let workspace_ids: BTreeSet<_> = workspaces.iter().map(|ws| ws.id.clone()).collect();
    let surface_ids: BTreeSet<_> = model
        .list_surfaces(None)
        .iter()
        .map(|s| s.id.clone())
        .collect();

    for workspace in &workspaces {
        assert!(
            surface_ids.contains(&workspace.focused_surface_id),
            "focused surface {} must exist",
            workspace.focused_surface_id
        );
        let leaf_ids = leaf_surface_ids(&workspace.pane_tree);
        assert!(
            leaf_ids.contains(&workspace.focused_surface_id),
            "focused surface {} must be in the pane tree",
            workspace.focused_surface_id
        );
        for leaf_id in leaf_ids {
            assert!(
                surface_ids.contains(&leaf_id),
                "pane leaf {} must reference an existing surface",
                leaf_id
            );
        }
        for surface in model.list_surfaces(Some(&workspace.id)) {
            assert!(
                workspace_ids.contains(&surface.workspace_id),
                "surface {} must belong to an existing workspace",
                surface.id
            );
        }
    }
}

#[test]
fn close_workspace_removes_workspace_scoped_metadata() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/first");
    let second = model.create_workspace("second", "/tmp/second");
    model
        .set_status(&second.id, "qa", "QA", "Running", None)
        .expect("set status");
    model
        .set_progress(&second.id, "build", "Build", 1.0, Some(10.0))
        .expect("set progress");
    model
        .append_log(&second.id, LogLevel::Info, "hello")
        .expect("append log");

    let removed = model
        .close_workspace(WorkspaceSelector::Id(&second.id))
        .expect("workspace removed");
    assert_eq!(removed.id, second.id);
    assert!(model.list_status(&second.id).is_empty());
    assert!(model.list_progress(&second.id).is_empty());
    assert!(model.list_logs(&second.id).is_empty());
    assert_workspace_model_invariants(&model);
    assert_eq!(model.list_workspaces().len(), 1);
    assert_eq!(model.list_workspaces()[0].id, first.id);
}

#[test]
fn invariants_hold_after_split_focus_close_and_restore() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let initial = workspace.focused_surface_id.clone();
    let right = model
        .split_surface(&initial, SplitAxis::Horizontal)
        .expect("split");
    assert_workspace_model_invariants(&model);

    assert!(model.focus_surface(&initial));
    assert_workspace_model_invariants(&model);

    model.close_surface(&right.id).expect("close split surface");
    assert_workspace_model_invariants(&model);

    let session = model.to_session_data();
    let mut restored = WorkspaceModel::new();
    restored.restore_session(session);
    assert_workspace_model_invariants(&restored);
}
