import { useEffect, useState, useCallback, useMemo } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import PaneArea from "./components/PaneArea";
import Sidebar from "./components/Sidebar";
import NotificationPanel from "./components/NotificationPanel";
import SettingsPanel from "./components/SettingsPanel";
import CommandPalette from "./components/CommandPalette";
import ErrorToast, { showToast } from "./components/ErrorToast";
import { useWorkspaceStore, getSessionData } from "./stores/workspace";
import { useConfigStore } from "./stores/config";
import type { Direction } from "./stores/workspace";
import type { CommandEntry } from "./components/CommandPalette";
import {
  worktreeCreate,
  worktreeRunHook,
  writePty,
  socketRespond,
  sendDesktopNotification,
  saveSession,
  loadSession,
  writeLog,
} from "./lib/pty-bridge";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

function getWorkspacePtyId(workspaceId: string): number | null {
  const workspace = useWorkspaceStore.getState().workspaces[workspaceId];
  if (!workspace) {
    return null;
  }

  return (
    Object.values(workspace.surfaces).find((surface) => surface.ptyId != null)
      ?.ptyId ?? null
  );
}

function waitForWorkspacePty(
  workspaceId: string,
  timeoutMs = 5000,
): Promise<number> {
  const existingPtyId = getWorkspacePtyId(workspaceId);
  if (existingPtyId != null) {
    return Promise.resolve(existingPtyId);
  }

  return new Promise((resolve, reject) => {
    let unsubscribe = () => {};
    const timeoutId = window.setTimeout(() => {
      unsubscribe();
      reject(new Error("Timed out waiting for workspace PTY"));
    }, timeoutMs);

    unsubscribe = useWorkspaceStore.subscribe((state) => {
      const workspace = state.workspaces[workspaceId];
      if (!workspace) {
        return;
      }

      const ptyId =
        Object.values(workspace.surfaces).find(
          (surface) => surface.ptyId != null,
        )?.ptyId ?? null;

      if (ptyId != null) {
        window.clearTimeout(timeoutId);
        unsubscribe();
        resolve(ptyId);
      }
    });
  });
}

