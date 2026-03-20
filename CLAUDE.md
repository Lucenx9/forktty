# ForkTTY

Multi-agent terminal for Linux. See SPEC.md for architecture and ROADMAP.md for implementation phases.

## Stack

- **Backend**: Rust (Tauri v2 commands), portable-pty 0.9 for PTY, git2 for worktrees, tokio for async, notify-rust for desktop notifications
- **Frontend**: React 19 + TypeScript + Vite, @xterm/xterm 5.x with addons (fit, canvas, search, web-links), react-resizable-panels for splits, Zustand 5.x for state
- **IPC**: Tauri `invoke` for request/response, Tauri `Channel<String>` for PTY output streaming (push-based, ordered)
- **External API**: Unix domain socket at `$XDG_RUNTIME_DIR/forktty.sock` (fallback `/tmp`), JSON-RPC protocol, CLI client via clap
- **Alternative terminal renderer**: ghostty-web (Ghostty VT100 WASM, MIT, drop-in xterm.js API) — evaluate during Phase 1

## Project Structure

```
forktty/
├── CLAUDE.md              # This file
├── SPEC.md                # Full architecture and data model spec
├── ROADMAP.md             # Phased implementation plan with acceptance criteria
├── src-tauri/             # Rust backend (Tauri v2)
│   ├── Cargo.toml
│   ├── src/
│   │   ├── main.rs
│   │   ├── pty_manager.rs     # PTY spawn/write/resize via portable-pty
│   │   ├── output_scanner.rs  # OSC 133 parser + pattern matching
│   │   ├── worktree.rs        # Git worktree lifecycle via git2
│   │   ├── notification.rs    # notify-rust + custom command
│   │   ├── socket_api.rs      # Unix socket JSON-RPC server
│   │   └── config.rs          # TOML config + Ghostty theme parser
│   └── tauri.conf.json
├── src/                   # React frontend
│   ├── App.tsx
│   ├── components/
│   │   ├── Sidebar.tsx
│   │   ├── PaneArea.tsx       # Recursive react-resizable-panels splits
│   │   ├── TerminalPane.tsx   # xterm.js wrapper
│   │   └── NotificationPanel.tsx
│   ├── stores/
│   │   ├── workspace.ts       # Zustand store (workspaces, panes, notifications)
│   │   └── config.ts          # Config + theme store
│   └── lib/
│       ├── pty-bridge.ts      # Tauri invoke wrappers
│       └── ghostty-theme.ts   # Theme → xterm.js ITheme + CSS vars
└── package.json
```

## Conventions

