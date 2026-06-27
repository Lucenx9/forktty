//! Per-pane tab group model regression tests.

use super::*;

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
