//! Terminal notification filtering, command validation, and notification payload helpers.

use crate::command_safety::{is_executable_file, is_shell_trampoline};
use crate::{AppConfig, NotificationItem, NotificationKind};
use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};
#[cfg(unix)]
use std::{fs::Permissions, os::unix::fs::PermissionsExt};

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
const CUSTOM_COMMAND_REAPER_QUEUE_CAPACITY: usize = 8;
const DESKTOP_NOTIFICATION_HANDLE_CAPACITY: usize = 128;
const DESKTOP_NOTIFICATION_ACTION_LISTENER_CAPACITY: usize = 64;
const DESKTOP_ENTRY_ID: &str = "dev.forktty.forktty";
const TERMINAL_ICON_MAX_DIMENSION: u32 = 128;
const TERMINAL_ICON_MAX_PIXELS: u32 = TERMINAL_ICON_MAX_DIMENSION * TERMINAL_ICON_MAX_DIMENSION;

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
    let mut cache = dedupe_cache()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // Calls can race before taking the mutex, so insertion order is not a
    // reliable proxy for timestamp order.
    cache.retain(|(_, ts)| now.duration_since(*ts) < NOTIFICATION_DEDUPE_WINDOW);
    // Compare fields directly by reference to avoid allocating a DedupeKey tuple.
    // This saves 4 heap allocations (from String/Option<String> cloning) per
    // skipped notification when checking the cache.
    if cache.iter().any(|(existing, _)| {
        existing.0 == notification.kind
            && existing.1 == notification.title
            && existing.2 == notification.body
            && existing.3 == notification.workspace_id
            && existing.4 == notification.surface_id
    }) {
        return false;
    }
    let key: DedupeKey = (
        notification.kind,
        notification.title.clone(),
        notification.body.clone(),
        notification.workspace_id.clone(),
        notification.surface_id.clone(),
    );
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
        if let Err(message) = send_desktop_notification(notification, config.notifications.sound) {
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

pub fn close_desktop_notification(notification_id: &str) {
    if let Some(handle) = take_desktop_notification_handle(notification_id) {
        // notify-rust's zbus backend blocks internally; socket callers may run
        // inside Tokio workers, where nested blocking runtimes panic.
        let _ = std::thread::Builder::new()
            .name("forktty-notification-close".to_string())
            .spawn(move || handle.close());
    }
}

fn send_desktop_notification(item: &NotificationItem, play_sound: bool) -> Result<(), String> {
    let mut notification = notify_rust::Notification::new();
    let action_args = desktop_notification_context_action_args(item);
    let action_listener = action_args
        .as_ref()
        .and_then(|_| reserve_desktop_notification_action_listener());
    notification
        .summary(&item.title)
        .body(&item.body)
        .icon(desktop_notification_icon_name(item))
        .appname("ForkTTY");
    apply_desktop_notification_context_action(&mut notification, action_listener.is_some());

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use notify_rust::Hint;

        notification
            .hint(Hint::DesktopEntry(DESKTOP_ENTRY_ID.to_string()))
            .hint(Hint::Category("im.received".to_string()));

        apply_terminal_notification_metadata_hints(&mut notification, item, play_sound);
    }

    let image_path = desktop_notification_runtime_dir()
        .and_then(|runtime_dir| write_desktop_notification_icon_file(item, &runtime_dir));
    if let Some(path) = image_path.as_ref() {
        notification.image_path(path.to_string_lossy().as_ref());
    }

    if let Some(id) = desktop_notification_server_id(&item.id) {
        notification.id(id);
    }
    let handle = match notification.show() {
        Ok(handle) => handle,
        Err(err) => {
            remove_desktop_notification_icon_file(image_path);
            return Err(err.to_string());
        }
    };
    spawn_desktop_notification_action_listener(handle.id(), action_args, action_listener);
    remember_desktop_notification_handle(item.id.clone(), handle, image_path);
    Ok(())
}

fn desktop_notification_context_action_args(item: &NotificationItem) -> Option<Vec<String>> {
    if let Some(surface_id) = item.surface_id.as_deref() {
        return Some(vec!["focus-surface".to_string(), surface_id.to_string()]);
    }
    item.workspace_id.as_deref().map(|workspace_id| {
        vec![
            "focus".to_string(),
            "--workspace-id".to_string(),
            workspace_id.to_string(),
        ]
    })
}

