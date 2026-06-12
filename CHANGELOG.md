# Changelog

All notable changes to ForkTTY are documented here.

## [Unreleased]

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
- The MCP server now exposes a `forktty://agent/operating-guide` resource and a `forktty_operating_guide` prompt, plus matching initialize instructions, so agents can discover when ForkTTY tools are useful and when to keep working normally.
- `forktty --json hooks doctor <agent>` and `forktty --json hooks test <agent>` are now a stable machine-readable API (documented in SPEC.md): versioned report with an overall `ok`, per-method `{method, ok, error?}` results for `hooks test` (which keeps running after a failed method instead of aborting, so cleanup still happens and the report is complete), and exit code 0/1 reflecting overall health for CI gating. The Codex trust state stays a first-class field of the doctor report.
- The worktree open-workspace boundary rejection now carries the structured error code `precondition_failed` (documented in SPEC.md), and MCP tool errors with a known recovery carry machine-readable `remedy` and `suggested_tool` fields in `structuredContent` — the boundary error points at `workspace_create`, so an agent can recover without parsing prose.

### Changed
- Touchpad scrolling in terminal panes now accumulates smooth deltas instead of forcing chunky wheel ticks.
- The declared Rust MSRV is now 1.96, matching the current `rusqlite`/`libsqlite3-sys` dependency chain required by the workspace lockfile.
- The competitive gap inventory now includes the non-browser cmux gaps plus the additional control-plane gaps found in oh-my-codex and oh-my-claudecode: workflow state/artifacts, team runtime, HUD/statusline export, and agent/skill catalogs.
- Gemini CLI integrations are now legacy opt-in: default `forktty hooks setup` and `forktty mcp setup` skip Gemini and prefer Antigravity, while explicit Gemini setup/remove/doctor/test and persisted Gemini resume compatibility remain supported.
- Claude Code session-start context now includes the same concise ForkTTY tool-use policy as the MCP operating guide: use ForkTTY for panes, agents, worktrees, status, or cross-surface text, but avoid tool calls for ordinary single-repo edits.

### Fixed
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
