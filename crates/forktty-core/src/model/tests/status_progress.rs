//! Workspace status, progress, log, and hook-order regression tests.

use super::*;

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
