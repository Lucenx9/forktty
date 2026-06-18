# Release QA Checklist

Use this before tagging an alpha release. The goal is to catch GTK/Ghostty and
package regressions that unit tests cannot see.

## Automated Checks

```bash
cargo fmt --all --check
git submodule update --init vendor/ghostty
cargo run -p xtask -- check
scripts/ghostty-gtk-lib-probe.sh --ensure --print-path
cargo test --workspace --all-targets --no-default-features --features gtk-ghostty
cargo clippy --workspace --all-targets --no-default-features --features gtk-ghostty -- -D warnings
cargo build -p forktty-ui-gtk --no-default-features --features gtk-ghostty
scripts/gtk-ghostty-smoke.sh
cargo test -p forktty-ui-gtk --all-targets --no-default-features --features browser
desktop-file-validate packaging/linux/dev.forktty.forktty.desktop
bash scripts/build-deb.sh
bash scripts/build-appimage.sh
```

### Already covered by automated tests

The following release checks are now covered by headless Rust tests (core/socket
or CLI parser tests), so they should be treated as regressions if they fail in CI:

- socket CLI option parsing (`--socket` placement, missing values, unknown flags,
  selector ambiguity, and unexpected positional arguments)
- socket payload and parameter validation (`payload_too_large`, invalid enum/string
  fields, and ambiguous selectors)
- session/config quarantine and recovery paths (broken JSON, oversized payloads,
  broken symlinks, and invalid workspace/surface invariants)
- workspace model invariants around split/focus/close/restore and metadata cleanup
  after workspace removal
- hook installer safety paths (dry-run semantics, symlink handling, invalid JSON,
  and backup behavior)
- worktree `.forktty/setup` hook failure visibility: failure stays non-fatal but
  is now returned as `WorktreeInfo.setup_warning`, included in the
  `worktree.create`/`worktree.attach` socket responses as `setup_warning`, and
  surfaced as a `Worktree Setup Hook Failed` error notification (core + socket
  tests)

Manual QA below should focus on runtime integration that headless tests cannot
fully prove (GTK/Ghostty lifecycle, desktop environment integration, and
packaging/runtime service availability). Browser panes are source-only and
experimental in this alpha; they have a separate opt-in smoke section.

`scripts/gtk-ghostty-smoke.sh` runs a short GTK/Ghostty launch in a fresh
DBus session against isolated config/data/state/socket paths under
`$XDG_RUNTIME_DIR`, and verifies socket ping, surface listing, terminal
input/readback, tab create/select/close, runtime zoom reflow/reset, GTK action
split/focus behavior, socket split readback, live pane close, restart with
scrollback restore, and the socket notification create/list/clear flow. The
temporary config disables desktop notifications and telemetry so the smoke does
not depend on host notification services. It uses the current display or
`xvfb-run` when available.

## Manual Runtime Smoke

- Start from a clean config/session directory.
- Launch with `cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty`.
- Confirm the app opens a usable terminal in the current directory.
- Relaunch from a clean config with an invalid shell environment (`SHELL=relative-shell cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty`) and confirm ForkTTY falls back to a usable absolute shell instead of opening a dead pane.
- Split right and split down until at least three panes exist.
- Move focus between panes with keyboard shortcuts and pointer clicks.
- Copy and paste with `Ctrl+Shift+C` / `Ctrl+Shift+V`.
- Click an unfocused terminal pane and immediately drag; confirm the same click focuses the pane and starts selection instead of being swallowed.
- In a mouse-tracking app such as `vim` or `htop`, confirm normal drag reaches the app, then Shift+drag selects terminal text locally.
- Start a terminal text selection, wheel/touchpad-scroll before releasing, and confirm the released selection still covers the intended text and copies correctly.
- Drag a selection above and below the terminal viewport edge long enough to trigger autoscroll; confirm the highlight tracks the text and the final copied text matches the selected range.
- Open the terminal context menu in a small split pane and use Paste.
- If GTK cannot spawn the target workspace terminal while switching workspaces, it shows a Workspace Switch Failed notification and keeps the previous workspace active.
- Close one pane and confirm focus moves to a remaining pane.
- Close the focused pane while a long-running shell command is exiting; confirm only one "Terminal exited" notification is created and it does not reference the just-closed surface.
- If GTK cannot spawn the replacement terminal while closing the only pane in a workspace, it shows a Close Pane Failed notification and keeps the old pane and terminal visible.
- Close the last workspace from the GTK UI and confirm a replacement `main` workspace opens a live terminal in the same directory.
- If GTK cannot spawn the replacement terminal while closing the last workspace, it shows a Close Workspace Failed notification and keeps the old workspace and terminal visible.
- If GTK cannot close a workspace's terminal surface, it shows a Close Workspace Failed notification and leaves the workspace visible instead of dropping it from the sidebar.
- Toggle the sidebar twice and confirm `~/.config/forktty/config.toml` remains valid TOML with no `config.toml.tmp-*` sibling left behind.
- Symlink `~/.config/forktty/config.toml` to a real managed TOML file, toggle
  the sidebar, and confirm the target updates while the symlink remains a symlink.
