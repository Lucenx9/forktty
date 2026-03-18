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

// --- Store interface ---

interface WorkspaceState {
  root: PaneNode;
  surfaces: Record<string, Surface>;
  focusedPaneId: string;

  splitPane: (paneId: string, direction: "horizontal" | "vertical") => void;
  closePane: (paneId: string) => void;
  setFocusedPane: (paneId: string) => void;
  moveFocus: (direction: Direction) => void;
  registerSurface: (paneId: string, ptyId: number) => void;
  unregisterSurface: (paneId: string) => void;
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
    // The entire subtree was removed (shouldn't happen for single leaf removal)
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

// --- Initial state ---

const initialLeaf = makeLeaf();

// --- Store ---

export const useWorkspaceStore = create<WorkspaceState>((set, get) => ({
  root: initialLeaf,
  surfaces: { [initialLeaf.surfaceId]: makeSurface(initialLeaf.surfaceId) },
  focusedPaneId: initialLeaf.id,

  splitPane: (paneId, direction) => {
    const { root, surfaces } = get();
    const existingLeaf = findLeaf(root, paneId);
    if (!existingLeaf) return;

    const newLeaf = makeLeaf();

    const splitNode: PaneSplit = {
      type: direction,
      id: crypto.randomUUID(),
      children: [existingLeaf, newLeaf],
      sizes: [50, 50],
    };

    const newRoot = replaceNode(root, paneId, splitNode);
    if (!newRoot) return;

    set({
      root: newRoot,
      surfaces: {
        ...surfaces,
        [newLeaf.surfaceId]: makeSurface(newLeaf.surfaceId),
      },
      focusedPaneId: newLeaf.id,
    });
  },

  closePane: (paneId) => {
    const { root, surfaces } = get();
    const leaves = collectLeafIds(root);
    // Don't close the last pane
    if (leaves.length <= 1) return;

    const result = removeLeaf(root, paneId);
    if (!result.tree) return;

    // Remove the surface for the closed pane
    const newSurfaces = { ...surfaces };
    delete newSurfaces[paneId];

    set({
      root: result.tree,
      surfaces: newSurfaces,
      focusedPaneId: result.focusId ?? firstLeafId(result.tree),
    });
  },

  setFocusedPane: (paneId) => {
    set({ focusedPaneId: paneId });
  },

  moveFocus: (direction) => {
    const { root, focusedPaneId } = get();
    const rects = buildLayoutRects(root, 0, 0, 1000, 1000);
    const neighborId = findNeighbor(rects, focusedPaneId, direction);
    if (neighborId) {
      set({ focusedPaneId: neighborId });
    }
  },

  registerSurface: (paneId, ptyId) => {
    const { surfaces } = get();
    const surface = surfaces[paneId];
    if (surface) {
      set({
        surfaces: {
          ...surfaces,
          [paneId]: { ...surface, ptyId },
        },
      });
    }
  },

  unregisterSurface: (paneId) => {
    const { surfaces } = get();
    const surface = surfaces[paneId];
    if (surface) {
      set({
        surfaces: {
          ...surfaces,
          [paneId]: { ...surface, ptyId: null },
        },
      });
    }
  },
}));

export type { PaneNode, PaneLeaf, PaneSplit, Surface, Direction };
