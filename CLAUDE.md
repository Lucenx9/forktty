# ForkTTY

Multi-agent terminal for Linux. See SPEC.md for architecture and ROADMAP.md for implementation phases.

## Stack

- **Backend**: Rust (Tauri v2 commands), portable-pty 0.9 for PTY, git2 for worktrees, tokio for async, notify-rust for desktop notifications, chrono for timestamps
- **Frontend**: React 19 + TypeScript + Vite, @xterm/xterm 5.x with addons (fit, canvas, search), react-resizable-panels for splits, Zustand 5.x for state
- **IPC**: Tauri `invoke` for request/response, Tauri `Channel<String>` for PTY output streaming (push-based, ordered)
- **External API**: Unix domain socket at `$XDG_RUNTIME_DIR/forktty.sock` (fallback `/tmp`), JSON-RPC protocol, CLI client via clap

## Project Structure

```
forktty/
├── CLAUDE.md              # This file
├── SPEC.md                # Full architecture and data model spec
├── ROADMAP.md             # Phased implementation plan with acceptance criteria
├── SECURITY.md            # Vulnerability reporting + security model
├── PRIVACY.md             # Privacy notice (zero data collection)
├── src-tauri/             # Rust backend (Tauri v2)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs            # Binary entry point
│   │   ├── lib.rs             # Tauri app: state, all commands, run()
│   │   ├── pty_manager.rs     # PTY spawn/write/resize via portable-pty
│   │   ├── output_scanner.rs  # OSC 133/9/99/777 parser + pattern matching
│   │   ├── worktree.rs        # Git worktree lifecycle via git2
│   │   ├── notification.rs    # notify-rust + custom command (absolute path only)
│   │   ├── socket_api.rs      # Unix socket JSON-RPC server
│   │   ├── config.rs          # TOML config + Ghostty theme parser
│   │   ├── session.rs         # Session persistence + structured logging
│   │   └── cli.rs             # forktty-cli binary (clap-based)
│   └── tauri.conf.json
├── src/                   # React frontend
│   ├── App.tsx                # Root component: layout, shortcuts, session lifecycle
│   ├── App.css                # All styles via CSS custom properties
│   ├── components/
│   │   ├── Sidebar.tsx            # Workspace list, context menu, help, drag-and-drop
│   │   ├── PaneArea.tsx           # Recursive react-resizable-panels splits
│   │   ├── TerminalPane.tsx       # xterm.js wrapper, notifications, find, context menu
│   │   ├── NotificationPanel.tsx  # Notification list overlay
│   │   ├── SettingsPanel.tsx      # Settings UI (lazy-loaded)
│   │   ├── CommandPalette.tsx     # Fuzzy command search (lazy-loaded)
│   │   ├── BranchPicker.tsx       # Git branch picker for worktrees (lazy-loaded)
│   │   ├── FindBar.tsx            # Terminal search bar
│   │   ├── ErrorToast.tsx         # Toast notification system
│   │   ├── Icons.tsx              # SVG icon components
│   │   └── WorkspaceMetadataView.tsx  # Status pills, progress, logs
│   ├── stores/
│   │   ├── pane-tree.ts       # Pure pane tree types + algorithms (no Zustand)
│   │   ├── workspace.ts       # Zustand store (workspaces, panes, notifications)
│   │   ├── config.ts          # Config + theme store
│   │   └── metadata.ts        # Metadata store (status pills, progress, logs)
│   └── lib/
│       ├── pty-bridge.ts          # Tauri invoke wrappers
│       ├── ghostty-theme.ts       # Theme → xterm.js ITheme + CSS vars
│       ├── socket-handler.ts      # Socket API bridge (RPC dispatch)
│       ├── session-persistence.ts # Session save/restore helpers
│       └── terminal-registry.ts   # Terminal instance registry for read-screen
└── package.json
```

## Conventions

