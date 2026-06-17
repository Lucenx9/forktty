# Changelog

All notable changes to ForkTTY are documented here.

## [Unreleased]

### Added
- ForkTTY now pins the full upstream Ghostty source as `vendor/ghostty` for the
  cmux-style renderer/widget integration spike; release builds still use the
  existing GTK/libghostty-vt runtime until that bridge is proven.
- The Ghostty renderer spike is now documented: upstream's current public C
  surface embedding API is macOS/iOS-only, so ForkTTY's next renderer step is a
  minimal Ghostty-side GTK widget embedding API instead of more parity shims.
- `scripts/ghostty-gtk-build-probe.sh` now records the reduced upstream Ghostty
  GTK build used before attempting the Linux renderer embedding patch.
- A manual `Ghostty GTK Probe` GitHub Actions workflow can run that upstream
  Ghostty GTK build on Ubuntu without blocking the normal ForkTTY CI.
- `forktty ghostty-gtk-probe` can auto-exit with
  `FORKTTY_GHOSTTY_GTK_PROBE_EXIT_AFTER_MS`, and the manual Ghostty GTK Probe
  workflow now smoke-tests the Rust GTK widget bridge under Xvfb after building
  the vendored Ghostty GTK embedding library.
- The vendored Ghostty GTK embedding library now avoids standalone-app theme
  startup when registered inside ForkTTY's host GTK application.
- The vendored Ghostty GTK embedding ABI now returns a sunk full widget
  reference so the Rust probe can parent the surface without premature dispose.
- The vendored Ghostty GTK embedding context now initializes Ghostty's GTK app
  state in-place so internal runtime pointers stay valid after context setup.
- Team orchestration state is now available as a provider-neutral control
  plane through `team.*` socket methods, `forktty team-*` CLI commands, and MCP
  tools, covering leader/worker metadata, task DAGs, mailbox messages,
  heartbeats, provider worker launch into tabs, pane dispatch confirmations,
  worker health/lifecycle snapshots, idle nudges, safe shutdown requests,
  summaries, and event polling without adding parity UI yet.
- `agent.hibernate`, `agent.reclaim`, `forktty hibernate-agent`,
  `forktty reclaim-agents`, and MCP `agent_hibernate`/`agent_reclaim` can now
  close idle, locally resumable agent terminal processes, mark their persisted
  sessions `suspended`, and leave them resumable through the existing
  `agent.resume` path without adding parity UI panels.
- Workflow control-plane methods (`workflow.list`, `workflow.get`,
  `workflow.upsert`, `workflow.plan.set`, `workflow.evidence.add`,
  `workflow.replay`) plus `forktty workflows`/`workflow-*` CLI commands and MCP
  tools now persist bounded goal, mode/session memory, plan, evidence, and
  replay events without adding parity UI panels.
- Repo-local `forktty.json` project actions can now be listed and launched
  through `project.action.list` / `project.action.run` and the `forktty actions`
  / `forktty action-run` CLI. Actions are argv-only and limited to git repos
  already open in ForkTTY.
- `feed.list` and `forktty feed` now expose a minimal read-only feed snapshot
  that normalizes current notifications, approval prompts, status, and progress
  without adding durable feed history yet.
- Feed history now persists bounded notification, approval, status, and progress
  events to `feed.json`; `feed.approval.respond` / `forktty feed respond` can
  mark approval rows approved or denied for later workflow consumers.
- `forktty top` / `system.top` now return a read-only workspace and surface
  health snapshot with focus, unread, kind, cwd, shell, size, PID when known,
  agent lifecycle, status, and progress fields.
- `remote.list` / `remote.status`, `forktty remotes` /
  `forktty remote-status`, and MCP `remote_list` / `remote_status` now expose
  read-only SSH workspace inventory and connection state without adding a
  remote daemon yet.
- `forktty remote-helper hello` now prints a one-shot stdio JSON handshake for
  future SSH-launched remote helpers.
- `forktty remote-helper pty -- <program> [args...]` now runs an argv command
  under a PTY and relays stdin/stdout bytes over stdio as the first remote
  helper PTY path.
- `appearance.persistent_scrollback_lines` can opt into saving a bounded
  plain-text terminal tail per surface and restoring it with the session.
- OSC 99 terminal notifications can now keep activation/close report metadata,
  icon names, bounded icon data cache entries, and expose basic same-id buttons
  in the in-app notification panel.

### Changed
- Settings no longer exposes terminal font family, font size, or terminal palette controls; GTK terminal panes now read font, color, and `scrollback-limit` appearance from Ghostty's config, including `config-file`, `theme`, named colors, and ANSI palette entries, while legacy ForkTTY appearance keys are loaded only for compatibility and omitted from new saves.
- Repeated Ghostty `font-family`, `font-family-bold`, `font-family-italic`, and `font-family-bold-italic` entries now build Pango fallback lists, and empty entries reset each list.
- Ghostty `font-feature` and `font-variation*` entries now apply to GTK terminal text through Pango.
- Ghostty `cell-foreground`/`cell-background` cursor and selection color references, plus legacy `cursor-invert-fg-bg` and `selection-invert-fg-bg`, are now honored by GTK terminal panes.
- Ghostty `bold-color` and legacy `bold-is-bright` are now honored by GTK terminal panes, including bright ANSI mapping for bold base-color text.
- Ghostty `cursor-opacity` now controls the GTK terminal cursor overlay.
- Ghostty `cursor-style` and `cursor-style-blink` now seed the GTK terminal
  cursor default for DECSCUSR-backed cursor styles.
- Ghostty `faint-opacity` now controls SGR faint text opacity in GTK terminal panes.
- Ghostty `selection-clear-on-typing` now controls whether typing after a
  scroll-to-bottom keeps or clears a finished terminal selection.
- Ghostty `selection-clear-on-copy` now controls whether copying clears the
  finished terminal selection.
- Ghostty `selection-word-chars` now controls double-click word boundaries in
  libghostty word selection and GTK fallback selection.
- Ghostty `clipboard-trim-trailing-spaces` now trims trailing whitespace from
  copied terminal lines.
- Ghostty `clipboard-codepoint-map` now maps configured Unicode codepoints or
  ranges while copying terminal text.
- Ghostty `copy-on-select` now controls selection publication: default/`true`
  keeps PRIMARY selection behavior, `false` disables it, and `clipboard`
  publishes to both PRIMARY and the regular clipboard.
- Ghostty `right-click-action` now controls terminal right-click behavior for
  context menu, copy, paste, copy-or-paste, and ignore.
- Ghostty `scroll-to-bottom` now controls whether input and/or new output snap
  the terminal viewport back to the bottom.
- Ghostty `mouse-reporting = false` now keeps mouse press/release/motion/scroll
  local even when terminal applications request mouse tracking.
- Ghostty `mouse-shift-capture` now controls whether Shift+click stays local
  for selection or can be forwarded to mouse-tracking applications, including
  XTSHIFTESCAPE runtime overrides for `true`/`false`.
- Ghostty `mouse-hide-while-typing` now hides the GTK terminal pointer after
  user typing or paste until mouse movement restores it.
- Ghostty `mouse-scroll-multiplier` now controls GTK terminal precision and discrete scroll distance.
- Ghostty `font-style*` and `font-synthetic-style` now control GTK terminal style selection and fallback synthesis.
- Ghostty `adjust-cell-width` and `adjust-cell-height` now adjust GTK terminal cell metrics using pixel or percentage values.
- Ghostty text metric adjustments now affect GTK terminal text baseline,
  underline/strikethrough/overline position and thickness, and cursor thickness/height.
- Ghostty `unfocused-split-opacity` and `unfocused-split-fill` now control ForkTTY's inactive split dim overlay.
- Terminal panes now support runtime zoom with `Ctrl++`/`Ctrl+=`, `Ctrl+-`, and `Ctrl+0` without adding persistent font settings.
- Terminal child shells now use Ghostty shell-integration resources when available, including upstream zsh/bash/fish/elvish/nushell startup injection and bundled Linux package resources.
- Linux packages now bundle Ghostty terminfo so packaged terminals can advertise `TERM=xterm-ghostty`.
- Ghostty-backed terminals now use Ghostty's 320MB Kitty image storage default instead of libghostty-vt's lower library default, enable Ghostty's file/temp/shared-memory Kitty image loading media, decode and draw Kitty PNG image uploads, and honor Ghostty `image-storage-limit`.
- Finished terminal selections now format their clipboard payload through `libghostty-vt`'s selection formatter, with the existing GTK frame extraction kept as a fallback.
- Double-click word selection now asks `libghostty-vt` for the word range first, falling back to the GTK frame logic when Ghostty has no selectable word.
- The GTK/Ghostty smoke script now verifies GTK action split/focus behavior, socket split readback, and the socket notification create/list/clear flow.

### Fixed
- GTK terminal panes now keep Ghostty steady cursor styles visible instead of
  hiding every focused cursor during the blink timer's off phase.
