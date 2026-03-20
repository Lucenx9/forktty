import {
  useEffect,
  useState,
  useCallback,
  useMemo,
  lazy,
  Suspense,
  Component,
} from "react";
import type { ReactNode, ErrorInfo } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import PaneArea from "./components/PaneArea";
import Sidebar from "./components/Sidebar";
import NotificationPanel from "./components/NotificationPanel";
const SettingsPanel = lazy(() => import("./components/SettingsPanel"));
const CommandPalette = lazy(() => import("./components/CommandPalette"));
const BranchPicker = lazy(() => import("./components/BranchPicker"));
import type { BranchPickerResult } from "./components/BranchPicker";
import ErrorToast, { showToast } from "./components/ErrorToast";
import { useWorkspaceStore } from "./stores/workspace";
import { useConfigStore } from "./stores/config";
import type { Direction } from "./stores/workspace";
import type { CommandEntry } from "./components/CommandPalette";
import {
  worktreeCreate,
  worktreeAttach,
  worktreeRunHook,
  saveSession,
  loadSession,
  writeLog,
  logError,
  updateTrayTooltip,
} from "./lib/pty-bridge";
import { handleSocketRequest } from "./lib/socket-handler";
import { buildSessionPayload } from "./lib/session-persistence";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

/** Error boundary for lazy-loaded panels. Catches chunk load failures gracefully. */
class LazyErrorBoundary extends Component<
  { children: ReactNode },
  { hasError: boolean }
> {
  state = { hasError: false };
  static getDerivedStateFromError(): { hasError: boolean } {
    return { hasError: true };
  }
  componentDidCatch(error: Error, info: ErrorInfo) {
    logError(
      `LazyErrorBoundary caught: ${error.message} ${info.componentStack}`,
    );
  }
  render() {
    if (this.state.hasError) return null;
    return this.props.children;
  }
}

