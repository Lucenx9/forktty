import { describe, it, expect, vi, beforeEach } from "vitest";
import { dispatchWorkspaceNotification } from "./notification-dispatch";
import { useConfigStore } from "../stores/config";
import { useWorkspaceStore } from "../stores/workspace";
import {
  sendCustomNotification,
  sendDesktopNotification,
  logError,
} from "./pty-bridge";

vi.mock("../stores/config", () => ({
  useConfigStore: {
    getState: vi.fn(),
  },
}));

vi.mock("../stores/workspace", () => ({
  useWorkspaceStore: {
    getState: vi.fn(),
  },
}));

vi.mock("./pty-bridge", () => ({
  sendCustomNotification: vi.fn().mockResolvedValue(undefined),
  sendDesktopNotification: vi.fn().mockResolvedValue(undefined),
  logError: vi.fn(),
}));

import type { Mock } from "vitest";

describe("dispatchWorkspaceNotification", () => {
  let mockWorkspaceState: {
    workspaces: Record<string, { focusedPaneId: string }>;
    activeWorkspaceId: string;
    addNotification: Mock;
    setSurfaceUnread: Mock;
  };
  let mockConfigState: {
    config: {
      notifications: { sound: boolean; desktop: boolean };
      general: { notification_command: string };
    } | null;
  };

  beforeEach(() => {
    vi.clearAllMocks();

    mockWorkspaceState = {
      workspaces: {
        workspace1: {
          focusedPaneId: "pane1",
        },
      },
      activeWorkspaceId: "workspace1",
      addNotification: vi.fn(),
      setSurfaceUnread: vi.fn(),
    };

    mockConfigState = {
      config: {
        notifications: {
          sound: true,
          desktop: true,
        },
        general: {
          notification_command: "",
        },
      },
    };

    (useWorkspaceStore.getState as unknown as Mock).mockReturnValue(mockWorkspaceState);
    (useConfigStore.getState as unknown as Mock).mockReturnValue(mockConfigState);
  });

  it("should do nothing if workspace is not found", () => {
    dispatchWorkspaceNotification({
      workspaceId: "unknown",
      title: "Title",
      body: "Body",
    });

    expect(mockWorkspaceState.addNotification).not.toHaveBeenCalled();
    expect(sendDesktopNotification).not.toHaveBeenCalled();
  });

  it("should add notification and send desktop notification by default", () => {
    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    expect(mockWorkspaceState.addNotification).toHaveBeenCalledWith(
      "workspace1",
      "Test Title",
      "Test Body",
    );
    expect(sendDesktopNotification).toHaveBeenCalledWith(
      "Test Title",
      "Test Body",
      true,
    );
    expect(sendCustomNotification).not.toHaveBeenCalled();
    expect(mockWorkspaceState.setSurfaceUnread).not.toHaveBeenCalled();
  });

  it("should mark surface as unread if paneId provided but workspace is not active", () => {
    mockWorkspaceState.activeWorkspaceId = "workspace2";

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
      paneId: "pane1",
    });

    expect(mockWorkspaceState.setSurfaceUnread).toHaveBeenCalledWith("pane1", true);
  });

  it("should mark surface as unread if paneId provided but pane is not focused", () => {
    mockWorkspaceState.workspaces.workspace1.focusedPaneId = "pane2";

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
      paneId: "pane1",
    });

    expect(mockWorkspaceState.setSurfaceUnread).toHaveBeenCalledWith("pane1", true);
  });

  it("should not mark surface as unread if pane is focused in active workspace", () => {
    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
      paneId: "pane1",
    });

    expect(mockWorkspaceState.setSurfaceUnread).not.toHaveBeenCalled();
  });

  it("should send custom notification if command is configured", () => {
    mockConfigState.config.general.notification_command = "echo 'test'";

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    expect(sendCustomNotification).toHaveBeenCalledWith(
      "echo 'test'",
      "Test Title",
      "Test Body",
    );
  });

  it("should not send desktop notification if disabled in config", () => {
    mockConfigState.config.notifications.desktop = false;

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    expect(sendDesktopNotification).not.toHaveBeenCalled();
  });

  it("should respect playSound config", () => {
    mockConfigState.config.notifications.sound = false;

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    expect(sendDesktopNotification).toHaveBeenCalledWith(
      "Test Title",
      "Test Body",
      false,
    );
  });

  it("should handle missing config gracefully", () => {
    mockConfigState.config = null;

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    expect(sendDesktopNotification).toHaveBeenCalledWith(
      "Test Title",
      "Test Body",
      true,
    );
    expect(sendCustomNotification).not.toHaveBeenCalled();
  });

  it("should log errors from notification functions", async () => {
    const error = new Error("Test error");
    (sendDesktopNotification as Mock).mockRejectedValueOnce(error);

    dispatchWorkspaceNotification({
      workspaceId: "workspace1",
      title: "Test Title",
      body: "Test Body",
    });

    // Wait for promises to resolve
    await new Promise((resolve) => setTimeout(resolve, 0));

    expect(logError).toHaveBeenCalledWith(error);
  });
});