- Ghostty config and theme appearance loading now enforces the oversized-file guard before reading or applying colors.
- OSC 99 terminal notifications now decode `e=1` base64 title/body payloads and accumulate same-id `d=0`/`d=1` title/body chunks instead of dropping them as unsupported metadata.
- OSC 99 multipart title/body notifications now keep the title separate from later body updates instead of concatenating both into notification text.
- OSC 99 notification identifiers now follow the protocol identifier character set before ForkTTY tracks or echoes them in replies.
- OSC 99 payload types ForkTTY does not implement are now ignored instead of surfacing as terminal status text.
- OSC 99 terminal notifications with the same `i` identifier now update the existing ForkTTY notification, and `p=close` dismisses that tracked notification.
- OSC 99 single-part `p=title` notifications now use the payload as the notification title, same-id title/body updates preserve the prior title and terminal metadata, and no-id `p=close` closes the default `i=0` notification.
- OSC 99 terminal notification payloads, report ids, same-id routing entries, expiry timers, and icon data are now bounded before decode/allocation so malformed PTY output cannot grow memory indefinitely.
- OSC 99 desktop notifications now reuse the prior OS notification id on updates and close the tracked OS notification on `p=close` or in-app dismiss.
- OSC 99 notifications that request reports now send in-app Open/Dismiss/Clear All activation and close replies back to the source terminal.
- OSC 99 `n=` icon names now feed in-app and desktop notification icons instead of always showing the ForkTTY icon.
- OSC 99 `f=` application names now act as in-app and desktop icon fallbacks when no `n=` icon name is provided.
- OSC 99 `p=icon` binary payloads now render in the in-app notification panel when GTK can decode the image, respect `n=` icon-name precedence, and are passed to desktop notifications through bounded temporary image files under `$XDG_RUNTIME_DIR`.
- OSC 99 desktop icon data is now ignored unless it has a recognized PNG, JPEG, GIF, or WebP signature.
- OSC 99 `o=unfocused` and `o=invisible` notification occasions now suppress notifications for focused or already-visible terminal panes.
- OSC 99 `f` application name and `t` notification type metadata are now retained, can be blocked with `notifications.blocked_terminal_apps` / `blocked_terminal_types`, and are exposed to `notification_command` for external filtering.
- OSC 99 `u` urgency, `w` expiry, and base64 `s` sound metadata now feed desktop notification hints when supported, and positive `w` values also auto-dismiss in-app notifications.
- OSC 99 button payloads now accept base64 text and the spec's U+2028 separator.
- OSC 99 `p=?` and `p=alive` queries now reply to the source terminal with ForkTTY's supported notification capabilities and same-surface live notification ids.
- Sidebar badges, duplicate GTK spawns, closed-terminal status handling, corrupt tab leaves, and scrollback settings copy now handle stale or delayed terminal state without misleading UI or backend readiness loss.
- Persistent terminal scrollback is no longer deleted merely because restore is disabled, session saves now reject serialized data beyond the load cap, and scrollback snapshots are throttled while still flushing on child exit.
- Terminal panes now size rows from one shared widget-measured cell size plus a small vertical guard, preventing agent TUIs from being clipped after resizes without inflating terminal line spacing.
- Terminal styled text runs now fit the terminal cell grid, preventing colored inline-code spans from leaving visual gaps between words.
- Terminal text selection and mouse hit-testing now use GTK content-box coordinates directly, fixing selections that were offset from the pointer.
- Terminal shortcuts, Meta-key input, unread output tracking, OSC99 status updates, browser-pane cleanup, and uppercase HTTP(S) links now behave consistently from focused panes and delayed events.
- Terminal selection finalization now releases the render borrow before formatting through Ghostty, avoiding a RefCell panic when a pointer gesture is released or cancelled.
- Terminal drag selection now snaps at cell midpoints and preserves real one-cell drags, keeping highlights aligned with the pointer.
- Terminal selections now preserve selected whitespace, invalidate select-all payloads on new output, clear stale search highlights, keep adjacent OSC 8 links separated by URI, handle wide-character spacer tails, and avoid drawing scrollback indicators outside tiny panes.
- Terminal mouse release suppression is now tracked per button, avoiding spurious release forwarding during left/middle button chords.
- Agent HUD, workspace closing, worktree, restart, settings reset, and welcome telemetry flows now keep their captured context through delayed UI actions and failure paths.

## [0.2.0-alpha.13] - 2026-06-16

### Security
- Stop hooks now preserve agent permission-mode warnings when the provider has a later session-end cleanup, while providers without session-end hooks still clear the warning on final stop.
- Terminal smooth-scroll handling now rejects non-finite deltas and caps per-event line replay, preventing oversized synthetic scroll events from monopolizing the UI thread while mouse tracking is active.
- Browser automation now injects and evaluates its driver in an isolated WebKit script world, preventing visited pages from detecting or tampering with `window.__forktty`.
- Chromium cookie import now verifies the version-24+ encrypted host digest before accepting decrypted values, rejecting malformed or cross-host cookie rows.
- Agent resume PTY spawns now resolve bare provider commands using only absolute `PATH` entries before applying the recorded session cwd, preventing relative/empty `PATH` entries from executing project-local binaries during restore or resume.
- Socket hook correlation now rejects `hook_session_id` values larger than the metadata text limit before caching them, preventing a local client from retaining many near-request-size session IDs in memory.
- Socket-triggered notification dispatch and custom notification command reaping now use bounded queues instead of spawning unbounded OS threads per `notification.create`, preventing a local client from exhausting threads by flooding notifications.

### Added
- `forktty doctor` now accepts `--hooks`, `--socket`, and `--packaging` scopes for running only the relevant local diagnostics.
- First launch now shows a one-time welcome dialog: an informed (default-on) telemetry toggle linking to the privacy notice, and a one-click "Set up agent integration" button that runs `hooks setup` and `mcp setup`. The first anonymous ping is deferred until this dialog is dismissed, so the toggle is always seen before any data leaves the machine; the welcome is recorded in `$XDG_STATE_HOME/forktty/welcome-seen.json` and the update check is skipped on that first launch.
- The GTK app now sends at most one anonymous daily usage ping when `telemetry.anonymous_ping = true` (the default). The payload contains only schema/kind/app/version/date, can be disabled in Settings or config, and crash uploads remain unimplemented.
- The GTK app now checks GitHub Releases at most once per day when `updates.auto_check = true`, shows update availability in-app, opens the release page for non-AppImage installs, and can self-update writable AppImages after explicit confirmation by downloading the AppImage plus `SHA256SUMS`, verifying SHA256, and atomically replacing the current file.
- Release AppImages can now embed AppImage update information and ship a matching `.zsync` asset when `APPIMAGE_UPDATE_INFO=1` is used during packaging; release CI enables this and includes `.zsync` in `SHA256SUMS`.
- Agent HUD rows now include a Forget action with Undo, so stale tracked sessions can be removed from the HUD without closing the terminal or deleting provider data.
- Agent HUD rows now show an accent unread dot when an agent has produced output you have not viewed since last focusing it, and float those rows up within their lifecycle group — so a finished (idle) agent whose result is still unseen stands out instead of sinking to the bottom of the list.

