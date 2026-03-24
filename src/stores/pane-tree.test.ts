import { describe, it, expect } from "vitest";
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
  makeLeaf,
  makeSurface,
  makeWorkspace,
  generateWorkspaceName,
  findWorkspaceIdByPane,
  swapLeaves,
  rebuildPaneTree,
  snapshotPaneTree,
  MAX_SPLIT_DEPTH,
} from "./pane-tree";
import type { PaneLeaf, PaneSplit, PaneNode } from "./pane-tree";

// --- Helpers ---

function leaf(id: string): PaneLeaf {
  return { type: "leaf", id, surfaceId: id };
}

function hsplit(id: string, children: PaneNode[], sizes?: number[]): PaneSplit {
  return {
    type: "horizontal",
    id,
    children,
    sizes: sizes ?? children.map(() => 50),
  };
}

function vsplit(id: string, children: PaneNode[], sizes?: number[]): PaneSplit {
  return {
    type: "vertical",
    id,
    children,
    sizes: sizes ?? children.map(() => 50),
  };
}

// --- replaceNode ---

describe("replaceNode", () => {
  it("replaces root node", () => {
    const root = leaf("a");
    const replacement = leaf("b");
    const result = replaceNode(root, "a", replacement);
    expect(result).toBe(replacement);
  });

  it("replaces child in split", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    const replacement = leaf("c");
    const result = replaceNode(root, "b", replacement);
    expect(result).not.toBeNull();
    expect((result as PaneSplit).children[1]).toBe(replacement);
  });

  it("replaces deep nested node", () => {
    const root = hsplit("s1", [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")])]);
    const replacement = leaf("x");
    const result = replaceNode(root, "c", replacement);
    expect(result).not.toBeNull();
    const inner = (result as PaneSplit).children[1] as PaneSplit;
    expect(inner.children[1]).toBe(replacement);
  });

  it("returns null when target not found", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    expect(replaceNode(root, "missing", leaf("x"))).toBeNull();
  });

  it("does not mutate original tree", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    const original = JSON.stringify(root);
    replaceNode(root, "b", leaf("c"));
    expect(JSON.stringify(root)).toBe(original);
  });
});

// --- removeLeaf ---

describe("removeLeaf", () => {
  it("removes sole leaf, returns null tree", () => {
    const result = removeLeaf(leaf("a"), "a");
    expect(result.tree).toBeNull();
    expect(result.focusId).toBeNull();
  });

  it("does nothing if target not found", () => {
    const root = leaf("a");
    const result = removeLeaf(root, "missing");
    expect(result.tree).toBe(root);
  });

  it("collapses split to single child when removing one of two", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const result = removeLeaf(root, "a");
    expect(result.tree).toEqual(leaf("b"));
    expect(result.focusId).toBe("b");
  });

  it("focuses sibling when removing from split", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const result = removeLeaf(root, "b");
    expect(result.focusId).toBe("a");
  });

  it("removes deeply nested leaf and collapses parent", () => {
    const root = hsplit("s1", [
      leaf("a"),
      vsplit("s2", [leaf("b"), leaf("c")], [50, 50]),
    ]);
    const result = removeLeaf(root, "b");
    expect(result.tree).not.toBeNull();
    // s2 had 2 children, removing one collapses it to just leaf("c")
    const newRoot = result.tree as PaneSplit;
    expect(newRoot.children[1]).toEqual(leaf("c"));
  });

  it("normalizes sizes when removing from 3-child split", () => {
    const root: PaneSplit = {
      type: "horizontal",
      id: "s1",
      children: [leaf("a"), leaf("b"), leaf("c")],
      sizes: [33, 34, 33],
    };
    const result = removeLeaf(root, "b");
    const newRoot = result.tree as PaneSplit;
    expect(newRoot.children).toHaveLength(2);
    const totalSize = newRoot.sizes.reduce((a, b) => a + b, 0);
    expect(totalSize).toBeCloseTo(100, 5);
  });
});

// --- containsLeaf ---

