/// Send a desktop notification via notify-rust (XDG/D-Bus).
pub fn send_desktop(title: &str, body: &str) -> Result<(), String> {
    notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .icon("dialog-information")
        .appname("ForkTTY")
        .show()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Run a custom notification command with env vars.
pub fn run_custom_command(command: &str, title: &str, body: &str) -> Result<(), String> {
    if command.is_empty() {
        return Ok(());
    }

    std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("FORKTTY_NOTIFICATION_TITLE", title)
        .env("FORKTTY_NOTIFICATION_BODY", body)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}
