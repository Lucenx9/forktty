# Security Policy

## Supported Versions

| Version | Supported |
| ------- | --------- |
| 0.2.0-alpha.x | Yes |
| 0.1.x | No |

## Reporting a Vulnerability

Please report vulnerabilities responsibly:

1. Do not open a public GitHub issue for security vulnerabilities.
2. Email the maintainer directly or use [GitHub private vulnerability reporting](https://github.com/Lucenx9/forktty/security/advisories/new).
3. Include reproduction steps, impact, affected version/commit, and any suggested fix.

We aim to acknowledge reports within 48 hours and provide a fix within 7 days for critical issues when feasible.

## Security Model

ForkTTY is a Linux-only local desktop application. Its primary trust boundary is the current user account.

Assumptions:

- The user running ForkTTY is trusted.
- The local user account and filesystem are not already compromised.
- Same-user local processes may interact with user-owned runtime resources.
- User-authored hooks and notification commands execute with the user's privileges.

ForkTTY makes no telemetry or product-service network calls and does not treat the local Unix socket as a remote security boundary. Optional update checks query GitHub Releases at most once per day and can be disabled. Optional browser panes load user-requested URLs, and optional PR lookup delegates to the local `gh` CLI.

## Security Boundaries

| Component | Current protection |
| --------- | ------------------ |
| Unix socket | Defaults to `$XDG_RUNTIME_DIR/forktty.sock`; fallback is `/tmp/forktty-<uid>/forktty.sock`; parent directory and socket permissions are owner-only; request lines are capped at 1 MiB; live ForkTTY sockets are probed before stale cleanup. |
| ForkTTY hook spawn | Configured shell must be an absolute executable file. Spawned shells receive controlled terminal identity/color variables (`TERM`, `COLORTERM`, `TERM_PROGRAM`, `TERM_PROGRAM_VERSION`) plus ForkTTY socket/workspace targeting variables. User-provided extra environment cannot override reserved terminal or `FORKTTY_*` keys. |
| ForkTTY config | `~/.config/forktty/config.toml` must be a regular file and no larger than 1 MiB before parsing. Saved config validates shell, font size, sidebar position, renderer, window mode, worktree layout, and notification command. |
| Session restore | `session-v2.json` is size-bounded, regular-file checked, structurally validated, and quarantined on invalid input. |
| Notification command | Empty disables it. If set, it is parsed with `shell_words`; the first token must be an absolute executable file; argv execution is used; no `sh -c`; title/body are passed through environment variables. |
| AppImage updater | Uses HTTPS GitHub release assets only, requires `SHA256SUMS`, writes the new AppImage to a temp file in the target directory, verifies SHA256 before chmod/rename, fsyncs file and parent directory, and never invokes a shell. Non-AppImage, extracted, read-only, or unsafe AppImage paths fall back to opening the release page. |
| Desktop notifications | Sent through `notify-rust` / XDG D-Bus. Notification text can include local terminal output. |
| Worktree names | Rejects empty names, `/`, `\`, `..`, and NUL. |
| Worktree paths | Canonicalized and verified against Git repository boundaries before sensitive operations. |
| Worktree removal | Rejects dirty or tampered linked worktrees before deletion. |
| Worktree merge | Rejects dirty target checkouts and merge conflicts before completing the operation. |
| Worktree hooks | Only `.forktty/setup` and `.forktty/teardown` are executed; hook paths are canonicalized and must remain inside the worktree; execution uses argv, not `sh -c`. |
| Logs | Newlines are sanitized before writing structured log lines. |
| Browser profiles | Profile IDs are UUID-backed and validated before becoming directory names under `~/.local/share/forktty/browser_profiles/`; profile metadata writes are atomic. |
| Browser scripting | `browser.snapshot`/`click`/`fill`/`eval` are local socket operations. `browser.eval` runs caller-provided JavaScript in the addressed WebKit page, so it is inside the same-user automation trust boundary. |

## Notification Command Details

`notification_command` supports static argv arguments:

```toml
[general]
notification_command = "/usr/bin/notify-send --app-name ForkTTY"
```

ForkTTY executes the first token directly. It does not invoke a shell, so pipes, redirects, `&&`, command substitution, globbing, and environment variable expansion are not interpreted.

The payload is provided through:

- `FORKTTY_NOTIFICATION_TITLE`
- `FORKTTY_NOTIFICATION_BODY`

Do not point `notification_command` at a script unless you trust that script with local terminal output.

## Residual Risks

- Same-user local processes remain inside the local trust model and may interact with user-owned files or sockets.
- Terminal output can include sensitive text. Desktop notifications or custom notification commands may expose that text to local desktop services or configured executables.
- User-created `.forktty/setup` and `.forktty/teardown` hooks run with the user's privileges.
- Worktree operations modify local Git checkouts and branches; users should review changes before merging or removing worktrees.
- Session files and logs can contain local paths, branch names, workspace names, and notification text.
- Optional browser profile data can contain cookies, cache, localStorage, history, bookmarks, and URLs for pages the user opens.
- Embedded Ghostty owns the child PTY and terminal escape handling for current GTK panes; ForkTTY treats terminal output as local, user-owned data and keeps socket/config/session boundaries size-limited and owner-scoped.
- AppImage checksum verification protects transport/integrity errors, not compromise of the GitHub repository, release account, or checksum asset. Detached release signing is not implemented yet.

## Dependencies and CI

- Rust dependencies are audited with `cargo audit` in CI.
- CodeQL and GitHub dependency review are enabled.
- Dependabot is configured for Cargo and GitHub Actions updates.

Security automation reduces risk but does not replace manual review of privileged local execution paths.