describe("containsLeaf", () => {
  it("finds leaf at root", () => {
    expect(containsLeaf(leaf("a"), "a")).toBe(true);
  });

  it("returns false for missing leaf", () => {
    expect(containsLeaf(leaf("a"), "b")).toBe(false);
  });

  it("finds leaf in nested tree", () => {
    const root = hsplit("s1", [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")])]);
    expect(containsLeaf(root, "c")).toBe(true);
    expect(containsLeaf(root, "missing")).toBe(false);
  });
});

// --- firstLeafId / collectLeafIds ---

describe("firstLeafId", () => {
  it("returns id of a single leaf", () => {
    expect(firstLeafId(leaf("a"))).toBe("a");
  });

  it("returns leftmost leaf in split", () => {
    const root = hsplit("s1", [leaf("x"), leaf("y")]);
    expect(firstLeafId(root)).toBe("x");
  });

  it("returns leftmost leaf in deep tree", () => {
    const root = hsplit("s1", [vsplit("s2", [leaf("deep"), leaf("b")]), leaf("c")]);
    expect(firstLeafId(root)).toBe("deep");
  });
});

describe("collectLeafIds", () => {
  it("returns single id for leaf", () => {
    expect(collectLeafIds(leaf("a"))).toEqual(["a"]);
  });

  it("returns all leaf ids in order", () => {
    const root = hsplit("s1", [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")])]);
    expect(collectLeafIds(root)).toEqual(["a", "b", "c"]);
  });
});

// --- findLeaf ---

describe("findLeaf", () => {
  it("finds leaf at root", () => {
    const l = leaf("a");
    expect(findLeaf(l, "a")).toBe(l);
  });

  it("finds leaf in nested tree", () => {
    const target = leaf("b");
    const root = hsplit("s1", [leaf("a"), target]);
    expect(findLeaf(root, "b")).toBe(target);
  });

  it("returns null for missing leaf", () => {
    expect(findLeaf(leaf("a"), "x")).toBeNull();
  });
});

// --- getNodeDepth ---

describe("getNodeDepth", () => {
  it("returns 0 for root", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    expect(getNodeDepth(root, "s1")).toBe(0);
  });

  it("returns correct depth for nested node", () => {
    const root = hsplit("s1", [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")])]);
    expect(getNodeDepth(root, "a")).toBe(1);
    expect(getNodeDepth(root, "s2")).toBe(1);
    expect(getNodeDepth(root, "c")).toBe(2);
  });

  it("returns -1 for missing node", () => {
    expect(getNodeDepth(leaf("a"), "missing")).toBe(-1);
  });
});

// --- updateSplitSizes ---

describe("updateSplitSizes", () => {
  it("updates sizes of matching split", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const result = updateSplitSizes(root, "s1", [30, 70]);
    expect((result as PaneSplit).sizes).toEqual([30, 70]);
  });

  it("rejects size array with wrong length", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const result = updateSplitSizes(root, "s1", [30, 40, 30]);
    expect(result).toBe(root); // unchanged
  });

  it("returns same reference if sizes unchanged", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const result = updateSplitSizes(root, "s1", [50, 50]);
    expect(result).toBe(root);
  });

  it("updates nested split", () => {
    const root = hsplit("s1", [
      leaf("a"),
      vsplit("s2", [leaf("b"), leaf("c")], [60, 40]),
    ]);
    const result = updateSplitSizes(root, "s2", [30, 70]);
    const inner = (result as PaneSplit).children[1] as PaneSplit;
    expect(inner.sizes).toEqual([30, 70]);
  });

  it("is a no-op on leaf", () => {
    const l = leaf("a");
    expect(updateSplitSizes(l, "s1", [50, 50])).toBe(l);
  });
});

// --- buildLayoutRects ---

describe("buildLayoutRects", () => {
  it("returns single rect for leaf", () => {
    const rects = buildLayoutRects(leaf("a"), 0, 0, 100, 100);
    expect(rects).toEqual([{ id: "a", x: 0, y: 0, w: 100, h: 100 }]);
  });

  it("splits horizontally into two halves", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const rects = buildLayoutRects(root, 0, 0, 100, 100);
    expect(rects).toHaveLength(2);
    expect(rects[0]!.w).toBeCloseTo(50);
    expect(rects[1]!.x).toBeCloseTo(50);
    expect(rects[1]!.w).toBeCloseTo(50);
  });

  it("splits vertically into two halves", () => {
    const root = vsplit("s1", [leaf("a"), leaf("b")], [50, 50]);
    const rects = buildLayoutRects(root, 0, 0, 100, 100);
    expect(rects).toHaveLength(2);
    expect(rects[0]!.h).toBeCloseTo(50);
    expect(rects[1]!.y).toBeCloseTo(50);
  });

  it("handles unequal sizes", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")], [25, 75]);
    const rects = buildLayoutRects(root, 0, 0, 200, 100);
    expect(rects[0]!.w).toBeCloseTo(50);
    expect(rects[1]!.w).toBeCloseTo(150);
  });
});

// --- findNeighbor ---

describe("findNeighbor", () => {
  const root = hsplit(
    "s1",
    [leaf("left"), vsplit("s2", [leaf("top-right"), leaf("bot-right")])],
    [50, 50],
  );
  const rects = buildLayoutRects(root, 0, 0, 100, 100);

  it("finds right neighbor", () => {
    expect(findNeighbor(rects, "left", "right")).toBe("top-right");
  });

  it("finds left neighbor", () => {
    expect(findNeighbor(rects, "top-right", "left")).toBe("left");
  });

  it("finds down neighbor", () => {
    expect(findNeighbor(rects, "top-right", "down")).toBe("bot-right");
  });

  it("finds up neighbor", () => {
    expect(findNeighbor(rects, "bot-right", "up")).toBe("top-right");
  });

  it("returns null at boundary", () => {
    expect(findNeighbor(rects, "left", "left")).toBeNull();
  });

  it("returns null for missing pane id", () => {
    expect(findNeighbor(rects, "nonexistent", "right")).toBeNull();
  });
});

// --- swapLeaves ---

