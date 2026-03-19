import { useEffect } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import PaneArea from "./components/PaneArea";
import Sidebar from "./components/Sidebar";
import NotificationPanel from "./components/NotificationPanel";
import { useWorkspaceStore } from "./stores/workspace";
import type { Direction } from "./stores/workspace";
import {
  worktreeCreate,
  worktreeRunHook,
  writePty,
  socketRespond,
  sendDesktopNotification,
} from "./lib/pty-bridge";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

export default function App() {
  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const closePane = useWorkspaceStore((s) => s.closePane);
  const moveFocus = useWorkspaceStore((s) => s.moveFocus);
  const createWorkspace = useWorkspaceStore((s) => s.createWorkspace);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const createWorktreeWorkspace = useWorkspaceStore(
    (s) => s.createWorktreeWorkspace,
  );
  const toggleNotificationPanel = useWorkspaceStore(
    (s) => s.toggleNotificationPanel,
  );
  const jumpToUnread = useWorkspaceStore((s) => s.jumpToUnread);
  const markWorkspaceRead = useWorkspaceStore((s) => s.markWorkspaceRead);
  const showNotificationPanel = useWorkspaceStore(
    (s) => s.showNotificationPanel,
  );
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);

  // Mark workspace as read when it becomes active
  useEffect(() => {
    markWorkspaceRead(activeWorkspaceId);
  }, [activeWorkspaceId, markWorkspaceRead]);

  // Listen for socket API bridge events
  useEffect(() => {
    const unlisten = listen<{
      id: string;
      method: string;
      params: Record<string, unknown>;
    }>("socket-request", (event) => {
      const { id, method, params } = event.payload;
      handleSocketRequest(id, method, params);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  function handleSocketRequest(
    id: string,
    method: string,
    params: Record<string, unknown>,
  ) {
    const state = useWorkspaceStore.getState();
    let result: unknown;

    try {
      switch (method) {
        case "workspace.list": {
          const list = state.workspaceOrder.map((wsId) => {
            const ws = state.workspaces[wsId];
            return ws
              ? {
                  id: ws.id,
                  name: ws.name,
                  gitBranch: ws.gitBranch,
                  workingDir: ws.workingDir,
                  surfaces: Object.keys(ws.surfaces).length,
                  active: wsId === state.activeWorkspaceId,
                }
              : null;
          });
          result = { result: list.filter(Boolean) };
          break;
        }
        case "workspace.create": {
          const name = params.name as string | undefined;
          const wsId = state.createWorkspace(name ?? undefined);
          result = { result: { id: wsId } };
          break;
        }
        case "workspace.select": {
          const name = params.name as string;
          const target = state.workspaceOrder.find(
            (wsId) => state.workspaces[wsId]?.name === name,
          );
          if (target) {
            state.switchWorkspace(target);
            result = { result: true };
          } else {
            result = { error: `Workspace "${name}" not found` };
          }
          break;
        }
        case "workspace.close": {
          const name = params.name as string;
          const target = state.workspaceOrder.find(
            (wsId) => state.workspaces[wsId]?.name === name,
          );
          if (target) {
            state.closeWorkspace(target);
            result = { result: true };
          } else {
            result = { error: `Workspace "${name}" not found` };
          }
          break;
        }
        case "surface.list": {
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) {
            result = {
              result: Object.values(ws.surfaces).map((s) => ({
                id: s.id,
                ptyId: s.ptyId,
                title: s.title,
              })),
            };
          } else {
            result = { result: [] };
          }
          break;
        }
        case "surface.split": {
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) {
            const dir =
              (params.direction as string) === "down"
                ? "vertical"
                : "horizontal";
            state.splitPane(ws.focusedPaneId, dir as "horizontal" | "vertical");
            result = { result: true };
          } else {
            result = { error: "No active workspace" };
          }
          break;
        }
        case "surface.close": {
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) {
            state.closePane(ws.focusedPaneId);
            result = { result: true };
          } else {
            result = { error: "No active workspace" };
          }
          break;
        }
        case "surface.send_text": {
          // Direct PTY write — find the surface's pty_id
          const surfaceId = params.surface_id as string | undefined;
          const text = params.text as string;
          if (surfaceId) {
            for (const ws of Object.values(state.workspaces)) {
              const surface = ws.surfaces[surfaceId];
              if (surface?.ptyId != null) {
                writePty(surface.ptyId, text).catch(console.error);
                result = { result: true };
                break;
              }
            }
            if (!result) result = { error: "Surface not found" };
          } else if (params.pty_id != null) {
            // Handled directly in Rust, shouldn't reach here
            result = { result: true };
          } else {
            result = { error: "Missing surface_id or pty_id" };
          }
          break;
        }
        case "notification.create": {
          const title = (params.title as string) || "ForkTTY";
          const body = (params.body as string) || "";
          state.addNotification(state.activeWorkspaceId, title, body);
          sendDesktopNotification(title, body).catch(console.error);
          result = { result: true };
          break;
        }
        case "notification.list": {
          result = {
            result: state.notifications.map((n) => ({
              id: n.id,
              workspaceName: n.workspaceName,
              title: n.title,
              body: n.body,
              timestamp: n.timestamp,
              read: n.read,
            })),
          };
          break;
        }
        case "notification.clear": {
          state.clearNotifications();
          result = { result: true };
          break;
        }
        default:
          result = { error: `Unknown method: ${method}` };
      }
    } catch (err) {
      result = { error: String(err) };
    }

    socketRespond(id, result).catch(console.error);
  }

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // NOTE: These shortcuts override terminal keys (Ctrl+D = EOF, Ctrl+W = delete word,
      // Ctrl+N = next history). This matches SPEC.md. Users can still exit shells via `exit`.

      // Ctrl+Shift+W: close workspace
      if (e.ctrlKey && e.shiftKey && e.key === "W") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        if (state.workspaceOrder.length <= 1) return;
        const ws = state.workspaces[state.activeWorkspaceId];
        if (!ws) return;
        const paneCount = Object.keys(ws.surfaces).length;
        if (paneCount > 1) {
          if (
            !window.confirm(
              `Close workspace "${ws.name}" with ${paneCount} panes?`,
            )
          ) {
            return;
          }
        }
        closeWorkspace(state.activeWorkspaceId);
        return;
      }

      // Ctrl+Shift+N: new worktree workspace
      if (e.ctrlKey && e.shiftKey && e.key === "N") {
        e.preventDefault();
        const name = window.prompt("Worktree name (becomes branch name):");
        if (!name || !name.trim()) return;
        const trimmed = name.trim();
        worktreeCreate(trimmed)
          .then((info) => {
            createWorktreeWorkspace(
              trimmed,
              info.path,
              info.branch,
              info.path,
              info.name,
            );
            worktreeRunHook(info.path, "setup").catch(console.error);
          })
          .catch((err) => {
            console.error("Failed to create worktree:", err);
          });
        return;
      }

      // Ctrl+Shift+I: toggle notification panel
      if (e.ctrlKey && e.shiftKey && e.key === "I") {
        e.preventDefault();
        toggleNotificationPanel();
        return;
      }

      // Ctrl+Shift+U: jump to latest unread workspace
      if (e.ctrlKey && e.shiftKey && e.key === "U") {
        e.preventDefault();
        jumpToUnread();
        return;
      }

      // Ctrl+Shift+D: split down (vertical)
      if (e.ctrlKey && e.shiftKey && e.key === "D") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) splitPane(ws.focusedPaneId, "vertical");
        return;
      }

      // Ctrl+N: new workspace
      if (e.ctrlKey && !e.shiftKey && e.key === "n") {
        e.preventDefault();
        createWorkspace();
        return;
      }

      // Ctrl+D: split right (horizontal)
      if (e.ctrlKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) splitPane(ws.focusedPaneId, "horizontal");
        return;
      }

      // Ctrl+W: close pane
      if (e.ctrlKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) closePane(ws.focusedPaneId);
        return;
      }

      // Ctrl+1..9: jump to workspace by position
      if (e.ctrlKey && !e.shiftKey && !e.altKey) {
        const digit = parseInt(e.key, 10);
        if (digit >= 1 && digit <= 9) {
          e.preventDefault();
          const state = useWorkspaceStore.getState();
          const targetId = state.workspaceOrder[digit - 1];
          if (targetId) {
            switchWorkspace(targetId);
          }
          return;
        }
      }

      // Alt+Arrow: navigate panes
      if (e.altKey && e.key.startsWith("Arrow")) {
        e.preventDefault();
        const dirMap: Record<string, Direction> = {
          ArrowLeft: "left",
          ArrowRight: "right",
          ArrowUp: "up",
          ArrowDown: "down",
        };
        const direction = dirMap[e.key];
        if (direction) {
          moveFocus(direction);
        }
        return;
      }
    }

    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [
    splitPane,
    closePane,
    moveFocus,
    createWorkspace,
    closeWorkspace,
    switchWorkspace,
    createWorktreeWorkspace,
    toggleNotificationPanel,
    jumpToUnread,
  ]);

  return (
    <div className="app">
      <Group orientation="horizontal">
        <Panel id="sidebar" defaultSize={15} minSize={8} maxSize={30}>
          <Sidebar />
        </Panel>
        <Separator className="resize-handle sidebar-separator" />
        <Panel id="main" defaultSize={85}>
          <div className="workspace-container">
            {workspaceOrder.map((id) => (
              <div
                key={id}
                className="workspace-pane-area"
                style={{
                  display: id === activeWorkspaceId ? "flex" : "none",
                }}
              >
                <PaneArea workspaceId={id} />
              </div>
            ))}
          </div>
        </Panel>
      </Group>
      {showNotificationPanel && <NotificationPanel />}
    </div>
  );
}
