import { describe, it, expect, vi, beforeEach } from "vitest";
import { buildSessionPayload } from "./session-persistence";
import { getSessionData } from "../stores/workspace";

// Mock the workspace store
vi.mock("../stores/workspace", () => ({
  getSessionData: vi.fn(),
}));

describe("buildSessionPayload", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("should build a valid session payload from store data", () => {
    const mockSessionData = {
      activeIndex: 1,
      workspaces: [
        {
          name: "Workspace 1",
          workingDir: "/home/user/project1",
          gitBranch: "main",
          worktreeDir: "",
          worktreeName: "",
          paneTree: { type: "leaf", id: "pane-1", surfaceId: "surface-1" },
          focusedLeafIndex: 0,
        },
        {
          name: "Workspace 2",
          workingDir: "/home/user/project2",
          gitBranch: "feature-branch",
          worktreeDir: "/home/user/project2-worktree",
          worktreeName: "worktree-1",
          paneTree: {
            type: "horizontal",
            id: "split-1",
            sizes: [50, 50],
            children: [
              { type: "leaf", id: "pane-2", surfaceId: "surface-2" },
              { type: "leaf", id: "pane-3", surfaceId: "surface-3" },
            ],
          },
          focusedLeafIndex: 1,
        },
      ],
    };

    vi.mocked(getSessionData).mockReturnValue(mockSessionData as ReturnType<typeof getSessionData>);

    const payload = buildSessionPayload();

    expect(payload).toEqual({
      version: 1,
      active_workspace_index: 1,
      workspaces: [
        {
          name: "Workspace 1",
          working_dir: "/home/user/project1",
          git_branch: "main",
          worktree_dir: "",
          worktree_name: "",
          pane_tree: { type: "leaf", id: "pane-1", surfaceId: "surface-1" },
          focused_leaf_index: 0,
        },
        {
          name: "Workspace 2",
          working_dir: "/home/user/project2",
          git_branch: "feature-branch",
          worktree_dir: "/home/user/project2-worktree",
          worktree_name: "worktree-1",
          pane_tree: {
            type: "horizontal",
            id: "split-1",
            sizes: [50, 50],
            children: [
              { type: "leaf", id: "pane-2", surfaceId: "surface-2" },
              { type: "leaf", id: "pane-3", surfaceId: "surface-3" },
            ],
          },
          focused_leaf_index: 1,
        },
      ],
    });
  });

  it("should handle empty workspaces correctly", () => {
    const mockSessionData = {
      activeIndex: 0,
      workspaces: [],
    };

    vi.mocked(getSessionData).mockReturnValue(mockSessionData as ReturnType<typeof getSessionData>);

    const payload = buildSessionPayload();

    expect(payload).toEqual({
      version: 1,
      active_workspace_index: 0,
      workspaces: [],
    });
  });
});
