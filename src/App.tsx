import {
  useEffect,
  useRef,
  useState,
  useCallback,
  useMemo,
  lazy,
  Suspense,
  Component,
} from "react";
import type { ReactNode, ErrorInfo } from "react";
import {
  Group,
  Panel,
  Separator,
  type PanelImperativeHandle,
} from "react-resizable-panels";
import PaneArea from "./components/PaneArea";
import Sidebar from "./components/Sidebar";
import NotificationPanel from "./components/NotificationPanel";
const SettingsPanel = lazy(() => import("./components/SettingsPanel"));
const CommandPalette = lazy(() => import("./components/CommandPalette"));
const BranchPicker = lazy(() => import("./components/BranchPicker"));
import type { BranchPickerResult } from "./components/BranchPicker";
import ErrorToast, { showToast } from "./components/ErrorToast";
import { ConfirmModal, PromptModal } from "./components/InlineModal";
import ShortcutBar from "./components/ShortcutBar";
const WelcomeScreen = lazy(() => import("./components/WelcomeScreen"));
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
  hasTauriRuntime,
} from "./lib/pty-bridge";
import { handleSocketRequest } from "./lib/socket-handler";
import {
  createWorkspaceWithInheritedCwd,
  splitPaneWithInheritedCwd,
} from "./lib/workspace-launch";
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

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.classList.contains("xterm-helper-textarea")) return false;
  const tagName = target.tagName;
  return (
    target.isContentEditable ||
    tagName === "INPUT" ||
    tagName === "TEXTAREA" ||
    tagName === "SELECT"
  );
}

const SIDEBAR_COLLAPSE_STORAGE_KEY = "forktty.sidebar-collapsed";
const SIDEBAR_COLLAPSE_WIDTH_PX = 56;
const SIDEBAR_EXPANDED_DEFAULT_PX = 280;
const SIDEBAR_COLLAPSE_THRESHOLD_PX = 160;

