use crate::command_safety::{is_executable_file, is_shell_trampoline};
use crate::{AppConfig, NotificationItem, NotificationKind};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDispatchError {
    pub channel: &'static str,
    pub message: String,
}

/// Suppress identical notifications fired in rapid succession. Flapping
/// agents would otherwise spawn one desktop notification and one custom
/// command per emission, with one OS thread per spawn.
const NOTIFICATION_DEDUPE_WINDOW: Duration = Duration::from_secs(2);
const NOTIFICATION_DEDUPE_CAPACITY: usize = 64;
const DESKTOP_ENTRY_ID: &str = "dev.forktty.ForkTTY";

type DedupeKey = (
    NotificationKind,
    String,
    String,
    Option<String>,
    Option<String>,
);

fn dedupe_cache() -> &'static Mutex<VecDeque<(DedupeKey, Instant)>> {
    static CACHE: OnceLock<Mutex<VecDeque<(DedupeKey, Instant)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn should_dispatch(notification: &NotificationItem, now: Instant) -> bool {
    let key: DedupeKey = (
        notification.kind,
        notification.title.clone(),
        notification.body.clone(),
        notification.workspace_id.clone(),
        notification.surface_id.clone(),
    );
    let mut cache = dedupe_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // Calls can race before taking the mutex, so insertion order is not a
    // reliable proxy for timestamp order.
    cache.retain(|(_, ts)| now.duration_since(*ts) < NOTIFICATION_DEDUPE_WINDOW);
    if cache.iter().any(|(existing, _)| existing == &key) {
        return false;
    }
    if cache.len() >= NOTIFICATION_DEDUPE_CAPACITY {
        cache.pop_front();
    }
    cache.push_back((key, now));
    true
}

pub fn dispatch_notification(
    config: &AppConfig,
    notification: &NotificationItem,
) -> Vec<NotificationDispatchError> {
    if !should_dispatch(notification, Instant::now()) {
        return Vec::new();
    }
    let mut errors = Vec::new();
    if config.notifications.desktop {
        if let Err(message) = send_desktop_notification(
            &notification.title,
            &notification.body,
            config.notifications.sound,
        ) {
            errors.push(NotificationDispatchError {
                channel: "desktop",
                message,
            });
        }
    }
    if let Err(message) = run_custom_command(&config.general.notification_command, notification) {
        errors.push(NotificationDispatchError {
            channel: "custom_command",
            message,
        });
    }
    errors
}

fn send_desktop_notification(title: &str, body: &str, play_sound: bool) -> Result<(), String> {
    let mut notification = notify_rust::Notification::new();
    notification
        .summary(title)
        .body(body)
        .icon("forktty")
        .appname("ForkTTY");

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use notify_rust::Hint;

        notification
            .hint(Hint::DesktopEntry(DESKTOP_ENTRY_ID.to_string()))
            .hint(Hint::Category("im.received".to_string()));

        if !play_sound {
            notification.hint(Hint::SuppressSound(true));
        }
    }

    notification.show().map_err(|err| err.to_string())?;
    Ok(())
}

fn run_custom_command(command: &str, notification: &NotificationItem) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(());
    }

    let parts = shell_words::split(command).map_err(|err| err.to_string())?;
    let (program, args) = parts
        .split_first()
        .ok_or_else(|| "Empty notification command".to_string())?;
    let program_path = Path::new(program);
    if is_shell_trampoline(program, args.first().map(String::as_str)) {
        return Err("notification_command must not invoke a shell with -c".to_string());
    }
    if !is_executable_file(program_path) {
        return Err(format!(
            "notification_command must start with an absolute executable file: {program}"
        ));
    }

    let mut child = std::process::Command::new(program)
        .args(args)
        .env("FORKTTY_NOTIFICATION_ID", &notification.id)
        .env("FORKTTY_NOTIFICATION_TITLE", &notification.title)
        .env("FORKTTY_NOTIFICATION_BODY", &notification.body)
        .env(
            "FORKTTY_NOTIFICATION_KIND",
            notification_kind_name(notification),
        )
        .env(
            "FORKTTY_NOTIFICATION_WORKSPACE_ID",
            notification.workspace_id.as_deref().unwrap_or(""),
        )
        .env(
            "FORKTTY_NOTIFICATION_SURFACE_ID",
            notification.surface_id.as_deref().unwrap_or(""),
        )
        .spawn()
        .map_err(|err| err.to_string())?;

    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn notification_kind_name(notification: &NotificationItem) -> &'static str {
    match notification.kind {
        crate::NotificationKind::Prompt => "prompt",
        crate::NotificationKind::Error => "error",
        crate::NotificationKind::Info => "info",
        crate::NotificationKind::Custom => "custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NotificationKind, WorkspaceModel};

    #[test]
    fn desktop_entry_hint_matches_packaged_desktop_id() {
        assert_eq!(DESKTOP_ENTRY_ID, "dev.forktty.ForkTTY");
    }

    #[test]
    fn empty_command_is_noop_when_desktop_is_disabled() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "empty-command-test",
            "Body",
            NotificationKind::Info,
            None,
            None,
        );

        assert!(dispatch_notification(&config, &notification).is_empty());
    }

    #[test]
    fn rejects_relative_custom_command() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = "notify-send".to_string();
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "relative-command-test",
            "Body",
            NotificationKind::Info,
            None,
            None,
        );

        let errors = dispatch_notification(&config, &notification);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].channel, "custom_command");
        assert!(errors[0].message.contains("absolute executable"));
    }

    #[test]
    fn rejects_shell_trampoline_custom_command() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = "/bin/sh -c notify-send".to_string();
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "trampoline-test",
            "Body",
            NotificationKind::Info,
            None,
            None,
        );

        let errors = dispatch_notification(&config, &notification);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].channel, "custom_command");
        assert!(errors[0].message.contains("must not invoke a shell"));
    }

    #[test]
    fn dedupes_identical_back_to_back_notifications() {
        let now = Instant::now();
        let mut model = WorkspaceModel::new();
        let workspace = model.create_workspace("main", "/tmp");
        let notification = model.create_notification(
            "Title",
            "Body",
            NotificationKind::Info,
            Some(workspace.id.clone()),
            None,
        );

        assert!(should_dispatch(&notification, now));
        assert!(!should_dispatch(&notification, now));
        // After the dedupe window elapses, the same key dispatches again.
        assert!(should_dispatch(
            &notification,
            now + NOTIFICATION_DEDUPE_WINDOW
        ));
        // A different surface_id is treated as a distinct notification.
        let other = NotificationItem {
            surface_id: Some("surface-other".to_string()),
            ..notification
        };
        assert!(should_dispatch(&other, now));
    }

    #[test]
    fn accepts_absolute_custom_command() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = "/bin/true".to_string();
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "absolute-command-test",
            "Body",
            NotificationKind::Info,
            None,
            None,
        );

        assert!(dispatch_notification(&config, &notification).is_empty());
    }
}
