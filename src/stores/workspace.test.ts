import { beforeEach, describe, expect, it } from "vitest";
import { closeWorkspaceEnsuringOneRemains, useWorkspaceStore } from "./workspace";
import { useMetadataStore } from "./metadata";
import { collectLeafIds, makeWorkspace } from "./pane-tree";
import type { SessionSnapshot } from "./pane-tree";

function resetStores(): void {
  const workspaceState = useWorkspaceStore.getState();
  const metadataState = useMetadataStore.getState();
  const initialWorkspace = makeWorkspace("Workspace 1");

  useWorkspaceStore.setState(
    {
      ...workspaceState,
      workspaces: { [initialWorkspace.id]: initialWorkspace },
      activeWorkspaceId: initialWorkspace.id,
      workspaceOrder: [initialWorkspace.id],
      notifications: [],
      showNotificationPanel: false,
    },
    true,
  );

  useMetadataStore.setState(
    {
      ...metadataState,
      metadata: {},
    },
    true,
  );
}

describe("workspace lifecycle state transitions", () => {
  beforeEach(() => {
    resetStores();
  });

  it("marks the fallback workspace read when closing the active workspace", () => {
    const store = useWorkspaceStore.getState();
    const initialId = store.activeWorkspaceId;
    const fallbackId = store.createWorkspace("Workspace 2", "/tmp");

    store.switchWorkspace(initialId);
    store.addNotification(fallbackId, "Prompt waiting", "Workspace 2 needs attention");

    const unreadBeforeClose = useWorkspaceStore.getState().workspaces[fallbackId];
    expect(unreadBeforeClose?.unreadCount).toBe(1);

    store.closeWorkspace(initialId);

    const state = useWorkspaceStore.getState();
    expect(state.activeWorkspaceId).toBe(fallbackId);
    expect(state.workspaces[fallbackId]?.unreadCount).toBe(0);
    expect(state.workspaces[fallbackId]?.lastNotificationText).toBe("");
    expect(state.notifications.every((notification) => notification.read)).toBe(true);
  });

  it("treats repeated closePane calls as a no-op after the first close", () => {
    const store = useWorkspaceStore.getState();
    const workspaceId = store.activeWorkspaceId;
    const originalPaneId = store.workspaces[workspaceId]!.focusedPaneId;

    store.splitPane(originalPaneId, "horizontal", "/tmp");

    const splitState = useWorkspaceStore.getState();
    const leafIds = collectLeafIds(splitState.workspaces[workspaceId]!.root);
    const closedPaneId = leafIds.find((id) => id !== originalPaneId)!;

    splitState.closePane(closedPaneId);

    const afterFirstClose = useWorkspaceStore.getState();
    const firstSnapshot = {
      root: JSON.stringify(afterFirstClose.workspaces[workspaceId]!.root),
      surfaces: Object.keys(afterFirstClose.workspaces[workspaceId]!.surfaces),
      focusedPaneId: afterFirstClose.workspaces[workspaceId]!.focusedPaneId,
    };

    afterFirstClose.closePane(closedPaneId);

    const afterSecondClose = useWorkspaceStore.getState();
    expect(JSON.stringify(afterSecondClose.workspaces[workspaceId]!.root)).toBe(
      firstSnapshot.root,
    );
    expect(Object.keys(afterSecondClose.workspaces[workspaceId]!.surfaces)).toEqual(
      firstSnapshot.surfaces,
    );
    expect(afterSecondClose.workspaces[workspaceId]!.focusedPaneId).toBe(
      firstSnapshot.focusedPaneId,
    );
  });

  it("clears pane unread state when moveFocus lands on that pane", () => {
    const store = useWorkspaceStore.getState();
    const workspaceId = store.activeWorkspaceId;
    const originalPaneId = store.workspaces[workspaceId]!.focusedPaneId;

    store.splitPane(originalPaneId, "horizontal", "/tmp");

    const splitState = useWorkspaceStore.getState();
    const [leftPaneId, rightPaneId] = collectLeafIds(
      splitState.workspaces[workspaceId]!.root,
    );
    splitState.setFocusedPane(leftPaneId!);
    splitState.setSurfaceUnread(rightPaneId!, true);

    splitState.moveFocus("right");

    const finalState = useWorkspaceStore.getState();
    expect(finalState.workspaces[workspaceId]!.focusedPaneId).toBe(rightPaneId);
    expect(
      finalState.workspaces[workspaceId]!.surfaces[rightPaneId!]?.hasUnreadNotification,
    ).toBe(false);
  });

  it("skips invalid persisted pane trees and restores focused leaf index", () => {
    const snapshots: SessionSnapshot[] = [
      {
        name: "Broken",
        workingDir: "/tmp",
        gitBranch: "",
        worktreeDir: "",
        worktreeName: "",
        paneTree: {
          type: "horizontal",
          children: [{ type: "leaf" }],
          sizes: [100],
        },
        focusedLeafIndex: 0,
      },
      {
        name: "Valid",
        workingDir: "/tmp",
        gitBranch: "main",
        worktreeDir: "",
        worktreeName: "",
        paneTree: {
          type: "horizontal",
          children: [{ type: "leaf" }, { type: "leaf" }],
          sizes: [40, 60],
        },
        focusedLeafIndex: 1,
      },
    ];

    useWorkspaceStore.getState().restoreSession(snapshots, 0);

    const state = useWorkspaceStore.getState();
    expect(state.workspaceOrder).toHaveLength(1);
    const workspace = state.workspaces[state.workspaceOrder[0]!]!;
    const leafIds = collectLeafIds(workspace.root);
    expect(workspace.name).toBe("Valid");
    expect(workspace.focusedPaneId).toBe(leafIds[1]);
  });

  it("uses a safe fallback working dir when closing the last removed worktree workspace", () => {
    const store = useWorkspaceStore.getState();
    const workspaceId = store.activeWorkspaceId;

    useWorkspaceStore.setState((state) => ({
      workspaces: {
        ...state.workspaces,
        [workspaceId]: {
          ...state.workspaces[workspaceId]!,
          workingDir: "/tmp/deleted-worktree",
          worktreeDir: "/tmp/deleted-worktree",
          worktreeName: "feature/remove-me",
        },
      },
    }));

    closeWorkspaceEnsuringOneRemains(workspaceId, "/tmp");

    const finalState = useWorkspaceStore.getState();
    expect(finalState.workspaceOrder).toHaveLength(1);
    expect(finalState.activeWorkspaceId).not.toBe(workspaceId);
    expect(finalState.workspaces[finalState.activeWorkspaceId]?.workingDir).toBe(
      "/tmp",
    );
  });
});