describe("swapLeaves", () => {
  it("swaps two leaves in a split", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    const result = swapLeaves(root, "a", "b") as PaneSplit;
    expect(result.children[0]).toEqual(leaf("b"));
    expect(result.children[1]).toEqual(leaf("a"));
  });

  it("is no-op when swapping with self", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    const result = swapLeaves(root, "a", "a");
    expect(result).toBe(root);
  });

  it("is no-op when one id is missing", () => {
    const root = hsplit("s1", [leaf("a"), leaf("b")]);
    const result = swapLeaves(root, "a", "missing");
    expect(result).toBe(root);
  });

  it("swaps across nested splits", () => {
    const root = hsplit("s1", [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")])]);
    const result = swapLeaves(root, "a", "c") as PaneSplit;
    expect((result.children[0] as PaneLeaf).id).toBe("c");
    const inner = result.children[1] as PaneSplit;
    expect((inner.children[1] as PaneLeaf).id).toBe("a");
  });
});

// --- Factory helpers ---

describe("makeLeaf", () => {
  it("creates a leaf with matching id and surfaceId", () => {
    const l = makeLeaf();
    expect(l.type).toBe("leaf");
    expect(l.id).toBe(l.surfaceId);
    expect(l.id).toBeTruthy();
  });

  it("creates unique ids", () => {
    const a = makeLeaf();
    const b = makeLeaf();
    expect(a.id).not.toBe(b.id);
  });
});

describe("makeSurface", () => {
  it("creates surface with defaults", () => {
    const s = makeSurface("s1");
    expect(s.id).toBe("s1");
    expect(s.ptyId).toBeNull();
    expect(s.title).toBe("");
    expect(s.hasUnreadNotification).toBe(false);
  });
});

describe("makeWorkspace", () => {
  it("creates workspace with default values", () => {
    const ws = makeWorkspace("Test");
    expect(ws.name).toBe("Test");
    expect(ws.root.type).toBe("leaf");
    expect(Object.keys(ws.surfaces)).toHaveLength(1);
    expect(ws.unreadCount).toBe(0);
  });

  it("accepts optional parameters", () => {
    const ws = makeWorkspace("WS", {
      workingDir: "/tmp",
      gitBranch: "main",
    });
    expect(ws.workingDir).toBe("/tmp");
    expect(ws.gitBranch).toBe("main");
  });
});

describe("generateWorkspaceName", () => {
  it("generates 'Workspace 1' for empty map", () => {
    expect(generateWorkspaceName({})).toBe("Workspace 1");
  });

  it("skips existing names", () => {
    const ws1 = makeWorkspace("Workspace 1");
    const ws2 = makeWorkspace("Workspace 2");
    const workspaces = { [ws1.id]: ws1, [ws2.id]: ws2 };
    expect(generateWorkspaceName(workspaces)).toBe("Workspace 3");
  });
});

describe("findWorkspaceIdByPane", () => {
  it("finds workspace containing the surface", () => {
    const ws = makeWorkspace("Test");
    const surfaceId = Object.keys(ws.surfaces)[0]!;
    const workspaces = { [ws.id]: ws };
    expect(findWorkspaceIdByPane(workspaces, surfaceId)).toBe(ws.id);
  });

  it("returns null for unknown pane", () => {
    const ws = makeWorkspace("Test");
    expect(findWorkspaceIdByPane({ [ws.id]: ws }, "unknown")).toBeNull();
  });
});

// --- Session snapshot ---

describe("snapshotPaneTree / rebuildPaneTree", () => {
  it("roundtrips a single leaf", () => {
    const snap = snapshotPaneTree(leaf("original"));
    expect(snap).toEqual({ type: "leaf" });

    const rebuilt = rebuildPaneTree(snap);
    expect(rebuilt.node.type).toBe("leaf");
    expect(Object.keys(rebuilt.surfaces)).toHaveLength(1);
  });

  it("roundtrips a nested tree preserving structure", () => {
    const root = hsplit(
      "s1",
      [leaf("a"), vsplit("s2", [leaf("b"), leaf("c")], [40, 60])],
      [50, 50],
    );

    const snap = snapshotPaneTree(root);
    expect(snap.type).toBe("horizontal");
    expect(snap.children).toHaveLength(2);
    expect(snap.children![1]!.type).toBe("vertical");
    expect(snap.sizes).toEqual([50, 50]);

    const rebuilt = rebuildPaneTree(snap);
    const newRoot = rebuilt.node as PaneSplit;
    expect(newRoot.type).toBe("horizontal");
    expect(newRoot.children).toHaveLength(2);
    expect(Object.keys(rebuilt.surfaces)).toHaveLength(3);
  });

  it("generates fresh ids on rebuild (no id leakage)", () => {
    const snap = snapshotPaneTree(leaf("original-id"));
    const rebuilt = rebuildPaneTree(snap);
    expect((rebuilt.node as PaneLeaf).id).not.toBe("original-id");
  });
});

// --- Constants ---

describe("MAX_SPLIT_DEPTH", () => {
  it("is 5", () => {
    expect(MAX_SPLIT_DEPTH).toBe(5);
  });
});
