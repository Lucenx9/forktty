//! Embedded tick cadence plus tab and workspace drag regressions.

use super::*;

#[test]
fn embedded_ghostty_context_tick_fallback_is_not_frame_rate() {
    assert!(
        EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL >= Duration::from_secs(1),
        "fallback context tick only drains app mailbox; frame-rate polling leaks idle memory"
    );
    assert!(
        EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL < EMBEDDED_GHOSTTY_CONTEXT_TICK_FALLBACK_INTERVAL,
        "event-driven wakeup checks may be frequent because they only read an atomic flag"
    );
}

#[test]
fn embedded_ghostty_event_driven_ticks_are_not_capped_below_wakeup_rate() {
    // The wakeup-check timer already coalesces output bursts to its own cadence;
    // the tick floor must not throttle below it, or agent/TUI redraws would be
    // capped well under the display frame rate. The old 100ms floor existed only
    // to slow the cairo software-renderer leak; the GL renderer makes it
    // unnecessary, and GTK's frame clock paces the actual redraw.
    assert!(
        EMBEDDED_GHOSTTY_CONTEXT_TICK_MIN_INTERVAL <= EMBEDDED_GHOSTTY_WAKEUP_CHECK_INTERVAL,
        "tick floor must not throttle below the wakeup-check cadence"
    );
}

#[test]
fn tab_drop_target_uses_whole_strip_geometry() {
    let targets = vec![
        ("surface-1".to_string(), 10.0),
        ("surface-2".to_string(), 30.0),
        ("surface-3".to_string(), 50.0),
    ];

    assert_eq!(
        tab_drop_target_at_x(&targets, 0.0),
        Some(("surface-1", forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_drop_target_at_x(&targets, 20.0),
        Some(("surface-2", forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_drop_target_at_x(&targets, 60.0),
        Some(("surface-3", forktty_core::MovePosition::After))
    );
    assert_eq!(tab_drop_target_at_x(&[], 20.0), None);
}

#[test]
fn dnd_payload_types_keep_drag_kinds_distinct() {
    assert_ne!(tab_dnd_type(), workspace_dnd_type());
    assert_ne!(tab_dnd_type(), pane_dnd_type());
    assert_ne!(workspace_dnd_type(), pane_dnd_type());

    let tab = tab_dnd_value("surface-7");
    assert_eq!(tab_dnd_id_from_value(&tab).as_deref(), Some("surface-7"));
    assert!(tab.get::<String>().is_err());
}

#[test]
fn drop_position_splits_on_midpoint() {
    assert_eq!(drop_position(0.0, 40), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(19.0, 40), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(20.0, 40), forktty_core::MovePosition::After);
    // A zero/degenerate span must not divide by zero; the clamped half still splits.
    assert_eq!(drop_position(0.0, 0), forktty_core::MovePosition::Before);
    assert_eq!(drop_position(1.0, 0), forktty_core::MovePosition::After);
}

#[test]
fn workspace_drop_position_matches_model_reorder_direction() {
    let mut model = WorkspaceModel::new();
    let first = model.create_workspace("first", "/tmp/first").id;
    let second = model.create_workspace("second", "/tmp/second").id;
    let third = model.create_workspace("third", "/tmp/third").id;

    assert!(model.move_workspace(&first, &third, drop_position(39.0, 40)));
    assert_eq!(
        model
            .list_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        vec![second.clone(), third.clone(), first.clone()]
    );

    assert!(model.move_workspace(&first, &second, drop_position(0.0, 40)));
    assert_eq!(
        model
            .list_workspaces()
            .into_iter()
            .map(|workspace| workspace.id)
            .collect::<Vec<_>>(),
        vec![first, second, third]
    );
}

#[test]
fn tab_drop_target_at_x_handles_edge_insertions() {
    let targets = vec![
        ("surface-1".to_string(), 10.0),
        ("surface-2".to_string(), 30.0),
    ];
    // Dropping far left of every midpoint inserts before the first tab.
    assert_eq!(
        tab_drop_target_at_x(&targets, -50.0),
        Some(("surface-1", forktty_core::MovePosition::Before))
    );
    // Dropping past the last midpoint appends after the last tab.
    assert_eq!(
        tab_drop_target_at_x(&targets, 9_999.0),
        Some(("surface-2", forktty_core::MovePosition::After))
    );
}

#[test]
fn tab_move_target_uses_adjacent_tabs_without_wrapping() {
    let tree = PaneNode::Leaf {
        tabs: vec![
            "surface-1".to_string(),
            "surface-2".to_string(),
            "surface-3".to_string(),
        ],
        active: 1,
    };

    assert_eq!(
        tab_move_target(&tree, "surface-2", TabMoveDirection::Left),
        Some(("surface-1".to_string(), forktty_core::MovePosition::Before))
    );
    assert_eq!(
        tab_move_target(&tree, "surface-2", TabMoveDirection::Right),
        Some(("surface-3".to_string(), forktty_core::MovePosition::After))
    );
    assert_eq!(
        tab_move_target(&tree, "surface-1", TabMoveDirection::Left),
        None
    );
    assert_eq!(
        tab_move_target(&tree, "surface-3", TabMoveDirection::Right),
        None
    );
    assert_eq!(
        tab_move_target(&tree, "missing-surface", TabMoveDirection::Right),
        None
    );
}

#[test]
fn tab_move_would_keep_order_detects_adjacent_noops() {
    let order = vec![
        "surface-1".to_string(),
        "surface-2".to_string(),
        "surface-3".to_string(),
    ];

    assert!(tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-1",
        forktty_core::MovePosition::Before
    ));
    assert!(tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-2",
        forktty_core::MovePosition::Before
    ));
    assert!(tab_move_would_keep_order(
        &order,
        "surface-2",
        "surface-1",
        forktty_core::MovePosition::After
    ));
    assert!(!tab_move_would_keep_order(
        &order,
        "surface-1",
        "surface-3",
        forktty_core::MovePosition::Before
    ));
    assert!(!tab_move_would_keep_order(
        &order,
        "surface-3",
        "surface-1",
        forktty_core::MovePosition::After
    ));
}

#[test]
fn workspace_move_would_keep_order_detects_adjacent_noops() {
    let order = vec![
        "workspace-1".to_string(),
        "workspace-2".to_string(),
        "workspace-3".to_string(),
    ];

    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-1",
        "workspace-2",
        forktty_core::MovePosition::Before
    ));
    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-2",
        "workspace-1",
        forktty_core::MovePosition::After
    ));
    assert!(workspace_move_would_keep_order(
        &order,
        "workspace-2",
        "workspace-2",
        forktty_core::MovePosition::After
    ));
    assert!(!workspace_move_would_keep_order(
        &order,
        "workspace-1",
        "workspace-3",
        forktty_core::MovePosition::After
    ));
}
