import { findWorkspaceIdByPane, useWorkspaceStore } from "../stores/workspace";
import { getCwd, getGitBranch, getPtyCwd, logError } from "./pty-bridge";

export type SplitDirection = "horizontal" | "vertical";

async function refreshWorkspaceGitBranch(
  workspaceId: string,
  cwd: string,
): Promise<void> {
  const branch = await getGitBranch(cwd);
  const latest = useWorkspaceStore.getState().workspaces[workspaceId];
  if (latest && latest.workingDir === cwd) {
    useWorkspaceStore.getState().setWorkspaceGitBranch(workspaceId, branch);
  }
}

function updateWorkspaceContext(workspaceId: string, cwd: string): void {
  const latest = useWorkspaceStore.getState().workspaces[workspaceId];
  if (!latest) return;
  if (latest.workingDir !== cwd) {
    useWorkspaceStore.getState().setWorkspaceWorkingDir(workspaceId, cwd);
  }
  refreshWorkspaceGitBranch(workspaceId, cwd).catch(logError);
}

export async function resolveWorkspaceSpawnCwd(
  workspaceId: string,
  preferredCwd?: string,
): Promise<string> {
  const explicitCwd = preferredCwd?.trim();
  if (explicitCwd) {
    return explicitCwd;
  }

  const workspace = useWorkspaceStore.getState().workspaces[workspaceId];
  if (workspace?.workingDir) {
    return workspace.workingDir;
  }

  const cwd = await getCwd();
  updateWorkspaceContext(workspaceId, cwd);
  return cwd;
}

export async function resolvePaneCwd(paneId: string): Promise<string> {
  const state = useWorkspaceStore.getState();
  const workspaceId = findWorkspaceIdByPane(state.workspaces, paneId);
  const workspace = workspaceId ? state.workspaces[workspaceId] : undefined;
  const ptyId = workspace?.surfaces[paneId]?.ptyId ?? null;

  if (ptyId != null) {
    try {
      const cwd = await getPtyCwd(ptyId);
      if (workspaceId) {
        updateWorkspaceContext(workspaceId, cwd);
      }
      return cwd;
    } catch (err) {
      logError(err);
    }
  }

  if (workspace?.workingDir) {
    return workspace.workingDir;
  }

  const cwd = await getCwd();
  if (workspaceId) {
    updateWorkspaceContext(workspaceId, cwd);
  }
  return cwd;
}

export async function createWorkspaceWithCwd(
  name?: string,
  workingDir?: string,
): Promise<string> {
  const cwd = workingDir?.trim() || (await getCwd());
  const id = useWorkspaceStore.getState().createWorkspace(name, cwd);
  refreshWorkspaceGitBranch(id, cwd).catch(logError);
  return id;
}

export async function createWorkspaceWithInheritedCwd(name?: string): Promise<string> {
  const state = useWorkspaceStore.getState();
  const active = state.workspaces[state.activeWorkspaceId];
  const cwd = active ? await resolvePaneCwd(active.focusedPaneId) : await getCwd();
  const id = state.createWorkspace(name, cwd);
  refreshWorkspaceGitBranch(id, cwd).catch(logError);
  return id;
}

export async function splitPaneWithInheritedCwd(
  paneId: string,
  direction: SplitDirection,
): Promise<void> {
  const cwd = await resolvePaneCwd(paneId);
  useWorkspaceStore.getState().splitPane(paneId, direction, cwd);
}