- Rust: follow standard Rust conventions, `cargo fmt` and `cargo clippy`
- TypeScript: strict mode, no `any`, prefer interfaces over types
- Components: functional React with hooks, no class components
- State: Zustand stores, no prop drilling beyond 2 levels
- Errors: Rust `Result<T, E>` with `thiserror`, frontend try/catch with user-visible error toasts
- IPC: Tauri `invoke` for request/response, Tauri `Channel<String>` for PTY output streaming
- PTY data flow: PTY output → Rust output_scanner (parse OSC, check patterns) → Channel.send() → frontend xterm.js `write()`
- Terminal rendering: Canvas renderer by default (WebGL is unstable on WebKitGTK — Tauri issues #6559, #8498)

## Key Decisions

- **No node-pty**: Tauri has no Node.js. Use portable-pty (Rust, from WezTerm project) instead.
- **No AttachAddon**: Can't use xterm.js AttachAddon (requires WebSocket). Use Tauri Channels for PTY streaming.
- **Channels, not Events**: Tauri events are "not designed for low latency or high throughput". Channels are push-based, ordered, and built for streaming.
- **Canvas renderer by default**: WebGL inside WebKitGTK has unresolved upstream bugs (context lost, freezes). Canvas is 2-3x faster than DOM and reliable. Try WebGL on startup with catch-and-fallback.
- **Output scanning in Rust**: All PTY output passes through Rust before reaching frontend. This is where OSC 133 parsing and pattern matching happen, with zero frontend overhead.
- **react-resizable-panels over Allotment**: More actively maintained (2M+ weekly downloads), React 19 native support, used by shadcn/ui. Allotment development has slowed.
- **portable-pty is sync-only**: Use `tokio::task::spawn_blocking` for the read loop. Call `take_writer()` once and share via `Arc<Mutex<>>`. Drop the slave after spawn.
- **ghostty-web as potential upgrade**: MIT-licensed Ghostty VT100 parser compiled to WASM, drop-in xterm.js API, ~400KB. Used in production by Coder Mux. Evaluate during Phase 1.

## System Dependencies (Debian 13)

```bash
# Verified: all packages available on Debian 13 Trixie
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev

# Rust: use rustup (minimum 1.88 for tauri-cli)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

## Verification Commands

Run after every change:

```bash
# Rust: lint + test
cargo clippy --manifest-path src-tauri/Cargo.toml -- -W clippy::all
cargo test --manifest-path src-tauri/Cargo.toml

# Frontend: build + format check
npm run build
npx prettier --check src/

# Integration: does the app launch?
cargo tauri dev
```

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

- `/verify` — run all 5 verification gates
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

- `socket_api.rs`: socket permissions 0o600, XDG_RUNTIME_DIR default, 1MiB request size limit
- `notification.rs`: argv splitting only, never `sh -c`
- `lib.rs`: shell path must be absolute+exist, worktree_run_hook/status must canonicalize + verify git-workdir boundary
- `worktree.rs`: name validation rejects `/`, `\`, `..`, `\0`; `validate_worktree_name()` called at top of `create()`
- `tauri.conf.json`: CSP must never be null
- `config.rs`: Ghostty theme name must not contain `/` or `..`

## Common Mistakes (learned from Phases 1-8)

- **Don't use `sh -c` for external commands** — always split into argv. Review found command injection via notification_command.
- **Don't pass frontend paths to backend without canonicalize** — worktree_run_hook accepted arbitrary paths, enabling hook execution outside the repo.
- **Don't forget `index.write()` before `write_tree()` in git2 merge** — merge commit silently failed without it.
- **Don't set CSP to null** — Tauri v2 default CSP is protective; null removes all protection.
- **Don't allocate in hot paths** — output_scanner runs on every PTY read chunk (thousands/sec). Avoid `data.to_vec()` when a reference suffices.
- **Don't fire Zustand writes on every mouse pixel** — debounce `onLayoutChange` with requestAnimationFrame.
- **Don't use `console.log` in production** — use `showToast` for user-visible feedback, `writeLog` for structured logging.
- **Don't use `filter_map(ok())` on compile-time constant regex patterns** — if a pattern fails to compile, it silently disappears. Use `unwrap_or_else(panic!)` for patterns that must always be valid.
- **Don't use empty `.catch(() => {})` blocks** — this violates error handling principles. At minimum set an error state so the user gets feedback.
- **Socket `read_limited_line`: don't use `BufReader::lines()` for untrusted input** — it allocates the full line before yielding, allowing OOM. Use `fill_buf`/`consume` with incremental size checking.
- **Don't continue serving if socket permissions fail** — if `set_permissions(0o600)` fails, remove the socket and return. The socket could be world-accessible.
- **Always call `index.write()` before `index.write_tree()` in git2** — this was already documented but was still missed in the merge path. Double-check every git2 index→tree codepath.

## Debugging Tips

- **xterm.js Canvas rendering issues**: Canvas addon is required (WebGL crashes on WebKitGTK). If terminal looks wrong, check that CanvasAddon is loaded before any write.
- **PTY not receiving input**: Check that `take_writer()` was called exactly once. It's one-shot; calling twice returns an error.
- **Socket connection refused**: Check `$XDG_RUNTIME_DIR/forktty.sock` exists with `ls -la`. Verify permissions are `srw-------`.
- **OSC 133 not detected**: Ensure your shell has prompt integration enabled (`bash-preexec` or zsh `precmd`/`preexec` hooks). Test with `echo -e '\033]133;A\007'`.
- **Worktree operations fail**: Run `git worktree list` to check state. Stale worktrees need `git worktree prune`.
- **Session restore creates empty workspaces**: The session file at `~/.local/share/forktty/session.json` stores pane layout but not PTY state. Each pane spawns a fresh shell on restore.
