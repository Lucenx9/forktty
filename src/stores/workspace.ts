import { create } from "zustand";
import { useMetadataStore } from "./metadata";
import { hasTauriRuntime, killPty, logError } from "../lib/pty-bridge";
import {
  replaceNode,
  removeLeaf,
  containsLeaf,
  updateSplitSizes,
  firstLeafId,
  collectLeafIds,
  findLeaf,
  getNodeDepth,
  buildLayoutRects,
  findNeighbor,
  MAX_SPLIT_DEPTH,
  makeLeaf,
  makeSurface,
  makeWorkspace,
  generateWorkspaceName,
  findWorkspaceIdByPane,
  rebuildPaneTree,
  isValidPaneTreeSnap,
  snapshotPaneTree,
  swapLeaves,
} from "./pane-tree";
import type {
  PaneNode,
  PaneLeaf,
  PaneSplit,
  PaneRect,
  Direction,
  PaneTreeSnap,
  SessionSnapshot,
  Surface,
  Workspace,
} from "./pane-tree";

// Re-export pane-tree types and functions so existing consumers still work
export {
  replaceNode,
  removeLeaf,
  containsLeaf,
  updateSplitSizes,
  firstLeafId,
  collectLeafIds,
  findLeaf,
  getNodeDepth,
  buildLayoutRects,
  findNeighbor,
  MAX_SPLIT_DEPTH,
  makeLeaf,
  makeSurface,
  makeWorkspace,
  generateWorkspaceName,
  findWorkspaceIdByPane,
  rebuildPaneTree,
  isValidPaneTreeSnap,
  snapshotPaneTree,
};
export type {
  PaneNode,
  PaneLeaf,
  PaneSplit,
  PaneRect,
  Direction,
  PaneTreeSnap,
  SessionSnapshot,
  Surface,
  Workspace,
};

// --- App-specific types ---

interface AppNotification {
  id: string;
  workspaceId: string;
  workspaceName: string;
  title: string;
  body: string;
  timestamp: number;
  read: boolean;
}

// --- Store interface ---

interface WorkspaceState {
  workspaces: Record<string, Workspace>;
  activeWorkspaceId: string;
  workspaceOrder: string[];
  notifications: AppNotification[];
  showNotificationPanel: boolean;

  // Workspace actions
  createWorkspace: (name?: string, workingDir?: string) => string;
  createWorktreeWorkspace: (
    name: string,
    workingDir: string,
    gitBranch: string,
    worktreeDir: string,
    worktreeName: string,
  ) => string;
  switchWorkspace: (id: string) => void;
  closeWorkspace: (id: string) => void;
  renameWorkspace: (id: string, name: string) => void;
  setWorkspaceGitBranch: (id: string, branch: string) => void;
  setWorkspaceWorkingDir: (id: string, dir: string) => void;
  setWorktreeStatus: (id: string, status: string) => void;

  // Pane actions (scoped to active workspace)
  splitPane: (
    paneId: string,
    direction: "horizontal" | "vertical",
    cwdOverride?: string,
  ) => void;
  closePane: (paneId: string) => void;
  swapPanes: (idA: string, idB: string) => void;
  setFocusedPane: (paneId: string) => void;
  moveFocus: (direction: Direction) => void;
  updatePaneSizes: (splitId: string, sizes: number[]) => void;

  // Surface lifecycle (finds workspace by paneId)
  registerSurface: (paneId: string, ptyId: number) => void;
  unregisterSurface: (paneId: string) => void;

  // Surface notification state
  setSurfaceUnread: (surfaceId: string, unread: boolean) => void;

  // Notification actions
  addNotification: (workspaceId: string, title: string, body: string) => void;
  markWorkspaceRead: (workspaceId: string) => void;
  clearNotifications: () => void;
  toggleNotificationPanel: () => void;
  jumpToUnread: () => void;

  // Workspace reorder
  reorderWorkspaces: (fromIndex: number, toIndex: number) => void;