### Fixed
- The terminal pane header now keeps the Close Pane button on the far right of split-pane headers instead of clustering it beside the split and new-tab actions.
- Worktree list/status reporting now treats registered worktree paths replaced by files, symlinks, or invalid directories as unknown instead of clean/dirty.
- Command palette and Settings selection states now use neutral row highlights instead of heavy accent rails.
- Settings toggles are now smaller and use a subtler checked state.
- Settings sidebar navigation is now more compact, with item descriptions kept in tooltips and accessibility labels instead of visible subtitles.
- Settings pages now use shorter page and section copy, hiding redundant section descriptions.
- Settings, Agent HUD, and Notifications now expose a maximize/restore titlebar control and enforce minimum window sizes, while Command Palette windows stay fixed-size.
- Settings now labels the privacy and reset page as Privacy instead of Advanced.
- Agent HUD now relies on its titlebar close button and Esc instead of showing a duplicate Close footer button.
- Agent HUD now opens shorter when sessions are present, keeps its empty state unclipped, uses flatter rows, and gives row actions clearer visual hierarchy.
- Worktree merge selected by worktree name now merges that worktree's branch even when an unrelated local branch shares the worktree's derived name (e.g. worktree `feat-a` for branch `feat/a` alongside a separate `feat-a` branch), matching the worktree the cleanliness check already validates instead of silently merging the wrong branch.
- Config loading now normalizes a `notification_command` that tokenizes to zero words (for example an inline shell comment like `"# disabled"`) to an empty command instead of rejecting it and quarantining the entire config, so a benign command value no longer resets every other setting to defaults.
- Nested worktree creation now appends `.worktrees/` to `.git/info/exclude` even when the existing exclude file contains non-UTF-8 bytes, instead of failing before creating the worktree.
- Agent session-end hooks now mark the persisted agent binding as ended when clearing its live status, so agents whose providers emit a session-end event no longer remain shown as running in the Agent HUD.
- Chromium bookmark import, browser bookmark loading, and browser profile metadata loading now reject or skip non-regular files before reading, preventing local FIFO/device paths from blocking the import or profile workflows.
- Socket metadata calls now reject stale explicit `surface_id` values even when `workspace_id` is valid, oversized request lines return the documented `payload_too_large` code, and invalid parameter errors use the documented `invalid_param` code.
- Config recovery now quarantines config paths that resolve to FIFOs without blocking application startup.
- Worktree create now propagates branch lookup errors other than `NotFound` instead of treating every libgit2 failure as a missing branch.
- Creating or attaching a worktree whose registration survived an external deletion of its working directory now prunes the stale registration and recreates the worktree in place, instead of failing with an unresolved-path error; `create` adopts the existing branch in this case and never deletes a pre-existing branch during cleanup.
- `ProfileStore::save` now creates the `browser_profiles` directory with owner-only (`0700`) permissions instead of inheriting the umask, so profile metadata is not world-readable on multi-user systems when the directory is first created via the socket API.
- `ProfileStore::save` now hardens an existing `browser_profiles` directory owned by the current uid/gid by removing group/other permission bits before writing profile metadata.
- Browser import spooling now uses anonymous (unlinked) temp files instead of named temp files, so spooled pre-read data is reclaimed automatically if the process is killed mid-import.
- Browser imports now spool pre-read source data to temporary files instead of retaining every selected profile in memory, preventing large all-source imports from exhausting memory while preserving all-or-nothing read validation before writes begin.
- Update checks now honor HTTP-date `Retry-After` headers from GitHub rate-limit responses instead of retrying before the requested deadline.
- Restarting an agent pane now resumes the agent session (provider resume argv and recorded cwd) instead of relaunching a plain shell, matching the session-restore and worktree spawn paths.
- Session restore now quarantines a session file containing invalid UTF-8 instead of returning an error that crashed startup on every launch.
- The first-run welcome dialog now stays open when persisting a telemetry opt-out fails (e.g. a read-only config directory), so it cannot silently fall back to the default-enabled ping without giving the user a chance to fix permissions or re-enable telemetry.
- Saving browser bookmarks now creates the profile directory with owner-only (`0700`) permissions instead of inheriting the umask, matching the history database directory, so bookmark URLs are not exposed when a profile's first write is a bookmark.
- Bookmark entries now bound the stored URL and title to the same size caps as history visits, preventing an oversized imported bookmark from growing `bookmarks.json` until it becomes unreadable.
- Shell trampoline detection now keeps scanning after shell options that take a value, so `bash -o vi -c ...` and `bash --rcfile file -c ...` notification commands are rejected instead of bypassing the `-c` guard.
- Shell trampoline detection now recognizes PowerShell's command grammar, so `pwsh -Command ...`, `pwsh -EncodedCommand ...`, and `pwsh -CommandWithArgs ...` notification commands (and their `-c`/`-e`/`-ec`/`-cwa` aliases) are rejected instead of bypassing the shell-command guard.
- `PtySession::read_until` now reports `UnexpectedEof` when a child exits before the requested bytes arrive, instead of returning partial output as success.
- `forktty hooks test` now sanitizes socket error text before rendering human-readable failures, preventing local socket responses from injecting terminal control sequences.
- Browser import is now limited to the in-app Settings workflow and is no longer advertised or accepted over the socket/CLI automation boundary, preventing local socket clients from using ForkTTY to read external browser profile data.
- Notification command validation now rejects `rbash -c` shell trampolines instead of allowing restricted Bash aliases to bypass the shell-command guard.
- Session restore now quarantines FIFO and other non-regular session paths without blocking application startup.
- Browser history now ignores oversized URLs and truncates oversized page titles before writing to SQLite, preventing web-controlled title or URL churn from causing unbounded history database growth.
- Browser imports now report oversized history URLs as skipped writes instead of counting them as imported rows.
- Browser automation CLI fill now supports `--value-file` (with `-` for stdin) so sensitive values do not have to be exposed in process arguments.
- Removed the raw `browser.eval` socket/CLI command so same-user socket clients can no longer execute arbitrary JavaScript inside browser panes.
- Agent HUD terminal-tail polling now formats only the bounded tail rows instead of dumping the full scrollback each second, preventing noisy agents from freezing the GTK UI while the HUD is open.
- Browser committed-URI synchronization now rejects URLs over the shared 8 KiB browser URL limit while preserving non-hierarchical URLs such as `about:blank`.
- OpenCode hooks now sanitize and size-bound plugin payloads before spawning the ForkTTY CLI, preventing oversized tool output from blocking or crashing the OpenCode process before the CLI stdin cap applies.
- Antigravity hook setup now hardens `~/.gemini`, the config directory, and generated wrapper directory to owner-only permissions before planning or writing executable hook scripts, preventing local users from replacing wrappers through group/world-writable directories.
- Browser profile storage directories are now created with private Unix permissions before WebKit persists cookies, local storage, and cache data.
- Terminal scrollback search now caps stored matches and shows a capped count, avoiding unbounded memory/CPU use on repetitive untrusted terminal output.
- MCP `surface_send_text` is now annotated as destructive and open-world, reflecting that terminal input can execute shell commands or interact with files and networks.
- Browser imports now read all selected source profiles before preparing destinations or writing data, avoiding partial imports when any later source is unreadable.
- Terminal-originated OSC 9/basic OSC 99 notifications are now rate-limited per surface, preventing untrusted terminal output from spamming desktop notifications or repeatedly spawning `notification_command`.
- Session locking now creates and hardens the state directory and lock file with private permissions, preventing other local users from reading or pre-locking `session.lock` to block startup.
- Atomic profile metadata saves now preserve an existing `profiles.json` file mode on Unix when ownership matches, and drop group/other bits when replacing with a temp inode owned by a different uid or gid.
- Browser history databases and SQLite WAL/SHM sidecars are now created with owner-only file permissions inside owner-only profile directories.
- Bookmark files and corrupt-bookmark backups are now saved with owner-only permissions to avoid exposing sensitive URLs to other local users.
- Stale Ghostty event batches from an old pane spawn are now discarded before they can mark a restarted pane not ready, overwrite its terminal status, or emit stale notifications.
- Browser imports now copy temporary SQLite databases and WAL/SHM sidecars into a private `0700` directory with newly-created `0600` files, preventing local temp-file races from exposing browser data.
- Browser-feature socket dispatch fuzz tests now isolate `XDG_DATA_HOME`, preventing adversarial method sweeps from clearing a developer's real browser history.
- `forktty events` now mirrors lag notices to stderr regardless of the JSON object key order used by the socket server.
- Browser import planning now lowercases Unicode titlecase profile names before matching and de-duplicating destinations, preventing duplicate-looking profile creation.
- Command palette shortcut searches such as `ctrl shift c` now match the intended shortcut instead of treating the key token as a fuzzy match against modifier text on earlier commands.
- Ctrl+keypad Home/End/PageUp/PageDown are now reserved for tab-navigation accelerators like their non-keypad equivalents instead of being consumed by terminal input handling.
- Retrying Worktree Create for an already-linked branch no longer deletes that existing worktree and branch if terminal spawning fails.
- Sidebar visibility persistence now rebases onto the latest config under a process-wide update lock, avoiding stale background saves that could overwrite newer settings-dialog changes.
- Malformed browser bookmark files are now moved aside after backup so repeated opens cannot create unbounded backup copies.
- `forktty doctor` once again exits 2 whenever the diagnostics report contains warnings, preserving the documented health-check behavior even without `--strict`.
- Ctrl+click terminal links now open only `http://` and `https://` targets, blocking terminal-controlled `file://` and custom URI handlers.
- AppImage self-updates now create downloaded replacement files with owner-only permissions before checksum verification, closing a local same-group temp-file tampering window under permissive umasks.
- Notification commands using SSH/mosh options that contain `-c` are no longer rejected as shell trampolines, and a ForkTTY binary built without `gtk-ghostty` now exits with failure when asked to launch the GTK app.
- Worktree and workspace rollback paths now close spawned replacement terminals instead of only forgetting bookkeeping entries, preventing untracked terminal processes after cleanup failures.
- OSC 8 hyperlink lookup now caps URI buffers at 8 KiB and fails closed for larger terminal-provided targets, avoiding attacker-controlled memory growth when resolving links.
- Panic logs are now created in a private state directory with owner-only file permissions, and older permissive logs are rotated before new panic entries are written.
- Terminal text snapshot truncation now treats a zero-byte internal limit as an empty, truncated result instead of disabling truncation.
- Terminal spawning now preserves non-UTF-8 working-directory bytes on Unix instead of converting the cwd through lossy UTF-8.
- Large PTY writes now keep waiting after `poll()` reports no writable fd before the per-write deadline, instead of treating the poll timeout as readiness.
- PTY `read_until` now reports `TimedOut` when the requested bytes do not arrive before its deadline.
- Metadata OSC parsing now aborts an unterminated OSC string on a bare `ESC`, so OSC 9 notifications and OSC 99 agent metadata that follow in the same PTY chunk are no longer swallowed.
- The Worktree dialog no longer overwrites a typed Create/Attach branch name when the asynchronous existing-worktree list finishes loading.
- Agent HUD Resume buttons are re-enabled after a failed resume attempt instead of staying disabled until the HUD is reopened.
- Releasing a terminal text selection after wheel-scrolling mid-drag now preserves the scroll-compensated selection endpoint unless the pointer actually moved.
- Bad config/session quarantine paths are now reserved atomically before rename, avoiding races between simultaneous ForkTTY instances.
- Update checks now strip only one leading `v` from GitHub release tags, so malformed tags like `vv1.2.3` are ignored instead of parsed as `1.2.3`.
- Worktree and branch names now reject leading dashes and control characters before reaching git APIs.
- OSC 8 hyperlink lookup now retries with a large enough buffer for long multibyte UTF-8 URIs.
- The MCP stdio server now returns a JSON-RPC parse error and continues after an invalid UTF-8 line instead of ending the session.
- Custom terminal theme colors are now re-applied when an OSC color reset (`OSC 104`/`110`/`111`) follows an aborted OSC sequence in the same output chunk; previously the reset was swallowed as payload of the aborted sequence and the pane kept the wrong colors.
- The MCP stdio server now reads incoming messages through a bounded buffer, so an oversized message is rejected at the 1 MiB limit without first allocating the entire message in memory.
- Clicking an unfocused terminal pane now focuses it *and* lets the same click start a text selection (or reach the application), instead of swallowing the first click so the drag was lost and had to be repeated.
- Scrolling the wheel or touchpad while dragging a selection now keeps the drag anchored to the same text, like drag-autoscroll already did, instead of silently dropping the in-progress selection; a finished selection is still cleared when the viewport scrolls.
- A terminal color reset (`OSC 110`/`111`/`104`) immediately followed by an explicit color set in the same output chunk now keeps the application's color, instead of clobbering it with the re-seeded theme color.
- Hook/MCP socket requests no longer reject a parameter sent as an explicit JSON `null` (e.g. `hook_session_id: null`) with a type error; `null` is now treated as absent, matching the numeric parameter handling.
- A completed worktree merge whose post-commit cleanup fails is now reported as success instead of failure, avoiding a retry that would create a duplicate merge commit.
- An agent hook event now still runs its later cleanup actions (clearing a stale status or permission marker) when an earlier action fails transiently, instead of stopping at the first error.
- The `appearance.terminal_renderer` validation error message now lists `vte`, which is an accepted value.
- Closing a terminal pane no longer risks freezing the UI: the dropped PTY session now reaps its killed child on a background thread instead of blocking the GTK main thread in `waitpid`, which a child stuck in uninterruptible sleep (D state on a dead NFS/FUSE mount) could otherwise wedge forever.
- The PTY read loop now retries a read interrupted by a signal (`EINTR`) instead of surfacing it as a spurious error on every pump tick.
- Large PTY writes now honor their overall deadline even when repeatedly interrupted by signals, instead of being able to retry indefinitely under a pathological signal rate.
- Socket `surface.read_text`/`surface.capture_tail` no longer block a tokio worker thread while waiting for the GTK main loop: the wait is offloaded via `block_in_place`, so many concurrent read requests (as agent hooks issue) can no longer starve the socket server and stall every other request.
- Removing a worktree now deletes its working-tree directory before deregistering it from git, so a failed directory removal leaves a recoverable (git-pruneable) registration instead of stranding the directory permanently with no way for git to find it.
- A failed fast-forward merge rollback now logs the underlying ref-reset/HEAD-restore errors instead of silently discarding them, making a wedged repository diagnosable.
- Re-running `worktree.create` for a branch that already has a ForkTTY-supported linked worktree now reopens that worktree instead of failing on the already-created branch, recovering the crash window between Git worktree registration and ForkTTY session persistence.
- Concurrent nested worktree creation now serializes updates to `.git/info/exclude`, keeping the `.worktrees/` entry idempotent.
- Closing a non-last tab now keeps the model locked through backend close and model removal, so concurrent UI/socket closes cannot observe a half-closed surface.
- Terminal copy, mouse selection, and Select All now omit invisible terminal cells, so escape-hidden text cannot be copied to the clipboard.

