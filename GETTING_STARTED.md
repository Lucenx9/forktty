# Getting Started with ForkTTY

ForkTTY is a Linux-only GTK/Ghostty workspace terminal for shells and coding
agents. The shortest useful path is terminal-first.

## 1. Start ForkTTY

Install a release artifact or run from source:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

In another terminal, verify the local runtime:

```bash
forktty doctor
forktty ping
forktty identify --json
```

## 2. Open workspaces and panes

Use `Ctrl+Shift+N` for a new workspace, `Ctrl+Shift+O` to open a directory,
and `Ctrl+Shift+H` / `Ctrl+Shift+E` for horizontal or vertical splits. The
command palette at `Ctrl+Shift+P` exposes the same common actions.

The socket CLI can inspect the live layout without taking over the process in
the pane:

```bash
forktty list
forktty surfaces --workspace-name main
forktty tree --workspace-name main
forktty capture-tail --lines 40
```

## 3. Optionally install lifecycle hooks

Hooks make supported agents visible in the Agent HUD and notification model.
They are optional and are never installed or refreshed automatically.

```bash
forktty hooks setup codex --dry-run
forktty hooks setup codex
forktty hooks doctor codex
forktty hooks test codex
```

Equivalent setup targets exist for `claude`, `antigravity`, and `opencode`.
ForkTTY does not install MCP servers or managed agent skills.

## 4. Use worktrees for isolated edits

```bash
forktty worktree-doctor --cwd "$PWD" --json
forktty worktree-create feature/my-fix --cwd "$PWD"
forktty worktree-status --cwd "$PWD"
```

ForkTTY owns the worktree and terminal workspace; the agent or shell in the pane
owns task planning and coordination.

## 5. Watch attention

Use unread workspace state, OSC/hook notifications, the notification panel, and
the Agent HUD to find work that needs input. Status/progress/log metadata can be
inspected with `forktty context-snapshot`, `forktty status explain`, and
`forktty events`.
