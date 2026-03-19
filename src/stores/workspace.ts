import { create } from "zustand";

// --- PaneTree types (matches SPEC.md) ---

interface PaneLeaf {
  type: "leaf";
  id: string;
  surfaceId: string;
}

interface PaneSplit {
  type: "horizontal" | "vertical";
  id: string;
  children: PaneNode[];
  sizes: number[];
}

type PaneNode = PaneLeaf | PaneSplit;

interface Surface {
  id: string;
  ptyId: number | null;
  title: string;
}

// --- Bounding rect for spatial navigation ---

interface PaneRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

type Direction = "left" | "right" | "up" | "down";

// --- Workspace ---

interface Workspace {
  id: string;
  name: string;
  root: PaneNode;
  surfaces: Record<string, Surface>;
  focusedPaneId: string;
  workingDir: string;
  gitBranch: string;
  worktreeDir: string;
  worktreeName: string;
  worktreeStatus: string;
  unreadCount: number;
  createdAt: string;
}

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
  setFocusedPane: (paneId: string) => void;
  moveFocus: (direction: Direction) => void;

  // Surface lifecycle (finds workspace by paneId)
  registerSurface: (paneId: string, ptyId: number) => void;
  unregisterSurface: (paneId: string) => void;

  // Notification actions
  addNotification: (workspaceId: string, title: string, body: string) => void;
  markWorkspaceRead: (workspaceId: string) => void;
  clearNotifications: () => void;
  toggleNotificationPanel: () => void;
  jumpToUnread: () => void;
}

// --- Helper functions ---

function makeLeaf(): PaneLeaf {
  const id = crypto.randomUUID();
  return { type: "leaf", id, surfaceId: id };
}

function makeSurface(id: string): Surface {
  return { id, ptyId: null, title: "" };
}

/** Find and replace a node in the tree by ID (immutable). */
function replaceNode(
  node: PaneNode,
  targetId: string,
  replacement: PaneNode,
): PaneNode | null {
  if (node.id === targetId) return replacement;
  if (node.type === "leaf") return null;

  for (let i = 0; i < node.children.length; i++) {
    const result = replaceNode(node.children[i]!, targetId, replacement);
    if (result) {
      const newChildren = [...node.children];
      newChildren[i] = result;
      return { ...node, children: newChildren };
    }
  }
  return null;
}

/** Remove a leaf from the tree. Returns the new tree (or null if tree is empty)
 *  and the ID of the sibling that should receive focus. */
function removeLeaf(
  node: PaneNode,
  targetId: string,
): { tree: PaneNode | null; focusId: string | null } {
  if (node.type === "leaf") {
    if (node.id === targetId) return { tree: null, focusId: null };
    return { tree: node, focusId: null };
  }

  // Find which child contains the target
  const childIndex = node.children.findIndex(
    (child) =>
      child.id === targetId ||
      (child.type !== "leaf" && containsLeaf(child, targetId)),
  );

  if (childIndex === -1) return { tree: node, focusId: null };

  const child = node.children[childIndex]!;

  // If the target is a direct child leaf
  if (child.type === "leaf" && child.id === targetId) {
    const remaining = node.children.filter((_, i) => i !== childIndex);
    if (remaining.length === 0) return { tree: null, focusId: null };
    if (remaining.length === 1) {
      // Collapse: parent becomes the sole remaining child
      const survivor = remaining[0]!;
      const focusId = firstLeafId(survivor);
      return { tree: survivor, focusId };
    }
    // More than 2 children (shouldn't happen in binary splits, but handle it)
    const newSizes = node.sizes.filter((_, i) => i !== childIndex);
    const totalSize = newSizes.reduce((a, b) => a + b, 0);
    const normalizedSizes = newSizes.map((s) => (s / totalSize) * 100);
    const focusId = firstLeafId(
      remaining[Math.min(childIndex, remaining.length - 1)]!,
    );
    return {
      tree: {
        ...node,
        children: remaining,
        sizes: normalizedSizes,
      },
      focusId,
    };
  }

  // Target is deeper in this child
  const result = removeLeaf(child, targetId);
  if (result.tree === null) {
    // The entire subtree was removed
    const remaining = node.children.filter((_, i) => i !== childIndex);
    if (remaining.length === 1) {
      return { tree: remaining[0]!, focusId: result.focusId };
    }
    const newSizes = node.sizes.filter((_, i) => i !== childIndex);
    const totalSize = newSizes.reduce((a, b) => a + b, 0);
    const normalizedSizes = newSizes.map((s) => (s / totalSize) * 100);
    return {
      tree: { ...node, children: remaining, sizes: normalizedSizes },
      focusId: result.focusId,
    };
  }

  const newChildren = [...node.children];
  newChildren[childIndex] = result.tree;
  return {
    tree: { ...node, children: newChildren },
    focusId: result.focusId,
  };
}