fn apply_desktop_notification_context_action(
    notification: &mut notify_rust::Notification,
    listener_reserved: bool,
) {
    if listener_reserved {
        notification.action("default", "Open");
    }
}

struct DesktopNotificationActionListenerToken;

impl Drop for DesktopNotificationActionListenerToken {
    fn drop(&mut self) {
        desktop_notification_action_listener_count().fetch_sub(1, Ordering::AcqRel);
    }
}

fn desktop_notification_action_listener_count() -> &'static AtomicUsize {
    static COUNT: OnceLock<AtomicUsize> = OnceLock::new();
    COUNT.get_or_init(|| AtomicUsize::new(0))
}

fn reserve_desktop_notification_action_listener() -> Option<DesktopNotificationActionListenerToken>
{
    let count = desktop_notification_action_listener_count();
    let mut current = count.load(Ordering::Acquire);
    loop {
        if current >= DESKTOP_NOTIFICATION_ACTION_LISTENER_CAPACITY {
            return None;
        }
        match count.compare_exchange_weak(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return Some(DesktopNotificationActionListenerToken),
            Err(next) => current = next,
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_desktop_notification_action_listener(
    server_id: u32,
    args: Option<Vec<String>>,
    token: Option<DesktopNotificationActionListenerToken>,
) {
    let (Some(args), Some(token)) = (args, token) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("forktty-desktop-notification-action".to_string())
        .spawn(move || {
            let _token = token;
            let _ = notify_rust::handle_action(server_id, move |response| {
                if matches!(response, notify_rust::ActionResponse::Custom("default")) {
                    run_desktop_notification_context_action(args);
                }
            });
        });
}

#[cfg(any(not(unix), target_os = "macos"))]
fn spawn_desktop_notification_action_listener(
    _server_id: u32,
    _args: Option<Vec<String>>,
    _token: Option<DesktopNotificationActionListenerToken>,
) {
}

fn run_desktop_notification_context_action(args: Vec<String>) {
    let Ok(program) = std::env::current_exe() else {
        return;
    };
    let Ok(mut child) = std::process::Command::new(program).args(args).spawn() else {
        return;
    };
    let _ = child.wait();
}

fn write_desktop_notification_icon_file(
    notification: &NotificationItem,
    runtime_dir: &Path,
) -> Option<PathBuf> {
    let data = notification
        .terminal_metadata
        .as_ref()?
        .icon_data
        .as_deref()
        .filter(|data| !data.is_empty())?;
    let dir = runtime_dir.join("forktty-notification-icons");
    std::fs::create_dir_all(&dir).ok()?;
    #[cfg(unix)]
    {
        let _ = std::fs::set_permissions(&dir, Permissions::from_mode(0o700));
    }

    for _ in 0..4 {
        let extension = terminal_notification_icon_extension(data)?;
        let path = dir.join(format!("icon-{}.{}", uuid::Uuid::new_v4(), extension));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let Ok(mut file) = options.open(&path) else {
            continue;
        };
        if file.write_all(data).is_ok() {
            return Some(path);
        }
        let _ = std::fs::remove_file(path);
    }
    None
}

pub fn terminal_notification_icon_extension(data: &[u8]) -> Option<&'static str> {
    let (extension, width, height) = terminal_notification_icon_metadata(data)?;
    valid_terminal_notification_icon_dimensions(width, height).then_some(extension)
}

fn valid_terminal_notification_icon_dimensions(width: u32, height: u32) -> bool {
    width > 0
        && height > 0
        && width <= TERMINAL_ICON_MAX_DIMENSION
        && height <= TERMINAL_ICON_MAX_DIMENSION
        && width.saturating_mul(height) <= TERMINAL_ICON_MAX_PIXELS
}

fn terminal_notification_icon_metadata(data: &[u8]) -> Option<(&'static str, u32, u32)> {
    if data.len() >= 24 && data.starts_with(b"\x89PNG\r\n\x1a\n") && &data[12..16] == b"IHDR" {
        Some((
            "png",
            u32::from_be_bytes(data[16..20].try_into().ok()?),
            u32::from_be_bytes(data[20..24].try_into().ok()?),
        ))
    } else if data.len() >= 10 && (data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a")) {
        Some((
            "gif",
            u16::from_le_bytes(data[6..8].try_into().ok()?) as u32,
            u16::from_le_bytes(data[8..10].try_into().ok()?) as u32,
        ))
    } else if data.len() >= 30 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        webp_dimensions(data).map(|(width, height)| ("webp", width, height))
    } else if data.starts_with(b"\xff\xd8\xff") {
        jpeg_dimensions(data).map(|(width, height)| ("jpg", width, height))
    } else {
        None
    }
}

fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut index = 2;
    while index + 4 <= data.len() {
        if data[index] != 0xff {
            return None;
        }
        while index < data.len() && data[index] == 0xff {
            index += 1;
        }
        let marker = *data.get(index)?;
        index += 1;
        if marker == 0xd8 || marker == 0xd9 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        let segment_len = u16::from_be_bytes(data.get(index..index + 2)?.try_into().ok()?) as usize;
        if segment_len < 2 || index + segment_len > data.len() {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            if segment_len < 7 {
                return None;
            }
            let height =
                u16::from_be_bytes(data.get(index + 3..index + 5)?.try_into().ok()?) as u32;
            let width = u16::from_be_bytes(data.get(index + 5..index + 7)?.try_into().ok()?) as u32;
            return Some((width, height));
        }
        index += segment_len;
    }
    None
}

