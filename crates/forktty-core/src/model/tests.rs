//! Workspace model regression tests.

use super::*;

mod pane_tabs;
mod status_progress;
mod surface_kind;
mod surface_persistence;

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
fn mark_notifications_read_preserves_output_unread_surface_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    assert!(model.mark_surface_unread(&workspace.focused_surface_id, true));
    model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    model.mark_notifications_read();

    assert_eq!(model.unread_notification_count(), 0);
    assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(model.list_workspaces()[0].needs_attention);

    assert!(model.mark_surface_unread(&workspace.focused_surface_id, false));
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn clear_notifications_preserves_output_unread_surface_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    assert!(model.mark_surface_unread(&workspace.focused_surface_id, true));
    model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    model.clear_notifications();

    assert!(model.list_notifications().is_empty());
    assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(model.list_workspaces()[0].needs_attention);
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
fn dismiss_notification_preserves_output_unread_surface_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    assert!(model.mark_surface_unread(&workspace.focused_surface_id, true));
    let notification = model.create_notification(
        "Prompt",
        "Ready",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
    );

    assert!(model.dismiss_notification(&notification.id));

    assert_eq!(model.unread_notification_count(), 0);
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
fn restored_session_keeps_closed_workspace_and_surface_ids_reserved() {
    let mut model = WorkspaceModel::new();
    let main = model.create_workspace("main", "/tmp/main");
    let closed = model.create_workspace("closed", "/tmp/closed");
    let closed_surface_id = closed.focused_surface_id.clone();

    model.close_workspace(WorkspaceSelector::Id(&closed.id));
    let data = model.to_session_data();
    assert_eq!(data.next_workspace, 2);
    assert_eq!(data.next_surface, 2);

    let mut restored = WorkspaceModel::new();
    restored.restore_session(data);
    let fresh = restored.create_workspace("fresh", "/tmp/fresh");

    assert_eq!(fresh.id, "workspace-3");
    assert_eq!(fresh.focused_surface_id, "surface-3");
    assert_ne!(fresh.id, closed.id);
    assert_ne!(fresh.focused_surface_id, closed_surface_id);
    assert!(restored
        .workspace_id_for(WorkspaceSelector::Id(&main.id))
        .is_some());
}

#[test]
fn reserved_ids_are_not_reused_by_fresh_sessions() {
    let mut model = WorkspaceModel::new();
    model.reserve_workspace_id("workspace-3");
    model.reserve_surface_id("surface-5");

    let workspace = model.create_workspace("fresh", "/tmp/fresh");

    assert_eq!(workspace.id, "workspace-4");
    assert_eq!(workspace.focused_surface_id, "surface-6");
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
        next_workspace: 0,
        next_surface: 0,
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
