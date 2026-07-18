use crate::{DispatchError, SocketAppState};
use forktty_core::{config, dispatch_notification, NotificationItem, NotificationKind};
use std::sync::{mpsc, OnceLock};

/// Pending desktop notification dispatch jobs. Dispatch can block in
/// notify-rust/custom commands, so keep a small bounded queue rather than
/// spawning one OS thread per socket request.
const NOTIFICATION_DISPATCH_QUEUE_CAPACITY: usize = 64;

static NOTIFICATION_DISPATCHER: OnceLock<mpsc::SyncSender<NotificationItem>> = OnceLock::new();

/// Surfaces a non-fatal worktree `setup` hook failure as a workspace-scoped
/// error notification so it is visible in the UI instead of only on stderr.
pub(crate) fn notify_worktree_setup_warning(
    state: &SocketAppState,
    workspace_id: &str,
    warning: Option<&str>,
) -> Result<(), DispatchError> {
    let Some(warning) = warning else {
        return Ok(());
    };
    let creation = {
        let mut model = state
            .model
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        model.create_notification_with_evictions(
            "Worktree Setup Hook Failed",
            warning,
            NotificationKind::Error,
            Some(workspace_id.to_string()),
            None,
        )
    };
    for notification_id in creation.evicted_desktop_notification_ids {
        state.close_desktop_notification(&notification_id);
    }
    let item = creation.notification;
    if state.notification_dispatch {
        dispatch_notification_with_loaded_config(&item);
    }
    Ok(())
}

pub(crate) fn dispatch_notification_with_loaded_config(notification: &NotificationItem) {
    let dispatcher = NOTIFICATION_DISPATCHER.get_or_init(spawn_notification_dispatcher);
    match dispatcher.try_send(notification.clone()) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            eprintln!(
                "Dropping desktop notification dispatch because the bounded dispatch queue is full"
            );
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {
            eprintln!("Dropping desktop notification dispatch because the dispatch worker stopped");
        }
    }
}

fn spawn_notification_dispatcher() -> mpsc::SyncSender<NotificationItem> {
    let (sender, receiver) = mpsc::sync_channel(NOTIFICATION_DISPATCH_QUEUE_CAPACITY);
    // notify_rust's show() blocks on its own async runtime, and doing that from
    // a tokio worker panics ("Cannot start a runtime from within a runtime"),
    // killing the connection task that carried the request. Use one dedicated
    // blocking worker with a bounded queue so repeated socket notifications
    // cannot create unbounded native threads.
    if let Err(err) = std::thread::Builder::new()
        .name("forktty-notification-dispatch".to_string())
        .spawn(move || {
            for notification in receiver {
                dispatch_notification_from_worker(&notification);
            }
        })
    {
        eprintln!("Failed to start desktop notification dispatch worker: {err}");
    }
    sender
}

fn dispatch_notification_from_worker(notification: &NotificationItem) {
    let config = match config::load_config() {
        Ok(config) => config,
        Err(err) => {
            // Surface the underlying cause so a misconfigured custom command or
            // a corrupted config.toml is debuggable rather than silently
            // turning into "default behavior with no custom command".
            eprintln!("Falling back to default notification settings: {err}");
            forktty_core::AppConfig::default()
        }
    };
    for error in dispatch_notification(&config, notification) {
        eprintln!(
            "Failed to dispatch {} notification: {}",
            error.channel, error.message
        );
    }
}
