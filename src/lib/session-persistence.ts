import { getSessionData } from "../stores/workspace";
import type { SessionData } from "./pty-bridge";

/** Build a session payload suitable for the Tauri save_session command. */
export function buildSessionPayload(): SessionData {
  const { workspaces, activeIndex } = getSessionData();
  return {
    version: 1,
    workspaces: workspaces.map((ws) => ({
      name: ws.name,
      working_dir: ws.workingDir,
      git_branch: ws.gitBranch,
      worktree_dir: ws.worktreeDir,
      worktree_name: ws.worktreeName,
      pane_tree: ws.paneTree,
      focused_leaf_index: ws.focusedLeafIndex,
    })),
    active_workspace_index: activeIndex,
  };
}
