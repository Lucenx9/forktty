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
/// Uses argv splitting instead of sh -c to prevent command injection.
pub fn run_custom_command(command: &str, title: &str, body: &str) -> Result<(), String> {
    if command.is_empty() {
        return Ok(());
    }

    let parts: Vec<&str> = command.split_whitespace().collect();
    let (prog, args) = parts.split_first().ok_or("Empty command")?;
    let prog_path = std::path::Path::new(prog);
    if !prog_path.is_absolute() || !prog_path.exists() {
        return Err(format!(
            "notification_command must be an absolute path to an existing file: {prog}"
        ));
    }
    std::process::Command::new(prog)
        .args(args)
        .env("FORKTTY_NOTIFICATION_TITLE", title)
        .env("FORKTTY_NOTIFICATION_BODY", body)
        .spawn()
        .map_err(|e| e.to_string())?;

    Ok(())
}