## [0.2.0-alpha.12] - 2026-06-13

### Added
- Terminal panes now show terse toasts for copy/paste failures and flash an accent border for visual bell events.
- Terminal panes now show a minimal overlay scrollback indicator while viewing history.
- Terminal content now has balanced 6px inner padding so text does not touch pane edges.
- Split terminal panes now dim unfocused panes slightly so the focused pane is easier to pick out.
- Ctrl+click opens links: OSC 8 hyperlinks and plain `http(s)://`/`file://` URLs in the output (also when wrapped across lines). Hovering with Ctrl held shows a pointer cursor and underlines the target.
- Middle-click pastes the PRIMARY selection (select text, middle-click to paste — the standard Linux flow); Shift+middle-click pastes even inside mouse-tracking apps.
- Shift+PageUp/PageDown page through the terminal scrollback, Shift+Home/End jump to its start/end; inside full-screen apps (vim, htop) the keys keep going to the app.
- Typing or pasting in a scrolled-up terminal now snaps the viewport back to the bottom, like other terminals; output arriving while you read scrollback still leaves the viewport where it is.
- Agent hook status updates now persist a per-surface agent session binding (`agent` + provider `session_id`) when `metadata.set_status` carries `hook_session_id`, plus the provider session cwd as `resume_cwd` when available, giving future resume work stable session state instead of a runtime-only hook cache.
- Persisted agent session bindings now carry lifecycle state (`running`, `idle`, `needs_input`, `ended`, or `unknown`) derived from hook events, giving future hibernation/reclaim work an explicit ended-vs-idle signal.
- Persisted agent session bindings now track hook-derived `last_activity_ms`, so automation can reason about idle age without scraping provider files.
- `agent.list`, `forktty agents`, and the read-only MCP tool `agent_list` now expose persisted per-surface agent session ids, resume cwd, lifecycle, and last activity, so resume/HUD automation can discover Codex/Claude/Gemini/OpenCode/Antigravity sessions without scraping `session-v2.json`.
- `agent.health`, `forktty agent-health`, and MCP `agent_health` now report whether persisted agent sessions have a supported argv-only resume command and provider executable on PATH before attempting a resume.
- `agent.reclaim.plan`, `forktty agent-reclaim-plan`, and MCP `agent_reclaim_plan` now provide a read-only reclaim plan that classifies old idle, locally-resumable agent sessions as candidates and protects running/input-needed/ended/recent/not-ready sessions with explicit reasons.
- `agent.resume`, `forktty resume-agent`, and MCP `agent_resume` now resume a persisted Codex, Claude Code, Gemini, OpenCode, or Antigravity session in a new ForkTTY tab using provider-specific argv-only commands.
- Restored terminal surfaces with a persisted supported agent session now respawn through the provider's argv-only resume command instead of opening a plain shell after a ForkTTY restart; Codex sessions with a persisted hook cwd, or a cwd found in Codex's local `session_meta` JSONL, use `codex resume -C <cwd> <id>` to avoid Codex's resume-directory prompt when the pane cwd differs from the session cwd, and providers without a cwd flag such as Claude Code are spawned with the recorded cwd as their process directory.
- `status.summary`, `forktty statusline`, and MCP `status_summary` now provide a compact read-only workspace summary with persisted agent sessions, status entries, and progress entries for agent statusline/HUD integrations.
- The GTK app now has an Agent HUD in the titlebar and command palette, showing persisted agent sessions across workspaces with lifecycle, last activity, cwd/session context, needs-input highlighting, focus, and resume actions.
- The Agent HUD updates live while open (one-second model re-snapshot that rebuilds rows only when they changed), shows each agent's last terminal output line refreshed in place (generation-gated so idle agents cost nothing), and its rows are keyboard-activatable — Enter or a click on a row focuses that agent's pane.
- Agent HUD needs-input rows now show what the agent is actually waiting on (the hook prompt message, e.g. a permission request) instead of the raw terminal tail, and gain an inline reply entry that types the answer (plus Enter) straight into the agent's terminal without leaving the HUD; the list never rebuilds while a reply is being typed.
- The MCP server now exposes a `forktty://agent/operating-guide` resource and a `forktty_operating_guide` prompt, plus matching initialize instructions, so agents can discover when ForkTTY tools are useful and when to keep working normally.
- `surface.read_text`, `surface.capture_tail`, `topology.tree`, CLI `forktty read-screen`/`capture-tail`/`tree`, and MCP `surface_read_text`/`surface_capture_tail`/`topology_tree` now give agents read-only terminal inspection primitives before they focus or drive another pane.
- `forktty --json hooks doctor <agent>` and `forktty --json hooks test <agent>` are now a stable machine-readable API (documented in SPEC.md): versioned report with an overall `ok`, per-method `{method, ok, error?}` results for `hooks test` (which keeps running after a failed method instead of aborting, so cleanup still happens and the report is complete), and exit code 0/1 reflecting overall health for CI gating. The Codex trust state stays a first-class field of the doctor report.
- The worktree open-workspace boundary rejection now carries the structured error code `precondition_failed` (documented in SPEC.md), and MCP tool errors with a known recovery carry machine-readable `remedy` and `suggested_tool` fields in `structuredContent` — the boundary error points at `workspace_create`, so an agent can recover without parsing prose.

### Changed
- Touchpad scrolling in terminal panes now accumulates smooth deltas instead of forcing chunky wheel ticks.
- The declared Rust MSRV is now 1.96, matching the current `rusqlite`/`libsqlite3-sys` dependency chain required by the workspace lockfile.
- The competitive gap inventory now includes the non-browser cmux gaps plus the additional control-plane gaps found in oh-my-codex and oh-my-claudecode: workflow state/artifacts, team runtime, HUD/statusline export, and agent/skill catalogs.
- Gemini CLI integrations are now legacy opt-in: default `forktty hooks setup` and `forktty mcp setup` skip Gemini and prefer Antigravity, while explicit Gemini setup/remove/doctor/test and persisted Gemini resume compatibility remain supported.
- Claude Code session-start context now includes the same concise ForkTTY tool-use policy as the MCP operating guide: use ForkTTY for panes, agents, worktrees, status, or cross-surface text, but avoid tool calls for ordinary single-repo edits.

### Fixed
- Agent resume now treats hook-reported permission modes as display-only metadata, so forged or stale `bypassPermissions` hook/status updates cannot add dangerous Claude Code or Codex resume flags.
- Copying a soft-wrapped line (a long command or paragraph the terminal wrapped to fit the width) no longer inserts a spurious newline at each wrap point: selection copy, the no-selection viewport copy, and select-all now rejoin soft-wrapped rows into their logical line, so pasting a wrapped command back into a shell runs it as one line instead of splitting it.
- Selecting text by clicking an unfocused terminal pane no longer leaves the selection stuck following the pointer: the focus gesture claiming that first click used to cancel the selection drag without a release, stranding the `selecting` flag, so the highlight kept extending on every mouse move with no button held. The drag now finalizes on gesture cancel and whenever button 1 is no longer physically down.
- Dragging a left-click selection in an agent pane (deferred local drag) no longer aborts the app with a `RefCell already borrowed` panic in the motion handler.
- Terminal pane edge polish: double/triple-clicks and Ctrl+click/Ctrl+hover a few pixels into a pane's trailing gutter now select the last row/word and resolve last-row links instead of doing nothing; the pane being searched no longer dims while its search entry holds focus; middle-click or paste with an empty clipboard no longer shows a spurious "Paste failed" toast; and copying with an active selection still works when the terminal backend fails to render a frame.
- Wayland touchpad scrolling in terminal panes no longer overscrolls roughly 30x: smooth-scroll deltas arrive in surface (logical pixel) units on Wayland and are now converted through the pane's cell height, while X11 smooth deltas and wheel ticks keep the 3-lines-per-tick mapping.
- Scrolling inside mouse-tracking applications (vim, htop, tmux) now forwards one wheel press per three accumulated lines — matching physical-wheel speed — instead of one press per line, and hi-resolution wheels' fractional ticks accumulate into whole presses/lines instead of overscrolling on every fraction of a notch.
- Mouse events in terminal padding now clamp to the nearest grid edge instead of reporting past the last cell to mouse-tracking applications.
- Terminal copy failures no longer clear the existing clipboard contents, and the scrollback indicator no longer risks a panic during very small transient GTK allocations.
- Terminal resize no longer aborts inside libghostty when GTK briefly reports a one-row allocation after wrapped output.
- Terminal resize no longer aborts inside libghostty when maximizing a window with wrapped scrollback; the vendored Ghostty build now uses a temporary cursor-preservation pin and bounded wrap-count walks during column reflow.
- Terminal mouse selection, Ctrl+click link detection, and mouse-tracking coordinates now account for the terminal widget's CSS padding, so highlighted/copied text lines up with the visible character grid.
- Agent terminal panes now keep plain clicks working in mouse-tracking TUIs while treating a real left-button drag as local ForkTTY text selection, so Claude Code/OpenCode-style panes can select text without holding Shift.
- Antigravity `PreToolUse` hooks now return an explicit `{"decision":"approve"}` response, and generated wrapper fallback scripts do the same, so ForkTTY status hooks no longer make `agy` deny every tool call when the hook response is parsed strictly.
- Antigravity agent resume metadata now uses the hook payload's `workspacePaths` instead of the generated wrapper script cwd (`~/.gemini/config`), so `agent-health` reports the real project directory after `agy` publishes a new hook event.
- Session restore path repair now uses the pane tree, not stale persisted surface metadata, to choose the owning workspace directory for browser/SSH surfaces whose saved cwd no longer exists.
- Shell-trampoline detection for `notification_command` now catches `env -u VAR sh -c ...`, `env --unset=VAR sh -c ...`, and `env -S "sh -c ..."` wrappers instead of only plain `env sh -c ...`.
- Closed terminal panes no longer stay alive through GTK controller/search/context-menu reference cycles, so their PTY child and UI timers can be dropped.
- Worktree merge failures now restore the checkout before returning an error, including failed fast-forward ref updates and failures after `repo.merge()`. If the merge commit was already created, a recovered finalization error no longer reports the merge as failed.
- Large PTY writes now retry `poll()` interrupted by signals instead of treating `EINTR` as a fatal partial-paste error.
- Config recovery no longer quarantines a valid config file on transient I/O errors such as permission/read failures.

