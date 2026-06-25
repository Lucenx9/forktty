use crate::{current_unix_epoch_ms, feed_view, SocketAppState};
use forktty_core::{
    events::ModelEvent, FeedApprovalState, FeedEntry, FeedEntryType, NotificationItem,
    NotificationKind,
};

pub(crate) fn record_feed_events(
    state: &SocketAppState,
    events: &[ModelEvent],
) -> Result<(), String> {
    let mut store = state
        .feed_store
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let Some(store) = store.as_mut() else {
        return Ok(());
    };
    for event in events {
        if let Some(entry) = feed_entry_from_event(event) {
            store.append(entry).map_err(|err| err.to_string())?;
        }
    }
    Ok(())
}

pub(crate) fn record_feed_entry(state: &SocketAppState, entry: FeedEntry) -> Result<(), String> {
    let mut store = state
        .feed_store
        .lock()
        .map_err(|_| "Lock poisoned".to_string())?;
    let Some(store) = store.as_mut() else {
        return Ok(());
    };
    store.append(entry).map_err(|err| err.to_string())
}

pub(crate) fn feed_entry_from_notification(item: &NotificationItem) -> FeedEntry {
    let is_approval = item.kind == NotificationKind::Prompt;
    FeedEntry {
        id: feed_notification_entry_id(item),
        entry_type: if is_approval {
            FeedEntryType::Approval
        } else {
            FeedEntryType::Notification
        },
        kind: Some(notification_kind_name(item.kind).to_string()),
        read: item.read,
        key: None,
        value: None,
        total: None,
        title: item.title.clone(),
        body: item.body.clone(),
        workspace_id: item.workspace_id.clone(),
        surface_id: item.surface_id.clone(),
        created_at_ms: item.created_at_ms,
        approval_state: is_approval.then_some(FeedApprovalState::Pending),
    }
}

pub(crate) fn feed_notification_entry_id(item: &NotificationItem) -> String {
    format!("feed:{}:notification:{}", item.created_at_ms, item.id)
}

fn feed_entry_from_event(event: &ModelEvent) -> Option<FeedEntry> {
    match event {
        ModelEvent::NotificationAdded {
            id,
            workspace_id,
            surface_id,
            kind,
            title,
            body,
            created_at_ms,
        } => Some(feed_entry_from_notification(&NotificationItem {
            id: id.clone(),
            title: title.clone(),
            body: body.clone(),
            kind: *kind,
            created_at_ms: *created_at_ms,
            read: false,
            workspace_id: workspace_id.clone(),
            surface_id: surface_id.clone(),
            terminal_metadata: None,
        })),
        ModelEvent::StatusChanged {
            workspace_id,
            key,
            label,
            value: Some(value),
        } => {
            let now = u128::from(current_unix_epoch_ms());
            Some(FeedEntry {
                id: format!("feed:{now}:{workspace_id}:status:{key}"),
                entry_type: FeedEntryType::Status,
                kind: Some("status".to_string()),
                read: false,
                key: Some(key.clone()),
                value: None,
                total: None,
                title: label.clone(),
                body: value.clone(),
                workspace_id: Some(workspace_id.clone()),
                surface_id: None,
                created_at_ms: now,
                approval_state: None,
            })
        }
        ModelEvent::ProgressChanged {
            workspace_id,
            key,
            label,
            value: Some(value),
            total,
        } => {
            let now = u128::from(current_unix_epoch_ms());
            let body = match total {
                Some(total) => format!(
                    "{}/{}",
                    feed_view::number(*value),
                    feed_view::number(*total)
                ),
                None => feed_view::number(*value),
            };
            Some(FeedEntry {
                id: format!("feed:{now}:{workspace_id}:progress:{key}"),
                entry_type: FeedEntryType::Progress,
                kind: Some("progress".to_string()),
                read: false,
                key: Some(key.clone()),
                value: Some(*value),
                total: *total,
                title: label.clone(),
                body,
                workspace_id: Some(workspace_id.clone()),
                surface_id: None,
                created_at_ms: now,
                approval_state: None,
            })
        }
        _ => None,
    }
}

fn notification_kind_name(kind: NotificationKind) -> &'static str {
    match kind {
        NotificationKind::Prompt => "prompt",
        NotificationKind::Error => "error",
        NotificationKind::Info => "info",
        NotificationKind::Custom => "custom",
    }
}