  // Session persistence
  restoreSession: (snapshots: SessionSnapshot[], activeIndex: number) => void;
}

// --- Initial state ---

const initialWorkspace = makeWorkspace("Workspace 1");

function clearWorkspaceUnreadState(workspace: Workspace): Workspace {
  let surfacesChanged = false;
  const clearedSurfaces: Record<string, Surface> = {};

  for (const [surfaceId, surface] of Object.entries(workspace.surfaces)) {
    if (surface.hasUnreadNotification) {
      surfacesChanged = true;
      clearedSurfaces[surfaceId] = { ...surface, hasUnreadNotification: false };
    } else {
      clearedSurfaces[surfaceId] = surface;
    }
  }

  if (
    workspace.unreadCount === 0 &&
    workspace.lastNotificationText === "" &&
    !surfacesChanged
  ) {
    return workspace;
  }

  return {
    ...workspace,
    unreadCount: 0,
    lastNotificationText: "",
    surfaces: clearedSurfaces,
  };
}

function buildWorkspaceSelectionPatch(
  workspaces: Record<string, Workspace>,
  notifications: AppNotification[],
  workspaceId: string,
): Pick<WorkspaceState, "workspaces" | "notifications" | "activeWorkspaceId"> | null {
  const workspace = workspaces[workspaceId];
  if (!workspace) {
    return null;
  }

  lastWorkspaceSwitchTime = Date.now();
  const nextWorkspace = clearWorkspaceUnreadState(workspace);
  const nextWorkspaces =
    nextWorkspace === workspace
      ? workspaces
      : { ...workspaces, [workspaceId]: nextWorkspace };

  const hasUnreadNotifications = notifications.some(
    (notification) => notification.workspaceId === workspaceId && !notification.read,
  );
  const nextNotifications = hasUnreadNotifications
    ? notifications.map((notification) =>
        notification.workspaceId === workspaceId && !notification.read
          ? { ...notification, read: true }
          : notification,
      )
    : notifications;

  return {
    activeWorkspaceId: workspaceId,
    workspaces: nextWorkspaces,
    notifications: nextNotifications,
  };
}

function collectWorkspacePtyIds(workspace: Workspace): number[] {
  return [
    ...new Set(
      Object.values(workspace.surfaces).flatMap((surface) => {
        if (surface.ptyId == null) {
          return [];
        }
        return [surface.ptyId];
      }),
    ),
  ];
}

function disposePtys(ptyIds: number[]): void {
  if (!hasTauriRuntime() || ptyIds.length === 0) {
    return;
  }

  for (const ptyId of ptyIds) {
    killPty(ptyId).catch(logError);
  }
}

