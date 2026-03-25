import { useConfigStore } from "../stores/config";
import { useWorkspaceStore } from "../stores/workspace";
import {
  sendCustomNotification,
  sendDesktopNotification,
  logError,
} from "./pty-bridge";

interface WorkspaceNotificationOptions {
  workspaceId: string;
  title: string;
  body: string;
  paneId?: string;
}

export function dispatchWorkspaceNotification({
  workspaceId,
  title,
  body,
  paneId,
}: WorkspaceNotificationOptions): void {
  const workspaceState = useWorkspaceStore.getState();
  const workspace = workspaceState.workspaces[workspaceId];
  if (!workspace) return;

  const config = useConfigStore.getState().config;
  const playSound = config?.notifications.sound ?? true;
  const notificationCommand = config?.general.notification_command.trim() ?? "";

  workspaceState.addNotification(workspaceId, title, body);

  if (
    paneId &&
    (workspaceId !== workspaceState.activeWorkspaceId ||
      workspace.focusedPaneId !== paneId)
  ) {
    workspaceState.setSurfaceUnread(paneId, true);
  }

  if (config?.notifications.desktop ?? true) {
    sendDesktopNotification(title, body, playSound).catch(logError);
  }

  if (notificationCommand) {
    sendCustomNotification(notificationCommand, title, body).catch(logError);
  }
}
