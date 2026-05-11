# Privacy Notice

## Data Collection

ForkTTY does not collect, transmit, sell, or process personal data through any external service. It has no telemetry, analytics, crash reporting, update checks, or product network calls.

## Network Activity

ForkTTY makes no external network connections.

The automation interface is a local Unix domain socket, normally at:

```text
$XDG_RUNTIME_DIR/forktty.sock
```

The socket is intended for same-machine, same-user automation and is removed on exit.

## Locally Stored Files

ForkTTY stores local application data only:

| File | Location | Purpose |
| ---- | -------- | ------- |
| Configuration | `~/.config/forktty/config.toml` | Theme, font, shell, worktree, and notification settings |
| Session data | `~/.local/share/forktty/session.json` | Workspace layout and metadata needed for session restore |
| Quarantined sessions | `~/.local/share/forktty/session.json.bad-*` | Invalid/corrupt session files kept for debugging |
| Logs | `~/.local/share/forktty/logs/` | Local structured logs for debugging |
| IPC socket | `$XDG_RUNTIME_DIR/forktty.sock` | Ephemeral local socket for automation |

These files may contain local paths, usernames embedded in paths, workspace names, branch names, and notification text derived from terminal output. They remain on your machine unless you share them.

## Desktop Notifications

When desktop notifications are enabled, ForkTTY sends notification title/body text to the local desktop notification service through `notify-rust` / XDG D-Bus. Notification content can include terminal output or agent prompt text.

If `notification_command` is configured, ForkTTY runs the configured local executable and provides the notification payload through:

- `FORKTTY_NOTIFICATION_TITLE`
- `FORKTTY_NOTIFICATION_BODY`

Use a custom notification command only if you trust that executable with local terminal notification content.

## Third-Party Components

ForkTTY uses WebKitGTK, the system WebView on Linux, to render the UI. ForkTTY's Content Security Policy restricts the app to local content and does not load external URLs.

## How to Delete Local Data

```bash
rm -rf ~/.config/forktty
rm -rf ~/.local/share/forktty
```

The runtime socket is ephemeral and normally lives under `$XDG_RUNTIME_DIR`.

## EU/GDPR Note

ForkTTY is local-only software with no external data collection by the application. You control any data stored on your machine and any local scripts you configure.

## Contact

For privacy questions, open an issue at https://github.com/Lucenx9/forktty/issues or use [GitHub private vulnerability reporting](https://github.com/Lucenx9/forktty/security/advisories/new).