## [0.2.0-alpha.11] - 2026-06-11

### Added
- MCP tools now declare spec tool annotations (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) so clients can surface risk before invoking: list/status tools are read-only, `worktree_remove` is flagged destructive, `status_set`/`surface_focus` are idempotent, and every tool is closed-world (local instance only).
- New MCP tool `workspace_create` (working_dir + optional name): agents can open a workspace on a repository themselves, which is the precondition the worktree tools enforce — precondition error, `workspace_create`, retry, without leaving MCP.

### Changed
- Every socket CLI subcommand now answers `--help` with its accepted options (generated from the same allow-list validation uses), and `create-workspace` accepts `--cwd` as an alias for `--working-dir`, matching the worktree commands' spelling.
- The worktree open-workspace boundary rejection now names the open workspace roots and the `forktty create-workspace --working-dir <repo>` remedy instead of a bare "cwd must be inside the git repository of an open workspace"; the MCP worktree tool descriptions state the precondition up front and SPEC.md documents the boundary as deliberate.

### Fixed
- Worktree socket operations no longer run their git work on the socket server's async runtime: `worktree.create`/`remove` execute the repo's setup/teardown hook (up to 30s) and every worktree method walks the repository on disk, which pinned tokio workers and could starve other socket connections (agent hook status updates timing out) while worktrees were being created or removed in parallel. The git2 work, the hooks, the open-workspace boundary validation, and the config read now run on the blocking pool.
- Creating a notification through the socket (`notification.create` from the CLI, MCP server, or agent hooks) no longer kills the connection without a response when desktop notifications are enabled: the desktop notifier blocks on its own async runtime, which panics inside the socket server's runtime ("Cannot start a runtime from within a runtime"); dispatch now runs on a dedicated thread.
- MCP list tools (`workspace_list`, `surface_list`, `worktree_list`) no longer fail with strict MCP clients (including Claude Code): the socket returns bare JSON arrays for list methods and the server passed them straight through as `structuredContent`, which the MCP spec requires to be an object. Non-object results are now wrapped as `{"result": ...}`.

## [0.2.0-alpha.10] - 2026-06-11

### Fixed
- The pty master and slave are now opened with `O_CLOEXEC` atomically (`posix_openpt`) instead of setting the flag after `openpty()`: a process forked on another thread in that window (worktree hooks, notification commands) inherited the descriptors and kept the pty alive past its session. Slave duplicates for the child's stdio use `F_DUPFD_CLOEXEC` for the same reason.
- Release binaries no longer inherit the CPU feature set of the CI build machine: libghostty's zig build now targets the generic x86-64 baseline (`-Dcpu=baseline`, via a vendored one-line patch in `vendor/libghostty-rs`). The first alpha.10 cut was built on an AVX-512 runner and its statically linked `memset` crashed every non-AVX-512 machine with SIGILL at startup; the same lottery silently applied to every release since alpha.7.

