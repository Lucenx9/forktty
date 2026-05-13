use crate::{AppConfig, NotificationItem};
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationDispatchError {
    pub channel: &'static str,
    pub message: String,
}

pub fn dispatch_notification(
    config: &AppConfig,
    notification: &NotificationItem,
) -> Vec<NotificationDispatchError> {
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
            .hint(Hint::DesktopEntry("forktty-gtk".to_string()))
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

fn is_executable_file(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NotificationKind, WorkspaceModel};

    #[test]
    fn empty_command_is_noop_when_desktop_is_disabled() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);

        assert!(dispatch_notification(&config, &notification).is_empty());
    }

    #[test]
    fn rejects_relative_custom_command() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = "notify-send".to_string();
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);

        let errors = dispatch_notification(&config, &notification);

        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].channel, "custom_command");
        assert!(errors[0].message.contains("absolute executable"));
    }

    #[test]
    fn accepts_absolute_custom_command() {
        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = "/bin/true".to_string();
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);

        assert!(dispatch_notification(&config, &notification).is_empty());
    }
}
