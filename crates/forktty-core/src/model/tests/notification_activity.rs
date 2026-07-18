//! Notification attention and hook-prompt correlation regressions.

use super::*;

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
fn notification_page_borrows_newest_entries_with_exclusive_cursor() {
    let mut model = WorkspaceModel::new();
    let ids = (0..5)
        .map(|index| {
            model
                .create_notification(
                    format!("n{index}"),
                    "body",
                    NotificationKind::Info,
                    None,
                    None,
                )
                .id
        })
        .collect::<Vec<_>>();

    let newest = model.notification_page(2, None).unwrap();
    assert_eq!(
        newest
            .iter()
            .map(|notification| notification.id.as_str())
            .collect::<Vec<_>>(),
        vec![ids[3].as_str(), ids[4].as_str()]
    );

    let previous = model.notification_page(2, Some(&ids[3])).unwrap();
    assert_eq!(
        previous
            .iter()
            .map(|notification| notification.id.as_str())
            .collect::<Vec<_>>(),
        vec![ids[1].as_str(), ids[2].as_str()]
    );
    assert!(model
        .notification_page(2, Some("notification-not-retained"))
        .is_none());

    assert_eq!(model.list_notifications().len(), 5);
}

#[test]
fn updating_notification_moves_it_to_newest_page_without_changing_identity() {
    let mut model = WorkspaceModel::new();
    let first = model.create_notification("first", "old body", NotificationKind::Info, None, None);
    let second = model.create_notification("second", "body", NotificationKind::Info, None, None);

    let updated = model
        .update_notification(
            &first.id,
            "first updated",
            "new body",
            NotificationKind::Prompt,
        )
        .unwrap();

    assert_eq!(updated.id, first.id);
    assert!(updated.created_at_ms > first.created_at_ms);
    assert_eq!(model.list_notifications().len(), 2);
    assert_eq!(
        model
            .list_notifications()
            .into_iter()
            .map(|notification| notification.id)
            .collect::<Vec<_>>(),
        vec![second.id.clone(), first.id.clone()]
    );
    assert_eq!(model.notification_page(1, None).unwrap()[0].id, first.id);
    assert_eq!(
        model.notification_page(1, Some(&first.id)).unwrap()[0].id,
        second.id
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
fn resolve_hook_prompt_removes_only_matching_correlation_and_keeps_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let target = Some(workspace.focused_surface_id.clone());
    let first = model.create_hook_prompt_notification(
        "Permission",
        "Approve first?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target.clone(),
        HookPromptMetadata {
            id: "opencode/session/permission/id:first".to_string(),
            provider: "opencode".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: Some("id:first".to_string()),
        },
    );
    let second = model.create_hook_prompt_notification(
        "Permission",
        "Approve second?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target.clone(),
        HookPromptMetadata {
            id: "opencode/session/permission/id:second".to_string(),
            provider: "opencode".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 11,
            correlation_id: Some("id:second".to_string()),
        },
    );

    let resolved = model
        .resolve_hook_prompt(&HookPromptResolution {
            provider: "opencode".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 12,
            correlation_id: Some("id:first".to_string()),
            workspace_id: Some(workspace.id.clone()),
            surface_id: target,
        })
        .expect("matching prompt resolved");

    assert_eq!(resolved.id, first.id);
    assert!(resolved.read);
    assert_eq!(model.list_notifications(), vec![resolved, second]);
    assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn repeated_hook_prompt_id_reuses_notification_and_desktop_identity() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let prompt = HookPromptMetadata {
        id: "claude/session/permission/100".to_string(),
        provider: "claude".to_string(),
        session_id: "session-1".to_string(),
        kind: HookPromptKind::Permission,
        event_order: 100,
        correlation_id: None,
    };
    let first = model.create_hook_prompt_notification(
        "Permission",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
        prompt.clone(),
    );

    let repeated = model.create_hook_prompt_notification(
        "Permission updated",
        "Still approve?",
        NotificationKind::Prompt,
        Some(workspace.id),
        Some(workspace.focused_surface_id),
        prompt,
    );

    assert_eq!(repeated.id, first.id);
    assert_eq!(model.list_notifications(), vec![repeated]);
}

#[test]
fn older_hook_result_does_not_resolve_newer_correlated_prompt() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let prompt = model.create_hook_prompt_notification(
        "Permission",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
        HookPromptMetadata {
            id: "opencode/session/permission/id:one".to_string(),
            provider: "opencode".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: Some("id:one".to_string()),
        },
    );

    let resolved = model.resolve_hook_prompt(&HookPromptResolution {
        provider: "opencode".to_string(),
        session_id: "session-1".to_string(),
        kind: HookPromptKind::Permission,
        event_order: 9,
        correlation_id: Some("id:one".to_string()),
        workspace_id: Some(workspace.id),
        surface_id: Some(workspace.focused_surface_id),
    });

    assert!(resolved.is_none());
    assert_eq!(model.list_notifications(), vec![prompt]);
}

