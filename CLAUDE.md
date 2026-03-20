# ForkTTY

Multi-agent terminal for Linux. See SPEC.md for architecture, ROADMAP.md for phases.

## Stack

- **Backend**: Rust (Tauri v2), portable-pty, git2, tokio, notify-rust, chrono
- **Frontend**: React 19 + TypeScript + Vite, @xterm/xterm 5.x (canvas), react-resizable-panels, Zustand 5.x
- **IPC**: Tauri `invoke` (request/response), `Channel<String>` (PTY streaming)
- **External API**: Unix socket JSON-RPC at `$XDG_RUNTIME_DIR/forktty.sock`, CLI via clap

## Project Structure

```
src-tauri/src/
  lib.rs             # Tauri app: state, all commands, run()
  pty_manager.rs     # PTY spawn/write/resize via portable-pty
  output_scanner.rs  # OSC 133/9/99/777 parser + prompt pattern matching
  worktree.rs        # Git worktree lifecycle via git2
  notification.rs    # notify-rust + custom command (absolute path only)
  socket_api.rs      # Unix socket JSON-RPC server
  config.rs          # TOML config + Ghostty theme parser
  session.rs         # Session persistence + structured logging
  cli.rs             # forktty-cli binary (clap-based)

src/
  App.tsx            # Root: layout, shortcuts, session lifecycle
  stores/
    pane-tree.ts     # Pure pane tree types + algorithms (no Zustand)
    workspace.ts     # Zustand store (workspaces, panes, notifications)
    config.ts        # Config + theme store
    metadata.ts      # Status pills, progress bars, logs
  lib/
    pty-bridge.ts    # Tauri invoke wrappers
    socket-handler.ts    # Socket API RPC dispatch
    ghostty-theme.ts     # Theme → xterm.js ITheme + CSS vars
    session-persistence.ts
    terminal-registry.ts
  components/
    Sidebar.tsx, TerminalPane.tsx, PaneArea.tsx
    NotificationPanel.tsx, SettingsPanel.tsx (lazy)
    CommandPalette.tsx (lazy), BranchPicker.tsx (lazy)
    FindBar.tsx, ErrorToast.tsx, Icons.tsx
    WorkspaceMetadataView.tsx
```

## Verification

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npx prettier --check src/
npm run build
```

## Building

```bash
npm install && cargo tauri dev          # Dev mode
cargo tauri build                       # Release (.deb + AppImage)
```

## Key Decisions

- **Canvas renderer**: WebGL crashes on WebKitGTK. Canvas is default, DOM is fallback.
- **Output scanning in Rust**: Zero frontend overhead. OSC parsing before Channel.send().
- **Pane tree extraction**: Pure algorithms in `pane-tree.ts`, re-exported through `workspace.ts`.
- **Socket handler extraction**: RPC dispatch in `socket-handler.ts` using `.getState()` — no stale closures.
- **Lazy loading**: SettingsPanel, CommandPalette, BranchPicker via `React.lazy()`.
- **ghostty-web**: Evaluated, not adopted. xterm.js + Canvas is stable.

## Security Invariants

<important>
These MUST NOT be broken. Run `/security-audit` after changes to these files.
</important>

- `socket_api.rs`: permissions 0o600, 1MiB request limit, 100 pending bridge cap
- `notification.rs`: argv split only (no `sh -c`), command must be absolute path
- `lib.rs`: `verify_repo_path()` canonicalizes + checks git-workdir boundary; shell path must be absolute+exist
- `worktree.rs`: `validate_worktree_name()` rejects `/`, `\`, `..`, `\0` — called at top of create/remove/merge/attach
- `config.rs`: theme name **allowlist** (alphanumeric, `-`, `_`, space) — not a denylist
- `tauri.conf.json`: CSP must not be null
- `pty_manager.rs`: PTY ID uses `checked_add`; CWD validated absolute+existing

## Agents / Commands

- `/verify` — run all verification gates
- `/security-audit` — launch security auditor
- `/check-roadmap` — display ROADMAP status
- `code-reviewer`, `senior-engineer`, `security-auditor`, `requirement-parser`

## Rules

Detailed conventions and common mistakes are in scoped rule files:
- `.claude/rules/rust-backend.md` — Rust patterns, mistakes, debugging (Glob: `src-tauri/**/*.rs`)
- `.claude/rules/react-frontend.md` — React/TS patterns, mistakes, debugging (Glob: `src/**/*.{ts,tsx}`)