- Restart the app and confirm workspace/pane layout restores.
- Enter an invalid shell path in Settings and apply it: the Shell command row is marked with a red error state, an error toast names the validation failure, and the config file keeps the previous shell (the invalid value is never saved).
- Open the Settings font family picker and confirm the dropdown lists installed monospace fonts with a working search field; the font size and scrollback spin rows show theme-independent −/+ glyph buttons.
- Open Notifications, dismiss one notification, then Clear All.

## Config Recovery Smoke

- Stop the app, then corrupt `$XDG_CONFIG_HOME/forktty/config.toml` (`echo "{ broken" >$XDG_CONFIG_HOME/forktty/config.toml`). Relaunch; ForkTTY should start with defaults, show a Config Issue notification that names the quarantined file, and rename the corrupt config to `*.bad-<timestamp>` or `*.bad-<timestamp>-N` if that name already exists.
- Replace `$XDG_CONFIG_HOME/forktty/config.toml` with a broken symlink, then relaunch; ForkTTY should start with defaults and rename the symlink aside instead of repeatedly treating it as a missing config.
- Replace `$XDG_CONFIG_HOME/forktty/config.toml` with a directory, then relaunch; ForkTTY should start with defaults and rename the directory aside instead of warning on every launch.
- Hand-edit enum values with harmless whitespace or case, for example `worktree_layout = " SIBLING "`; relaunch should use `sibling` instead of resetting to `nested`.

## Socket API Smoke

Run these after starting the GTK app so the daemon is listening on the
default socket. Useful for catching protocol regressions without
rebuilding.

- Before starting the app, run `forktty ping` and confirm the error names the socket path and suggests launching ForkTTY, an absolute `FORKTTY_SOCKET_PATH`, or `--socket <path>`.
- Launch with `FORKTTY_SOCKET_PATH=" "` and confirm the app still binds the default socket path instead of disabling automation.
- Launch with `FORKTTY_SOCKET_PATH=relative.sock` and confirm both the app and
  `forktty ping` ignore the relative env value and use the default socket path.