- Rust: follow standard Rust conventions, `cargo fmt` and `cargo clippy`
- TypeScript: strict mode, no `any`, prefer interfaces over types
- Components: functional React with hooks, no class components
- State: Zustand stores, no prop drilling beyond 2 levels
- Errors: Rust `Result<T, E>` with `thiserror`, frontend try/catch with user-visible error toasts via `showToast()`
- Logging: use `writeLog()` for structured backend logs, `logError()` for fire-and-forget error logging
- IPC: Tauri `invoke` for request/response, Tauri `Channel<String>` for PTY output streaming
- PTY data flow: PTY output → Rust output_scanner (parse OSC, check patterns) → Channel.send() → frontend xterm.js `write()`
- Terminal rendering: Canvas renderer by default (WebGL is unstable on WebKitGTK — Tauri issues #6559, #8498)

## Key Decisions

- **No node-pty**: Tauri has no Node.js. Use portable-pty (Rust, from WezTerm project) instead.
- **No AttachAddon**: Can't use xterm.js AttachAddon (requires WebSocket). Use Tauri Channels for PTY streaming.
- **Channels, not Events**: Tauri events are "not designed for low latency or high throughput". Channels are push-based, ordered, and built for streaming.
- **Canvas renderer by default**: WebGL inside WebKitGTK has unresolved upstream bugs (context lost, freezes). Canvas is 2-3x faster than DOM and reliable.
- **Output scanning in Rust**: All PTY output passes through Rust before reaching frontend. This is where OSC 133 parsing and pattern matching happen, with zero frontend overhead.
- **react-resizable-panels over Allotment**: More actively maintained (2M+ weekly downloads), React 19 native support, used by shadcn/ui. Allotment development has slowed.
- **portable-pty is sync-only**: Use `tauri::async_runtime::spawn_blocking` for the read loop. Call `take_writer()` once and share via `Arc<Mutex<>>`. Drop the slave after spawn.
- **ghostty-web**: Evaluated but not adopted. Sticking with xterm.js + Canvas renderer for stability.
- **Lazy loading**: SettingsPanel, CommandPalette, BranchPicker use `React.lazy()` for code splitting.
- **Pane tree extraction**: Pure pane tree algorithms live in `stores/pane-tree.ts` (zero Zustand dependency), re-exported through `workspace.ts`.
- **Socket handler extraction**: RPC dispatch logic in `lib/socket-handler.ts`, not in App.tsx. Uses `.getState()` pattern — no stale closures.

## System Dependencies (Debian 13)

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Rust: use rustup (minimum 1.88 for tauri-cli)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Verification Commands

Run after every change:

```bash
# Rust: format + lint + test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend: format + build
npx prettier --check src/
npm run build

# Integration: does the app launch?
cargo tauri dev
```

Note: CI uses `-D warnings` (deny all warnings). Match this locally to catch issues before push.

## Building

```bash
# Install frontend deps
npm install

# Dev mode
cargo tauri dev

# Build release (.deb + AppImage)
cargo tauri build
```

## Development Workflow

Use the RPI (Research/Plan/Implement) commands for feature work:

```
/rpi:research <feature>    # Explore feasibility, write research doc
/rpi:plan <feature>        # Generate implementation plan, wait for approval
/rpi:implement <feature>   # Execute plan with validation gates
```

Plans are stored in `plans/`. Each phase can run in a separate session.

### Agents

- `code-reviewer` — reviews diffs for correctness and convention adherence
- `senior-engineer` — implements features following SPEC/ROADMAP with tests
- `security-auditor` — ForkTTY-specific security checks (socket, PTY, hooks, CSP)
- `requirement-parser` — parses feature requests against SPEC/ROADMAP

### Commands

- `/verify` — run all verification gates
- `/format-all` — cargo fmt + prettier
- `/security-audit` — launch security auditor
- `/check-roadmap` — display ROADMAP completion status

### Rules

- `.claude/rules/rust-backend.md` — Rust/Tauri conventions (Glob: src-tauri/**/*.rs)
- `.claude/rules/react-frontend.md` — React/TypeScript conventions (Glob: src/**/*.{ts,tsx})

## When Implementing

1. Always check ROADMAP.md for current phase and task list
2. Read SPEC.md for data models, API contracts, and keyboard shortcuts
3. Each phase has explicit acceptance criteria — verify against those
4. Keep phases independent: each phase should produce a working app
5. Test with real terminal programs (htop, vim, less) not just echo commands

## Security Invariants

These MUST NOT be broken. Run `/security-audit` after any change to these files:

- `socket_api.rs`: socket permissions 0o600, XDG_RUNTIME_DIR default, 1MiB request size limit, 100 max pending bridge requests
- `notification.rs`: argv splitting only, never `sh -c`; notification_command must be absolute path to existing file
- `lib.rs`: shell path must be absolute+exist, worktree_run_hook/status must canonicalize + verify git-workdir boundary via `verify_repo_path()`
- `worktree.rs`: name validation rejects `/`, `\`, `..`, `\0`; `validate_worktree_name()` called at top of `create()`, `remove()`, `merge()`, `attach()`
- `tauri.conf.json`: CSP must never be null — current: `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'`
- `config.rs`: Ghostty theme name validated via **allowlist** (alphanumeric, `-`, `_`, space only) — not a denylist
- `pty_manager.rs`: PTY ID uses `checked_add` to prevent overflow; CWD validated as absolute + existing

## Common Mistakes (learned from Phases 1-9)

- **Don't use `sh -c` for external commands** — always split into argv. Review found command injection via notification_command.
- **Don't pass frontend paths to backend without canonicalize** — worktree_run_hook accepted arbitrary paths, enabling hook execution outside the repo.
- **Don't forget `index.write()` before `write_tree()` in git2 merge** — merge commit silently failed without it.
- **Don't set CSP to null** — Tauri v2 default CSP is protective; null removes all protection.
- **Don't allocate in hot paths** — output_scanner runs on every PTY read chunk (thousands/sec). Avoid `data.to_vec()` when a reference suffices.
- **Don't fire Zustand writes on every mouse pixel** — debounce `onLayoutChange` with requestAnimationFrame.
- **Don't use `console.log` in production** — use `showToast` for user-visible feedback, `writeLog` for structured logging, `logError` for fire-and-forget.
- **Don't use `filter_map(ok())` on compile-time constant regex patterns** — if a pattern fails to compile, it silently disappears. Use `unwrap_or_else(panic!)` for patterns that must always be valid.
- **Don't use empty `.catch(() => {})` blocks** — use `.catch(logError)` at minimum. The only legitimate empty catch is inside `logError` itself (last-resort sentinel).
- **Socket `read_limited_line`: don't use `BufReader::lines()` for untrusted input** — it allocates the full line before yielding, allowing OOM. Use `fill_buf`/`consume` with incremental size checking.
- **Don't continue serving if socket permissions fail** — if `set_permissions(0o600)` fails, remove the socket and return. The socket could be world-accessible.
- **Always call `index.write()` before `index.write_tree()` in git2** — double-check every git2 index→tree codepath.
- **Don't use magic numbers for errno** — use `libc::EIO` instead of bare `5`. Makes intent explicit and avoids platform-specific bugs.
- **Don't use `unwrap_or` for parent() on paths** — return an explicit error instead of silently falling back to root. `worktree_path()` returns `Result` for this reason.
- **Always build frontend before cargo clippy** — `tauri::generate_context!()` requires `../dist` to exist at compile time. CI must run `npm run build` before Rust checks.

## Debugging Tips

- **xterm.js Canvas rendering issues**: Canvas addon is required (WebGL crashes on WebKitGTK). If terminal looks wrong, check that CanvasAddon is loaded before any write.
- **PTY not receiving input**: Check that `take_writer()` was called exactly once. It's one-shot; calling twice returns an error.
- **Socket connection refused**: Check `$XDG_RUNTIME_DIR/forktty.sock` exists with `ls -la`. Verify permissions are `srw-------`.
- **OSC 133 not detected**: Ensure your shell has prompt integration enabled (`bash-preexec` or zsh `precmd`/`preexec` hooks). Test with `echo -e '\033]133;A\007'`.
- **Worktree operations fail**: Run `git worktree list` to check state. Stale worktrees need `git worktree prune`.
- **Session restore creates empty workspaces**: The session file at `~/.local/share/forktty/session.json` stores pane layout but not PTY state. Each pane spawns a fresh shell on restore.
- **CI Rust job fails with "frontendDist path doesn't exist"**: The Rust job must build the frontend first. Check that `npm ci && npm run build` runs before `cargo clippy`.
- **Log files**: Check `~/.local/share/forktty/logs/forktty-YYYY-MM-DD.log`. Auto-pruned after 30 days.