fn webp_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let chunk = data.get(12..16)?;
    match chunk {
        b"VP8X" if data.len() >= 30 => {
            let width = 1 + u32::from_le_bytes([data[24], data[25], data[26], 0]);
            let height = 1 + u32::from_le_bytes([data[27], data[28], data[29], 0]);
            Some((width, height))
        }
        b"VP8 " if data.len() >= 30 && data.get(23..26) == Some(b"\x9d\x01\x2a") => {
            let width = u16::from_le_bytes(data[26..28].try_into().ok()?) as u32 & 0x3fff;
            let height = u16::from_le_bytes(data[28..30].try_into().ok()?) as u32 & 0x3fff;
            Some((width, height))
        }
        b"VP8L" if data.len() >= 25 && data[20] == 0x2f => {
            let bits = u32::from_le_bytes(data[21..25].try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        _ => None,
    }
}

fn desktop_notification_runtime_dir() -> Option<PathBuf> {
    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    path.is_absolute().then_some(path)
}

fn remove_desktop_notification_icon_file(path: Option<PathBuf>) {
    if let Some(path) = path {
        let _ = std::fs::remove_file(path);
    }
}

struct DesktopNotificationEntry {
    id: String,
    handle: notify_rust::NotificationHandle,
    image_path: Option<PathBuf>,
}

fn desktop_notification_handles() -> &'static Mutex<VecDeque<DesktopNotificationEntry>> {
    static HANDLES: OnceLock<Mutex<VecDeque<DesktopNotificationEntry>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn desktop_notification_server_id(notification_id: &str) -> Option<u32> {
    let handles = desktop_notification_handles()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    handles
        .iter()
        .find(|entry| entry.id == notification_id)
        .map(|entry| entry.handle.id())
}

fn remember_desktop_notification_handle(
    notification_id: String,
    handle: notify_rust::NotificationHandle,
    image_path: Option<PathBuf>,
) {
    let mut handles = desktop_notification_handles()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(index) = handles.iter().position(|entry| entry.id == notification_id) {
        if let Some(entry) = handles.remove(index) {
            remove_desktop_notification_icon_file(entry.image_path);
        }
    }
    handles.push_back(DesktopNotificationEntry {
        id: notification_id,
        handle,
        image_path,
    });
    while handles.len() > DESKTOP_NOTIFICATION_HANDLE_CAPACITY {
        if let Some(entry) = handles.pop_front() {
            remove_desktop_notification_icon_file(entry.image_path);
        }
    }
}

fn take_desktop_notification_handle(
    notification_id: &str,
) -> Option<notify_rust::NotificationHandle> {
    let mut handles = desktop_notification_handles()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let index = handles
        .iter()
        .position(|entry| entry.id == notification_id)?;
    handles.remove(index).map(|entry| {
        remove_desktop_notification_icon_file(entry.image_path);
        entry.handle
    })
}

fn desktop_notification_icon_name(notification: &NotificationItem) -> &str {
    let Some(metadata) = notification.terminal_metadata.as_ref() else {
        return "forktty";
    };
    if let Some(name) = metadata.icon_names.first() {
        match name.as_str() {
            "error" => "dialog-error",
            "warn" | "warning" => "dialog-warning",
            "info" => "dialog-information",
            "question" => "dialog-question",
            "help" => "help-browser",
            "file-manager" => "system-file-manager",
            "system-monitor" => "utilities-system-monitor",
            "text-editor" => "accessories-text-editor",
            other => other,
        }
    } else {
        metadata.app_name.as_deref().unwrap_or("forktty")
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply_terminal_notification_metadata_hints(
    notification: &mut notify_rust::Notification,
    item: &NotificationItem,
    play_sound: bool,
) {
    use notify_rust::{Hint, Urgency};

    let metadata = item.terminal_metadata.as_ref();
    if let Some(urgency) = metadata.and_then(|metadata| match metadata.urgency {
        Some(0) => Some(Urgency::Low),
        Some(1) => Some(Urgency::Normal),
        Some(2) => Some(Urgency::Critical),
        _ => None,
    }) {
        notification.urgency(urgency);
    }
    if let Some(timeout) = metadata.and_then(|metadata| metadata.expires_after_ms) {
        notification.timeout(timeout);
    }

    match metadata.and_then(|metadata| metadata.sound_name.as_deref()) {
        Some("silent") => {
            notification.hint(Hint::SuppressSound(true));
        }
        Some(name) if play_sound => {
            notification.sound_name(name);
        }
        _ if !play_sound => {
            notification.hint(Hint::SuppressSound(true));
        }
        _ => {}
    }
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
    if is_shell_trampoline(program, args) {
        return Err("notification_command must not invoke a shell with -c".to_string());
    }
    if !is_executable_file(program_path) {
        return Err(format!(
            "notification_command must start with an absolute executable file: {program}"
        ));
    }

    let mut command = std::process::Command::new(program);
    command.args(args);
    for (key, value) in custom_command_env(notification) {
        command.env(key, value);
    }
    let child = command.spawn().map_err(|err| err.to_string())?;

    enqueue_custom_command_child(custom_command_reaper(), child)
}

fn custom_command_env(notification: &NotificationItem) -> Vec<(&'static str, String)> {
    let metadata = notification.terminal_metadata.as_ref();
    vec![
        ("FORKTTY_NOTIFICATION_ID", notification.id.clone()),
        ("FORKTTY_NOTIFICATION_TITLE", notification.title.clone()),
        ("FORKTTY_NOTIFICATION_BODY", notification.body.clone()),
        (
            "FORKTTY_NOTIFICATION_KIND",
            notification_kind_name(notification).to_string(),
        ),
        (
            "FORKTTY_NOTIFICATION_WORKSPACE_ID",
            notification.workspace_id.clone().unwrap_or_default(),
        ),
        (
            "FORKTTY_NOTIFICATION_SURFACE_ID",
            notification.surface_id.clone().unwrap_or_default(),
        ),
        (
            "FORKTTY_NOTIFICATION_TERMINAL_APP",
            metadata
                .and_then(|metadata| metadata.app_name.clone())
                .unwrap_or_default(),
        ),
        (
            "FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON",
            metadata
                .map(|metadata| {
                    serde_json::to_string(&metadata.notification_types).unwrap_or_default()
                })
                .unwrap_or_else(|| "[]".to_string()),
        ),
    ]
}

fn custom_command_reaper() -> &'static mpsc::SyncSender<Child> {
    static REAPER: OnceLock<mpsc::SyncSender<Child>> = OnceLock::new();
    REAPER.get_or_init(|| {
        let (sender, receiver) = mpsc::sync_channel::<Child>(CUSTOM_COMMAND_REAPER_QUEUE_CAPACITY);
        if let Err(err) = std::thread::Builder::new()
            .name("forktty-notification-command-reaper".to_string())
            .spawn(move || {
                for mut child in receiver {
                    let _ = child.wait();
                }
            })
        {
            eprintln!("Failed to start notification command reaper: {err}");
        }
        sender
    })
}

fn enqueue_custom_command_child(
    sender: &mpsc::SyncSender<Child>,
    child: Child,
) -> Result<(), String> {
    match sender.try_send(child) {
        Ok(()) => Ok(()),
        Err(mpsc::TrySendError::Full(mut child)) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("notification_command wait queue is full".to_string())
        }
        Err(mpsc::TrySendError::Disconnected(mut child)) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("notification_command waiter is unavailable".to_string())
        }
    }
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

    const ONE_BY_ONE_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 1, 3,
        0, 0, 0, 37, 219, 86, 202, 0, 0, 0, 32, 99, 72, 82, 77, 0, 0, 122, 38, 0, 0, 128, 132, 0,
        0, 250, 0, 0, 0, 128, 232, 0, 0, 117, 48, 0, 0, 234, 96, 0, 0, 58, 152, 0, 0, 23, 112, 156,
        186, 81, 60, 0, 0, 0, 6, 80, 76, 84, 69, 255, 0, 0, 255, 255, 255, 65, 29, 52, 17, 0, 0, 0,
        1, 98, 75, 71, 68, 1, 255, 2, 45, 222, 0, 0, 0, 7, 116, 73, 77, 69, 7, 234, 6, 16, 21, 59,
        23, 1, 222, 59, 31, 0, 0, 0, 37, 116, 69, 88, 116, 100, 97, 116, 101, 58, 99, 114, 101, 97,
        116, 101, 0, 50, 48, 50, 54, 45, 48, 54, 45, 49, 54, 84, 50, 49, 58, 53, 57, 58, 50, 51,
        43, 48, 48, 58, 48, 48, 245, 241, 18, 134, 0, 0, 0, 37, 116, 69, 88, 116, 100, 97, 116,
        101, 58, 109, 111, 100, 105, 102, 121, 0, 50, 48, 50, 54, 45, 48, 54, 45, 49, 54, 84, 50,
        49, 58, 53, 57, 58, 50, 51, 43, 48, 48, 58, 48, 48, 132, 172, 170, 58, 0, 0, 0, 40, 116,
        69, 88, 116, 100, 97, 116, 101, 58, 116, 105, 109, 101, 115, 116, 97, 109, 112, 0, 50, 48,
        50, 54, 45, 48, 54, 45, 49, 54, 84, 50, 49, 58, 53, 57, 58, 50, 51, 43, 48, 48, 58, 48, 48,
        211, 185, 139, 229, 0, 0, 0, 10, 73, 68, 65, 84, 8, 215, 99, 96, 0, 0, 0, 2, 0, 1, 226, 33,
        188, 51, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    use crate::{NotificationKind, WorkspaceModel};

    #[test]
    fn desktop_entry_hint_matches_packaged_desktop_id() {
        assert_eq!(DESKTOP_ENTRY_ID, "dev.forktty.forktty");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn targeted_desktop_notifications_expose_default_open_action() {
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some("workspace-1".to_string()),
            Some("surface-1".to_string()),
        );
        let mut desktop = notify_rust::Notification::new();

        assert!(desktop_notification_context_action_args(&notification).is_some());
        apply_desktop_notification_context_action(&mut desktop, true);

        assert_eq!(
            desktop.actions,
            vec!["default".to_string(), "Open".to_string()]
        );
    }

    #[test]
    fn desktop_notification_action_args_focus_surface_or_workspace() {
        let mut model = WorkspaceModel::new();
        let surface_notification = model.create_notification(
            "Prompt",
            "Ready",
            NotificationKind::Prompt,
            Some("workspace-1".to_string()),
            Some("surface-1".to_string()),
        );
        let workspace_notification = model.create_notification(
            "Workspace",
            "Ready",
            NotificationKind::Info,
            Some("workspace-1".to_string()),
            None,
        );
        let global_notification =
            model.create_notification("Global", "Ready", NotificationKind::Info, None, None);

        assert_eq!(
            desktop_notification_context_action_args(&surface_notification),
            Some(vec!["focus-surface".to_string(), "surface-1".to_string()])
        );
        assert_eq!(
            desktop_notification_context_action_args(&workspace_notification),
            Some(vec![
                "focus".to_string(),
                "--workspace-id".to_string(),
                "workspace-1".to_string()
            ])
        );
        assert_eq!(
            desktop_notification_context_action_args(&global_notification),
            None
        );
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

    #[cfg(unix)]
    #[test]
    fn rejects_shell_trampoline_custom_command_after_option_value() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let bash = dir.path().join("bash");
        std::fs::write(&bash, "").unwrap();
        let mut permissions = std::fs::metadata(&bash).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&bash, permissions).unwrap();

        let mut config = AppConfig::default();
        config.notifications.desktop = false;
        config.general.notification_command = format!("{} -o vi -c notify-send", bash.display());
        let mut model = WorkspaceModel::new();
        let notification = model.create_notification(
            "trampoline-option-test",
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
    fn terminal_notification_icon_names_override_default_desktop_icon() {
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: vec!["warning".to_string()],
                    icon_data: None,
                    icon_cache_id: None,
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: None,
                    notification_types: Vec::new(),
                }),
            )
            .unwrap();

        assert_eq!(
            desktop_notification_icon_name(&notification),
            "dialog-warning"
        );
    }

    #[test]
    fn terminal_notification_app_name_falls_back_to_desktop_icon() {
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: None,
                    icon_cache_id: None,
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: Some("make".to_string()),
                    notification_types: Vec::new(),
                }),
            )
            .unwrap();

        assert_eq!(desktop_notification_icon_name(&notification), "make");
    }

    #[test]
    fn terminal_notification_icon_data_writes_desktop_image_under_runtime_dir() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: Some(ONE_BY_ONE_PNG.to_vec()),
                    icon_cache_id: Some("icon-1".to_string()),
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: None,
                    notification_types: Vec::new(),
                }),
            )
            .unwrap();

        let path = write_desktop_notification_icon_file(&notification, dir.path()).unwrap();

        assert!(path.starts_with(dir.path().join("forktty-notification-icons")));
        assert_eq!(std::fs::read(&path).unwrap(), ONE_BY_ONE_PNG);
    }

    #[test]
    fn terminal_notification_icon_data_rejects_oversized_png_dimensions() {
        let mut png = ONE_BY_ONE_PNG.to_vec();
        png[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&20_000u32.to_be_bytes());

        assert_eq!(terminal_notification_icon_extension(&png), None);
    }

    #[test]
    fn terminal_notification_icon_data_ignores_unknown_image_format() {
        let dir = tempfile::tempdir().unwrap();
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: Some(b"not an image".to_vec()),
                    icon_cache_id: Some("icon-1".to_string()),
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: None,
                    notification_types: Vec::new(),
                }),
            )
            .unwrap();

        assert!(write_desktop_notification_icon_file(&notification, dir.path()).is_none());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn terminal_notification_metadata_sets_desktop_urgency_sound_and_timeout() {
        use notify_rust::{Hint, Timeout, Urgency};

        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: None,
                    icon_cache_id: None,
                    urgency: Some(2),
                    sound_name: Some("message-new-instant".to_string()),
                    expires_after_ms: Some(5000),
                    app_name: None,
                    notification_types: Vec::new(),
                }),
            )
            .unwrap();
        let mut desktop = notify_rust::Notification::new();

        apply_terminal_notification_metadata_hints(&mut desktop, &notification, true);

        assert!(desktop.hints.contains(&Hint::Urgency(Urgency::Critical)));
        assert!(desktop
            .hints
            .contains(&Hint::SoundName("message-new-instant".to_string())));
        assert_eq!(desktop.timeout, Timeout::Milliseconds(5000));
    }

    #[test]
    fn terminal_notification_filter_metadata_is_exposed_to_custom_command_env() {
        let mut model = WorkspaceModel::new();
        let notification =
            model.create_notification("Title", "Body", NotificationKind::Info, None, None);
        let notification = model
            .set_notification_terminal_metadata(
                &notification.id,
                Some(crate::TerminalNotificationMetadata {
                    id: "build".to_string(),
                    report_activation: false,
                    report_close: false,
                    buttons: Vec::new(),
                    icon_names: Vec::new(),
                    icon_data: None,
                    icon_cache_id: None,
                    urgency: None,
                    sound_name: None,
                    expires_after_ms: None,
                    app_name: Some("make".to_string()),
                    notification_types: vec!["build.error".to_string(), "ci".to_string()],
                }),
            )
            .unwrap();
        let env = custom_command_env(&notification)
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(
            env.get("FORKTTY_NOTIFICATION_TERMINAL_APP")
                .map(String::as_str),
            Some("make")
        );
        assert_eq!(
            env.get("FORKTTY_NOTIFICATION_TERMINAL_TYPES_JSON")
                .map(String::as_str),
            Some("[\"build.error\",\"ci\"]")
        );
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

    #[test]
    #[cfg(unix)]
    fn custom_command_child_is_killed_when_reaper_queue_is_full() {
        if !Path::new("/bin/sleep").exists() {
            return;
        }
        let (sender, _receiver) = mpsc::sync_channel(0);
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();

        let error = enqueue_custom_command_child(&sender, child).unwrap_err();

        assert!(error.contains("wait queue is full"));
    }
}
