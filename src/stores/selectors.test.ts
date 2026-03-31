import { beforeEach, describe, expect, it } from "vitest";
import { useWorkspaceStore } from "./workspace";
import { useMetadataStore } from "./metadata";
import { makeWorkspace } from "./pane-tree";
import {
  selectActiveWorkspaceId,
  selectWorkspaceCount,
  selectTotalUnread,
  selectTotalPaneCount,
  selectNotifications,
} from "./selectors";

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

describe("shared selectors", () => {
  beforeEach(() => {
    resetStores();
  });

  it("selectActiveWorkspaceId returns the active workspace id", () => {
    const state = useWorkspaceStore.getState();
    expect(selectActiveWorkspaceId(state)).toBe(state.activeWorkspaceId);
  });

  it("selectWorkspaceCount returns the number of workspaces", () => {
    const state = useWorkspaceStore.getState();
    expect(selectWorkspaceCount(state)).toBe(1);

    // Add a second workspace
    state.createWorkspace("Workspace 2");
    const updated = useWorkspaceStore.getState();
    expect(selectWorkspaceCount(updated)).toBe(2);
  });

  it("selectTotalUnread returns 0 when no unread notifications", () => {
    const state = useWorkspaceStore.getState();
    expect(selectTotalUnread(state)).toBe(0);
  });

  it("selectTotalUnread sums unread counts across all workspaces", () => {
    const state = useWorkspaceStore.getState();
    const wsId = state.activeWorkspaceId;

    // Manually set unread count
    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [wsId]: { ...s.workspaces[wsId]!, unreadCount: 5 },
      },
    }));

    const updated = useWorkspaceStore.getState();
    expect(selectTotalUnread(updated)).toBe(5);
  });

  it("selectTotalPaneCount returns correct count across workspaces", () => {
    const state = useWorkspaceStore.getState();
    // A fresh workspace has 1 pane
    expect(selectTotalPaneCount(state)).toBe(1);

    // Create second workspace (also 1 pane)
    state.createWorkspace("Workspace 2");
    const updated = useWorkspaceStore.getState();
    expect(selectTotalPaneCount(updated)).toBe(2);
  });

  it("selectNotifications returns the notifications array", () => {
    const state = useWorkspaceStore.getState();
    expect(selectNotifications(state)).toEqual([]);

    // Add a notification
    state.addNotification(state.activeWorkspaceId, "Test alert", "Something happened");
    const updated = useWorkspaceStore.getState();
    expect(selectNotifications(updated).length).toBe(1);
    expect(selectNotifications(updated)[0]!.title).toBe("Test alert");
  });

  it("selectTotalUnread handles multiple workspaces", () => {
    const state = useWorkspaceStore.getState();
    const ws1Id = state.activeWorkspaceId;
    const ws2Id = state.createWorkspace("Workspace 2");

    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [ws1Id]: { ...s.workspaces[ws1Id]!, unreadCount: 3 },
        [ws2Id]: { ...s.workspaces[ws2Id]!, unreadCount: 7 },
      },
    }));

    const updated = useWorkspaceStore.getState();
    expect(selectTotalUnread(updated)).toBe(10);
  });
});
