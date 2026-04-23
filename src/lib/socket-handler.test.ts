// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useMetadataStore } from "../stores/metadata";
import { makeWorkspace } from "../stores/pane-tree";
import { useWorkspaceStore } from "../stores/workspace";
import { handleSocketRequest } from "./socket-handler";
import { socketRespond, writePty } from "./pty-bridge";

vi.mock("./pty-bridge", () => ({
  writePty: vi.fn().mockRejectedValue(new Error("write failed")),
  socketRespond: vi.fn().mockResolvedValue(undefined),
  worktreeCreate: vi.fn(),
  worktreeMerge: vi.fn(),
  worktreeRemove: vi.fn(),
  worktreeRunHook: vi.fn().mockResolvedValue(null),
  logError: vi.fn(),
  hasTauriRuntime: vi.fn().mockReturnValue(false),
  killPty: vi.fn().mockResolvedValue(undefined),
  getCwd: vi.fn().mockResolvedValue("/tmp"),
  getGitBranch: vi.fn().mockResolvedValue("main"),
  getPtyCwd: vi.fn(),
}));

vi.mock("./notification-dispatch", () => ({
  dispatchWorkspaceNotification: vi.fn(),
}));

vi.mock("./terminal-registry", () => ({
  readScreen: vi.fn().mockReturnValue(""),
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

describe("handleSocketRequest", () => {
  beforeEach(() => {
    resetStores();
    vi.clearAllMocks();
  });

  it("rolls back a created workspace when prompt delivery fails", async () => {
    const initialWorkspaceId = useWorkspaceStore.getState().activeWorkspaceId;

    const request = handleSocketRequest("r1", "workspace.create", {
      name: "feature/socket",
      workingDir: "/tmp/forktty-worktree",
      gitBranch: "feature/socket",
      worktreeDir: "/tmp/forktty-worktree",
      worktreeName: "feature-socket",
      prompt: "run tests\n",
    });

    const createdWorkspaceId = useWorkspaceStore
      .getState()
      .workspaceOrder.find((id) => id !== initialWorkspaceId);
    expect(createdWorkspaceId).toBeTruthy();

    const createdWorkspace =
      useWorkspaceStore.getState().workspaces[createdWorkspaceId!];
    expect(createdWorkspace).toBeTruthy();
    useWorkspaceStore.getState().registerSurface(createdWorkspace!.focusedPaneId, 42);

    await request;

    expect(writePty).toHaveBeenCalledWith(42, "run tests\n");
    expect(
      useWorkspaceStore.getState().workspaces[createdWorkspaceId!],
    ).toBeUndefined();
    expect(socketRespond).toHaveBeenCalledWith("r1", {
      error: "Error: write failed",
    });
  });
});
