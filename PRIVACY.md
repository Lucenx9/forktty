# Privacy Notice

## Data Collection

**ForkTTY does not collect, transmit, or process any personal data.** All data remains entirely on your local machine. There is no telemetry, no analytics, no crash reporting, no update checking, and no network connections to external servers.

## Locally Stored Files

ForkTTY creates the following files on your machine:

| File | Location | Purpose |
|------|----------|---------|
| Configuration | `~/.config/forktty/config.toml` | User preferences (theme, font, shell, notification settings) |
| Session data | `~/.local/share/forktty/session.json` | Workspace layout for session restore on restart |
| Application logs | `~/.local/share/forktty/logs/` | Structured logs for debugging (one file per day) |
| IPC socket | `$XDG_RUNTIME_DIR/forktty.sock` | Ephemeral Unix socket for CLI communication (deleted on exit) |

These files may contain filesystem paths that include your username (e.g., `/home/yourname/project`). This data never leaves your machine.

## How to Delete Your Data

To remove all ForkTTY data from your machine:

```bash
rm -rf ~/.config/forktty
rm -rf ~/.local/share/forktty
```

## Third-Party Components

ForkTTY uses the system WebView (WebKitGTK on Linux) to render its UI. WebKitGTK is a system component maintained by the GNOME project and does not collect telemetry. ForkTTY's Content Security Policy restricts the WebView to loading local content only — no external URLs are ever loaded.

## Network Activity

ForkTTY makes **zero network connections**. The only IPC mechanism is a local Unix domain socket restricted to the current user (permissions `0600`).

## EU/GDPR Compliance

As a local-only desktop application with no data collection, ForkTTY falls outside the scope of GDPR data processing obligations. You are the sole controller of any data stored on your machine.

## Contact

If you have privacy questions, open an issue at https://github.com/Lucenx9/forktty/issues or use [GitHub's private reporting](https://github.com/Lucenx9/forktty/security/advisories/new).
