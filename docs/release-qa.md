# Release QA Checklist

Use this before tagging an alpha release. The goal is to catch GTK/VTE and
package regressions that unit tests cannot see.

## Automated Checks

```bash
cargo fmt --all --check
cargo test --workspace
cargo test -p forktty-ui-gtk --features gtk-vte
cargo clippy --workspace --features gtk-vte -- -D warnings
cargo build -p forktty-ui-gtk --features gtk-vte
node --test scripts/forktty.test.mjs
desktop-file-validate packaging/linux/forktty.desktop
bash scripts/build-deb.sh
```

## Manual Runtime Smoke

- Start from a clean config/session directory.
- Launch with `cargo run -p forktty-ui-gtk --features gtk-vte`.
- Confirm the app opens a usable terminal in the current directory.
- Split right and split down until at least three panes exist.
- Move focus between panes with keyboard shortcuts and pointer clicks.
- Copy and paste with `Ctrl+Shift+C` / `Ctrl+Shift+V`.
- Open the terminal context menu in a small split pane and use Paste.
- Close one pane and confirm focus moves to a remaining pane.
- Close the focused pane while a long-running shell command is exiting; confirm only one "Terminal exited" notification is created and it does not reference the just-closed surface.
- Toggle the sidebar twice and confirm `~/.config/forktty/config.toml` remains valid TOML with no `config.toml.tmp-*` sibling left behind.
- Restart the app and confirm workspace/pane layout restores.
- Set an invalid shell path in Settings and confirm the pane shows a recovery state.
- Open Notifications, dismiss one notification, then Clear All.

## Config Recovery Smoke

- Stop the app, then corrupt `$XDG_CONFIG_HOME/forktty/config.toml` (`echo "{ broken" >$XDG_CONFIG_HOME/forktty/config.toml`). Relaunch; ForkTTY should start with defaults, show a Config Issue notification that names the quarantined file, and rename the corrupt config to `*.bad-<timestamp>` or `*.bad-<timestamp>-N` if that name already exists.

## Socket API Smoke

Run these after starting the GTK app so the daemon is listening on the
default socket. Useful for catching protocol regressions without
rebuilding.

- Before starting the app, run `./scripts/forktty.mjs ping` and confirm the error names the socket path and suggests `cargo run -p forktty-ui-gtk --features gtk-vte` or `--socket <path>`.
- `forktty list` — returns at least one workspace.
- `forktty surfaces --workspace-name main` — returns only surfaces for the `main` workspace.
- `forktty notify --title "hi" --body "test"` — notification appears in the sidebar.
- `forktty send-text "echo hello\n"` — text reaches the focused VTE pane.
- `printf '{"id":"x","method":"nonsense.bogus","params":{}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"method_not_found"`.
- Against a stub socket that returns a different response `id`, `forktty ping --socket <stub>` errors with a response-id mismatch that names the method and socket path.
- Close a split pane, then send `surface.send_text` or `notification.create` for that closed pane's `surface_id` — response includes `"code":"not_found"` and no notification row is added.
- `printf '{"id":"x","method":"metadata.set_status","params":{"workspace_id":"workspace-missing","key":"agent:test","label":"Test","value":"Running"}}\n' | nc -U $XDG_RUNTIME_DIR/forktty.sock` — response includes `"code":"not_found"`.
- `python3 -c 'import json,sys; sys.stdout.write(json.dumps({"id":"x","method":"surface.send_text","params":{"surface_id":"main:1","text":"x"*300000}})+"\n")' | nc -U "$XDG_RUNTIME_DIR/forktty.sock"` — response includes `"code":"payload_too_large"` (the 256 KiB `surface.send_text` cap).

## Session Restore Smoke

- Change a workspace or split pane, then confirm `$XDG_DATA_HOME/forktty/` does not accumulate `session-v2.json.tmp-*` files after normal autosave/restart.
- Stop the app, then corrupt `$XDG_DATA_HOME/forktty/session-v2.json` (`echo "{ broken" >$XDG_DATA_HOME/forktty/session-v2.json`). Relaunch; ForkTTY should start with a fresh workspace even if an old `session.json` exists, the corrupt file should now be renamed `*.bad-<timestamp>` or `*.bad-<timestamp>-N` if that name already exists, and stderr should log the quarantine reason.
- Stop the app, truncate the session to >1 MiB (`yes x | head -c 2000000 >$XDG_DATA_HOME/forktty/session-v2.json`). Relaunch; same behavior.

## Hook Installer Smoke

- `node scripts/forktty.mjs hooks setup codex --dry-run` — prints `would update` but does not create the Codex config.
- `node scripts/forktty.mjs hooks setup` (first run) — creates/updates all three agent configs, prints `updated` and a backup path.
- Repeat the previous command — prints `already configured` for each agent and does not create new backups.
- Modify an existing agent hook config and re-run setup twice quickly — each changed run creates a distinct `.bak-*` file and does not overwrite a prior backup.
- Corrupt `~/.codex/hooks.json` (`echo '{ not json' >~/.codex/hooks.json`), re-run `hooks setup codex` — error message names both the agent and the path; the file is left untouched.
- Replace `~/.codex/hooks.json` with a JSON array (`echo '[]' >~/.codex/hooks.json`), re-run `hooks setup codex` — error message says the top-level config must be a JSON object; the file is left untouched.

## Worktree Smoke

- From an open repo workspace, run `forktty worktree-list` without `--cwd` — it uses the shell's current `PWD`, not the app launch directory.
- `forktty worktree-create feature/x --cwd <path-to-clean-repo>` — new workspace opens at `.worktrees/feature-x`.
- Run `forktty worktree-attach feature/x --cwd <path-to-clean-repo>` again — it opens/reuses the existing worktree instead of creating a duplicate or failing.
- In the original workspace, run `forktty worktree-status` — returns `clean`; ForkTTY's `.worktrees/` directory should not make the target checkout dirty.
- From a subdirectory inside that worktree, run `forktty worktree-status` — returns `clean` or `dirty`, not a repository error.
- Create an uncommitted file in the worktree, then run `forktty worktree-merge feature/x` from the original workspace — merge is rejected and the error says to commit, stash, or resolve the source worktree first.
- Inside the new workspace, commit a change and then `forktty worktree-merge feature/x` from the original workspace's prompt.
- `forktty worktree-remove feature/x` — the workspace closes and the worktree is removed from the repo.

## Debian Package Smoke

- Install the generated `.deb` with `sudo dpkg -i target/packaging/deb/forktty_*.deb`.
- Launch `forktty` from a terminal.
- Launch ForkTTY from the desktop/app launcher.
- Confirm the app icon and desktop name render correctly.
- Confirm `forktty --help` exits cleanly if CLI flags are supported, or document the current behavior.
- Remove the package and confirm `/usr/bin/forktty` and the desktop entry are removed.

## Suggested Matrix

- Ubuntu 24.04 or newer, GNOME Wayland.
- Ubuntu 24.04 or newer, X11 session if available.
- Debian testing/stable where VTE 0.76+ is available.
- One Arch/CachyOS system for rolling-release dependency drift.
- One Fedora-family system; note that the dependency package names differ from Debian (e.g. `vte291-gtk4-devel` instead of `libvte-2.91-gtk4-dev`).