- Run both `forktty --socket <path> ping` and `forktty ping --socket <path>` against a stub socket; both forms should use the supplied socket path.
- `forktty ping --socket=` — exits with `--socket requires a value` instead of trying to connect to an empty path.
- Start a stub Unix socket at the default path that replies `{"id":"other","ok":true,"result":"pong"}` to `system.ping`, then launch ForkTTY; startup should treat it as a foreign socket, not as another ForkTTY instance.
- Replace the default socket path with a broken symlink, then launch ForkTTY; startup should refuse to replace the non-socket path and leave it for manual inspection.
- Against a stub socket that resets before replying, `forktty ping --socket <stub>` reports the socket path and reset code instead of a raw socket error.
- `forktty list` — returns at least one workspace.
- `forktty ping --wat` — exits with `ping: unexpected argument --wat` before trying to connect to the socket.
- `forktty capabilities` — lists `system.capabilities`, `events.subscribe`, and the advertised socket methods.
- Start `forktty events` in one terminal, change focus or create a workspace from another, and confirm NDJSON events stream until interrupted.
- `forktty clear-notifications --workspace-id main` — exits with `clear-notifications: unexpected argument --workspace-id` instead of clearing all notifications.
- `forktty create-workspace --working-dir=` — exits with `--working-dir requires a value` instead of opening a workspace in the default directory.
- `forktty create-workspace project` — exits with `create-workspace: unexpected argument project` instead of creating a default-named workspace.
- `forktty create-workspace --workingdir /tmp` — exits with `create-workspace: unknown option --workingdir` instead of opening a workspace in the default directory.
- `forktty surfaces --workspace-name main` — returns only surfaces for the `main` workspace.
- `forktty surfaces --workspace main` — exits with `surfaces: unknown option --workspace` instead of listing every surface.
- `forktty focus " "` — exits with `workspace selector requires a value` before contacting the socket.
- `forktty focus <selector-a> --workspace-name <selector-b>` — exits with `focus: cannot combine a positional selector with --workspace-name` instead of silently focusing one selector.
- If positional `forktty focus <selector>` hits an existing workspace id but the socket returns a spawn error, the CLI reports that spawn error instead of retrying the selector as a workspace name and masking it as not found.
- If selecting a workspace over the socket needs to respawn its terminal and that spawn fails, the focus request reports the spawn error and keeps the previous workspace active.
- `FORKTTY_WORKSPACE_ID=workspace-1 forktty close-workspace --workspace main` — exits with `close-workspace: unknown option --workspace` instead of falling back to and closing the inherited workspace id.
- `forktty notify --title "hi" --body "test"` — notification appears in the sidebar.
- `forktty notify --workspace-name main --title "target" "body"` — notification is targeted to the `main` workspace, not listed as global.
- `FORKTTY_WORKSPACE_ID=" workspace-1 " forktty set-status --key qa --value ok` — trims the inherited workspace id before targeting metadata.
- `forktty set-status --key qa --value ok --color=` — exits with `--color requires a value` instead of silently creating an uncolored status.
- `FORKTTY_WORKSPACE_ID=workspace-1 forktty set-status --workspace-id= --key qa --value ok` — exits with `--workspace-id requires a value` instead of falling back to the inherited id.
- `forktty set-status --workspace-id workspace-1 --workspace-name main --key qa --value ok` — exits with `set-status: cannot combine --workspace-id and --workspace-name` instead of silently picking one target.
- `printf '{"id":"x","method":"surface.list","params":{"workspace_name":" main "}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — trims the raw socket selector and returns the `main` workspace surfaces.
- `printf '{"id":"x","method":"surface.list","params":{"workspace_id":"workspace-1","workspace_name":"main"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an ambiguous workspace selector instead of silently picking one target.
- `printf '{"id":"x","method":"workspace.create","params":{"name":"","workingDir":"/tmp"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `name`; no default-named workspace is created.
- `printf '{"id":"x","method":"metadata.set_status","params":{"workspace_id":"","key":"qa","label":"QA","value":"ok"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `workspace_id`; the status is not added to the active workspace.
- `printf '{"id":"x","method":"metadata.set_status","params":{"key":42,"label":"QA","value":"ok"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `key` instead of treating it as missing.
- Send `notification.create`, `metadata.set_status`, `metadata.set_progress`, or `metadata.log` with any text field over 16 KiB — response code is `payload_too_large` and names the oversized field.
- `printf '{"id":"x","method":"notification.create","params":{"title":"bad target","surface_id":""}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `surface_id`; no global notification is created.
- `printf '{"id":"x","method":"surface.send_text","params":{"surface_id":"<surface-id-a>","surfaceId":"<surface-id-b>","text":"echo bad\n"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an ambiguous surface selector instead of silently picking one target.
- `printf '{"id":"x","method":"notification.create","params":{"title":""}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `title`; no blank notification is created.
- `printf '{"id":"x","method":"metadata.set_status","params":{"key":" qa ","label":"QA","value":"ok"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock`, then clear `qa` — the status key is stored trimmed and can be cleared without spaces.
- `printf '{"id":"x","method":"metadata.set_progress","params":{"key":" build ","label":" Build ","value":1}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock`, then list progress — the key and label are stored trimmed.
- `printf '{"id":"x","method":"metadata.set_status","params":{"key":"qa","label":"QA","value":"ok","color":"purple"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `color`; no status row is created.
- `printf '{"id":"x","method":"notification.create","params":{"title":"bad kind","kind":"promtp"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `kind`; no notification is created.
- `printf '{"id":"x","method":"metadata.log","params":{"level":"","message":"blank level"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `level`; no info log is created.
- `forktty notify --kind=` — exits with `--kind requires a value` instead of silently sending an `info` notification.
- `forktty notify --knd prompt "review"` — exits with `notify: unknown option --knd` instead of silently sending an `info` notification.
- `forktty notify --title=` — exits with `--title requires a value` instead of creating a notification with an empty title.
- `forktty log --level=` — exits with `--level requires a value` instead of logging at the default level.
- `forktty log --levl warn "review"` — exits with `log: unknown option --levl` instead of recording an `info` log.
- `forktty set-status --key qa --value ok --colour red` — exits with `set-status: unknown option --colour` instead of creating an uncolored status.
- `forktty set-progress --key build --value 1 --totl 100` — exits with `set-progress: unknown option --totl` instead of creating progress without a total.
- `forktty list-status --workspace main`, `forktty list-progress --workspace main`, `forktty logs --workspace main`, and `forktty clear-logs --workspace main` — each exits with an unknown option error instead of querying or clearing the active workspace.
- `forktty set-status qa --key qa --value ok`, `forktty list-status qa`, `forktty set-progress build --key build --value 1`, and `forktty clear-logs build` — each exits with an unexpected argument error instead of ignoring the stray selector.
- `forktty clear-status --key=` — exits with `--key requires a value` instead of clearing every status entry in the targeted workspace.
- `forktty clear-progress --kee build` — exits with `clear metadata: unknown option --kee` instead of clearing every progress entry in the targeted workspace.
- `printf '{"id":"x","method":"metadata.clear_progress","params":{"key":""}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `key`; progress entries are left intact.
- `forktty send-text "echo hello\n"` — text reaches the focused Ghostty pane.
- `forktty send-text --txt "echo hello"` — exits with `send-text: unknown option --txt` instead of sending the wrong text or target.
- `forktty --socket <stub> send-text "echo explicit" </dev/zero` — sends the explicit text without waiting to drain stdin.
- `forktty --socket <stub> send-text -- --socket --json` — sends the literal text `--socket --json`; flags after `--` are not parsed.
- `forktty close-surface <surface-id> extra` — exits with `close-surface: unexpected argument extra` instead of ignoring the extra value.
- `forktty focus-surface <surface-id-a> --surface-id <surface-id-b>` — exits with `focus-surface: cannot combine --surface-id with a positional surface id` instead of silently focusing one of them.
- `forktty split-surface --axis=` — exits with `--axis requires a value` instead of creating an unintended horizontal split.
- `forktty split-surface --axs vertical` — exits with `split-surface: unknown option --axs` instead of creating the default horizontal split.
- `forktty split-surface <surface-id> extra` — exits with `split-surface: unexpected argument extra` instead of ignoring the extra value.
- `printf '{"id":"x","method":"surface.split","params":{"surface_id":"<surface-id>","axis":"diagonal"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `axis`; no horizontal split is created.
- `printf '{"id":"x","method":"surface.send_text","params":{"surface_id":"<surface-id>","text":42}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `text` parameter instead of treating it as missing.
- `printf '{"id":"x","method":"surface.send_text","params":{"surface_id":"<surface-id>","text":""}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an invalid `text` parameter instead of reporting a successful no-op send.
- `printf '{"id":"x","method":"nonsense.bogus","params":{}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"method_not_found"`.
- Against a stub socket that returns a different response `id`, `forktty ping --socket <stub>` errors with a response-id mismatch that names the method and socket path.
- Against a stub socket that returns `{"id":null,"ok":false,"error":{"code":"request_too_large","message":"Request exceeds 1 MiB"}}`, the CLI surfaces `request_too_large` instead of reporting a response-id mismatch.
- Close a split pane, then send `surface.send_text` or `notification.create` for that closed pane's `surface_id` — response includes `"code":"not_found"` and no notification row is added.
- If replacement terminal spawn fails while closing the only pane in a workspace over the socket, the close request reports the spawn error and keeps the old pane and terminal visible.
- Close the last workspace over the socket from a project directory — ForkTTY creates the replacement `main` workspace in that project directory, not the app launch directory.
- If replacement terminal spawn fails while closing the last workspace over the socket, the close request reports the spawn error and keeps the old workspace and terminal visible.
- If a terminal backend close fails during `workspace.close`, the socket returns the close error and keeps the workspace in the model instead of orphaning its panes.
- `printf '{"id":"x","method":"metadata.set_status","params":{"workspace_id":"workspace-missing","key":"agent:test","label":"Test","value":"Running"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"not_found"`.
- `python3 -c 'import json,sys; sys.stdout.write(json.dumps({"id":"x","method":"surface.send_text","params":{"surface_id":"main:1","text":"x"*300000}})+"\n")' | nc -U "$XDG_RUNTIME_DIR/forktty.sock"` — response includes `"code":"payload_too_large"` (the 256 KiB `surface.send_text` cap).