function containsLeaf(node: PaneNode, leafId: string): boolean {
  if (node.type === "leaf") return node.id === leafId;
  return node.children.some((child) => containsLeaf(child, leafId));
}

function firstLeafId(node: PaneNode): string {
  if (node.type === "leaf") return node.id;
  return firstLeafId(node.children[0]!);
}

function collectLeafIds(node: PaneNode): string[] {
  if (node.type === "leaf") return [node.id];
  return node.children.flatMap(collectLeafIds);
}

function findLeaf(node: PaneNode, id: string): PaneLeaf | null {
  if (node.type === "leaf") return node.id === id ? node : null;
  for (const child of node.children) {
    const found = findLeaf(child, id);
    if (found) return found;
  }
  return null;
}

/** Build bounding rectangles for all leaves based on tree structure. */
function buildLayoutRects(
  node: PaneNode,
  x: number,
  y: number,
  w: number,
  h: number,
): PaneRect[] {
  if (node.type === "leaf") {
    return [{ id: node.id, x, y, w, h }];
  }

  const rects: PaneRect[] = [];
  let offset = 0;
  const totalSize = node.sizes.reduce((a, b) => a + b, 0);

  for (let i = 0; i < node.children.length; i++) {
    const fraction = node.sizes[i]! / totalSize;
    if (node.type === "horizontal") {
      const childW = w * fraction;
      rects.push(
        ...buildLayoutRects(node.children[i]!, x + offset, y, childW, h),
      );
      offset += childW;
    } else {
      const childH = h * fraction;
      rects.push(
        ...buildLayoutRects(node.children[i]!, x, y + offset, w, childH),
      );
      offset += childH;
    }
  }

  return rects;
}

/** Find the best pane to focus when moving in a direction. */
function findNeighbor(
  rects: PaneRect[],
  currentId: string,
  direction: Direction,
): string | null {
  const current = rects.find((r) => r.id === currentId);
  if (!current) return null;

  const cx = current.x + current.w / 2;
  const cy = current.y + current.h / 2;

  let candidates: PaneRect[];
  switch (direction) {
    case "left":
      candidates = rects.filter(
        (r) => r.id !== currentId && r.x + r.w <= current.x + 0.01,
      );
      break;
    case "right":
      candidates = rects.filter(
        (r) => r.id !== currentId && r.x >= current.x + current.w - 0.01,
      );
      break;
    case "up":
      candidates = rects.filter(
        (r) => r.id !== currentId && r.y + r.h <= current.y + 0.01,
      );
      break;
    case "down":
      candidates = rects.filter(
        (r) => r.id !== currentId && r.y >= current.y + current.h - 0.01,
      );
      break;
  }

  if (candidates.length === 0) return null;

  // Pick the candidate closest to the current center
  let best = candidates[0]!;
  let bestDist = Infinity;
  for (const c of candidates) {
    const dx = c.x + c.w / 2 - cx;
    const dy = c.y + c.h / 2 - cy;
    const dist = dx * dx + dy * dy;
    if (dist < bestDist) {
      bestDist = dist;
      best = c;
    }
  }

  return best.id;
}