#[test]
fn repeated_hook_prompt_id_on_different_surface_keeps_distinct_correlations() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let second_surface = model
        .split_surface(&workspace.focused_surface_id, SplitAxis::Horizontal)
        .unwrap();
    let prompt = HookPromptMetadata {
        id: "claude/session/permission/id:shared".to_string(),
        provider: "claude".to_string(),
        session_id: "session-1".to_string(),
        kind: HookPromptKind::Permission,
        event_order: 100,
        correlation_id: Some("id:shared".to_string()),
    };
    let first = model.create_hook_prompt_notification(
        "First surface",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
        prompt.clone(),
    );
    let second = model.create_hook_prompt_notification(
        "Second surface",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(second_surface.id.clone()),
        prompt,
    );

    assert_ne!(first.id, second.id);
    assert_eq!(model.list_notifications().len(), 2);
    let resolved = model
        .resolve_hook_prompt(&HookPromptResolution {
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 101,
            correlation_id: Some("id:shared".to_string()),
            workspace_id: Some(workspace.id),
            surface_id: Some(second_surface.id),
        })
        .unwrap();
    assert_eq!(resolved.id, second.id);
    assert!(resolved.read);
    assert_eq!(model.list_notifications(), vec![first, resolved]);
}

#[test]
fn hook_result_without_correlation_resolves_only_latest_compatible_prompt() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let target = Some(workspace.focused_surface_id.clone());
    let first = model.create_hook_prompt_notification(
        "Permission",
        "Approve first?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target.clone(),
        HookPromptMetadata {
            id: "claude/session/permission/10".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: None,
        },
    );
    let second = model.create_hook_prompt_notification(
        "Permission",
        "Approve second?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target.clone(),
        HookPromptMetadata {
            id: "claude/session/permission/11".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 11,
            correlation_id: None,
        },
    );

    let resolved = model
        .resolve_hook_prompt(&HookPromptResolution {
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 12,
            correlation_id: None,
            workspace_id: Some(workspace.id),
            surface_id: target,
        })
        .expect("latest compatible prompt resolved");

    assert_eq!(resolved.id, second.id);
    assert!(resolved.read);
    assert_eq!(model.list_notifications(), vec![first, resolved]);
}

#[test]
fn evict_hook_prompt_correlations_marks_history_read_and_returns_desktop_ids() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let notification = model.create_hook_prompt_notification(
        "Permission",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
        HookPromptMetadata {
            id: "claude/session/permission/10".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: None,
        },
    );

    let desktop_ids = model.evict_hook_prompt_correlations_for_surfaces(std::slice::from_ref(
        &workspace.focused_surface_id,
    ));

    assert_eq!(desktop_ids, vec![notification.id]);
    let retained = &model.list_notifications()[0];
    assert!(retained.read);
    assert!(!model.hook_prompt_correlations.contains_key(&retained.id));
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn evict_hook_prompt_correlations_for_session_target_preserves_other_sessions() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let target = Some(workspace.focused_surface_id.clone());
    let moved = model.create_hook_prompt_notification(
        "Permission",
        "Moved session",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target.clone(),
        HookPromptMetadata {
            id: "claude/session-1/permission/10".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: None,
        },
    );
    let retained = model.create_hook_prompt_notification(
        "Permission",
        "Other session",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        target,
        HookPromptMetadata {
            id: "claude/session-2/permission/11".to_string(),
            provider: "claude".to_string(),
            session_id: "session-2".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 11,
            correlation_id: None,
        },
    );

    let desktop_ids = model.evict_hook_prompt_correlations_for_session_target(
        "claude",
        "session-1",
        &workspace.focused_surface_id,
    );

    assert_eq!(desktop_ids, vec![moved.id.clone()]);
    let notifications = model.list_notifications();
    let moved = notifications
        .iter()
        .find(|notification| notification.id == moved.id)
        .unwrap();
    let retained = notifications
        .iter()
        .find(|notification| notification.id == retained.id)
        .unwrap();
    assert!(moved.read);
    assert!(!model.hook_prompt_correlations.contains_key(&moved.id));
    assert!(!retained.read);
    assert!(model.hook_prompt_correlations.contains_key(&retained.id));
    assert!(model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(model.list_workspaces()[0].needs_attention);
}

