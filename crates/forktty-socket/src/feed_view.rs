use forktty_core::{
    FeedApprovalState, FeedEntry, FeedEntryType, NotificationItem, NotificationKind,
    WorkspaceModel, WorkspaceSelector,
};
use serde_json::{json, Value};

use crate::SocketAppState;

pub(crate) fn list(model: &WorkspaceModel, workspace_id: Option<&str>, limit: usize) -> Vec<Value> {
    if limit == 0 {
        return Vec::new();
    }

    let mut prompt_notifications = Vec::new();
    let mut other_notifications = Vec::new();
    for notification in model.list_notifications() {
        if workspace_id.is_some_and(|id| notification.workspace_id.as_deref() != Some(id)) {
            continue;
        }
        if notification.kind == NotificationKind::Prompt {
            prompt_notifications.push(notification);
        } else {
            other_notifications.push(notification);
        }
    }
    prompt_notifications.sort_by_key(|notification| std::cmp::Reverse(notification.created_at_ms));
    other_notifications.sort_by_key(|notification| std::cmp::Reverse(notification.created_at_ms));

    let mut items = Vec::with_capacity(
        limit.min(
            prompt_notifications
                .len()
                .saturating_add(other_notifications.len()),
        ),
    );
    items.extend(
        prompt_notifications
            .into_iter()
            .take(limit)
            .map(|notification| notification_feed_item(model, notification)),
    );
    if items.len() >= limit {
        return items;
    }

    for workspace in model
        .list_workspaces()
        .into_iter()
        .filter(|workspace| workspace_id.is_none_or(|id| workspace.id == id))
    {
        let remaining = limit - items.len();
        if remaining == 0 {
            break;
        }
        for status in model.list_status_limited(&workspace.id, remaining) {
            items.push(json!({
                "id": format!("status:{}:{}", workspace.id, status.key),
                "type": "status",
                "kind": "status",
                "key": status.key,
                "title": status.label,
                "body": status.value,
                "workspace_id": workspace.id,
                "surface_id": null,
                "created_at_ms": 0,
            }));
        }

        let remaining = limit - items.len();
        if remaining == 0 {
            break;
        }
        for progress in model.list_progress_limited(&workspace.id, remaining) {
            let body = match progress.total {
                Some(total) => format!("{}/{}", number(progress.value), number(total)),
                None => number(progress.value),
            };
            items.push(json!({
                "id": format!("progress:{}:{}", workspace.id, progress.key),
                "type": "progress",
                "kind": "progress",
                "key": progress.key,
                "title": progress.label,
                "body": body,
                "value": progress.value,
                "total": progress.total,
                "workspace_id": workspace.id,
                "surface_id": null,
                "created_at_ms": 0,
            }));
        }
    }

    if items.len() < limit {
        items.extend(
            other_notifications
                .into_iter()
                .take(limit - items.len())
                .map(|notification| notification_feed_item(model, notification)),
        );
    }
    items
}

pub(crate) fn entries_for_model(model: &WorkspaceModel, entries: Vec<FeedEntry>) -> Vec<Value> {
    entries
        .into_iter()
        .map(|mut entry| {
            normalize_feed_entry_for_model(model, &mut entry);
            json!(entry)
        })
        .collect()
}

fn normalize_feed_entry_for_model(model: &WorkspaceModel, entry: &mut FeedEntry) {
    if entry.entry_type == FeedEntryType::Approval
        && entry.approval_state == Some(FeedApprovalState::Pending)
        && feed_entry_target_is_stale(model, entry)
    {
        entry.approval_state = Some(FeedApprovalState::Stale);
    }
}

pub(crate) fn feed_entry_target_is_stale(model: &WorkspaceModel, entry: &FeedEntry) -> bool {
    feed_target_is_stale(
        model,
        entry.workspace_id.as_deref(),
        entry.surface_id.as_deref(),
    )
}

fn feed_target_is_stale(
    model: &WorkspaceModel,
    workspace_id: Option<&str>,
    surface_id: Option<&str>,
) -> bool {
    if let Some(surface_id) = surface_id {
        let Some(surface) = model.surface(surface_id) else {
            return true;
        };
        return workspace_id.is_some_and(|workspace_id| workspace_id != surface.workspace_id);
    }
    workspace_id.is_some_and(|workspace_id| {
        model
            .workspace_id_for(WorkspaceSelector::Id(workspace_id))
            .is_none()
    })
}

fn notification_feed_item(model: &WorkspaceModel, notification: NotificationItem) -> Value {
    let item_type = if notification.kind == NotificationKind::Prompt {
        "approval"
    } else {
        "notification"
    };
    let mut item = json!({
        "id": notification.id,
        "type": item_type,
        "kind": notification.kind,
        "title": notification.title,
        "body": notification.body,
        "workspace_id": notification.workspace_id,
        "surface_id": notification.surface_id,
        "created_at_ms": notification.created_at_ms,
        "read": notification.read,
    });
    if notification.kind == NotificationKind::Prompt {
        item["approval_state"] = if feed_target_is_stale(
            model,
            item["workspace_id"].as_str(),
            item["surface_id"].as_str(),
        ) {
            json!("stale")
        } else {
            json!("pending")
        };
    }
    item
}

pub(crate) fn send_terminal_notification_close_report(
    state: &SocketAppState,
    notification: &NotificationItem,
) {
    let Some(metadata) = notification.terminal_metadata.as_ref() else {
        return;
    };
    if !metadata.report_close {
        return;
    }
    let Some(surface_id) = notification.surface_id.as_deref() else {
        return;
    };
    let report = format!("\x1b]99;i={}:p=close;\x1b\\", metadata.id);
    let _ = state.terminal.send_text(surface_id, &report);
}

pub(crate) fn number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}
