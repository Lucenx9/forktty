# Privacy Notice

## Data Collection

ForkTTY does not collect, transmit, sell, or process personal data through any external service. It has no telemetry, analytics, crash reporting, update checks, or product network calls.

## Network Activity

ForkTTY makes no telemetry, analytics, crash-reporting, update-check, or
product-service network connections.

Optional features can make user-directed network requests:

- Browser panes built with the `browser` feature load URLs opened by the
  user or by local socket automation.
- Linked PR lookup, when enabled in settings, shells out to the local
  `gh` CLI for the current git branch.

The automation interface is a local Unix domain socket, normally at:

```text
$XDG_RUNTIME_DIR/forktty.sock
```

The socket is intended for same-machine, same-user automation and is removed on exit when possible.

## Locally Stored Files

ForkTTY stores local application data only:

| File | Location | Purpose |
| ---- | -------- | ------- |
| Configuration | `~/.config/forktty/config.toml` | Theme, font, shell, scrollback, terminal bell, worktree, notification, sidebar, renderer, and window settings |
| Native session data | `~/.local/share/forktty/session-v2.json` | Workspace layout and metadata needed for GTK/Ghostty session restore |
| Legacy session import | `~/.local/share/forktty/session.json` | Imported if present; native saves do not overwrite it |
| Quarantined sessions | `~/.local/share/forktty/session-v2.json.bad-*` | Invalid/corrupt session files kept for debugging |
| Browser profiles | `~/.local/share/forktty/browser_profiles/profiles.json` | Browser profile names and IDs for the optional browser feature |
| Browser profile data | `~/.local/share/forktty/browser_profiles/<id>/` | Optional WebKit data/cache/cookies plus ForkTTY history/bookmark stores for that profile |
| Logs | `~/.local/share/forktty/logs/` | Local structured logs for debugging |
| IPC socket | `$XDG_RUNTIME_DIR/forktty.sock` | Ephemeral local socket for automation |

These files may contain local paths, usernames embedded in paths, workspace names, branch names, notification text derived from terminal output, and browser-profile URLs/cookies/history/bookmarks when the optional browser feature is used. They remain on your machine unless you share them.

## Desktop Notifications

When desktop notifications are enabled, ForkTTY sends notification title/body text to the local desktop notification service through `notify-rust` / XDG D-Bus. Notification content can include terminal output or agent prompt text.

If `notification_command` is configured, ForkTTY runs the configured local executable and provides the notification payload through:

- `FORKTTY_NOTIFICATION_TITLE`
- `FORKTTY_NOTIFICATION_BODY`

Use a custom notification command only if you trust that executable with local terminal notification content.

## Third-Party Components

ForkTTY uses GTK4/libadwaita and Ghostty from the local Linux system to render the window and terminal widgets. Source builds with the optional browser feature also use WebKitGTK6 to render pages the user opens.

The ForkTTY socket CLI talks only to the local Unix socket.

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