// --- Workspace helpers ---

function generateWorkspaceName(workspaces: Record<string, Workspace>): string {
  const existingNames = new Set(Object.values(workspaces).map((w) => w.name));
  let n = 1;
  while (existingNames.has(`Workspace ${n}`)) {
    n++;
  }
  return `Workspace ${n}`;
}

function makeWorkspace(
  name: string,
  opts?: {
    workingDir?: string;
    gitBranch?: string;
    worktreeDir?: string;
    worktreeName?: string;
  },
): Workspace {
  const leaf = makeLeaf();
  return {
    id: crypto.randomUUID(),
    name,
    root: leaf,
    surfaces: { [leaf.surfaceId]: makeSurface(leaf.surfaceId) },
    focusedPaneId: leaf.id,
    workingDir: opts?.workingDir ?? "",
    gitBranch: opts?.gitBranch ?? "",
    worktreeDir: opts?.worktreeDir ?? "",
    worktreeName: opts?.worktreeName ?? "",
    worktreeStatus: "",
    unreadCount: 0,
    createdAt: new Date().toISOString(),
  };
}

/** Find workspace ID that contains the given pane. */
function findWorkspaceIdByPane(
  workspaces: Record<string, Workspace>,
  paneId: string,
): string | null {
  for (const [wsId, ws] of Object.entries(workspaces)) {
    if (ws.surfaces[paneId]) {
      return wsId;
    }
  }
  return null;
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
    const { workspaces } = get();
    if (!workspaces[id]) return;
    set({ activeWorkspaceId: id });
  },

  closeWorkspace: (id) => {
    const { workspaces, workspaceOrder, activeWorkspaceId } = get();
    if (workspaceOrder.length <= 1) return; // Can't close last workspace

    const newWorkspaces = { ...workspaces };
    delete newWorkspaces[id];
    const newOrder = workspaceOrder.filter((wId) => wId !== id);

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
    });
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

  setFocusedPane: (paneId) => {
    const { workspaces, activeWorkspaceId } = get();
    const ws = workspaces[activeWorkspaceId];
    if (!ws) return;

    set({
      workspaces: {
        ...workspaces,
        [activeWorkspaceId]: { ...ws, focusedPaneId: paneId },
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

  // --- Notification actions ---

  addNotification: (workspaceId, title, body) => {
    const { workspaces, notifications } = get();
    const ws = workspaces[workspaceId];
    if (!ws) return;

    const notification: AppNotification = {
      id: crypto.randomUUID(),
      workspaceId,
      workspaceName: ws.name,
      title,
      body,
      timestamp: Date.now(),
      read: false,
    };

    set({
      notifications: [notification, ...notifications].slice(0, 100),
      workspaces: {
        ...workspaces,
        [workspaceId]: { ...ws, unreadCount: ws.unreadCount + 1 },
      },
    });
  },

  markWorkspaceRead: (workspaceId) => {
    const { workspaces, notifications } = get();
    const ws = workspaces[workspaceId];
    if (!ws || ws.unreadCount === 0) return;

    set({
      workspaces: {
        ...workspaces,
        [workspaceId]: { ...ws, unreadCount: 0 },
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
      cleared[id] = { ...ws, unreadCount: 0 };
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
      set({ activeWorkspaceId: target });
    }
  },
}));

// --- Activity tracking (outside Zustand to avoid re-render churn) ---

const surfaceActivityMap = new Map<string, number>();

export function updateSurfaceActivity(paneId: string): void {
  surfaceActivityMap.set(paneId, Date.now());
}

export function getLastActivity(paneId: string): number {
  return surfaceActivityMap.get(paneId) ?? 0;
}

export type {
  PaneNode,
  PaneLeaf,
  PaneSplit,
  Surface,
  Direction,
  Workspace,
  WorkspaceState,
  AppNotification,
};