// --- Store ---

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  workspaces: { [initialWorkspace.id]: initialWorkspace },
  activeWorkspaceId: initialWorkspace.id,
  workspaceOrder: [initialWorkspace.id],
  notifications: [],
  showNotificationPanel: false,

  // --- Workspace actions ---

  createWorkspace: (name?, workingDir?) => {
    const { workspaces, workspaceOrder } = get();
    const wsName = name ?? generateWorkspaceName(workspaces);
    const ws = makeWorkspace(wsName, {
      workingDir: workingDir?.trim() ?? "",
    });

    set({
      workspaces: { ...workspaces, [ws.id]: ws },
      activeWorkspaceId: ws.id,
      workspaceOrder: [...workspaceOrder, ws.id],
    });

    return ws.id;
  },

  createWorktreeWorkspace: (name, workingDir, gitBranch, worktreeDir, worktreeName) => {
    const { workspaces, workspaceOrder } = get();
    const ws = makeWorkspace(name, {
      workingDir,
      gitBranch,
      worktreeDir,
      worktreeName,
    });

    set({
      workspaces: { ...workspaces, [ws.id]: ws },
      activeWorkspaceId: ws.id,
      workspaceOrder: [...workspaceOrder, ws.id],
    });

    return ws.id;
  },

  switchWorkspace: (id) => {
    const { workspaces, notifications } = get();
    const patch = buildWorkspaceSelectionPatch(workspaces, notifications, id);
    if (patch) {
      set(patch);
    }
  },

  closeWorkspace: (id) => {
    const { workspaces, workspaceOrder, activeWorkspaceId, notifications } = get();
    if (workspaceOrder.length <= 1) return; // Can't close last workspace
    const workspaceToClose = workspaces[id];
    if (!workspaceToClose) return;
    const ptyIdsToDispose = collectWorkspacePtyIds(workspaceToClose);

    const newWorkspaces = { ...workspaces };
    delete newWorkspaces[id];
    const newOrder = workspaceOrder.filter((wId) => wId !== id);
    const newNotifications = notifications.filter((n) => n.workspaceId !== id);

    let newActiveId = activeWorkspaceId;
    if (activeWorkspaceId === id) {
      const oldIndex = workspaceOrder.indexOf(id);
      newActiveId = newOrder[Math.min(oldIndex, newOrder.length - 1)] ?? newOrder[0]!;
    }

    const selectionPatch =
      activeWorkspaceId === id
        ? buildWorkspaceSelectionPatch(newWorkspaces, newNotifications, newActiveId)
        : null;

    set({
      workspaces: selectionPatch?.workspaces ?? newWorkspaces,
      workspaceOrder: newOrder,
      activeWorkspaceId: selectionPatch?.activeWorkspaceId ?? newActiveId,
      notifications: selectionPatch?.notifications ?? newNotifications,
    });

    // Clean up ephemeral metadata for the closed workspace
    useMetadataStore.getState().pruneWorkspace(id);
    disposePtys(ptyIdsToDispose);
  },

  renameWorkspace: (id, name) => {
    const { workspaces } = get();
    const ws = workspaces[id];
    if (!ws) return;

    set({
      workspaces: { ...workspaces, [id]: { ...ws, name } },
    });
  },

  setWorkspaceGitBranch: (id, branch) => {
    const { workspaces } = get();
    const ws = workspaces[id];
    if (!ws) return;

    set({
      workspaces: { ...workspaces, [id]: { ...ws, gitBranch: branch } },
    });
  },

  setWorkspaceWorkingDir: (id, dir) => {
    const { workspaces } = get();
    const ws = workspaces[id];
    if (!ws) return;

    set({
      workspaces: { ...workspaces, [id]: { ...ws, workingDir: dir } },
    });
  },

  setWorktreeStatus: (id, status) => {
    const { workspaces } = get();
    const ws = workspaces[id];
    if (!ws) return;

    set({
      workspaces: {
        ...workspaces,
        [id]: { ...ws, worktreeStatus: status },
      },
    });
  },

  // --- Pane actions (scoped to active workspace) ---

  splitPane: (paneId, direction, cwdOverride) => {
    const { workspaces, activeWorkspaceId } = get();
    const workspaceId = findWorkspaceIdByPane(workspaces, paneId) ?? activeWorkspaceId;
    const ws = workspaces[workspaceId];
    if (!ws) return;

    const existingLeaf = findLeaf(ws.root, paneId);
    if (!existingLeaf) return;

    // Prevent excessively deep nesting
    const depth = getNodeDepth(ws.root, paneId);
    if (depth >= MAX_SPLIT_DEPTH) return;

    const newLeaf = makeLeaf();

    const splitNode: PaneSplit = {
      type: direction,
      id: crypto.randomUUID(),
      children: [existingLeaf, newLeaf],
      sizes: [50, 50],
    };

    const newRoot = replaceNode(ws.root, paneId, splitNode);
    if (!newRoot) return;

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: {
          ...ws,
          root: newRoot,
          workingDir: cwdOverride?.trim() || ws.workingDir,
          surfaces: {
            ...ws.surfaces,
            [newLeaf.surfaceId]: makeSurface(newLeaf.surfaceId),
          },
          focusedPaneId: newLeaf.id,
        },
      },
    });
  },

  closePane: (paneId) => {
    const { workspaces, activeWorkspaceId } = get();
    const workspaceId = findWorkspaceIdByPane(workspaces, paneId);
    if (!workspaceId || workspaceId !== activeWorkspaceId) return;

    const ws = workspaces[workspaceId];
    if (!ws || !findLeaf(ws.root, paneId)) return;
    const ptyIdToDispose = ws.surfaces[paneId]?.ptyId;

    const leaves = collectLeafIds(ws.root);
    // Don't close the last pane
    if (leaves.length <= 1) return;

    const result = removeLeaf(ws.root, paneId);
    if (!result.tree) return;

    // Remove the surface for the closed pane
    const newSurfaces = { ...ws.surfaces };
    delete newSurfaces[paneId];

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: {
          ...ws,
          root: result.tree,
          surfaces: newSurfaces,
          focusedPaneId: result.focusId ?? firstLeafId(result.tree),
        },
      },
    });

    if (ptyIdToDispose != null) {
      disposePtys([ptyIdToDispose]);
    }
  },

  swapPanes: (idA, idB) => {
    if (idA === idB) return;
    const { workspaces, activeWorkspaceId } = get();
    const ws = workspaces[activeWorkspaceId];
    if (!ws) return;

    const newRoot = swapLeaves(ws.root, idA, idB);
    if (newRoot === ws.root) return;

    set({
      workspaces: {
        ...workspaces,
        [activeWorkspaceId]: { ...ws, root: newRoot },
      },
    });
  },

  setFocusedPane: (paneId) => {
    const { workspaces, activeWorkspaceId } = get();
    const workspaceId = findWorkspaceIdByPane(workspaces, paneId);
    if (!workspaceId || workspaceId !== activeWorkspaceId) return;

    const ws = workspaces[workspaceId];
    if (!ws || !findLeaf(ws.root, paneId)) return;

    // Clear unread notification on the surface being focused
    const surface = ws.surfaces[paneId];
    const updatedSurfaces = surface?.hasUnreadNotification
      ? {
          ...ws.surfaces,
          [paneId]: { ...surface, hasUnreadNotification: false },
        }
      : ws.surfaces;

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: {
          ...ws,
          focusedPaneId: paneId,
          surfaces: updatedSurfaces,
        },
      },
    });
  },

  moveFocus: (direction) => {
    const { workspaces, activeWorkspaceId } = get();
    const ws = workspaces[activeWorkspaceId];
    if (!ws) return;

    const rects = buildLayoutRects(ws.root, 0, 0, 1000, 1000);
    const neighborId = findNeighbor(rects, ws.focusedPaneId, direction);
    if (neighborId) {
      get().setFocusedPane(neighborId);
    }
  },

  updatePaneSizes: (splitId, sizes) => {
    const { workspaces } = get();

    for (const [workspaceId, workspace] of Object.entries(workspaces)) {
      const nextRoot = updateSplitSizes(workspace.root, splitId, sizes);
      if (nextRoot !== workspace.root) {
        set({
          workspaces: {
            ...workspaces,
            [workspaceId]: { ...workspace, root: nextRoot },
          },
        });
        return;
      }
    }
  },

  // --- Surface lifecycle (finds workspace by paneId) ---

  registerSurface: (paneId, ptyId) => {
    const { workspaces } = get();
    const wsId = findWorkspaceIdByPane(workspaces, paneId);
    if (!wsId) return;

    const ws = workspaces[wsId]!;
    const surface = ws.surfaces[paneId];
    if (!surface) return;

    set({
      workspaces: {
        ...workspaces,
        [wsId]: {
          ...ws,
          surfaces: {
            ...ws.surfaces,
            [paneId]: { ...surface, ptyId },
          },
        },
      },
    });
  },

  unregisterSurface: (paneId) => {
    const { workspaces } = get();
    const wsId = findWorkspaceIdByPane(workspaces, paneId);
    if (!wsId) return;

    const ws = workspaces[wsId]!;
    const surface = ws.surfaces[paneId];
    if (!surface) return;

    set({
      workspaces: {
        ...workspaces,
        [wsId]: {
          ...ws,
          surfaces: {
            ...ws.surfaces,
            [paneId]: { ...surface, ptyId: null },
          },
        },
      },
    });
  },

  // --- Surface notification state ---

  setSurfaceUnread: (surfaceId, unread) => {
    const { workspaces } = get();
    for (const [wsId, ws] of Object.entries(workspaces)) {
      const surface = ws.surfaces[surfaceId];
      if (surface) {
        set({
          workspaces: {
            ...workspaces,
            [wsId]: {
              ...ws,
              surfaces: {
                ...ws.surfaces,
                [surfaceId]: { ...surface, hasUnreadNotification: unread },
              },
            },
          },
        });
        return;
      }
    }
  },

  // --- Notification actions ---

  addNotification: (workspaceId, title, body) => {
    const { workspaces, notifications, activeWorkspaceId, workspaceOrder } = get();
    const ws = workspaces[workspaceId];
    if (!ws) return;
    const isActiveWorkspace = workspaceId === activeWorkspaceId;

    const notification: AppNotification = {
      id: crypto.randomUUID(),
      workspaceId,
      workspaceName: ws.name,
      title,
      body,
      timestamp: Date.now(),
      read: isActiveWorkspace,
    };

    const previewText = body || title;

    // Auto-reorder: move workspace to top if not active
    let newOrder = workspaceOrder;
    if (!isActiveWorkspace) {
      const idx = workspaceOrder.indexOf(workspaceId);
      if (idx > 0) {
        newOrder = [workspaceId, ...workspaceOrder.filter((id) => id !== workspaceId)];
      }
    }

    set({
      notifications: [notification, ...notifications].slice(0, 100),
      workspaceOrder: newOrder,
      workspaces: {
        ...workspaces,
        [workspaceId]: {
          ...ws,
          unreadCount: isActiveWorkspace ? ws.unreadCount : ws.unreadCount + 1,
          lastNotificationText: isActiveWorkspace
            ? ws.lastNotificationText
            : previewText,
        },
      },
    });
  },

  markWorkspaceRead: (workspaceId) => {
    const { workspaces, notifications } = get();
    const ws = workspaces[workspaceId];
    if (!ws) return;

    const nextWorkspace = clearWorkspaceUnreadState(ws);
    const hasUnreadNotifications = notifications.some(
      (notification) => notification.workspaceId === workspaceId && !notification.read,
    );
    if (nextWorkspace === ws && !hasUnreadNotifications) return;

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: nextWorkspace,
      },
      notifications: hasUnreadNotifications
        ? notifications.map((n) =>
            n.workspaceId === workspaceId && !n.read ? { ...n, read: true } : n,
          )
        : notifications,
    });
  },

  clearNotifications: () => {
    const { workspaces } = get();
    const cleared: Record<string, Workspace> = {};
    for (const [id, ws] of Object.entries(workspaces)) {
      const clearedSurfaces: Record<string, Surface> = {};
      for (const [surfaceId, surface] of Object.entries(ws.surfaces)) {
        clearedSurfaces[surfaceId] = surface.hasUnreadNotification
          ? { ...surface, hasUnreadNotification: false }
          : surface;
      }
      cleared[id] = {
        ...ws,
        unreadCount: 0,
        lastNotificationText: "",
        surfaces: clearedSurfaces,
      };
    }
    set({ notifications: [], workspaces: cleared });
  },

  toggleNotificationPanel: () => {
    set({ showNotificationPanel: !get().showNotificationPanel });
  },

  jumpToUnread: () => {
    const { workspaces, workspaceOrder, activeWorkspaceId } = get();
    // Find first workspace with unread notifications (not the active one)
    const target = workspaceOrder.find(
      (id) =>
        id !== activeWorkspaceId && workspaces[id] && workspaces[id]!.unreadCount > 0,
    );
    if (target) {
      get().switchWorkspace(target);
    }
  },

  reorderWorkspaces: (fromIndex, toIndex) => {
    const { workspaceOrder } = get();
    if (fromIndex < 0 || fromIndex >= workspaceOrder.length) return;
    if (toIndex < 0 || toIndex >= workspaceOrder.length) return;
    if (fromIndex === toIndex) return;

    const newOrder = [...workspaceOrder];
    const [moved] = newOrder.splice(fromIndex, 1);
    newOrder.splice(toIndex, 0, moved!);
    set({ workspaceOrder: newOrder });
  },

  restoreSession: (snapshots, activeIndex) => {
    if (snapshots.length === 0) return;

    const newWorkspaces: Record<string, Workspace> = {};
    const newOrder: string[] = [];

    for (const snap of snapshots) {
      if (!isValidPaneTreeSnap(snap.paneTree)) {
        continue;
      }

      const { node, surfaces } = rebuildPaneTree(snap.paneTree);
      const leafIds = collectLeafIds(node);
      const focusedLeafIndex = Number.isInteger(snap.focusedLeafIndex)
        ? Math.max(0, Math.min(snap.focusedLeafIndex, leafIds.length - 1))
        : 0;
      const ws: Workspace = {
        id: crypto.randomUUID(),
        name: snap.name,
        root: node,
        surfaces,
        focusedPaneId: leafIds[focusedLeafIndex] ?? firstLeafId(node),
        workingDir: snap.workingDir,
        gitBranch: snap.gitBranch,
        worktreeDir: snap.worktreeDir,
        worktreeName: snap.worktreeName,
        worktreeStatus: "",
        unreadCount: 0,
        lastNotificationText: "",
        createdAt: new Date().toISOString(),
      };
      newWorkspaces[ws.id] = ws;
      newOrder.push(ws.id);
    }

    if (newOrder.length === 0) return;

    const safeIndex = Math.min(activeIndex, newOrder.length - 1);
    set({
      workspaces: newWorkspaces,
      workspaceOrder: newOrder,
      activeWorkspaceId: newOrder[safeIndex]!,
    });
  },
}));