### Added
- `forktty mcp` now runs a local stdio MCP server exposing ForkTTY workspaces, surfaces, worktrees, notifications, and status metadata as typed tools; `forktty mcp setup/remove` registers the server for Codex, Claude Code, Gemini CLI, and Antigravity while preserving foreign MCP servers (Codex `config.toml` is edited in place, keeping comments and formatting). Claude Code session-start hooks now include ForkTTY workspace/branch context and a short MCP/CLI capability cheat sheet.
- Agent hooks for Antigravity CLI (`agy`, Google's Gemini CLI successor): `forktty hooks setup antigravity` installs a ForkTTY-owned `"forktty"` group in `~/.gemini/config/hooks.json` for the verified `PreInvocation`/`PreToolUse`/`PostToolUse` events, plus generated wrapper scripts (Antigravity executes a hook command as one bare executable path, without arguments or a shell). Sessions are correlated via `conversationId`, hook responses use the strict-`protojson`-safe `{}`, and `hooks doctor antigravity` reports the launcher state from the generated scripts. Gemini CLI hooks remain supported.
- `forktty hooks doctor codex` now reports `trustCheck`: Codex requires per-hook trust approval (recorded under `[hooks.state]` in its `config.toml`) before running installed hooks, so the doctor lists events with no approval record yet and points at `/hooks` inside Codex.

### Changed
- `forktty hooks setup claude` now installs lifecycle hooks by default and omits the blocking per-tool `PreToolUse`/`PostToolUse`/`PostToolUseFailure`/`PostToolBatch` hooks; use `forktty hooks setup --full claude` to restore the previous full profile. Existing installs keep working, and re-running setup migrates Claude hooks to the lifecycle default unless `--full` is passed.
- Chromium bookmark import now deserializes only the bookmark fields ForkTTY uses, avoiding large extra allocations for ignored browser metadata.

### Security
- Pasted text is now encoded with libghostty's paste encoder, which neutralizes control bytes in the payload: clipboard content containing `\x1b[201~` can no longer terminate the bracketed-paste wrapper early and inject the remainder as typed input (including command execution in a shell).
- A socket client that sends a request and then stops reading the response is now disconnected after a write timeout instead of holding one of the 64 connection slots forever; enough stuck clients used to deny the socket to agent hooks.

### Fixed
- Agent hook status updates that lose `FORKTTY_WORKSPACE_ID`/`FORKTTY_SURFACE_ID` after session start (for example through tmux, ssh, or containers) can now be attributed back to the originating pane by `hook_session_id` when the socket server has seen that session with explicit targets.
- "Merge Worktree" now works when invoked from inside a linked worktree (it used to always fail with "Cannot resolve admin directory"), and fast-forward merges update the main checkout's files instead of only moving the branch ref.
- Large pastes (bigger than the kernel pty buffer, ~12KB) are no longer silently truncated; the terminal now waits for the child to drain its input, with a 10s safety timeout. Automatic VT query replies sent over the same path can no longer be cut off mid-sequence either.
- Splitting panes beyond the persistable depth (6 nested splits) is refused instead of silently breaking every subsequent session autosave and losing the layout on restart.
- Ctrl with non-letter keys (Space, `[`, `\`, `]`, `^`, `_`, `?`) now sends the standard C0 control codes instead of nothing.
- In maximize mode, focus changes that don't alter the pane tree (command palette "Focus Next Pane", socket `surface.focus`) now switch the visible pane instead of leaving a stale one on screen.
- The Close Pane confirmation now closes the pane it was opened for, even if a socket client switched the active workspace while the dialog was open.
- Piped CLI output (`forktty list --json | head -1` and similar) no longer panics with exit 101 and a panic.log entry when the consumer closes the pipe early.
- "Reset Terminal" no longer reverts the pane to libghostty's default colors; the configured theme is re-applied and stale paste/focus mode state is cleared.
- A configured shell or notification command that is temporarily missing from disk no longer quarantines the whole config and resets every setting to defaults; the value is normalized (default shell / cleared command) on load instead.
- The socket server keeps serving through transient `accept()` failures such as file-descriptor exhaustion (EMFILE/ENFILE) instead of shutting down for the rest of the app's lifetime.
- Two concurrent closes of the last workspace (or a close racing a worktree removal) no longer leave a duplicate replacement workspace.
- Replacing a pane or workspace context menu no longer risks a re-entrant `RefCell` crash from the popover's synchronous `closed` signal.
- Saving a setting from the Settings dialog no longer reverts config changes made outside the dialog while it was open (e.g. the F9 sidebar toggle); each change is rebased onto the config on disk.
- `forktty doctor --socket/--verbose/--debug` now explains that `doctor` runs locally and that the socket doctor takes global flags first (`forktty --json doctor`); the CLI help documents the reachable spelling.
- Hook event ordering now uses CLOCK_BOOTTIME instead of the wall clock, so hook status updates are no longer silently dropped after the system clock steps backwards; orders from different clock sources are no longer compared against each other.
- `forktty hooks` with a missing or typoed subcommand now exits with a helpful error instead of printing hook continue-JSON and exiting 0.
- `worktree-status` rejects combining a positional path with `--path`/`--cwd` instead of silently ignoring the positional, matching its sibling commands.
- `forktty --json doctor` and `forktty hooks setup` no longer hang forever when an inspected config path is a FIFO.
- Socket connections from the CLI and agent hooks are bounded by a connect timeout instead of hanging when the app's accept backlog is full.
- A poisoned model lock no longer makes the socket event stream broadcast false removal events for every workspace and surface.
- Workspace cwd validation no longer runs git repository discovery while holding the model lock shared with the UI thread, and the startup socket probe now has an overall deadline instead of only per-read timeouts.
- Shell-trampoline detection for the notification command now catches clustered flags (`bash -lc`), separated flags (`bash -x -c`), and `env`-wrapped shells (`env bash -c`).
- A child process flooding its terminal can no longer hold the UI in a single unbounded read; terminal reads are capped at 1MiB per pump tick.

## [0.2.0-alpha.9] - 2026-06-11

### Changed
- The Settings dialog was reorganized and rebuilt with standard GNOME preference rows: the terminal palette now lives on the Terminal page next to font and scrollback, the Alerts page is named Notifications, enum options (palette, window mode, sidebar position, worktree layout, font family) use combo rows with explicit value mapping instead of free-form combo boxes, font size and scrollback use spin rows with the validated bounds, the font picker is searchable, and an invalid shell or notification command now marks the row in red in addition to the error toast.
- Dropped the vendored `libghostty-vt-sys` copy: upstream libghostty-rs now makes the zig optimize mode follow the Cargo profile (uzaaft/libghostty-rs#55), so both libghostty crates are pinned to upstream master via `[patch.crates-io]`, with `LIBGHOSTTY_VT_SYS_OPTIMIZE=ReleaseSafe` pinned in `.cargo/config.toml` so debug and test builds keep the optimized VT parser. The lines-to-bytes scrollback conversion stays on our side (upstream C API issue, uzaaft/libghostty-rs#56).

### Added
- Double-click selects the word under the pointer (ghostty's default word boundaries, so paths like `/tmp/a.txt` select whole) and triple-click selects the visual row; both publish to the PRIMARY selection like a finished drag and work with Shift inside mouse-tracking apps.
- Dragging a selection past the top or bottom edge of a pane now autoscrolls the viewport (faster the further the pointer goes) and keeps extending the selection; the selection is re-anchored under the scroll instead of being dropped.
- Panics now also append message, location, and backtrace to `$XDG_STATE_HOME/forktty/panic.log` before the process dies, so field crashes (which abort inside GTK signal trampolines) no longer require coredump symbolization to diagnose.
- The session state is now guarded by a lock file: a second running instance (e.g. a deb-installed and an AppImage forktty that DBus could not deduplicate) refuses to start instead of silently fighting the first one's session autosave.

### Fixed
- An empty or malformed OSC 99 sequence no longer leaves a debug-formatted "Terminal metadata" status entry in the sidebar; it is ignored.
- Terminal pixel-size reports now keep the measured cell dimensions after pane-only cell resizes instead of reverting to the 10x20 startup fallback.
- Terminal OSC 110/111/104 color resets now restore ForkTTY's configured theme defaults instead of falling back to libghostty's built-in black/background palette.
- Dead-key and compose-key input now has the GTK input-method handoff documented at the terminal key fallback, clarifying why committed text is not duplicated by fallback key encoding.
- The search bar's match counter no longer shows a stale "current/total" after new terminal output removes the match highlight; it resets to 0/0 until the next step.
- Two processes quarantining the same corrupt profile/bookmark store at the same time can no longer overwrite each other's backup file.
- The sidebar toggle and the periodic PR lookup no longer read the config file on the GTK main thread, and the 2s session autosave no longer builds a debug dump of the whole session to detect changes.
- An `events.subscribe` client that stops reading is now disconnected after 10s instead of holding one of the 64 socket connection slots forever; enough stuck subscribers used to silently deny the socket to agent hooks.
- The event stream now reports a surface whose owning workspace changes (session-restore repair) by re-asserting it as removed + added; subscribers' per-workspace surface lists used to go silently stale.
- `forktty events` flushes every event line, so piped consumers (`| jq`, `| while read`) see events as they happen instead of in 8KB bursts; when the server drops events because the consumer lags, a warning now also lands on stderr.

### CI
- CI now verifies on every push that the binary carries the RUNPATH it needs to find the bundled libghostty, and the release smoke test runs the packaged binaries exactly the way the alpha.7 field failures did: the deb tree and the extracted AppImage's inner binary without any `LD_LIBRARY_PATH`, plus the `FORKTTY_APPIMAGE` exports in AppRun.

## [0.2.0-alpha.8] - 2026-06-10

### Added
- Added scrollback search: Ctrl+Shift+F (also in the command palette) opens a floating per-pane search bar with case-insensitive matching over the full scrollback, wrapping next/previous navigation, a match counter, and highlight-on-jump that feeds the copy shortcut.

### Changed
- The focused pane's cursor now blinks at the conventional cadence and snaps visible on keystrokes; unfocused panes keep a steady hollow cursor.
- Pane headers now slide in and out over 180ms when a workspace transitions between single-pane and split layouts.
- Split dividers tint with the accent color while being dragged, and the worktree dialog's mode selector uses the accent for the selected mode.
- libghostty is now compiled optimized (ReleaseSafe): upstream's build script left zig in Debug mode, so terminal output parsing ran ~870x slower than it should have (a 64 KiB burst took ~1 second; `cat` of a large file crawled at ~65 KB/s).
- The `events.subscribe` stream now emits `workspace_renamed` when a workspace changes name; subscribers previously kept the stale name forever.

### Fixed
- Scrollback now actually retains the configured number of lines: the limit was being passed to ghostty as a byte budget, so 10k configured lines kept only a few dozen rows of history.
- Scrollback search no longer re-dumps the entire scrollback on every keystroke and every Enter; matches are cached until terminal content or the query changes, keeping search instant even at 100k lines.
- Ctrl+Shift+C with nothing selected now copies the visible screen instead of silently filling the clipboard with the entire scrollback history.
- Mouse clicks and selection highlights no longer drift from the painted text grid with fonts whose metrics round differently (input mapping and rendering now share one cell measurement).
- Launching forktty while it is already running now presents the existing window instead of building a second window, workspace model, and socket server (which used to steal the IPC socket from the running instance).
- The quake window re-derives its size from the current monitor on each show instead of keeping its launch-time geometry after dock/undock or resolution changes.
- The IPC socket is now bound via a private staging directory instead of flipping the process-wide umask, which could corrupt files created concurrently by other threads.
- Wheel scrolling a pane whose application does not track the mouse (a plain shell prompt) aborted the whole app: the scroll handler double-borrowed the terminal runtime and the panic could not unwind across the GTK signal trampoline. Tracking-aware apps (tmux, vim, htop) were unaffected, which made the crash look random.
- AppImage: `forktty` CLI calls from shells inside the app (agent hooks, `forktty ping`) failed with "error while loading shared libraries: libghostty-vt.so.0"; the binary now locates the bundled library itself (RUNPATH `$ORIGIN/../lib`). Agent hooks set up from inside the AppImage now reference the stable `.AppImage` path instead of the temporary `/tmp/.mount_*` mount, which broke on the next launch.

## [0.2.0-alpha.7] - 2026-06-09

### Added
- Added mouse text selection in terminal panes: left-drag selects when application mouse tracking is off, Shift+drag overrides tracking-aware apps (vim, htop), the selection is highlighted with the theme highlight color, and the extracted text feeds Ctrl+Shift+C and the primary clipboard (middle-click paste).
- Surfaced OSC 9 and OSC 99 terminal escape notifications as ForkTTY notifications.

### Changed
- Replaced the VTE terminal backend with libghostty-vt and a custom GTK renderer: ghostty-driven key encoding, cursor styles, wide-cell rendering, OSC 8 hyperlinks, text decorations, bracketed paste tracking, focus reporting, mouse routing, configurable scrollback, and theme-color seeding.
- Worktree operations (list/create/attach/merge/remove) now run their git work off the GTK main thread, so slow repositories no longer freeze the UI; the worktree dialog opens immediately and populates its worktree chooser asynchronously.

### Fixed
- Terminal children now acquire the pty as their controlling terminal (`TIOCSCTTY`), fixing `/dev/tty` consumers such as fzf, less, and ssh/sudo prompts under zsh.
- The pty master fd is now CLOEXEC, so spawned children (and any other subprocess) no longer inherit one extra descriptor per open terminal.
- The PTY pump timer stops after the child exits instead of polling a dead pty at 60Hz per closed pane, and closed panes now release their terminal runtime.
- Closing a pane now focuses the adjacent sibling instead of teleporting to the first pane, and closing a background pane no longer steals focus; a stale Close Pane confirmation can no longer close the wrong pane.
- The IPC socket server survives transient `accept()` errors instead of shutting down for the rest of the session, and `forktty events` bounds its subscribe handshake so a wedged server cannot hang the CLI.
- Fixed terminal theme seeding (fresh surfaces rendered ghostty's built-in colors), CSS provider accumulation, Shift+Tab/Alt+Backspace encoding, and a panic on non-ASCII hex color strings in configs.
- Hardened agent hooks: the OpenCode plugin caps payload recursion (deeply nested MCP tool responses could crash the host session) and `hooks setup` warns before replacing a non-map `hooks` config value.

### CI
- Restored CI after the runner image dropped the `zig` apt package: Zig is now installed via the pinned `setup-zig` action.

## [0.2.0-alpha.6] - 2026-05-30

### Added
- Added `events.subscribe` NDJSON streaming and `system.capabilities` discovery, with `forktty events` and `forktty capabilities` CLI entry points.
- Added an optional source-build browser-pane path behind the `browser` feature: WebKitGTK6 pane surfaces, socket/CLI open/navigate/snapshot/click/fill/eval/back/forward/reload verbs, GUI open/close controls, persistent per-profile WebKit sessions, and browser profile CRUD.
- Added per-profile browser history and bookmark stores plus `browser.history.*` / `browser.bookmark.*` socket verbs and `forktty browser history|bookmark` CLI mirrors; GTK address-bar/history integration remains follow-up work.
- Added browser import via the new `forktty-import` crate: `browser.import.discover`/`preview`/`run` socket verbs, `forktty browser import discover|preview|run` CLI, and a Settings "Import Browser Data" dialog that imports history and bookmarks from local Firefox/Chromium-family profiles (cookies are preview-only, not yet written) with rollback on failure.
- Added SSH remote workspaces: `SurfaceKind::Ssh` panes spawned as `ssh <host>`, the `workspace.create_ssh` socket method, a `forktty ssh` CLI, sidebar `ssh:<host>` hints, and respawn on session restore.
- Added per-pane tabs: `pane.new_tab`/`pane.select_tab` socket methods, `forktty new-tab`/`select-tab` CLI, and pane-chrome/command-palette tab controls.

### Changed
- Promoted the AppImage from an experimental smoke-test artifact to the primary portable Linux download while keeping host-runtime caveats for glibc, GSettings/GIO, fontconfig, desktop services, and GPU drivers.
- Packaged builds, CI, and release QA now use `--features browser`, so browser panes and browser import ship in the `.deb` and AppImage.
- Updated the RustCrypto stack (`aes`, `cbc`, `hmac`, `pbkdf2`, and `sha1`) together with the cookie decryption API changes.
- Renamed Linux desktop/AppStream metadata to the reverse-DNS `dev.forktty.forktty` desktop id and refreshed app/icon assets across installed sizes.
- Refined GTK topbar, settings, about, notifications, workspace/sidebar, tab, pane, and drag-and-drop visuals for a more consistent native dark UI.

### Fixed
- Fixed `forktty ssh <user@host>` routing so the documented CLI command reaches the socket handler instead of being rejected as an unknown argument.
- Fixed mixed/phantom drag highlights by using typed drag-and-drop payloads for tabs, panes, and workspaces, with clearer drop acceptance.
- Fixed pane navigation/swap desync handling so a missing focused surface no longer falls back to pane index 0.
- Hardened session restore and config persistence with XDG state-dir migration, atomic saves, directory fsync, quarantine of corrupt/oversized files, and allocation-free pane-surface lookup.
- Hardened socket CLI reads, import readers, browser profile import, worktree lifecycle rollback, and terminal AppImage hook launching against oversized input, unsafe paths, stale handles, and AppImage runtime leakage.
- Fixed release and packaging docs so desktop validation paths, feature flags, and packaged artifact expectations match CI and the build scripts.

### Security
- Strengthened local robustness by bounding socket responses, browser/import file reads, config/session loads, and stdin payloads, while preserving owner-only Unix socket behavior and argv-based command execution.

### Documentation
- Audited Markdown docs against the current Rust workspace, scripts, feature gates, socket methods, browser profile/storage behavior, packaging flow, and support links, and brought SPEC/ROADMAP/cmux-gap docs in line with the shipped SSH workspace, per-pane tab, and browser import surfaces.

## [0.2.0-alpha.5] - 2026-05-23

### Added
- Added native Rust socket CLI and hook installer/test/doctor support inside the `forktty` binary, replacing the legacy Node.js CLI and making AppImage hook flows independent of a source checkout.
- Hook handling now surfaces Codex/Claude `permission_mode`, Claude risk colors, session ids, and supported events for richer local automation.

### Changed
- The socket CLI and agent hook bridge now run natively inside the `forktty` binary. `forktty hooks setup` installs hook commands that call the stable `forktty` launcher directly, so AppImage users no longer need a source checkout or Node.js for hook installation/execution.
- Packaging and release checks now align `.deb`, AppImage, and `SHA256SUMS` asset names, pin AppImage smoke-test tooling/packages, and ship consistent desktop/AppStream runtime metadata.
- README, hooks, native GTK/VTE, release QA, and contributor documentation now describe the prebuilt artifact flow, `forktty doctor`, and the native hook diagnostics.

### Documentation
- Restructured README install instructions around prebuilt AppImage and `.deb` artifacts, with a dedicated "Build from source" section and a first-run / troubleshooting flow that points at `forktty doctor`.
- Documented `forktty hooks doctor <agent>` and `forktty hooks test <agent>` in the README and `hooks/README.md`.
- Clarified that the experimental AppImage bundles GTK4/libadwaita/VTE via the `ldd` graph but still depends on the host's glibc, GSettings/GIO data, fontconfig, and desktop session services.

### Fixed
- `surface.send_text` now waits for terminal readiness before writing text, preventing early sends from racing pane startup.
- Session persistence now keeps saves working when the state path is a broken symlink and repairs duplicate or leafless pane trees before they can poison restore state.
- Codex and Claude hook timeout values are interpreted as seconds, and `forktty hooks doctor` reports stale launcher paths.
- GTK polish/stability fixes tightened the alpha pill, status/sidebar labels, command-palette and popover accent treatment, pane titles, settings layout, and destructive confirmation target names.

## [0.2.0-alpha.4] - 2026-05-22

### Added
- Added experimental AppImage packaging via `scripts/build-appimage.sh`, producing a tagged AppImage artifact under `target/packaging/appimage/` alongside the existing Debian package.
- Unread counter badge on the notifications toolbar button so the queue depth is visible without opening the panel.
- Configurable VTE terminal theme presets via `appearance.terminal_theme`, with System, Catppuccin Mocha, Rose Pine, Tokyo Night, Dracula, and Gruvbox Dark choices exposed in Settings.

### Changed
- GitHub release packaging now builds and uploads both the `.deb` and the experimental AppImage, with a shared `SHA256SUMS` file covering both artifacts.
- The README download link now points directly to the alpha.4 AppImage as the default downloadable artifact, while keeping the Debian package documented.
- Refreshed the README screenshot and updated terminal environment documentation after the alpha.3 release.
- Consolidated the shell-trampoline, executable-file, and worktree-name validators into a single `forktty_core::command_safety` module so the socket layer, GTK shell, and notification dispatcher cannot drift apart on the same security rules.
- Socket dispatch errors now carry structured codes (`method_not_found`, `missing_param`, `not_found`, `payload_too_large`) instead of the catch-all `error` code, so clients can branch on outcome rather than parsing message text.
- `surface.send_text` now rejects payloads larger than 256 KiB with a `payload_too_large` response instead of blocking the dispatch task on a wedged VTE pipe.
- GTK shell visual-polish pass: tightened sidebar / pane header / topbar / status-bar contrast and hierarchy, libadwaita-native header separator, neutral "exited" badge, premium focus rings and inner shadows on form controls, minimal overlay scrollbars, an 8 px / 16 px dialog spatial grid, tactile button feedback, settings dialog label/subtitle wrapping, and softer needs-input emphasis so the active workspace and pane read as the primary anchors.
- Audited project documentation: SPEC now lists the socket error codes and the `surface.send_text` cap, the ROADMAP no longer interleaves implemented appearance work with backlog items, and the stale `.jules/bolt.md` note targeting the removed React sidebar was removed.

### Fixed
- Resolved terminal font discovery through GTK/Pango instead of spawning `fc-list`/`fc-match` by name, removing a PATH-hijack risk when ForkTTY is launched from an untrusted environment.
- `forktty close-workspace <name-with-dash>` no longer misroutes to a workspace id lookup; the CLI now tries the positional selector as an id first and falls back to the name, matching `focus`.
- Notification dispatch no longer silently swallows config-load errors; a broken `config.toml` now logs the underlying cause before falling back to defaults.
- Socket connection-loop I/O failures are now logged to stderr instead of being silently dropped, so socket-layer regressions are visible without attaching a debugger.
- Session restore now logs the reason it quarantined a session file (parse failure, validation failure, oversized, or not a regular file) instead of silently moving it aside, so a session that fails to come back up is debuggable from stderr.
- `forktty hooks setup` now writes the agent config files atomically (tmp + rename) instead of truncate-then-write, eliminating the corruption window on SIGKILL or power loss. A `--dry-run` flag prints the would-be result without touching disk, and malformed existing configs now report which agent and path failed instead of bubbling up a raw `SyntaxError`.
- VTE `child-exited` and `bell` signals no longer create notifications when the user has already closed the originating pane, and `child-exited` is now latched per-surface so a duplicate emission from VTE cannot generate two "Terminal exited" notifications. Session restore also re-runs the workspace invariant repair as a defensive pass, matching what `save_session` already does.
- GTK font picker no longer collapses families whose synthesized IDs would collide with another real family, so every installed font is selectable.
- Sidebar refresh no longer races a closing workspace context popover, which previously could leave the sidebar pointing at a stale workspace entry.
- Worktree context menu actions now target the workspace the menu was opened on instead of the currently focused workspace.
- Workspace-scoped notifications (no specific surface) once again raise workspace attention reliably and clear it on read.
- Closing a pane preserves the workspace pane-tree invariants when the closed pane was the focused leaf of a deeper split, preventing a stale focused-surface id after collapse.

## [0.2.0-alpha.3] - 2026-05-15

### Added
- Rebuilt Settings with native libadwaita preferences pages/groups and added terminal scrollback and audible-bell controls.

### Changed
- Rebalanced the built-in VTE color palettes with a softer terminal background and full ANSI colors instead of relying on saturated VTE defaults.
- Aligned VTE child sessions with terminal conventions by advertising `COLORTERM=truecolor`, app identity variables, system cursor blink, hyperlink support, and non-bright bold text.
- Added standard terminal text actions for Select All and Reset/Clear to shortcuts, the command palette, and the terminal context menu.
- Reset/Clear now asks the child shell to redraw with `Ctrl+L` after clearing VTE state, so users return to a clean prompt instead of a blank pane.
- Softened the active-pane border so split-pane focus remains clear without drawing a heavy purple frame around the terminal.
- Moved the GTK polish design note into `docs/design/` and removed stale GTK/Tauri-era repository artifacts.
- Workspace-scoped notifications without a surface target now raise workspace attention until they are read or dismissed.
- Updated GTK/runtime helper dependencies (`gtk4`, `global-hotkey`, and `libloading`) after validating the GTK/VTE build and Debian package.

### Fixed
- Scoped global terminal clipboard shortcuts to the VTE widget that currently owns GTK focus, preventing stale-pane paste/copy when a dialog or search entry is focused.
- Avoided a GTK/Wayland crash when restoring sessions with three or more VTE panes by deferring terminal focus until widgets are rooted and cancelling stale pane-ratio tick callbacks after rebuilds.
- `Open Latest` in the notification panel now resolves the current latest openable notification at click time, so dismissing a notification cannot leave the button targeting a removed item.
- Cleared the persisted workspace attention badge on session restore so freshly restarted sessions no longer show stale unread state when no surfaces are unread and no notifications carry over.
- `Ctrl+Shift+W` and the close-pane button now succeed when the underlying terminal has already exited; the model surface is removed even if the backend reports it as `NotFound`, matching the socket close path.
- Rejected hand-edited session files that disagree about which workspace is active (multiple `active: true` flags, or a flag pointing to a workspace different from `active_workspace_id`) so loads quarantine corrupt state instead of silently picking one.
- Dropped the stale `version` field from `package.json` (Cargo workspace is the source of truth; the package was already `private: true`) to stop the two version strings drifting apart between releases.

## [0.2.0-alpha.2] - 2026-05-15

### Added
- Added a README screenshot of the GTK/VTE app running on Ubuntu.
- Added a release QA checklist for GTK/VTE runtime and Debian package smoke testing.
- Added an existing-worktree chooser for Merge and Remove in the worktree dialog.

### Changed
- Removed the Ubuntu Docker development wrapper from the main workflow; native dependency installation and CI remain the supported build paths.
- Updated README release links to point directly at the current prerelease.
- Opening the notification panel now marks notifications read while preserving history.

### Fixed
- Added GTK actions for terminal copy/paste so `Ctrl+Shift+C` and `Ctrl+Shift+V` target the focused VTE pane.
- Moved terminal context menus out of clipped pane widgets so right-click paste remains reachable in heavily split layouts.
- Added per-notification dismiss so users do not have to clear the entire notification list.
- Dismissing the last notification now collapses the panel to the empty state and disables the Clear All and Open Latest actions.
- Closing the last unread pane in a workspace now clears the workspace's attention badge instead of leaving it pinned to a removed surface.
- Retried transient text-file-busy hook spawns so freshly checked-out worktree hooks do not flake under CI load.

## [0.2.0-alpha.1] - 2026-05-14

### Architecture
- Replaced the old Tauri/React/WebKit runtime with the native GTK4/libadwaita/VTE implementation as the primary app.
- Removed the legacy frontend, Tauri backend, Vite/TypeScript build, and npm dependency tree from the main code path.
- Installed the native binary and Debian package as `forktty` instead of `forktty-gtk`.

### UI
- Added the native GTK shell with compact header, product wordmark, workspace sidebar, recursive split panes, global status bar, command palette, settings, notification panel, keyboard shortcut reference, and context menus.
- Added the refreshed ForkTTY app icon used by README, desktop integration, notifications, window chrome, and About dialog.
- Added workspace rename support from the workspace context menu and command palette.
- Added sidebar toggle persistence, theme selection, sidebar visibility setting, reset-to-defaults staging, destructive confirmations, and improved empty/error states.
- Polished pane chrome with single-pane header hiding, hover/focus-revealed pane actions, duplicate CWD suppression, active pane indicators, and terminal placeholder recovery actions.

### Terminal
- Moved terminal spawning to GTK/VTE realization to avoid Wayland/VTE startup crashes and duplicate shell spawns.
- Restored sessions now rebuild panes incrementally instead of spawning every VTE surface in the same main-loop turn.
- Clean terminal exits no longer create noisy warning notifications.
- Added safer quake mode fallback to a normal decorated window when layer-shell support is unavailable.

### Reliability
- Fixed `workspace.close` to close by the resolved workspace ID so surface cleanup and model mutation cannot diverge on ambiguous selectors.
- Limited VTE prompt fallback scanning to a bounded visible tail instead of copying the full terminal text on every contents-changed signal.
- Added immediate session saves after workspace and pane mutations.
- Added config-load and session-restore user-facing error notifications.

### Tooling
- Replaced Vitest/Vite frontend checks with Node built-in CLI tests.
- Updated CI, dependency review, security audit, desktop entry validation, and Debian packaging for the Rust GTK/VTE stack.
- Debian prerelease package versions now use Debian ordering (`0.2.0~alpha.1`) while Cargo and GitHub use SemVer (`0.2.0-alpha.1`).

### Known Limitations
- Linux only.
- The first alpha ships a `.deb` package. AppImage packaging is deferred until the native GTK/VTE bundle can be tested reliably.
- PTY processes and scrollback are not preserved across restart; restored sessions spawn fresh shells.
- Quake global shortcuts and layer-shell placement depend on desktop/compositor support.

## [0.1.2] - 2026-05-11

### Documentation
- Updated README, SPEC, ROADMAP, SECURITY, and PRIVACY to match current UI polish, session restore, config validation, notification, worktree, AppImage, and test coverage behavior
- Clarified that `notification_command` still supports static argv arguments after the required absolute executable path; a no-arguments policy remains a future hardening item

### UI Polish
- Refined WelcomeScreen, modal focus behavior, and empty/loading/error states across key frontend surfaces
- Added safer focus defaults for destructive modals

### Reliability & Security
- Session restore now validates persisted pane trees and quarantines corrupt or invalid session files instead of failing startup
- Restored sessions suppress spurious prompt notifications during startup
- Config loading for ForkTTY's TOML config is bounded to regular files up to 1 MiB
- Ghostty config and theme loading now ignores missing, non-regular, oversized, or unreadable files instead of reading them unbounded
- Shell and notification command configuration now validate executable paths more defensively
- AppImage packaging normalizes root desktop/icon symlinks, rejects unsafe icon values, and refuses absolute root symlinks
- Socket request reading now enforces the 1 MiB line limit without relying on `BufReader::lines()`

### Tests & Tooling
- Added frontend and Rust coverage for restore, notification, config, and packaging hardening paths
- Refreshed dependency and tooling versions where relevant

## [0.1.1] - 2026-04-23

### UI Polish
- Refined sidebar, pane chrome, command palette, branch picker, notifications, settings, menus, and find bar with a more consistent dark desktop visual language
- Split UI typography from terminal typography: proportional font for chrome, monospace for terminal content, shortcuts, and badges
- Added explicit inactive-pane dimming and more restrained focus/unread states
- Added extra breathing room around terminal surfaces without changing PTY behavior
- Replaced placeholder text controls with shared SVG iconography
- Added `prefers-contrast` and `prefers-reduced-motion` polish for dark-theme accessibility

### Interaction Fixes
- Help & Shortcuts menu now renders above the sidebar correctly instead of appearing behind other UI
- Workspace switching from the sidebar triggers earlier and feels more immediate
- Workspace name hover now shows the text cursor only over the actual name, not across the full row
- Workspace reordering now uses a dedicated drag handle instead of making the whole row draggable
- Reduced duplicate prompt notifications with stronger switch-time suppression and short-window deduplication
- Avoid repeated `Prompt waiting` notifications while a workspace is already unread

### Socket & Worktree Hardening
- Fixed socket-driven `worktree.create` prompts being written twice to the target PTY
- Fixed removal of the last worktree-backed workspace so the replacement workspace falls back to a valid repository root instead of a deleted directory
- Relaxed socket `cwd` validation to accept subdirectories and linked worktrees from the same open repository while preserving repo-boundary checks

## [0.1.0] - 2026-03-19

### Phase 1 — MVP Terminal
- Tauri v2 + React 19 + TypeScript scaffold
- portable-pty PTY management with Tauri Channel streaming
- xterm.js terminal with Canvas renderer (WebGL fallback disabled due to WebKitGTK bugs)
- Full TUI support (htop, vim, less all render correctly)
- Terminal resize via ResizeObserver + FitAddon

### Phase 2 — Multi-Pane Splits
- react-resizable-panels recursive split layout (horizontal/vertical)
- Zustand store tracking PaneTree structure and focus
- Keyboard: Ctrl+D (split right), Ctrl+Shift+D (split down), Alt+Arrow (navigate), Ctrl+W (close)

### Phase 3 — Sidebar + Workspaces
- Sidebar showing workspace list with metadata (branch, directory, status)
- Workspace creation (Ctrl+N), switching (Ctrl+1..9), closing (Ctrl+Shift+W)
- Git branch detection via git2

### Phase 4 — Git Worktree Integration
- git2 crate for native worktree create/merge/remove
- Setup/teardown hook support (.forktty/setup, .forktty/teardown)
- Worktree layout config (nested/sibling/outer-nested)
- Sidebar worktree status badges (clean/dirty/conflicts)

### Phase 5 — Notification System
- OSC 133 shell integration parsing in Rust backend
- Pattern matching for Claude Code prompt detection
- In-app blue dot + unread count on sidebar
- Desktop notifications via notify-rust (XDG/D-Bus)
- Notification panel (Ctrl+Shift+I), jump to unread (Ctrl+Shift+U)

### Phase 6 — Socket API
- Unix domain socket JSON-RPC server (tokio)
- 22 methods: system.ping, workspace.*, surface.*, notification.*, worktree.*, metadata.*
- Environment variables set in spawned shells (`FORKTTY_WORKSPACE_ID`, `FORKTTY_SURFACE_ID`, `FORKTTY_SOCKET_PATH`)

### Phase 7 — Theming + Config
- Ghostty config parser with theme file and palette support
- TOML config at ~/.config/forktty/config.toml
- Settings panel (Ctrl+,) for in-app config editing
- Catppuccin Mocha as default fallback theme
- Configurable sidebar position (left/right)

### Phase 8 — Polish + Release
- Session persistence (auto-save and restore on startup)
- Command palette (Ctrl+Shift+P) with keyboard navigation and inline filtering
- Find in terminal (Ctrl+F) via xterm.js SearchAddon
- Copy selection (Ctrl+Shift+C)
- ErrorToast component for user-visible error feedback
- Structured logging to ~/.local/share/forktty/logs/
- .deb and AppImage bundle targets
- License: AGPL-3.0

### Security Hardening
- Socket: owner-only permissions (0o600), XDG_RUNTIME_DIR default path, 1 MiB request size limit
- Notifications: argv splitting instead of sh -c (no command injection)
- Worktree: path traversal protection via canonicalize + git-workdir boundary check
- Worktree names: reject /, \, .., \0
- Shell path: must be absolute and point to an executable file
- CSP: strict Content Security Policy in tauri.conf.json
- Config: Ghostty theme path traversal guard
- Logging: newline injection sanitization

### Known Limitations
- `beforeunload` session save is fire-and-forget (async IPC may not complete)
- No idle detection (`idle_threshold_ms` config field reserved but not active)
- No dark/light mode toggle (dark theme only; CSS has a minimal system-preference fallback)
- No flow control / backpressure on PTY output
