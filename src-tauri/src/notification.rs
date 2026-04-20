/// Send a desktop notification via notify-rust (XDG/D-Bus).
pub fn send_desktop(title: &str, body: &str, play_sound: bool) -> Result<(), String> {
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
            .hint(Hint::DesktopEntry("forktty".to_string()))
            .hint(Hint::Category("im.received".to_string()));

        if !play_sound {
            notification.hint(Hint::SuppressSound(true));
        }
    }

    notification.show().map_err(|e| e.to_string())?;
    Ok(())
}

/// Run a custom notification command with env vars.
/// Uses argv splitting instead of sh -c to prevent command injection.
pub(crate) fn run_custom_command(command: &str, title: &str, body: &str) -> Result<(), String> {
    if command.is_empty() {
        return Ok(());
    }

    let parts = shell_words::split(command).map_err(|e| e.to_string())?;
    let (prog, args) = parts.split_first().ok_or("Empty command")?;
    let prog_path = std::path::Path::new(prog);
    if !prog_path.is_absolute() || !prog_path.exists() {
        return Err(format!(
            "notification_command must be an absolute path to an existing file: {prog}"
        ));
    }
    let mut child = std::process::Command::new(prog)
        .args(args)
        .env("FORKTTY_NOTIFICATION_TITLE", title)
        .env("FORKTTY_NOTIFICATION_BODY", body)
        .spawn()
        .map_err(|e| e.to_string())?;

    // Reap the child in a background thread to avoid leaving zombies.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Test 1: run_custom_command injection prevention ---

    #[test]
    fn empty_command_returns_ok() {
        let result = run_custom_command("", "title", "body");
        assert!(result.is_ok());
    }

    #[test]
    fn relative_path_returns_err_with_absolute_message() {
        let result = run_custom_command("notify-send", "title", "body");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("absolute path"),
            "Error should mention 'absolute path', got: {err}"
        );
    }

    #[test]
    fn nonexistent_absolute_path_returns_err() {
        let result = run_custom_command(
            "/nonexistent/path/to/binary_that_does_not_exist",
            "title",
            "body",
        );
        assert!(result.is_err());
    }

    #[test]
    fn valid_absolute_path_is_accepted() {
        // /bin/true exists on all Linux systems and exits with 0
        let result = run_custom_command("/bin/true", "title", "body");
        assert!(result.is_ok(), "Expected Ok for /bin/true, got: {result:?}");
    }

    // --- Test 5: run_custom_command with valid absolute paths ---

    #[test]
    fn bin_true_with_title_and_body_succeeds() {
        let result = run_custom_command("/bin/true", "Test Title", "Test Body");
        assert!(
            result.is_ok(),
            "Expected Ok for /bin/true with title/body, got: {result:?}"
        );
    }

    #[test]
    fn command_with_arguments_works() {
        // "/bin/echo hello" should split into prog="/bin/echo", args=["hello"]
        let result = run_custom_command("/bin/echo hello", "title", "body");
        assert!(
            result.is_ok(),
            "Expected Ok for '/bin/echo hello', got: {result:?}"
        );
    }

    #[test]
    fn quoted_arguments_are_parsed_correctly() {
        let result = run_custom_command("/bin/echo 'hello world'", "title", "body");
        assert!(
            result.is_ok(),
            "Expected quoted args to be parsed correctly, got: {result:?}"
        );
    }
}
