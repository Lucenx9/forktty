# ForkTTY Agent Hooks

These templates wire supported coding-agent hook systems into ForkTTY's local
socket API. The preferred path is the installed ForkTTY CLI:

```bash
forktty hooks setup
```

## Repository ownership

This README describes installed hook behavior and user-facing config
destinations. The executable hook installer lives in
`crates/forktty-ui-gtk/src/socket_cli/hooks.rs` and
`crates/forktty-ui-gtk/src/socket_cli/hooks/install.rs`; runtime hook event
handling lives in `crates/forktty-ui-gtk/src/socket_cli/hooks/event.rs`.
Provider lifecycle notes live in `docs/agents.md`.

When changing hook behavior, update the owning Rust module first, then keep
this README, `docs/agents.md`, `SPEC.md`, `README.md`, and the public website
context aligned.

That writes default agent-specific hook config into the user config for Codex,
Claude Code, Antigravity CLI, and OpenCode:

| Agent | Destination |
|---|---|
| Codex | `$CODEX_HOME/hooks.json` or `~/.codex/hooks.json` |
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` or `~/.claude/settings.json` |
| Antigravity CLI | `~/.gemini/config/hooks.json` + `~/.gemini/config/forktty-hooks.generated/` |
| OpenCode | `$OPENCODE_CONFIG_DIR/plugins/forktty.generated.js` or `~/.config/opencode/plugins/forktty.generated.js` |

The installer writes an absolute path to the `forktty` launcher so hooks can run
from any project. Re-run `forktty hooks setup` if the AppImage or installed
binary moves. When that launcher is an AppImage, generated hook commands set
`APPIMAGE_EXTRACT_AND_RUN=1` for the ForkTTY CLI child so short hooks do not
keep a FUSE AppImage mount alive. Runtime provenance, not the filename suffix,
identifies renamed AppImages. `--dry-run` prints the would-be diff without
touching disk:

```bash
forktty hooks setup --dry-run
forktty hooks setup codex --dry-run
forktty hooks setup --full claude
forktty hooks remove codex --dry-run
forktty hooks remove gemini        # legacy cleanup only
```

Each setup run writes the agent config or generated plugin atomically
(tmp + rename) and, when content changes, leaves a timestamped `.bak-*`
backup next to the original. The OpenCode file is intentionally generated
under its plugins directory so `opencode.json` does not need to be edited.
When Codex hook definitions change, setup output directs the user to `/hooks`
inside Codex: Codex approves each
non-managed hook by its current definition hash, so a previous approval may no
longer apply after an update.
`forktty hooks remove` deletes only ForkTTY-managed entries or the generated
OpenCode plugin; custom hook commands are preserved. `forktty hooks remove
gemini` is retained only to remove ForkTTY-managed entries from legacy
`~/.gemini/settings.json` files written by older releases; Gemini setup remains
unsupported.

Claude Code setup installs a lifecycle profile by default. That profile omits
the high-frequency `PreToolUse`, `PostToolUse`, and `PostToolUseFailure` hooks
that block every tool call; `PostToolBatch` remains installed for prompt-result
correlation. Pass `--full` to include the three per-tool events. Existing full
installs keep working, but re-running
`forktty hooks setup claude` migrates the ForkTTY-managed entries back to the
lifecycle profile unless `--full` is passed. Removal cleans either profile.

ForkTTY's built-in integration stops at hooks and the local socket CLI. It no
longer installs MCP registrations or managed agent skills. External MCP servers
and provider-specific skills remain independent terminal tooling.

Antigravity CLI (`agy`) executes a hook
`command` as one bare executable path — no argument splitting and no shell —
so the installer writes per-event wrapper scripts under
`~/.gemini/config/forktty-hooks.generated/` and points the ForkTTY-owned
`"forktty"` group in `~/.gemini/config/hooks.json` at them. Other top-level
groups in that file are left untouched, and `hooks remove antigravity`
deletes only the `"forktty"` group and the generated scripts directory.
Antigravity v1.0.3 supports `PreInvocation`, `PreToolUse`, and `PostToolUse`;
`PreInvocation` uses Antigravity's flat lifecycle-hook handler shape, while
tool hooks use the nested matcher plus `hooks` array shape.
unknown event names are dropped silently, and hook stdout is unmarshaled
strictly. ForkTTY therefore avoids the `continue` JSON used by other
providers; it returns an explicit `{"decision":"allow"}` for the gating
`PreToolUse` hook and `{}` for non-gating Antigravity hooks.
Antigravity runs the generated wrapper scripts from its config directory, so
ForkTTY derives Antigravity `resume_cwd` from the hook payload's
`workspacePaths` instead of the wrapper process cwd.

GTK startup does not install, refresh, or create reminders for optional hooks.
Use the welcome flow, Settings > Agents, or the CLI when you want setup or
diagnostics.

## Inspect and exercise installed hooks

```bash
forktty hooks doctor codex     # report socket, launcher, env, and hook config state
forktty hooks test codex       # round-trip a status update and a log over the socket
```

`hooks doctor` is local-only and never mutates state. `hooks test` writes a
single transient `agent:<name>:hook-test` status entry through the socket so
you can confirm the daemon is reachable.

`hooks doctor` inspects the launcher path baked into the managed assets and
reports `launcherCheck.status`
(`ok`, `stale`, `not_installed`, or `current_launcher_unknown`). A `stale`
status means the recorded launcher is no longer usable and differs from the
current executable; a still-executable recorded launcher remains healthy even
when doctor itself runs from another path. Re-run `forktty hooks setup` to
rewrite unusable managed commands.
The doctor JSON also exposes `supportedEvents`, the list of provider-side
event names ForkTTY can install hooks for (Codex: 10; Claude Code: 25
lifecycle / 28 full; Antigravity: 3; OpenCode plugin events: 11).
For Claude Code it also reports `installedProfile` as `lifecycle`, `full`, or
`not_installed`.

The additive version-1 `installationCheck` regenerates the expected provider
assets with the same setup planner used by `hooks setup`. Doctor health requires
the complete managed config/plugin, one usable recorded launcher, and for
Antigravity the exact generated group plus every wrapper as a regular executable
file with generated content. Missing groups or wrappers, wrapper-only installs,
malformed or modified files, partial Claude/Codex/OpenCode assets, and
non-executable wrappers set `installationCheck.ok` and top-level `ok` to false.

For Codex, `hooks doctor codex` additionally reports `trustCheck`: Codex
records per-hook trust approvals under `[hooks.state]` in its `config.toml`,
and an installed hook with no record silently does nothing until it is
approved via `/hooks` inside Codex. `trustCheck.status` is `all_recorded`,
`partial`, or `none_recorded`, with the affected events listed in
`unrecordedEvents`. `currentHashesVerified` is always `false`: ForkTTY can
detect trust records but cannot prove that their hashes match the current hook
definitions. This is informational — the approval semantics belong to Codex.

## Status entries published by hooks

ForkTTY hooks publish the following status keys via `metadata.set_status`.
They render in the ForkTTY UI status row and can be inspected from the CLI:

| Key | Set on | Cleared on | Color semantics |
|---|---|---|---|
| `agent:<key>` | lifecycle, prompt, permission, tool, compact, stop, and attention notification events | session-end | `green` ready, `blue` running, `yellow` needs input / compacting / permission, `red` error |
| `agent:<key>:permission` | events that include `permission_mode` | session-end | `muted` for documented-safe or unknown modes, `yellow` for `acceptEdits`/`auto`/`dontAsk`, `red` for `bypassPermissions` |
| `agent:claude:tokens` | prompt-submit (Claude only, when a transcript is available) | last Claude session ended, closed, forgotten, or hibernated | progress against `FORKTTY_HOOK_TOKEN_CEILING` (default 200,000) |

Unknown provider modes stay `muted`; ForkTTY only colors permission values
that are documented by the provider.
Tool-use events keep `agent:<key>` as the compact `Running` lifecycle status;
the exact tool name is recorded in hook log metadata instead of the primary
status value so snapshots stay stable for automation.
Non-attention notifications are logged without replacing the current lifecycle,
so informational notifications arriving after `Stop` do not revive an idle
workspace badge.
For Codex and Claude Code, `SubagentStop` leaves the parent session `Running`
because the event only reports a nested subagent completion. Claude Code
`TeammateIdle` publishes `Ready` and persists the teammate lifecycle as idle.

Permission, elicitation, and recognized attention hooks attach a normalized
provider/session/kind prompt identity to their ForkTTY notification. Accepted
result hooks retain only the matching in-app notification as read history and
close its desktop notification; when a
provider result has no correlation id, ForkTTY resolves only the newest
compatible older prompt. Stale results are inert. Session-end cleanup, hook
target remap, and surface/workspace removal retire affected correlations without
clearing unrelated prompts. Claude `Elicitation` creates a prompt notification;
`ElicitationResult`, `PermissionDenied`, and `PostToolBatch` close
the corresponding elicitation or permission prompt when one is pending.

Claude `SessionStart` uses ForkTTY workspace ID, surface ID, and absolute socket
path as one provenance tuple. If any component is missing or invalid, the hook
returns the exact continue response without reading stdin or contacting the
socket. Persisted `Suspended` is a tombstone: late hook events cannot revive the
session, publish metadata/notifications, or advance its event-order watermark.
Only explicit resume may replace it, and invalid persisted resume metadata
produces a visible terminal error rather than a plain-shell fallback.

## Manual editing

Files in this directory are canonical examples for review or manual repair:

- `codex-hooks.json`
- `claude-settings.json` (Claude lifecycle profile; use `hooks setup --full`
  to generate the full profile)
- OpenCode uses a generated plugin file instead of a JSON template.
- Antigravity uses a generated `"forktty"` group plus wrapper scripts
  instead of a JSON template (its hook commands cannot take arguments).

Replace `{{FORKTTY_LAUNCHER}}` with the absolute path to the `forktty` launcher
if you install these by hand; keep it shell-quoted. The installer handles this
quoting automatically.

The `timeout` field is provider-defined. Claude Code and Codex measure it in
**seconds** (Codex default 600 s; Claude default 600 s, 30 s for
`UserPromptSubmit`), and ForkTTY pins those entries at 30 s. Antigravity has no
verified timeout field, so its entries omit one. The intent is the same for
every provider: a hook should not block the agent loop longer than a local
socket round-trip needs.

Each command is guarded by a per-agent disable variable:

- `FORKTTY_CODEX_HOOKS_DISABLED=1`
- `FORKTTY_CLAUDE_HOOKS_DISABLED=1`
- `FORKTTY_ANTIGRAVITY_HOOKS_DISABLED=1`
- `FORKTTY_OPENCODE_HOOKS_DISABLED=1`
