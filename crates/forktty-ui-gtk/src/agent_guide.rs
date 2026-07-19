//! Compact context injected into supported agent session-start hooks.

pub(crate) fn session_context_lines() -> [&'static str; 3] {
    [
        "ForkTTY exposes local CLI and socket primitives for workspaces, panes, terminal text, notifications, status, worktrees, and agent session lifecycle.",
        "For ordinary edits in the current repository, work normally; use ForkTTY commands only when the task needs those terminal or workspace primitives.",
        "Inspect with context-snapshot, list, surfaces, tree, read-screen, capture-tail, agents, or agent-health before targeting another pane or changing agent session state.",
    ]
}