// --- Session snapshot for persistence ---

export function getSessionData(): {
  workspaces: SessionSnapshot[];
  activeIndex: number;
} {
  const state = useWorkspaceStore.getState();
  const workspaces = state.workspaceOrder.map((id) => {
    const ws = state.workspaces[id]!;
    return {
      name: ws.name,
      workingDir: ws.workingDir,
      gitBranch: ws.gitBranch,
      worktreeDir: ws.worktreeDir,
      worktreeName: ws.worktreeName,
      paneTree: snapshotPaneTree(ws.root),
      focusedLeafIndex: Math.max(0, collectLeafIds(ws.root).indexOf(ws.focusedPaneId)),
    };
  });
  const activeIndex = state.workspaceOrder.indexOf(state.activeWorkspaceId);
  return { workspaces, activeIndex: Math.max(0, activeIndex) };
}

// --- Activity tracking (outside Zustand to avoid re-render churn) ---

const surfaceActivityMap = new Map<string, number>();

export function updateSurfaceActivity(paneId: string): void {
  surfaceActivityMap.set(paneId, Date.now());
}

export function getLastActivity(paneId: string): number {
  return surfaceActivityMap.get(paneId) ?? 0;
}

/** Timestamp of the last workspace switch. Used to suppress spurious prompt notifications. */
let lastWorkspaceSwitchTime = 0;
export function getLastWorkspaceSwitchTime(): number {
  return lastWorkspaceSwitchTime;
}

export function closeWorkspaceEnsuringOneRemains(
  id: string,
  fallbackWorkingDir?: string,
): void {
  const state = useWorkspaceStore.getState();
  const workspace = state.workspaces[id];
  if (!workspace) return;

  if (state.workspaceOrder.length <= 1) {
    state.createWorkspace(
      undefined,
      fallbackWorkingDir?.trim() || workspace.workingDir,
    );
  }

  useWorkspaceStore.getState().closeWorkspace(id);
}

export type { WorkspaceState, AppNotification };