## Source-Only Browser Pane Smoke

This is not part of release artifact QA. Run it only when intentionally
checking that the opt-in browser feature still builds and starts.

- Launch with `cargo run -p forktty-ui-gtk --no-default-features --features browser`.
- `forktty browser profile list` — returns the built-in Default profile.
- `forktty browser profile create QA` — creates a named profile; `forktty browser profile delete <id>` removes it when no open pane uses it.
- `forktty browser open --profile Default https://example.com` — opens a browser pane with an address bar and WebKit content.
- `forktty browser navigate <surface-id> https://www.rust-lang.org` — updates the pane and address bar.
- `forktty browser snapshot <surface-id>` — returns JSON from the injected page driver.
- `forktty browser back <surface-id>`, `forward`, and `reload` reach the GTK WebView command channel.
- `forktty browser bookmark add https://example.com --title Example`, `forktty browser bookmark list`, and `forktty browser history list` return structured JSON against the Default profile.
- Close the browser pane with its in-pane close button and confirm the same close confirmation/path as terminal panes.

## Session Restore Smoke

- Change a workspace or split pane, then confirm `$XDG_DATA_HOME/forktty/` does not accumulate `session-v2.json.tmp-*` files after normal autosave/restart.
- Symlink `$XDG_DATA_HOME/forktty/session-v2.json` to a real managed JSON file,
  then change a workspace; the target file updates and the symlink remains a symlink.