export default function App() {
  const [showCommandPalette, setShowCommandPalette] = useState(false);
  const [showBranchPicker, setShowBranchPicker] = useState(false);
  const [pendingCloseWs, setPendingCloseWs] = useState<{
    id: string;
    name: string;
    paneCount: number;
  } | null>(null);
  const [pendingRename, setPendingRename] = useState<{
    id: string;
    currentName: string;
  } | null>(null);
  const [showWelcome, setShowWelcome] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => {
    if (typeof window === "undefined") return false;
    return (
      window.localStorage.getItem(SIDEBAR_COLLAPSE_STORAGE_KEY) === "1"
    );
  });
  const sidebarPanelRef = useRef<PanelImperativeHandle | null>(null);

  const closePane = useWorkspaceStore((s) => s.closePane);
  const moveFocus = useWorkspaceStore((s) => s.moveFocus);
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
  const renameWorkspace = useWorkspaceStore((s) => s.renameWorkspace);

  const setSidebarCollapsedPersisted = useCallback((collapsed: boolean) => {
    setSidebarCollapsed(collapsed);
    if (typeof window !== "undefined") {
      window.localStorage.setItem(
        SIDEBAR_COLLAPSE_STORAGE_KEY,
        collapsed ? "1" : "0",
      );
    }
  }, []);

  const toggleSidebarCollapsed = useCallback(() => {
    setSidebarCollapsedPersisted(!sidebarCollapsed);
  }, [setSidebarCollapsedPersisted, sidebarCollapsed]);

  const handleCreateWorkspace = useCallback(() => {
    createWorkspaceWithInheritedCwd().catch(logError);
  }, []);

  const handleSplitFocusedPane = useCallback(
    (direction: "horizontal" | "vertical") => {
      const state = useWorkspaceStore.getState();
      const ws = state.workspaces[state.activeWorkspaceId];
      if (!ws) return;
      splitPaneWithInheritedCwd(ws.focusedPaneId, direction).catch(logError);
    },
    [],
  );

  const requestCloseWorkspace = useCallback(
    (workspaceId: string) => {
      const state = useWorkspaceStore.getState();
      if (state.workspaceOrder.length <= 1) return;

      const ws = state.workspaces[workspaceId];
      if (!ws) return;

      const paneCount = Object.keys(ws.surfaces).length;
      if (paneCount > 1) {
        setPendingCloseWs({
          id: workspaceId,
          name: ws.name,
          paneCount,
        });
        return;
      }

      closeWorkspace(workspaceId);
    },
    [closeWorkspace],
  );

  const dispatchFocusedPaneAction = useCallback((action: "copy" | "find") => {
    const state = useWorkspaceStore.getState();
    const ws = state.workspaces[state.activeWorkspaceId];
    if (!ws) return;

    window.dispatchEvent(
      new CustomEvent<{
        action: "copy" | "find";
        paneId: string;
      }>("forktty-terminal-action", {
        detail: {
          action,
          paneId: ws.focusedPaneId,
        },
      }),
    );
  }, []);

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

  // Disable WebKitGTK default context menu (Back/Forward/Stop/Reload)
  useEffect(() => {
    function preventContextMenu(e: MouseEvent) {
      e.preventDefault();
    }
    document.addEventListener("contextmenu", preventContextMenu);
    return () =>
      document.removeEventListener("contextmenu", preventContextMenu);
  }, []);

  // Load config + theme on startup
  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  useEffect(() => {
    const panel = sidebarPanelRef.current;
    if (!panel) return;
    if (sidebarCollapsed) {
      panel.collapse();
    } else {
      panel.expand();
    }
  }, [sidebarCollapsed]);

  // Restore session on startup; show welcome if no session exists
  useEffect(() => {
    if (!hasTauriRuntime()) {
      setShowWelcome(true);
      return;
    }

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
        } else {
          setShowWelcome(true);
        }
      })
      .catch((err) => {
        logError(err);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Save session on any workspace state change (debounced 2s)
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const unsub = useWorkspaceStore.subscribe(() => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        saveSession(buildSessionPayload()).catch(logError);
      }, 2000);
    });
    return () => {
      unsub();
      if (timer) clearTimeout(timer);
    };
  }, []);

  // Listen for branch picker open event from Sidebar
  useEffect(() => {
    function handleOpenBranchPicker() {
      setShowBranchPicker(true);
    }
    function handleOpenCommandPalette() {
      setShowCommandPalette(true);
    }
    window.addEventListener(
      "forktty-open-branch-picker",
      handleOpenBranchPicker,
    );
    window.addEventListener(
      "forktty-open-command-palette",
      handleOpenCommandPalette,
    );
    return () => {
      window.removeEventListener(
        "forktty-open-branch-picker",
        handleOpenBranchPicker,
      );
      window.removeEventListener(
        "forktty-open-command-palette",
        handleOpenCommandPalette,
      );
    };
  }, []);

  // Window title badge: show unread count
  const totalUnread = useWorkspaceStore((s) =>
    Object.values(s.workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0),
  );
  useEffect(() => {
    document.title = totalUnread > 0 ? `ForkTTY (${totalUnread})` : "ForkTTY";
    if (!hasTauriRuntime()) return;
    updateTrayTooltip(totalUnread).catch(logError);
  }, [totalUnread]);

  // Listen for socket API bridge events.
  // Empty deps is intentional: handleSocketRequest reads all state via
  // useWorkspaceStore.getState() / useConfigStore.getState() at call time,
  // so the closure never goes stale. Do NOT add handleSocketRequest to deps
  // without wrapping it in useCallback first.
  useEffect(() => {
    if (!hasTauriRuntime()) {
      return;
    }

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
      const editableTarget = isEditableTarget(e.target);
      const modalOrBranchPickerOpen =
        document.querySelector(".modal-overlay, .branch-picker-overlay") !==
        null;
      const commandPaletteOpen =
        document.querySelector(".command-palette-overlay") !== null;
      const blockingOverlayOpen = modalOrBranchPickerOpen || commandPaletteOpen;

      // NOTE: These shortcuts override terminal keys (Ctrl+D = EOF, Ctrl+W = delete word,
      // Ctrl+N = next history). This matches SPEC.md. Users can still exit shells via `exit`.

      // Ctrl+Shift+P: command palette
      if (e.ctrlKey && e.shiftKey && e.key === "P") {
        if (modalOrBranchPickerOpen) {
          return;
        }
        if (!commandPaletteOpen && editableTarget) {
          return;
        }
        e.preventDefault();
        setShowCommandPalette((v) => !v);
        return;
      }

      if (blockingOverlayOpen || editableTarget) return;

      // Ctrl+Shift+W: close workspace
      if (e.ctrlKey && e.shiftKey && e.key === "W") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        requestCloseWorkspace(state.activeWorkspaceId);
        return;
      }

      // Ctrl+Shift+N: new worktree workspace (open branch picker)
      if (e.ctrlKey && e.shiftKey && e.key === "N") {
        e.preventDefault();
        setShowBranchPicker(true);
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
        handleSplitFocusedPane("vertical");
        return;
      }

      // Ctrl+N: new workspace
      if (e.ctrlKey && !e.shiftKey && e.key === "n") {
        e.preventDefault();
        handleCreateWorkspace();
        return;
      }

      // Ctrl+D: split right (horizontal)
      if (e.ctrlKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        handleSplitFocusedPane("horizontal");
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

      // Ctrl+=: zoom in terminal font
      if (e.ctrlKey && !e.shiftKey && (e.key === "=" || e.key === "+")) {
        e.preventDefault();
        useConfigStore.getState().zoomIn();
        return;
      }

      // Ctrl+-: zoom out terminal font
      if (e.ctrlKey && !e.shiftKey && e.key === "-") {
        e.preventDefault();
        useConfigStore.getState().zoomOut();
        return;
      }

      // Ctrl+0: reset terminal font zoom
      if (e.ctrlKey && !e.shiftKey && e.key === "0") {
        e.preventDefault();
        useConfigStore.getState().zoomReset();
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
    closePane,
    handleCreateWorkspace,
    handleSplitFocusedPane,
    moveFocus,
    requestCloseWorkspace,
    switchWorkspace,
    toggleNotificationPanel,
    jumpToUnread,
    toggleSettings,
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
        action: handleCreateWorkspace,
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
          setPendingRename({
            id: state.activeWorkspaceId,
            currentName: ws.name,
          });
        },
      },
      {
        id: "close-workspace",
        label: "Close Workspace",
        shortcut: "Ctrl+Shift+W",
        action: () => {
          const state = useWorkspaceStore.getState();
          if (state.workspaceOrder.length > 1) {
            requestCloseWorkspace(state.activeWorkspaceId);
          }
        },
      },
      {
        id: "split-right",
        label: "Split Right",
        shortcut: "Ctrl+D",
        action: () => handleSplitFocusedPane("horizontal"),
      },
      {
        id: "split-down",
        label: "Split Down",
        shortcut: "Ctrl+Shift+D",
        action: () => handleSplitFocusedPane("vertical"),
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
        action: () => dispatchFocusedPaneAction("find"),
      },
      {
        id: "copy-selection",
        label: "Copy Selection",
        shortcut: "Ctrl+Shift+C",
        action: () => dispatchFocusedPaneAction("copy"),
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
        id: "zoom-in",
        label: "Zoom In",
        shortcut: "Ctrl+=",
        action: () => useConfigStore.getState().zoomIn(),
      },
      {
        id: "zoom-out",
        label: "Zoom Out",
        shortcut: "Ctrl+-",
        action: () => useConfigStore.getState().zoomOut(),
      },
      {
        id: "zoom-reset",
        label: "Reset Zoom",
        shortcut: "Ctrl+0",
        action: () => useConfigStore.getState().zoomReset(),
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
        id: "toggle-sidebar",
        label: sidebarCollapsed ? "Expand Sidebar" : "Collapse Sidebar",
        action: toggleSidebarCollapsed,
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
      handleCreateWorkspace,
      handleSplitFocusedPane,
      requestCloseWorkspace,
      closePane,
      dispatchFocusedPaneAction,
      toggleNotificationPanel,
      jumpToUnread,
      markWorkspaceRead,
      activeWorkspaceId,
      toggleSettings,
      sidebarCollapsed,
      switchWorkspace,
      toggleSidebarCollapsed,
      moveFocus,
    ],
  );

  const sidebarPanel = (
    <Panel
      id="sidebar"
      panelRef={sidebarPanelRef}
      defaultSize={
        sidebarCollapsed
          ? `${SIDEBAR_COLLAPSE_WIDTH_PX}px`
          : `${SIDEBAR_EXPANDED_DEFAULT_PX}px`
      }
      minSize={`${SIDEBAR_COLLAPSE_WIDTH_PX}px`}
      maxSize="420px"
      collapsedSize={`${SIDEBAR_COLLAPSE_WIDTH_PX}px`}
      collapsible
      groupResizeBehavior="preserve-pixel-size"
      onResize={(size) => {
        const collapsed = size.inPixels <= SIDEBAR_COLLAPSE_THRESHOLD_PX;
        if (collapsed !== sidebarCollapsed) {
          setSidebarCollapsedPersisted(collapsed);
        }
      }}
    >
      <Sidebar
        collapsed={sidebarCollapsed}
        onToggleCollapsed={toggleSidebarCollapsed}
      />
    </Panel>
  );

  const mainPanel = (
    <Panel id="main">
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
      <div className="app-main">
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
      </div>
      <ShortcutBar />
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
          {showWelcome && (
            <WelcomeScreen onDismiss={() => setShowWelcome(false)} />
          )}
        </Suspense>
      </LazyErrorBoundary>
      {pendingCloseWs && (
        <ConfirmModal
          title="Close Workspace"
          message={`Close "${pendingCloseWs.name}" with ${pendingCloseWs.paneCount} panes? All terminals will be killed.`}
          confirmLabel="Close"
          danger
          onConfirm={() => {
            closeWorkspace(pendingCloseWs.id);
            setPendingCloseWs(null);
          }}
          onCancel={() => setPendingCloseWs(null)}
        />
      )}
      {pendingRename && (
        <PromptModal
          title="Rename Workspace"
          defaultValue={pendingRename.currentName}
          placeholder="Workspace name"
          confirmLabel="Rename"
          onConfirm={(name) => {
            renameWorkspace(pendingRename.id, name);
            setPendingRename(null);
          }}
          onCancel={() => setPendingRename(null)}
        />
      )}
      <ErrorToast />
    </div>
  );
}