export default function App() {
  const [showCommandPalette, setShowCommandPalette] = useState(false);

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
  const loadConfig = useConfigStore((s) => s.loadConfig);
  const toggleSettings = useConfigStore((s) => s.toggleSettings);
  const showSettings = useConfigStore((s) => s.showSettings);
  const sidebarPosition = useConfigStore(
    (s) => s.config?.appearance.sidebar_position ?? "left",
  );
  const worktreeLayout = useConfigStore(
    (s) => s.config?.general.worktree_layout ?? "nested",
  );

  const restoreSession = useWorkspaceStore((s) => s.restoreSession);

  // Load config + theme on startup
  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  // Restore session on startup
  useEffect(() => {
    loadSession()
      .then((data) => {
        if (data && data.workspaces.length > 0) {
          restoreSession(
            data.workspaces.map((ws) => ({
              name: ws.name,
              workingDir: ws.working_dir,
              gitBranch: ws.git_branch,
              worktreeDir: ws.worktree_dir,
              worktreeName: ws.worktree_name,
              paneTree: ws.pane_tree,
            })),
            data.active_workspace_index,
          );
          writeLog(
            "INFO",
            `Restored session with ${data.workspaces.length} workspaces`,
          ).catch(console.error);
        }
      })
      .catch((err) => {
        console.error("Failed to restore session:", err);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-save session every 30 seconds
  useEffect(() => {
    const interval = setInterval(() => {
      const { workspaces, activeIndex } = getSessionData();
      saveSession({
        workspaces: workspaces.map((ws) => ({
          name: ws.name,
          working_dir: ws.workingDir,
          git_branch: ws.gitBranch,
          worktree_dir: ws.worktreeDir,
          worktree_name: ws.worktreeName,
          pane_tree: ws.paneTree,
        })),
        active_workspace_index: activeIndex,
      }).catch(console.error);
    }, 30000);
    return () => clearInterval(interval);
  }, []);

  // Save session on window close
  useEffect(() => {
    function handleBeforeUnload() {
      const { workspaces, activeIndex } = getSessionData();
      // Use sync-ish approach: navigator.sendBeacon isn't available, but
      // invoke is async. Best effort save.
      saveSession({
        workspaces: workspaces.map((ws) => ({
          name: ws.name,
          working_dir: ws.workingDir,
          git_branch: ws.gitBranch,
          worktree_dir: ws.worktreeDir,
          worktree_name: ws.worktreeName,
          pane_tree: ws.paneTree,
        })),
        active_workspace_index: activeIndex,
      }).catch(console.error);
    }
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, []);

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
      void handleSocketRequest(id, method, params);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  async function handleSocketRequest(
    id: string,
    method: string,
    params: Record<string, unknown>,
  ): Promise<void> {
    const state = useWorkspaceStore.getState();
    const config = useConfigStore.getState().config;
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
          const prompt =
            typeof params.prompt === "string" && params.prompt.length > 0
              ? params.prompt
              : undefined;
          const worktreeDir =
            typeof params.worktreeDir === "string" ? params.worktreeDir : "";
          const worktreeName =
            typeof params.worktreeName === "string" ? params.worktreeName : "";
          const workingDir =
            typeof params.workingDir === "string"
              ? params.workingDir
              : worktreeDir;
          const gitBranch =
            typeof params.gitBranch === "string" ? params.gitBranch : "";
          const isWorktree = worktreeDir.length > 0;

          const wsId = isWorktree
            ? state.createWorktreeWorkspace(
                name ?? worktreeName,
                workingDir,
                gitBranch,
                worktreeDir,
                worktreeName || name || "",
              )
            : state.createWorkspace(name ?? undefined);

          const response: Record<string, unknown> = { id: wsId };
          if (prompt) {
            const ptyId = await waitForWorkspacePty(wsId);
            response.pty_id = ptyId;

            if (!isWorktree) {
              await writePty(ptyId, prompt);
            }
          }

          result = { result: response };
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
          if (config?.notifications.desktop ?? true) {
            sendDesktopNotification(title, body).catch(console.error);
          }
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
        worktreeCreate(trimmed, worktreeLayout)
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
            showToast(`Failed to create worktree: ${err}`, "error");
          });
        return;
      }

      // Ctrl+Shift+P: command palette
      if (e.ctrlKey && e.shiftKey && e.key === "P") {
        e.preventDefault();
        setShowCommandPalette((v) => !v);
        return;
      }

      // Ctrl+,: toggle settings panel
      if (e.ctrlKey && !e.shiftKey && e.key === ",") {
        e.preventDefault();
        toggleSettings();
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
    toggleSettings,
    worktreeLayout,
  ]);

  const closeCommandPalette = useCallback(
    () => setShowCommandPalette(false),
    [],
  );

  const commands: CommandEntry[] = useMemo(
    () => [
      {
        id: "new-workspace",
        label: "New Workspace",
        shortcut: "Ctrl+N",
        action: () => createWorkspace(),
      },
      {
        id: "close-workspace",
        label: "Close Workspace",
        shortcut: "Ctrl+Shift+W",
        action: () => {
          const state = useWorkspaceStore.getState();
          if (state.workspaceOrder.length > 1) {
            closeWorkspace(state.activeWorkspaceId);
          }
        },
      },
      {
        id: "split-right",
        label: "Split Right",
        shortcut: "Ctrl+D",
        action: () => {
          const state = useWorkspaceStore.getState();
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) splitPane(ws.focusedPaneId, "horizontal");
        },
      },
      {
        id: "split-down",
        label: "Split Down",
        shortcut: "Ctrl+Shift+D",
        action: () => {
          const state = useWorkspaceStore.getState();
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) splitPane(ws.focusedPaneId, "vertical");
        },
      },
      {
        id: "close-pane",
        label: "Close Pane",
        shortcut: "Ctrl+W",
        action: () => {
          const state = useWorkspaceStore.getState();
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) closePane(ws.focusedPaneId);
        },
      },
      {
        id: "notifications",
        label: "Toggle Notifications",
        shortcut: "Ctrl+Shift+I",
        action: toggleNotificationPanel,
      },
      {
        id: "jump-unread",
        label: "Jump to Unread",
        shortcut: "Ctrl+Shift+U",
        action: jumpToUnread,
      },
      {
        id: "settings",
        label: "Open Settings",
        shortcut: "Ctrl+,",
        action: toggleSettings,
      },
      {
        id: "nav-left",
        label: "Navigate Left",
        shortcut: "Alt+Left",
        action: () => moveFocus("left"),
      },
      {
        id: "nav-right",
        label: "Navigate Right",
        shortcut: "Alt+Right",
        action: () => moveFocus("right"),
      },
      {
        id: "nav-up",
        label: "Navigate Up",
        shortcut: "Alt+Up",
        action: () => moveFocus("up"),
      },
      {
        id: "nav-down",
        label: "Navigate Down",
        shortcut: "Alt+Down",
        action: () => moveFocus("down"),
      },
    ],
    [
      createWorkspace,
      closeWorkspace,
      splitPane,
      closePane,
      toggleNotificationPanel,
      jumpToUnread,
      toggleSettings,
      moveFocus,
    ],
  );

  const sidebarPanel = (
    <Panel id="sidebar" defaultSize={15} minSize={8} maxSize={30}>
      <Sidebar />
    </Panel>
  );

  const mainPanel = (
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
  );

  return (
    <div className="app">
      <Group orientation="horizontal">
        {sidebarPosition === "right" ? (
          <>
            {mainPanel}
            <Separator className="resize-handle sidebar-separator" />
            {sidebarPanel}
          </>
        ) : (
          <>
            {sidebarPanel}
            <Separator className="resize-handle sidebar-separator" />
            {mainPanel}
          </>
        )}
      </Group>
      {showNotificationPanel && <NotificationPanel />}
      {showSettings && <SettingsPanel />}
      {showCommandPalette && (
        <CommandPalette commands={commands} onClose={closeCommandPalette} />
      )}
      <ErrorToast />
    </div>
  );
}
