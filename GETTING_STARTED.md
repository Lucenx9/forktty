# Getting Started with ForkTTY

This is the shortest path to a useful local multi-agent workflow. ForkTTY is
Linux-only and currently alpha.

## 1. Start ForkTTY

Install an AppImage or build from source, then launch the GTK/Ghostty app:

```bash
cargo run -p forktty-ui-gtk --no-default-features --features gtk-ghostty
```

In another terminal, verify the local install:

```bash
forktty doctor
forktty ping
forktty identify --json
```

## 2. Install agent integration

Set up the managed ForkTTY agent skill and hook/MCP wiring for the tools you use:

```bash
forktty skills setup agents
forktty hooks setup codex
forktty mcp setup codex
```

Repeat `hooks setup` / `mcp setup` for `claude` or `antigravity` if you use
those providers.

## 3. Check provider command availability and agent state

```bash
forktty capabilities --json
forktty agents --json
```

In the GTK app, Settings > Agents shows provider command availability and the
team provider order. Command discovery does not prove authentication, quota, or
runtime health; task planning keeps those signals unverified until concrete
runtime evidence exists.

## 4. Plan before launching workers

Ask the router for a read-only strategy before starting a non-trivial task:

```bash
forktty task-plan "review this change and report concrete bugs" --cwd "$PWD" --review --json
```

Use the plan to decide whether to stay solo, add reviewers, use a workflow loop,
or isolate edits in a worktree.

## 5. Use worktrees for risky edits

Inspect the current repo/worktrees without changing anything:

```bash
forktty worktree-doctor --cwd "$PWD" --json
```

Create or attach a worktree when the router or dirty-repo state calls for
isolation:

```bash
forktty worktree-create feature/my-fix --cwd "$PWD"
forktty worktree-status --cwd "$PWD"
```

## 6. Watch attention, not noise

In the GTK workbench:

- Router rail: check Strategy, Approvals, Worker Health, Worker Reports, and
  Notifications.
- Bottom feed: use `ATTENTION` for approvals, needs-input, warnings, errors,
  stale state, and conflicts.
- Use `WORKFLOW FEED`, `EVENTS`, and `LOGS` when you need the full trace.

Keep loops visible: ForkTTY records workflow loop state and gates, but it does
not run hidden schedulers or background workers.
