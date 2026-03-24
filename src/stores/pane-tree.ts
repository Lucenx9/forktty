// --- PaneTree types (matches SPEC.md) ---

export interface PaneLeaf {
  type: "leaf";
  id: string;
  surfaceId: string;
}

export interface PaneSplit {
  type: "horizontal" | "vertical";
  id: string;
  children: PaneNode[];
  sizes: number[];
}

export type PaneNode = PaneLeaf | PaneSplit;

// --- Bounding rect for spatial navigation ---

export interface PaneRect {
  id: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Direction = "left" | "right" | "up" | "down";

// --- Session snapshot types ---

export interface PaneTreeSnap {
  type: "leaf" | "horizontal" | "vertical";
  children?: PaneTreeSnap[];
  sizes?: number[];
}

export interface SessionSnapshot {
  name: string;
  workingDir: string;
  gitBranch: string;
  worktreeDir: string;
  worktreeName: string;
  paneTree: PaneTreeSnap;
}

// --- Surface type (needed by factory helpers) ---

export interface Surface {
  id: string;
  ptyId: number | null;
  title: string;
  hasUnreadNotification: boolean;
}

// --- Workspace type (needed by factory/query helpers) ---

export interface Workspace {
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
  lastNotificationText: string;
  createdAt: string;
}

// --- Constants ---

export const MAX_SPLIT_DEPTH = 5;

// --- Pure pane tree algorithms ---

/** Find and replace a node in the tree by ID (immutable). */
export function replaceNode(
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
export function removeLeaf(
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
      child.id === targetId || (child.type !== "leaf" && containsLeaf(child, targetId)),
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
    const focusId = firstLeafId(remaining[Math.min(childIndex, remaining.length - 1)]!);
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

export function containsLeaf(node: PaneNode, leafId: string): boolean {
  if (node.type === "leaf") return node.id === leafId;
  return node.children.some((child) => containsLeaf(child, leafId));
}

export function updateSplitSizes(
  node: PaneNode,
  splitId: string,
  sizes: number[],
): PaneNode {
  if (node.type === "leaf") {
    return node;
  }

  if (node.id === splitId) {
    if (node.sizes.length !== sizes.length) {
      return node;
    }

    const nextSizes = sizes.map((size) => Number(size));
    const changed = nextSizes.some((size, index) => size !== node.sizes[index]);
    return changed ? { ...node, sizes: nextSizes } : node;
  }

  let changed = false;
  const children = node.children.map((child) => {
    const nextChild = updateSplitSizes(child, splitId, sizes);
    if (nextChild !== child) {
      changed = true;
    }
    return nextChild;
  });

  return changed ? { ...node, children } : node;
}

export function firstLeafId(node: PaneNode): string {
  if (node.type === "leaf") return node.id;
  return firstLeafId(node.children[0]!);
}

export function collectLeafIds(node: PaneNode): string[] {
  if (node.type === "leaf") return [node.id];
  return node.children.flatMap(collectLeafIds);
}

export function findLeaf(node: PaneNode, id: string): PaneLeaf | null {
  if (node.type === "leaf") return node.id === id ? node : null;
  for (const child of node.children) {
    const found = findLeaf(child, id);
    if (found) return found;
  }
  return null;
}

/** Get the depth of a node within the tree (0 = root). Returns -1 if not found. */
export function getNodeDepth(root: PaneNode, targetId: string, depth = 0): number {
  if (root.id === targetId) return depth;
  if (root.type === "leaf") return -1;
  for (const child of root.children) {
    const d = getNodeDepth(child, targetId, depth + 1);
    if (d >= 0) return d;
  }
  return -1;
}

/** Build bounding rectangles for all leaves based on tree structure. */
export function buildLayoutRects(
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
      rects.push(...buildLayoutRects(node.children[i]!, x + offset, y, childW, h));
      offset += childW;
    } else {
      const childH = h * fraction;
      rects.push(...buildLayoutRects(node.children[i]!, x, y + offset, w, childH));
      offset += childH;
    }
  }

  return rects;
}

/** Find the best pane to focus when moving in a direction. */
export function findNeighbor(
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

// --- Factory helpers ---

export function makeLeaf(): PaneLeaf {
  const id = crypto.randomUUID();
  return { type: "leaf", id, surfaceId: id };
}

export function makeSurface(id: string): Surface {
  return { id, ptyId: null, title: "", hasUnreadNotification: false };
}

export function generateWorkspaceName(workspaces: Record<string, Workspace>): string {
  const existingNames = new Set(Object.values(workspaces).map((w) => w.name));
  let n = 1;
  while (existingNames.has(`Workspace ${n}`)) {
    n++;
  }
  return `Workspace ${n}`;
}

export function makeWorkspace(
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
    lastNotificationText: "",
    createdAt: new Date().toISOString(),
  };
}

/** Swap two leaf nodes in the tree (positions swap, identities preserved). */
export function swapLeaves(node: PaneNode, idA: string, idB: string): PaneNode {
  if (idA === idB) return node;
  const leafA = findLeaf(node, idA);
  const leafB = findLeaf(node, idB);
  if (!leafA || !leafB) return node;

  // Single-pass: replace A's position with B and B's position with A
  function swap(n: PaneNode): PaneNode {
    if (n.type === "leaf") {
      if (n.id === leafA!.id) return leafB!;
      if (n.id === leafB!.id) return leafA!;
      return n;
    }
    let changed = false;
    const children = n.children.map((child) => {
      const result = swap(child);
      if (result !== child) changed = true;
      return result;
    });
    return changed ? { ...n, children } : n;
  }

  return swap(node);
}

/** Find workspace ID that contains the given pane. */
export function findWorkspaceIdByPane(
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

// --- Session snapshot helpers ---

/** Rebuild a PaneNode + surfaces from a snapshot. */
export function rebuildPaneTree(snap: PaneTreeSnap): {
  node: PaneNode;
  surfaces: Record<string, Surface>;
} {
  if (snap.type === "leaf") {
    const leaf = makeLeaf();
    return {
      node: leaf,
      surfaces: { [leaf.surfaceId]: makeSurface(leaf.surfaceId) },
    };
  }

  const children: PaneNode[] = [];
  const surfaces: Record<string, Surface> = {};
  for (const child of snap.children ?? []) {
    const result = rebuildPaneTree(child);
    children.push(result.node);
    Object.assign(surfaces, result.surfaces);
  }

  const node: PaneSplit = {
    type: snap.type,
    id: crypto.randomUUID(),
    children,
    sizes: snap.sizes ?? children.map(() => 100 / children.length),
  };

  return { node, surfaces };
}

/** Serialize a PaneNode to snapshot format (no ids, no surfaces). */
export function snapshotPaneTree(node: PaneNode): PaneTreeSnap {
  if (node.type === "leaf") {
    return { type: "leaf" };
  }
  return {
    type: node.type,
    children: node.children.map(snapshotPaneTree),
    sizes: [...node.sizes],
  };
}