- Stop the app, then corrupt `$XDG_DATA_HOME/forktty/session-v2.json` (`echo "{ broken" >$XDG_DATA_HOME/forktty/session-v2.json`). Relaunch; ForkTTY should start with a fresh workspace even if an old `session.json` exists, the corrupt file should now be renamed `*.bad-<timestamp>` or `*.bad-<timestamp>-N` if that name already exists, and stderr should log the quarantine reason.
- Stop the app, then blank the saved workspace name in `$XDG_DATA_HOME/forktty/session-v2.json` (`python3 - <<'PY'\nimport json, os, pathlib\np = pathlib.Path(os.environ.get("XDG_DATA_HOME", pathlib.Path.home()/".local/share"))/"forktty/session-v2.json"\ndata = json.loads(p.read_text())\ndata["workspaces"][0]["name"] = ""\np.write_text(json.dumps(data))\nPY`). Relaunch; ForkTTY should quarantine the session and bootstrap a usable workspace instead of showing a nameless workspace row.
- Replace `$XDG_DATA_HOME/forktty/session-v2.json` with a broken symlink, then relaunch; ForkTTY should start fresh and rename the symlink aside instead of silently leaving the bad path in place.
- Stop the app, truncate the session to >1 MiB (`yes x | head -c 2000000 >$XDG_DATA_HOME/forktty/session-v2.json`). Relaunch; same behavior.

