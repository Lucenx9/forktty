import { create } from "zustand";
import { useMetadataStore } from "./metadata";
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
  createWorkspace: (name?: string) => string;
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
  splitPane: (paneId: string, direction: "horizontal" | "vertical") => void;
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

// --- Store ---

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  workspaces: { [initialWorkspace.id]: initialWorkspace },
  activeWorkspaceId: initialWorkspace.id,
  workspaceOrder: [initialWorkspace.id],
  notifications: [],
  showNotificationPanel: false,

  // --- Workspace actions ---

  createWorkspace: (name?) => {
    const { workspaces, workspaceOrder } = get();
    const wsName = name ?? generateWorkspaceName(workspaces);
    const ws = makeWorkspace(wsName);

    set({
      workspaces: { ...workspaces, [ws.id]: ws },
      activeWorkspaceId: ws.id,
      workspaceOrder: [...workspaceOrder, ws.id],
    });

    return ws.id;
  },

  createWorktreeWorkspace: (
    name,
    workingDir,
    gitBranch,
    worktreeDir,
    worktreeName,
  ) => {
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
    const ws = workspaces[id];
    if (!ws) return;
    lastWorkspaceSwitchTime = Date.now();

    // Merge activeWorkspaceId + markWorkspaceRead into a single state update
    // to avoid a double render on every workspace switch.
    if (ws.unreadCount > 0) {
      const clearedSurfaces: Record<string, Surface> = {};
      for (const [sid, surface] of Object.entries(ws.surfaces)) {
        clearedSurfaces[sid] = surface.hasUnreadNotification
          ? { ...surface, hasUnreadNotification: false }
          : surface;
      }
      set({
        activeWorkspaceId: id,
        workspaces: {
          ...workspaces,
          [id]: {
            ...ws,
            unreadCount: 0,
            lastNotificationText: "",
            surfaces: clearedSurfaces,
          },
        },
        notifications: notifications.map((n) =>
          n.workspaceId === id ? { ...n, read: true } : n,
        ),
      });
    } else {
      set({ activeWorkspaceId: id });
    }
  },

  closeWorkspace: (id) => {
    const { workspaces, workspaceOrder, activeWorkspaceId, notifications } =
      get();
    if (workspaceOrder.length <= 1) return; // Can't close last workspace

    const newWorkspaces = { ...workspaces };
    delete newWorkspaces[id];
    const newOrder = workspaceOrder.filter((wId) => wId !== id);
    const newNotifications = notifications.filter((n) => n.workspaceId !== id);

    let newActiveId = activeWorkspaceId;
    if (activeWorkspaceId === id) {
      const oldIndex = workspaceOrder.indexOf(id);
      newActiveId =
        newOrder[Math.min(oldIndex, newOrder.length - 1)] ?? newOrder[0]!;
    }

    set({
      workspaces: newWorkspaces,
      workspaceOrder: newOrder,
      activeWorkspaceId: newActiveId,
      notifications: newNotifications,
    });

    // Clean up ephemeral metadata for the closed workspace
    useMetadataStore.getState().pruneWorkspace(id);
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

  splitPane: (paneId, direction) => {
    const { workspaces, activeWorkspaceId } = get();
    const ws = workspaces[activeWorkspaceId];
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
        [activeWorkspaceId]: {
          ...ws,
          root: newRoot,
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
    const ws = workspaces[activeWorkspaceId];
    if (!ws) return;

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
        [activeWorkspaceId]: {
          ...ws,
          root: result.tree,
          surfaces: newSurfaces,
          focusedPaneId: result.focusId ?? firstLeafId(result.tree),
        },
      },
    });
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
    const ws = workspaces[activeWorkspaceId];
    if (!ws) return;

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
        [activeWorkspaceId]: {
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
      set({
        workspaces: {
          ...workspaces,
          [activeWorkspaceId]: { ...ws, focusedPaneId: neighborId },
        },
      });
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
    const { workspaces, notifications, activeWorkspaceId, workspaceOrder } =
      get();
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
        newOrder = [
          workspaceId,
          ...workspaceOrder.filter((id) => id !== workspaceId),
        ];
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
    if (!ws || ws.unreadCount === 0) return;

    // Clear unread notification on all surfaces in this workspace
    const clearedSurfaces: Record<string, Surface> = {};
    for (const [id, surface] of Object.entries(ws.surfaces)) {
      clearedSurfaces[id] = surface.hasUnreadNotification
        ? { ...surface, hasUnreadNotification: false }
        : surface;
    }

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: {
          ...ws,
          unreadCount: 0,
          lastNotificationText: "",
          surfaces: clearedSurfaces,
        },
      },
      notifications: notifications.map((n) =>
        n.workspaceId === workspaceId ? { ...n, read: true } : n,
      ),
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
        id !== activeWorkspaceId &&
        workspaces[id] &&
        workspaces[id]!.unreadCount > 0,
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
      const { node, surfaces } = rebuildPaneTree(snap.paneTree);
      const ws: Workspace = {
        id: crypto.randomUUID(),
        name: snap.name,
        root: node,
        surfaces,
        focusedPaneId: firstLeafId(node),
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

export function closeWorkspaceEnsuringOneRemains(id: string): void {
  const state = useWorkspaceStore.getState();
  if (!state.workspaces[id]) return;

  if (state.workspaceOrder.length <= 1) {
    state.createWorkspace();
  }

  useWorkspaceStore.getState().closeWorkspace(id);
}

export type { WorkspaceState, AppNotification };
