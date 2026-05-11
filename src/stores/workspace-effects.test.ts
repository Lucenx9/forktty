// @vitest-environment jsdom
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest";
import { useWorkspaceStore } from "./workspace";
import { useMetadataStore } from "./metadata";
import { collectLeafIds, makeWorkspace } from "./pane-tree";
import { startWorkspaceEffects } from "./workspace-effects";

// Mock the pty-bridge module
vi.mock("../lib/pty-bridge", () => ({
  saveSession: vi.fn().mockResolvedValue(undefined),
  updateTrayTooltip: vi.fn().mockResolvedValue(undefined),
  hasTauriRuntime: vi.fn().mockReturnValue(false),
  logError: vi.fn(),
}));

vi.mock("../lib/session-persistence", () => ({
  buildSessionPayload: vi.fn().mockReturnValue({
    version: 1,
    workspaces: [],
    active_workspace_index: 0,
  }),
}));

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

describe("workspace-effects", () => {
  let cleanup: (() => void) | null = null;

  beforeEach(() => {
    vi.useFakeTimers();
    resetStores();
    vi.clearAllMocks();
  });

  afterEach(() => {
    if (cleanup) {
      cleanup();
      cleanup = null;
    }
    vi.useRealTimers();
  });

  it("sets document title on startup", () => {
    cleanup = startWorkspaceEffects();
    expect(document.title).toBe("ForkTTY");
  });

  it("debounces session save on state changes", async () => {
    const { saveSession } = await import("../lib/pty-bridge");
    cleanup = startWorkspaceEffects();

    // Trigger state change
    const state = useWorkspaceStore.getState();
    state.createWorkspace("Workspace 2");

    // Not saved yet (debounced)
    expect(saveSession).not.toHaveBeenCalled();

    // Advance past debounce threshold
    vi.advanceTimersByTime(2000);

    expect(saveSession).toHaveBeenCalledOnce();
  });

  it("coalesces multiple rapid changes into a single save", async () => {
    const { saveSession } = await import("../lib/pty-bridge");
    cleanup = startWorkspaceEffects();

    const state = useWorkspaceStore.getState();
    state.createWorkspace("Workspace 2");

    vi.advanceTimersByTime(500);

    state.createWorkspace("Workspace 3");

    vi.advanceTimersByTime(500);

    state.createWorkspace("Workspace 4");

    // Still not saved (debounce resets)
    expect(saveSession).not.toHaveBeenCalled();

    // Advance past debounce from last change
    vi.advanceTimersByTime(2000);

    expect(saveSession).toHaveBeenCalledOnce();
  });

  it("saves after split, resize, pane close, and workspace close changes", async () => {
    const { saveSession } = await import("../lib/pty-bridge");
    cleanup = startWorkspaceEffects();

    const initialState = useWorkspaceStore.getState();
    const workspaceId = initialState.activeWorkspaceId;
    const paneId = initialState.workspaces[workspaceId]!.focusedPaneId;

    initialState.splitPane(paneId, "horizontal", "/tmp");
    vi.advanceTimersByTime(2000);
    expect(saveSession).toHaveBeenCalledTimes(1);

    const splitState = useWorkspaceStore.getState();
    const root = splitState.workspaces[workspaceId]!.root;
    expect(root.type).toBe("horizontal");
    if (root.type === "horizontal") {
      splitState.updatePaneSizes(root.id, [35, 65]);
    }
    vi.advanceTimersByTime(2000);
    expect(saveSession).toHaveBeenCalledTimes(2);

    const resizedState = useWorkspaceStore.getState();
    const leafToClose = collectLeafIds(resizedState.workspaces[workspaceId]!.root)[1]!;
    resizedState.closePane(leafToClose);
    vi.advanceTimersByTime(2000);
    expect(saveSession).toHaveBeenCalledTimes(3);

    const closeState = useWorkspaceStore.getState();
    const workspaceToClose = closeState.createWorkspace("Workspace 2");
    vi.advanceTimersByTime(2000);
    expect(saveSession).toHaveBeenCalledTimes(4);

    useWorkspaceStore.getState().closeWorkspace(workspaceToClose);
    vi.advanceTimersByTime(2000);
    expect(saveSession).toHaveBeenCalledTimes(5);
  });

  it("updates document title when unread count changes", () => {
    cleanup = startWorkspaceEffects();

    const state = useWorkspaceStore.getState();
    const wsId = state.activeWorkspaceId;

    // Set unread count
    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [wsId]: { ...s.workspaces[wsId]!, unreadCount: 3 },
      },
    }));

    expect(document.title).toBe("ForkTTY (3)");
  });

  it("resets document title when unread count goes to zero", () => {
    cleanup = startWorkspaceEffects();

    const state = useWorkspaceStore.getState();
    const wsId = state.activeWorkspaceId;

    // Set then clear unread count
    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [wsId]: { ...s.workspaces[wsId]!, unreadCount: 5 },
      },
    }));
    expect(document.title).toBe("ForkTTY (5)");

    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [wsId]: { ...s.workspaces[wsId]!, unreadCount: 0 },
      },
    }));
    expect(document.title).toBe("ForkTTY");
  });

  it("flushes pending session save and cleans up subscriptions on cleanup", async () => {
    const { saveSession } = await import("../lib/pty-bridge");
    cleanup = startWorkspaceEffects();

    // Trigger a change, then immediately clean up
    const state = useWorkspaceStore.getState();
    state.createWorkspace("Workspace 2");

    expect(saveSession).not.toHaveBeenCalled();

    cleanup();
    cleanup = null;

    expect(saveSession).toHaveBeenCalledOnce();
    expect(saveSession).toHaveBeenCalledWith({
      version: 1,
      workspaces: [],
      active_workspace_index: 0,
    });

    // Advance past debounce — the flushed timer should not fire again
    vi.advanceTimersByTime(3000);
    expect(saveSession).toHaveBeenCalledOnce();

    // Further state changes should not update title
    document.title = "ForkTTY";
    useWorkspaceStore.setState((s) => ({
      workspaces: {
        ...s.workspaces,
        [s.activeWorkspaceId]: {
          ...s.workspaces[s.activeWorkspaceId]!,
          unreadCount: 99,
        },
      },
    }));
    expect(document.title).toBe("ForkTTY"); // unchanged
  });

  it("does not save on cleanup when no session save is pending", async () => {
    const { saveSession } = await import("../lib/pty-bridge");
    cleanup = startWorkspaceEffects();

    cleanup();
    cleanup = null;

    expect(saveSession).not.toHaveBeenCalled();

    vi.advanceTimersByTime(3000);
    expect(saveSession).not.toHaveBeenCalled();
  });
});
