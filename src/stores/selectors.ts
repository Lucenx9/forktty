/**
 * Shared Zustand selectors for workspace state.
 *
 * These are stable function references — Zustand only re-renders when
 * the returned value changes (via Object.is). Centralising them here
 * avoids duplicate inline reduces across DashboardChrome, ShortcutBar,
 * and Sidebar.
 */
import { useShallow } from "zustand/shallow";
import { useWorkspaceStore } from "./workspace";
import type { WorkspaceState } from "./workspace";

// --- Primitive selectors (safe with Object.is equality) ---

export const selectActiveWorkspaceId = (s: WorkspaceState) => s.activeWorkspaceId;

export const selectWorkspaceCount = (s: WorkspaceState) => s.workspaceOrder.length;

export const selectTotalUnread = (s: WorkspaceState) =>
  Object.values(s.workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0);

export const selectTotalPaneCount = (s: WorkspaceState) =>
  Object.values(s.workspaces).reduce(
    (sum, ws) => sum + Object.keys(ws.surfaces).length,
    0,
  );

export const selectNotifications = (s: WorkspaceState) => s.notifications;

// --- Composite hook: shallow-compared active workspace summary ---

export interface ActiveWorkspaceSummary {
  name: string;
  gitBranch: string;
  workingDir: string;
  worktreeDir: string;
  worktreeStatus: string;
  surfaceCount: number;
}

const selectActiveWsSummary = (s: WorkspaceState): ActiveWorkspaceSummary | null => {
  const ws = s.workspaces[s.activeWorkspaceId];
  if (!ws) return null;
  return {
    name: ws.name,
    gitBranch: ws.gitBranch,
    workingDir: ws.workingDir,
    worktreeDir: ws.worktreeDir,
    worktreeStatus: ws.worktreeStatus,
    surfaceCount: Object.keys(ws.surfaces).length,
  };
};

export function useActiveWorkspaceSummary(): ActiveWorkspaceSummary | null {
  return useWorkspaceStore(useShallow(selectActiveWsSummary));
}