## Hook Installer Smoke

- `forktty hooks setup codex --dry-run` — prints `would update` but does not create the Codex config.
- `forktty hooks setup --dry-run codex` — also prints `would update` without writing; `--dry-run` must not consume the agent name.
- `forktty hooks setup --dry-run=yes codex` — exits with `--dry-run must be true or false` and does not create or update hook configs.
- `forktty hooks setup --dryrun codex` — exits with `unknown option --dryrun` and does not create or update hook configs.
- `forktty hooks setup codex codex` — updates Codex once and prints one Codex summary, without creating redundant backups.
- `forktty hooks setup` (first run) — creates/updates all supported agent configs/plugins, prints `updated` and a backup path.
- `HOME=$(mktemp -d) CODEX_HOME= CLAUDE_CONFIG_DIR= OPENCODE_CONFIG_DIR= forktty hooks setup` — creates `.codex`, `.claude`, `.gemini`, and `.config/opencode/plugins/forktty.generated.js` under that temporary home, not the real home directory.
- `forktty hooks remove codex --dry-run` — prints `would remove` and leaves the Codex config unchanged.
- `forktty hooks remove codex opencode` — removes only ForkTTY-managed Codex entries and the generated OpenCode plugin, preserving unrelated hook commands.
- Launch the GTK app with no ForkTTY-managed hooks installed — it shows an Agent Hooks Available notification that suggests `forktty hooks setup`; if at least one provider is already configured and current, missing optional providers do not nag.
- Inspect one generated hook command — it calls the absolute `forktty` launcher directly, so AppImage and packaged installs do not need a source checkout or Node.js.
- Repeat the previous command — prints `already configured` for each agent and does not create new backups.
- `forktty hooks codex session-start --socket <stub>` without `FORKTTY_SOCKET_PATH` — sends status/log actions to the supplied socket and still prints the hook continue JSON.
- `forktty hooks codex sesion-start` — prints an unsupported hook event warning to stderr and still prints the hook continue JSON.
- `forktty hooks codex session-start extra` — prints an unexpected hook argument warning to stderr and still prints the hook continue JSON.
- Symlink `~/.codex/hooks.json` to a real managed JSON file, then run `hooks setup codex` —
  the target file is updated and backed up, and the symlink remains a symlink.
- Modify an existing agent hook config and re-run setup twice quickly — each changed run creates a distinct `.bak-*` file and does not overwrite a prior backup.
- Corrupt `~/.codex/hooks.json` (`echo '{ not json' >~/.codex/hooks.json`), re-run `hooks setup codex` — error message names both the agent and the path; the file is left untouched.
- Corrupt `~/.claude/settings.json`, then run `hooks setup codex claude` — setup fails before creating or updating the Codex hook config.
- Replace `~/.codex/hooks.json` with a JSON array (`echo '[]' >~/.codex/hooks.json`), re-run `hooks setup codex` — error message says the top-level config must be a JSON object; the file is left untouched.
- Replace `~/.codex/hooks.json` with a directory, re-run `hooks setup codex` —
  error message says the path is not a regular file; no backup or replacement is created.
- Replace `~/.codex/hooks.json` with a broken symlink, re-run `hooks setup codex` —
  setup warns about the broken symlink and replaces it with a regular managed hook file.

## Worktree Smoke