#[test]
fn evict_hook_prompt_session_target_preserves_same_session_from_other_provider() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let target = Some(workspace.focused_surface_id.clone());
    for provider in ["claude", "opencode"] {
        model.create_hook_prompt_notification(
            "Permission",
            "Approve?",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            target.clone(),
            HookPromptMetadata {
                id: format!("{provider}/shared/permission/10"),
                provider: provider.to_string(),
                session_id: "shared".to_string(),
                kind: HookPromptKind::Permission,
                event_order: 10,
                correlation_id: None,
            },
        );
    }

    let closed = model.evict_hook_prompt_correlations_for_session_target(
        "claude",
        "shared",
        &workspace.focused_surface_id,
    );

    assert_eq!(closed.len(), 1);
    let notifications = model.list_notifications();
    assert!(
        notifications
            .iter()
            .find(|item| item.title == "Permission" && item.id == closed[0])
            .unwrap()
            .read
    );
    assert_eq!(notifications.iter().filter(|item| !item.read).count(), 1);
}

#[test]
fn hook_prompt_correlation_map_tracks_dismiss_clear_and_retention_eviction() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let create_prompt = |model: &mut WorkspaceModel, id: &str| {
        model.create_hook_prompt_notification(
            "Permission",
            "Approve?",
            NotificationKind::Prompt,
            Some(workspace.id.clone()),
            Some(workspace.focused_surface_id.clone()),
            HookPromptMetadata {
                id: id.to_string(),
                provider: "claude".to_string(),
                session_id: "session-1".to_string(),
                kind: HookPromptKind::Permission,
                event_order: 10,
                correlation_id: None,
            },
        )
    };

    let dismissed = create_prompt(&mut model, "prompt-dismiss");
    assert!(model.dismiss_notification(&dismissed.id));
    assert!(model.hook_prompt_correlations.is_empty());

    create_prompt(&mut model, "prompt-clear");
    model.clear_notifications();
    assert!(model.hook_prompt_correlations.is_empty());

    create_prompt(&mut model, "prompt-retention");
    for index in 0..MAX_NOTIFICATIONS {
        model.create_notification(
            format!("Notification {index}"),
            "body",
            NotificationKind::Info,
            None,
            None,
        );
    }
    assert_eq!(model.list_notifications().len(), MAX_NOTIFICATIONS);
    assert!(model.hook_prompt_correlations.is_empty());
}

#[test]
fn hook_prompt_retention_eviction_returns_desktop_id_and_recomputes_attention() {
    let mut model = WorkspaceModel::new();
    let workspace = model.create_workspace("main", "/tmp");
    let prompt = model.create_hook_prompt_notification(
        "Permission",
        "Approve?",
        NotificationKind::Prompt,
        Some(workspace.id.clone()),
        Some(workspace.focused_surface_id.clone()),
        HookPromptMetadata {
            id: "claude/session-1/permission/10".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: None,
        },
    );
    let mut evicted_desktop_ids = Vec::new();
    for index in 0..MAX_NOTIFICATIONS {
        let created = model.create_notification_with_evictions(
            format!("Notification {index}"),
            "body",
            NotificationKind::Info,
            None,
            None,
        );
        evicted_desktop_ids.extend(created.evicted_desktop_notification_ids);
    }

    assert_eq!(evicted_desktop_ids, vec![prompt.id]);
    assert!(!model.surface(&workspace.focused_surface_id).unwrap().unread);
    assert!(!model.list_workspaces()[0].needs_attention);
}

#[test]
fn retention_returns_every_evicted_desktop_id_in_history_order() {
    let mut model = WorkspaceModel::new();
    let generic = model.create_notification(
        "Socket notification",
        "body",
        NotificationKind::Info,
        None,
        None,
    );
    let hook = model.create_hook_prompt_notification(
        "Permission",
        "Approve?",
        NotificationKind::Prompt,
        None,
        None,
        HookPromptMetadata {
            id: "claude/session-1/permission/10".to_string(),
            provider: "claude".to_string(),
            session_id: "session-1".to_string(),
            kind: HookPromptKind::Permission,
            event_order: 10,
            correlation_id: None,
        },
    );
    for index in 2..MAX_NOTIFICATIONS {
        model.create_notification(
            format!("Notification {index}"),
            "body",
            NotificationKind::Info,
            None,
            None,
        );
    }

    let first_eviction = model.create_notification_with_evictions(
        "Overflow 1",
        "body",
        NotificationKind::Info,
        None,
        None,
    );
    assert_eq!(
        first_eviction.evicted_desktop_notification_ids,
        vec![generic.id]
    );
    assert!(model.hook_prompt_correlations.contains_key(&hook.id));

    let second_eviction = model.create_notification_with_evictions(
        "Overflow 2",
        "body",
        NotificationKind::Info,
        None,
        None,
    );
    assert_eq!(
        second_eviction.evicted_desktop_notification_ids,
        vec![hook.id.clone()]
    );
    assert!(!model.hook_prompt_correlations.contains_key(&hook.id));
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
