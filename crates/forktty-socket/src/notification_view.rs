//! Socket-only notification projection that excludes raw terminal icon bytes.

use forktty_core::{NotificationItem, NotificationKind, TerminalNotificationMetadata};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct NotificationView<'a> {
    id: &'a str,
    title: &'a str,
    body: &'a str,
    kind: &'a NotificationKind,
    created_at_ms: u128,
    read: bool,
    workspace_id: Option<&'a str>,
    surface_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_metadata: Option<TerminalNotificationMetadataView<'a>>,
}

#[derive(Debug, Serialize)]
struct TerminalNotificationMetadataView<'a> {
    id: &'a str,
    report_activation: bool,
    report_close: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    buttons: &'a Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    icon_names: &'a Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon_cache_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    urgency: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sound_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_after_ms: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_name: Option<&'a str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    notification_types: &'a Vec<String>,
}

impl<'a> From<&'a NotificationItem> for NotificationView<'a> {
    fn from(notification: &'a NotificationItem) -> Self {
        Self {
            id: &notification.id,
            title: &notification.title,
            body: &notification.body,
            kind: &notification.kind,
            created_at_ms: notification.created_at_ms,
            read: notification.read,
            workspace_id: notification.workspace_id.as_deref(),
            surface_id: notification.surface_id.as_deref(),
            terminal_metadata: notification
                .terminal_metadata
                .as_ref()
                .map(TerminalNotificationMetadataView::from),
        }
    }
}

impl<'a> From<&'a TerminalNotificationMetadata> for TerminalNotificationMetadataView<'a> {
    fn from(metadata: &'a TerminalNotificationMetadata) -> Self {
        Self {
            id: &metadata.id,
            report_activation: metadata.report_activation,
            report_close: metadata.report_close,
            buttons: &metadata.buttons,
            icon_names: &metadata.icon_names,
            icon_cache_id: metadata.icon_cache_id.as_deref(),
            urgency: metadata.urgency,
            sound_name: metadata.sound_name.as_deref(),
            expires_after_ms: metadata.expires_after_ms,
            app_name: metadata.app_name.as_deref(),
            notification_types: &metadata.notification_types,
        }
    }
}

pub(crate) fn notification_views(notifications: &[NotificationItem]) -> Vec<NotificationView<'_>> {
    notifications.iter().map(NotificationView::from).collect()
}