export default function App() {
  const [showCommandPalette, setShowCommandPalette] = useState(false);
  const [showBranchPicker, setShowBranchPicker] = useState(false);

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

  const handleBranchPickerResult = useCallback(
    (result: BranchPickerResult) => {
      setShowBranchPicker(false);
      if (result.kind === "cancel") return;

      const createFn =
        result.kind === "new-branch"
          ? worktreeCreate(result.name, worktreeLayout)
          : worktreeAttach(result.branchName, worktreeLayout);

      createFn
        .then((info) => {
          createWorktreeWorkspace(
            info.name,
            info.path,
            info.branch,
            info.path,
            info.name,
          );
          worktreeRunHook(info.path, "setup").catch(logError);
        })
        .catch((err) => {
          showToast(`Failed to create worktree: ${err}`, "error");
        });
    },
    [worktreeLayout, createWorktreeWorkspace],
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
          ).catch(logError);
        }
      })
      .catch((err) => {
        logError(err);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-save session every 30 seconds
  useEffect(() => {
    const interval = setInterval(() => {
      saveSession(buildSessionPayload()).catch(logError);
    }, 30000);
    return () => clearInterval(interval);
  }, []);

  // Save session on window close (best effort — invoke is async)
  useEffect(() => {
    function handleBeforeUnload() {
      saveSession(buildSessionPayload()).catch(logError);
    }
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, []);

  // Listen for branch picker open event from Sidebar
  useEffect(() => {
    function handleOpenBranchPicker() {
      setShowBranchPicker(true);
    }
    window.addEventListener(
      "forktty-open-branch-picker",
      handleOpenBranchPicker,
    );
    return () =>
      window.removeEventListener(
        "forktty-open-branch-picker",
        handleOpenBranchPicker,
      );
  }, []);

  // Window title badge: show unread count
  const totalUnread = useWorkspaceStore((s) =>
    Object.values(s.workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0),
  );
  useEffect(() => {
    document.title = totalUnread > 0 ? `ForkTTY (${totalUnread})` : "ForkTTY";
    updateTrayTooltip(totalUnread).catch(logError);
  }, [totalUnread]);

  // Listen for socket API bridge events.
  // Empty deps is intentional: handleSocketRequest reads all state via
  // useWorkspaceStore.getState() / useConfigStore.getState() at call time,
  // so the closure never goes stale. Do NOT add handleSocketRequest to deps
  // without wrapping it in useCallback first.
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

      // Ctrl+Shift+N: new worktree workspace (open branch picker)
      if (e.ctrlKey && e.shiftKey && e.key === "N") {
        e.preventDefault();
        setShowBranchPicker(true);
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

  const renameWorkspace = useWorkspaceStore((s) => s.renameWorkspace);

  const commands: CommandEntry[] = useMemo(
    () => [
      {
        id: "new-workspace",
        label: "New Workspace",
        shortcut: "Ctrl+N",
        action: () => createWorkspace(),
      },
      {
        id: "new-worktree",
        label: "New Worktree Workspace",
        shortcut: "Ctrl+Shift+N",
        action: () => setShowBranchPicker(true),
      },
      {
        id: "rename-workspace",
        label: "Rename Workspace...",
        action: () => {
          const state = useWorkspaceStore.getState();
          const ws = state.workspaces[state.activeWorkspaceId];
          if (!ws) return;
          const name = window.prompt("New workspace name:", ws.name);
          if (name && name.trim()) {
            renameWorkspace(state.activeWorkspaceId, name.trim());
          }
        },
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
        id: "find-in-terminal",
        label: "Find in Terminal",
        shortcut: "Ctrl+F",
        action: () => {
          // Dispatch Ctrl+F to the focused terminal pane
          window.dispatchEvent(
            new KeyboardEvent("keydown", {
              key: "f",
              ctrlKey: true,
              bubbles: true,
            }),
          );
        },
      },
      {
        id: "copy-selection",
        label: "Copy Selection",
        shortcut: "Ctrl+Shift+C",
        action: () => {
          window.dispatchEvent(
            new KeyboardEvent("keydown", {
              key: "C",
              ctrlKey: true,
              shiftKey: true,
              bubbles: true,
            }),
          );
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
        id: "mark-read",
        label: "Mark Workspace as Read",
        action: () => markWorkspaceRead(activeWorkspaceId),
      },
      {
        id: "settings",
        label: "Open Settings",
        shortcut: "Ctrl+,",
        action: toggleSettings,
      },
      {
        id: "command-palette",
        label: "Command Palette",
        shortcut: "Ctrl+Shift+P",
        action: () => setShowCommandPalette(true),
      },
      {
        id: "next-workspace",
        label: "Next Workspace",
        action: () => {
          const state = useWorkspaceStore.getState();
          const idx = state.workspaceOrder.indexOf(state.activeWorkspaceId);
          const next =
            state.workspaceOrder[(idx + 1) % state.workspaceOrder.length];
          if (next) switchWorkspace(next);
        },
      },
      {
        id: "prev-workspace",
        label: "Previous Workspace",
        action: () => {
          const state = useWorkspaceStore.getState();
          const idx = state.workspaceOrder.indexOf(state.activeWorkspaceId);
          const prev =
            state.workspaceOrder[
              (idx - 1 + state.workspaceOrder.length) %
                state.workspaceOrder.length
            ];
          if (prev) switchWorkspace(prev);
        },
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
      createWorktreeWorkspace,
      closeWorkspace,
      renameWorkspace,
      splitPane,
      closePane,
      toggleNotificationPanel,
      jumpToUnread,
      markWorkspaceRead,
      activeWorkspaceId,
      toggleSettings,
      switchWorkspace,
      moveFocus,
      worktreeLayout,
    ],
  );

  const sidebarPanel = (
    <Panel id="sidebar" defaultSize="15" minSize="8" maxSize="30">
      <Sidebar />
    </Panel>
  );

  const mainPanel = (
    <Panel id="main" defaultSize="85">
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
      <LazyErrorBoundary>
        <Suspense fallback={null}>
          {showSettings && <SettingsPanel />}
          {showCommandPalette && (
            <CommandPalette commands={commands} onClose={closeCommandPalette} />
          )}
          {showBranchPicker && (
            <BranchPicker onResult={handleBranchPickerResult} />
          )}
        </Suspense>
      </LazyErrorBoundary>
      <ErrorToast />
    </div>
  );
}