- From an open repo workspace, run `forktty worktree-list` without `--cwd` — it uses the shell's current `PWD`, not the app launch directory.
- `env -u PWD forktty worktree-list` from an open repo workspace — it still uses the CLI process current directory, not the app launch directory.
- `forktty worktree-list feature/x` — exits with `worktree-list: unexpected argument feature/x` instead of ignoring the branch argument and listing the caller repo.
- `printf '{"id":"x","method":"worktree.create","params":{"name":"feature/no-cwd"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"missing_param"` for `cwd`; no worktree is created from the app launch directory.
- `printf '{"id":"x","method":"worktree.create","params":{"name":42,"cwd":"'"$PWD"'"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"error"` and says `name` must be a string.
- `printf '{"id":"x","method":"worktree.attach","params":{"name":"feature/x","branch":"feature/y","cwd":"'"$PWD"'"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response reports an ambiguous worktree selector instead of silently picking one target.
- `forktty worktree-create feature/x --cw <repo>` — exits with `worktree-create: unknown option --cw` instead of using the fallback cwd.
- `forktty worktree-create feature/x --branch feature/y --cwd <repo>` — exits with `cannot combine a positional name with --name or --branch` instead of ignoring one target.
- `forktty worktree-create --name feature/x --branch feature/y --cwd <repo>` — exits with `cannot combine --name and --branch` instead of ignoring one target.
- `forktty worktree-create feature/x --cwd <path-to-clean-repo>` — new workspace opens at `.worktrees/feature-x`.
- Run the same `forktty worktree-create feature/x --cwd <path-to-clean-repo>` again after closing the workspace without removing the Git worktree — it reopens the existing worktree instead of failing on the already-created branch.
- If terminal spawn fails during `forktty worktree-create feature/x`, the action
  reports the spawn failure and removes the newly created git worktree and branch.
- Run `forktty worktree-attach feature/x --cwd <path-to-clean-repo>` again — it opens/reuses the existing worktree instead of creating a duplicate or failing.
- If the GTK worktree dialog is opened while no workspace is active, actions report that no active workspace is available instead of using the app launch directory.
- In the original workspace, run `forktty worktree-status` — returns `clean`; ForkTTY's `.worktrees/` directory should not make the target checkout dirty.
- From a subdirectory inside that worktree, run `forktty worktree-status` — returns `clean` or `dirty`, not a repository error.
- `forktty worktree-status <path> extra` — exits with `worktree-status: unexpected argument extra` instead of ignoring the extra selector.
- Create an uncommitted file in the worktree, then run `forktty worktree-merge feature/x` from the original workspace — merge is rejected and the error says to commit, stash, or resolve the source worktree first.
- Inside the new workspace, commit a change and then `forktty worktree-merge feature/x` from the original workspace's prompt.
- `forktty worktree-remove feature/x` — the workspace closes, the worktree is removed from the repo, and the local `feature/x` branch still exists.
- From inside the linked worktree, run `forktty worktree-remove feature/x --cwd "$PWD"` — it removes that worktree without a git admin-directory error.
- If replacement terminal spawn fails while removing the last visible worktree workspace, the remove action reports the spawn failure and keeps the old workspace and terminal visible.
- If terminal close fails during worktree removal, the remove action reports the
  close failure and leaves the ForkTTY workspace visible instead of orphaning backend state.
- Manually delete a linked worktree directory, then run `forktty worktree-list` and `forktty worktree-remove <branch>` — the list marks it `missing`, remove prunes the stale git worktree metadata, and the branch remains.
- `forktty worktree-status --path=` — exits with `--path requires a value` instead of checking the caller's current repo.

## Debian Package Smoke

- Install the generated `.deb` with `sudo dpkg -i target/packaging/deb/forktty_*.deb`.
- Inspect the package contents and confirm `usr/lib/ghostty-gtk-embed.so` is
  present.
- Launch `forktty` from a terminal.
- Launch ForkTTY from the desktop/app launcher.
- Confirm the app icon and desktop name render correctly.
- `forktty --version` prints the workspace version and exits 0.
- `forktty --help` prints usage and exits 0.
- `forktty doctor` prints the diagnostics report and exits 0 on a clean
  environment, or 2 with explicit warning lines on a misconfigured one.
- With `$XDG_CONFIG_HOME/forktty/config.toml` symlinked to a real TOML file,
  `forktty doctor` reports it as a file and does not warn that it will be quarantined.
- With `$XDG_CONFIG_HOME/forktty/config.toml` as a broken symlink, `forktty doctor`
  warns that it could not be inspected and will be quarantined on launch.
- With an oversized `$XDG_DATA_HOME/forktty/session-v2.json`, `forktty doctor`
  warns that the session will be quarantined on launch.
- With `$XDG_DATA_HOME/forktty/session-v2.json` symlinked to a real JSON file,
  `forktty doctor` reports it as a file and does not warn that it will be quarantined.
- With `$XDG_DATA_HOME/forktty/session-v2.json` as a broken symlink, `forktty doctor`
  warns that it could not be inspected and will be quarantined on launch.
- With `FORKTTY_SOCKET_PATH=/tmp/forktty-doctor.sock`, `forktty doctor` reports
  `/tmp/forktty-doctor.sock`; with `FORKTTY_SOCKET_PATH=relative.sock`, it reports the default socket path.
- With `XDG_RUNTIME_DIR` symlinked to a private real directory, `forktty doctor`
  reports the socket parent as a directory instead of warning that it is not one.
- With a regular file at `$XDG_RUNTIME_DIR/forktty.sock`, `forktty doctor`
  warns that the socket path is not a Unix socket before launch fails.
- With `$XDG_RUNTIME_DIR` pointing at a regular file, `forktty doctor` warns
  that the socket parent is not a directory before launch fails.
- With `$XDG_RUNTIME_DIR` pointing under a regular file path, `forktty doctor`
  warns that the socket path could not be inspected and exits 2.
- `HOME=$(mktemp -d) CODEX_HOME= CLAUDE_CONFIG_DIR= forktty doctor` reports
  hook paths under that temporary home, matching `hooks setup` fallback paths.
- With a directory at `$HOME/.codex/hooks.json`, `forktty doctor` reports the
  Codex hook config as `blocked`, warns that it is not a regular file, and exits 2.
- With a broken symlink at `$HOME/.codex/hooks.json`, `forktty doctor` reports
  the Codex hook config as `blocked`, warns that `hooks setup` will replace the
  broken symlink, and exits 2.
- `forktty wat` prints `unknown argument: wat` to stderr and exits 2.
- `forktty doctor --wat` prints `unknown argument: --wat` to stderr and exits 2
  instead of silently ignoring the extra argument.
- `forktty doctor --json` prints valid JSON only.
- `forktty doctor --strict` exits 2 when warnings exist and 0 when clean.
- `forktty doctor --json --strict` still prints valid JSON only.
- TODO (follow-up): add scoped doctor flags (`--hooks`, `--socket`, and
  `--packaging`) once the output contract for targeted diagnostics is finalized.
- `sha256sum -c SHA256SUMS` (run from the release download dir) prints
  `OK` for the published `.deb`.
- Remove the package and confirm `/usr/bin/forktty` and the desktop entry are removed.

## AppImage Smoke

- Build or download the generated AppImage from `target/packaging/appimage/`.
- Mark it executable and launch it directly.
- Extract or inspect the AppImage and confirm `usr/lib/ghostty-gtk-embed.so` is
  present.
- Confirm `forktty --version`, `forktty --help`, `forktty doctor`, and `forktty hooks setup --dry-run codex` work from the AppImage.
- Launch the GTK app and walk the basic terminal, split-pane, desktop icon, and notification checks above.
- If the GTK UI renders incorrectly, retry with `GSK_RENDERER=ngl` and
  compare against the `.deb`; treat AppImage-only GL/Vulkan artifacts as
  package notes, not as proof that the native GTK runtime is broken.
- `sha256sum -c SHA256SUMS` (run from the release download dir) prints
  `OK` for the published AppImage.
- Treat any missing runtime library or desktop integration issue as an AppImage-specific alpha note unless it also reproduces from the `.deb`.

## Suggested Matrix

- Ubuntu 24.04 or newer, GNOME Wayland.
- Ubuntu 24.04 or newer, X11 session if available.
- Debian testing/stable with libadwaita 1.4+ and the packaged embedded Ghostty
  library loading.
- One Arch/CachyOS system for rolling-release dependency drift.
- One Fedora-family system; note that the dependency package names differ from Debian (e.g. `git zig` instead of `git zig`).
