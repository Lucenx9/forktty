import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  useWorkspaceStore,
  getLastActivity,
  closeWorkspaceEnsuringOneRemains,
} from "../stores/workspace";
import { useConfigStore } from "../stores/config";
import { selectTotalUnread } from "../stores/selectors";
import type { Workspace } from "../stores/workspace";
import {
  getCwd,
  getGitBranch,
  worktreeMerge,
  worktreeRemove,
  worktreeStatus,
  logError,
} from "../lib/pty-bridge";
import {
  createWorkspaceWithInheritedCwd,
  splitPaneWithInheritedCwd,
} from "../lib/workspace-launch";
import { showToast } from "./ErrorToast";
import { ConfirmModal } from "./InlineModal";
import {
  Plus,
  GitBranch,
  Columns2,
  Rows2,
  Bell,
  Command,
  ChevronsLeft,
  ChevronsRight,
  X,
  GripVertical,
} from "lucide-react";
import { truncatePath } from "../lib/path-utils";

const ACTIVITY_THRESHOLD_MS = 3000;

function worktreeStatusWarning(status: string): string {
  if (status === "dirty") return " (has uncommitted changes!)";
  if (status === "conflicts") return " (has merge conflicts!)";
  return "";
}

function worktreeLabel(workspace: Workspace): string {
  return workspace.gitBranch || workspace.worktreeName;
}

// --- Context menu ---

interface ContextMenuState {
  x: number;
  y: number;
  workspaceId: string;
}

interface ContextMenuProps {
  menu: ContextMenuState;
  onClose: () => void;
}

function ContextMenu({ menu, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const markWorkspaceRead = useWorkspaceStore((s) => s.markWorkspaceRead);
  const reorderWorkspaces = useWorkspaceStore((s) => s.reorderWorkspaces);

  const [pendingClose, setPendingClose] = useState(false);
  const [pendingRemoveWorktree, setPendingRemoveWorktree] = useState(false);

  const ws = workspaces[menu.workspaceId];
  const wsIndex = workspaceOrder.indexOf(menu.workspaceId);
  const canClose = workspaceOrder.length > 1;
  const isWorktree = ws ? ws.worktreeDir !== "" : false;
  const isFirst = wsIndex === 0;
  const isLast = wsIndex === workspaceOrder.length - 1;
  const hasOthers = workspaceOrder.length > 1;

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", handleClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [onClose]);

  // Clamp menu position so it doesn't overflow the viewport (wait for paint)
  useEffect(() => {
    const rafId = requestAnimationFrame(() => {
      if (!menuRef.current) return;
      const rect = menuRef.current.getBoundingClientRect();
      const el = menuRef.current;
      if (rect.right > window.innerWidth) {
        el.style.left = `${window.innerWidth - rect.width - 4}px`;
      }
      if (rect.bottom > window.innerHeight) {
        el.style.top = `${window.innerHeight - rect.height - 4}px`;
      }
    });
    return () => cancelAnimationFrame(rafId);
  }, []);

  if (!ws) return null;

  function handleRename() {
    window.dispatchEvent(
      new CustomEvent("forktty-rename-workspace", {
        detail: menu.workspaceId,
      }),
    );
    onClose();
  }

  function handleClose() {
    const paneCount = Object.keys(ws!.surfaces).length;
    if (paneCount > 1) {
      setPendingClose(true);
      return;
    }
    closeWorkspace(menu.workspaceId);
    onClose();
  }

  function handleCloseOthers() {
    const others = workspaceOrder.filter((id) => id !== menu.workspaceId);
    for (const id of others) {
      closeWorkspace(id);
    }
    onClose();
  }

  function handleCloseBelow() {
    const below = workspaceOrder.slice(wsIndex + 1);
    for (const id of below) {
      closeWorkspace(id);
    }
    onClose();
  }

  function handleCloseAbove() {
    const above = workspaceOrder.slice(0, wsIndex);
    for (const id of above) {
      closeWorkspace(id);
    }
    onClose();
  }

  function handleMoveUp() {
    if (wsIndex > 0) reorderWorkspaces(wsIndex, wsIndex - 1);
    onClose();
  }

  function handleMoveDown() {
    if (wsIndex < workspaceOrder.length - 1) reorderWorkspaces(wsIndex, wsIndex + 1);
    onClose();
  }

  function handleMoveToTop() {
    if (wsIndex > 0) reorderWorkspaces(wsIndex, 0);
    onClose();
  }

  function handleSplitRight() {
    splitPaneWithInheritedCwd(ws!.focusedPaneId, "horizontal").catch(logError);
    onClose();
  }

  function handleSplitDown() {
    splitPaneWithInheritedCwd(ws!.focusedPaneId, "vertical").catch(logError);
    onClose();
  }

  function handleMarkRead() {
    markWorkspaceRead(menu.workspaceId);
    onClose();
  }

  function handleMerge() {
    worktreeMerge(ws!.worktreeName, ws!.workingDir || ws!.worktreeDir)
      .then((msg) => showToast(String(msg), "info"))
      .catch((err) => showToast(`Merge failed: ${err}`, "error"));
    onClose();
  }

  function handleRemoveWorktree() {
    setPendingRemoveWorktree(true);
  }

  return (
    <div ref={menuRef} className="context-menu" style={{ left: menu.x, top: menu.y }}>
      <button className="context-menu-item" onClick={handleRename}>
        <span>Rename Workspace...</span>
      </button>
      <button className="context-menu-item" onClick={handleSplitRight}>
        <span>Split Right</span>
        <span className="context-menu-shortcut">Ctrl+D</span>
      </button>
      <button className="context-menu-item" onClick={handleSplitDown}>
        <span>Split Down</span>
        <span className="context-menu-shortcut">Ctrl+Shift+D</span>
      </button>

      {hasOthers && (
        <>
          <div className="context-menu-separator" />
          <button
            className={`context-menu-item ${isFirst ? "context-menu-item-disabled" : ""}`}
            onClick={handleMoveUp}
            disabled={isFirst}
          >
            <span>Move Up</span>
          </button>
          <button
            className={`context-menu-item ${isLast ? "context-menu-item-disabled" : ""}`}
            onClick={handleMoveDown}
            disabled={isLast}
          >
            <span>Move Down</span>
          </button>
          <button
            className={`context-menu-item ${isFirst ? "context-menu-item-disabled" : ""}`}
            onClick={handleMoveToTop}
            disabled={isFirst}
          >
            <span>Move to Top</span>
          </button>
        </>
      )}

      {isWorktree && (
        <>
          <div className="context-menu-separator" />
          <button className="context-menu-item" onClick={handleMerge}>
            <span>Merge Branch</span>
          </button>
          <button
            className="context-menu-item context-menu-item-danger"
            onClick={handleRemoveWorktree}
          >
            <span>Remove Worktree</span>
          </button>
        </>
      )}

      {canClose && (
        <>
          <div className="context-menu-separator" />
          <button
            className="context-menu-item context-menu-item-danger"
            onClick={handleClose}
          >
            <span>Close Workspace</span>
            <span className="context-menu-shortcut">Ctrl+Shift+W</span>
          </button>
          <button
            className="context-menu-item context-menu-item-danger"
            onClick={handleCloseOthers}
          >
            <span>Close Other Workspaces</span>
          </button>
          <button
            className={`context-menu-item context-menu-item-danger ${isLast ? "context-menu-item-disabled" : ""}`}
            onClick={handleCloseBelow}
            disabled={isLast}
          >
            <span>Close Workspaces Below</span>
          </button>
          <button
            className={`context-menu-item context-menu-item-danger ${isFirst ? "context-menu-item-disabled" : ""}`}
            onClick={handleCloseAbove}
            disabled={isFirst}
          >
            <span>Close Workspaces Above</span>
          </button>
        </>
      )}

      <div className="context-menu-separator" />
      {ws.unreadCount > 0 ? (
        <button className="context-menu-item" onClick={handleMarkRead}>
          <span>Mark as Read</span>
        </button>
      ) : (
        <button className="context-menu-item context-menu-item-disabled" disabled>
          <span>Mark as Read</span>
        </button>
      )}
      {pendingClose &&
        createPortal(
          <ConfirmModal
            title="Close Workspace"
            message={`Close workspace "${ws.name}" with ${Object.keys(ws.surfaces).length} panes?`}
            confirmLabel="Close"
            danger={true}
            onConfirm={() => {
              setPendingClose(false);
              closeWorkspace(menu.workspaceId);
              onClose();
            }}
            onCancel={() => {
              setPendingClose(false);
              onClose();
            }}
          />,
          document.body,
        )}
      {pendingRemoveWorktree &&
        createPortal(
          <ConfirmModal
            title="Remove Worktree"
            message={`Remove worktree "${worktreeLabel(ws)}" and delete branch?${worktreeStatusWarning(ws.worktreeStatus)}`}
            confirmLabel="Remove"
            danger={true}
            onConfirm={() => {
              setPendingRemoveWorktree(false);
              worktreeRemove(ws.worktreeName, ws.workingDir || ws.worktreeDir)
                .then((fallbackWorkingDir) =>
                  closeWorkspaceEnsuringOneRemains(
                    menu.workspaceId,
                    fallbackWorkingDir,
                  ),
                )
                .catch((err) => showToast(`Remove failed: ${err}`, "error"));
              onClose();
            }}
            onCancel={() => {
              setPendingRemoveWorktree(false);
              onClose();
            }}
          />,
          document.body,
        )}
    </div>
  );
}

// --- Workspace entry ---

function WorkspaceEntry({
  workspace,
  isActive,
  now,
  index,
  onGripMouseDown,
  onEntryMouseEnter,
  onEntryMouseUp,
  isDragOver,
  isDragging,
  onContextMenu,
}: {
  workspace: Workspace;
  isActive: boolean;
  now: number;
  index: number;
  onGripMouseDown: (index: number) => void;
  onEntryMouseEnter: (index: number) => void;
  onEntryMouseUp: (index: number) => void;
  isDragOver: boolean;
  isDragging: boolean;
  onContextMenu: (e: React.MouseEvent, workspaceId: string) => void;
}) {
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const renameWorkspace = useWorkspaceStore((s) => s.renameWorkspace);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(workspace.name);
  const [pendingClose, setPendingClose] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const canClose = workspaceOrder.length > 1;
  const paneCount = Object.keys(workspace.surfaces).length;

  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing]);

  // Listen for rename requests from context menu
  useEffect(() => {
    function handleRenameEvent(e: Event) {
      const detail = (e as CustomEvent).detail;
      if (detail === workspace.id) {
        setEditValue(workspace.name);
        setEditing(true);
      }
    }
    window.addEventListener("forktty-rename-workspace", handleRenameEvent);
    return () =>
      window.removeEventListener("forktty-rename-workspace", handleRenameEvent);
  }, [workspace.id, workspace.name]);

  const hasActivity = Object.keys(workspace.surfaces).some((id) => {
    const activity = getLastActivity(id);
    return activity > 0 && now - activity < ACTIVITY_THRESHOLD_MS;
  });

  const statusColor = hasActivity ? "var(--theme-green)" : "var(--theme-bright-black)";
  const activityLabel = hasActivity ? "Recent terminal activity" : "Idle";

  function handleMouseDown(e: React.MouseEvent) {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest("button, input")) return;
    switchWorkspace(workspace.id);
  }

  function handleDoubleClick(e: React.MouseEvent) {
    e.stopPropagation();
    setEditValue(workspace.name);
    setEditing(true);
  }

  function commitRename() {
    const trimmed = editValue.trim();
    if (trimmed && trimmed !== workspace.name) {
      renameWorkspace(workspace.id, trimmed);
    }
    setEditing(false);
  }

  function handleClose(e: React.MouseEvent) {
    e.stopPropagation();
    const paneCount = Object.keys(workspace.surfaces).length;
    if (paneCount > 1) {
      setPendingClose(true);
      return;
    }
    closeWorkspace(workspace.id);
  }

  const isWorktree = workspace.worktreeDir !== "";
  const worktreeStatusClass =
    isWorktree && workspace.worktreeStatus
      ? `sidebar-entry-wt-${workspace.worktreeStatus}`
      : "";
  const entryClassName = [
    "sidebar-entry",
    isActive ? "sidebar-entry-active" : "",
    isDragging ? "sidebar-entry-dragging" : "",
    hasActivity ? "sidebar-entry-live" : "",
    workspace.unreadCount > 0 ? "sidebar-entry-unread" : "",
    isWorktree ? "sidebar-entry-worktree" : "",
    worktreeStatusClass,
  ]
    .filter(Boolean)
    .join(" ");
  const entryAriaLabel = [
    `${index + 1}. ${workspace.name}`,
    isActive ? "active workspace" : "",
    activityLabel,
    isWorktree && workspace.worktreeStatus
      ? `worktree ${workspace.worktreeStatus}`
      : "",
    workspace.unreadCount > 0 ? `${workspace.unreadCount} unread alerts` : "",
  ]
    .filter(Boolean)
    .join(", ");

  return (
    <>
      {isDragOver && <div className="sidebar-drop-indicator" />}
      <div
        className={entryClassName}
        role="button"
        tabIndex={0}
        aria-current={isActive ? "page" : undefined}
        aria-label={entryAriaLabel}
        onMouseEnter={() => onEntryMouseEnter(index)}
        onMouseUp={() => onEntryMouseUp(index)}
        onMouseDown={handleMouseDown}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            switchWorkspace(workspace.id);
          }
        }}
        onContextMenu={(e) => onContextMenu(e, workspace.id)}
      >
        <div className="sidebar-entry-header">
          <span
            className="sidebar-status-dot"
            style={{ backgroundColor: statusColor }}
            title={activityLabel}
          />
          <span className="sidebar-entry-index" aria-hidden="true">
            {index + 1}
          </span>
          {editing ? (
            <input
              ref={inputRef}
              className="sidebar-rename-input"
              value={editValue}
              onChange={(e) => setEditValue(e.target.value)}
              onBlur={commitRename}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitRename();
                if (e.key === "Escape") setEditing(false);
              }}
              onClick={(e) => e.stopPropagation()}
            />
          ) : (
            <span className="sidebar-entry-name">
              <span
                className="sidebar-entry-name-text"
                onDoubleClick={handleDoubleClick}
              >
                {workspace.name}
              </span>
            </span>
          )}
          {workspace.unreadCount > 0 && (
            <span className="sidebar-unread-badge">{workspace.unreadCount}</span>
          )}
          <button
            type="button"
            className="sidebar-drag-handle"
            onMouseDown={(e) => {
              e.stopPropagation();
              e.preventDefault();
              onGripMouseDown(index);
            }}
            onClick={(e) => e.preventDefault()}
            title="Reorder workspaces"
            aria-label={`Reorder ${workspace.name}`}
          >
            <GripVertical size={11} />
          </button>
          {canClose && (
            <button
              type="button"
              className="sidebar-close-btn"
              onClick={handleClose}
              title="Close workspace"
              aria-label={`Close ${workspace.name}`}
            >
              <X size={10} />
            </button>
          )}
        </div>
        {workspace.gitBranch && (
          <div className="sidebar-entry-meta">
            <span className="sidebar-branch">{workspace.gitBranch}</span>
            {isWorktree && workspace.worktreeStatus && (
              <span
                className={`sidebar-wt-status sidebar-wt-${workspace.worktreeStatus}`}
              >
                {workspace.worktreeStatus}
              </span>
            )}
            {paneCount > 1 && (
              <span className="sidebar-pane-count">
                {paneCount} {paneCount === 1 ? "pane" : "panes"}
              </span>
            )}
          </div>
        )}
        {workspace.workingDir && (
          <div className="sidebar-entry-meta">
            <span className="sidebar-cwd">
              {truncatePath(workspace.workingDir, 28)}
            </span>
            {!workspace.gitBranch && paneCount > 1 && (
              <span className="sidebar-pane-count">
                {paneCount} {paneCount === 1 ? "pane" : "panes"}
              </span>
            )}
          </div>
        )}
      </div>
      {pendingClose &&
        createPortal(
          <ConfirmModal
            title="Close Workspace"
            message={`Close workspace "${workspace.name}" with ${Object.keys(workspace.surfaces).length} panes?`}
            confirmLabel="Close"
            danger={true}
            onConfirm={() => {
              setPendingClose(false);
              closeWorkspace(workspace.id);
            }}
            onCancel={() => setPendingClose(false)}
          />,
          document.body,
        )}
    </>
  );
}

// --- Sidebar ---

interface SidebarProps {
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

export default function Sidebar({ collapsed, onToggleCollapsed }: SidebarProps) {
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const setWorkspaceGitBranch = useWorkspaceStore((s) => s.setWorkspaceGitBranch);
  const setWorkspaceWorkingDir = useWorkspaceStore((s) => s.setWorkspaceWorkingDir);
  const setWorktreeStatus = useWorkspaceStore((s) => s.setWorktreeStatus);
  const reorderWorkspaces = useWorkspaceStore((s) => s.reorderWorkspaces);
  const showNotificationPanel = useWorkspaceStore((s) => s.showNotificationPanel);
  const sidebarPosition = useConfigStore(
    (s) => s.config?.appearance.sidebar_position ?? "left",
  );
  // Context menu state
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);

  function handleContextMenu(e: React.MouseEvent, workspaceId: string) {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ x: e.clientX, y: e.clientY, workspaceId });
  }

  // Drag-and-drop state
  const [dragFromIndex, setDragFromIndex] = useState<number | null>(null);
  const [dragOverIndex, setDragOverIndex] = useState<number | null>(null);

  function handleGripMouseDown(index: number) {
    setDragFromIndex(index);
    setDragOverIndex(index);
    document.body.classList.add("sidebar-dragging");
  }

  function handleEntryMouseEnter(index: number) {
    if (dragFromIndex !== null && dragOverIndex !== index) {
      setDragOverIndex(index);
    }
  }

  function handleEntryMouseUp(toIndex: number) {
    if (dragFromIndex !== null && dragFromIndex !== toIndex) {
      reorderWorkspaces(dragFromIndex, toIndex);
    }
  }

  // Clean up drag state on any mouseup (drop or cancel)
  useEffect(() => {
    function handleGlobalMouseUp() {
      setDragFromIndex(null);
      setDragOverIndex(null);
      document.body.classList.remove("sidebar-dragging");
    }
    window.addEventListener("mouseup", handleGlobalMouseUp);
    return () => window.removeEventListener("mouseup", handleGlobalMouseUp);
  }, []);

  // 1-second tick for status dot re-evaluation
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, []);

  // Fetch CWD and git branch for new workspaces (those without workingDir)
  useEffect(() => {
    const { workspaces: current } = useWorkspaceStore.getState();
    for (const id of workspaceOrder) {
      const ws = current[id];
      if (!ws) continue;

      const cwdPromise = ws.workingDir
        ? Promise.resolve(ws.workingDir)
        : getCwd().then((cwd) => {
            setWorkspaceWorkingDir(id, cwd);
            return cwd;
          });

      if (!ws.workingDir || !ws.gitBranch) {
        cwdPromise
          .then((cwd) => getGitBranch(cwd))
          .then((branch) => {
            setWorkspaceGitBranch(id, branch);
          })
          .catch(logError);
      }
    }
  }, [workspaceOrder, setWorkspaceWorkingDir, setWorkspaceGitBranch]);

  // Refresh git branch when switching workspaces
  useEffect(() => {
    const ws = useWorkspaceStore.getState().workspaces[activeWorkspaceId];
    if (ws?.workingDir) {
      getGitBranch(ws.workingDir)
        .then((branch) => {
          setWorkspaceGitBranch(activeWorkspaceId, branch);
        })
        .catch(logError);
    }
  }, [activeWorkspaceId, setWorkspaceGitBranch]);

  // Poll worktree status every 5 seconds for worktree-backed workspaces
  useEffect(() => {
    function refreshStatus() {
      const { workspaces: current } = useWorkspaceStore.getState();
      for (const [id, ws] of Object.entries(current)) {
        if (ws.worktreeDir) {
          worktreeStatus(ws.worktreeDir)
            .then((status) => setWorktreeStatus(id, status))
            .catch(() => setWorktreeStatus(id, "error"));
        }
      }
    }
    refreshStatus();
    const interval = setInterval(refreshStatus, 5000);
    return () => clearInterval(interval);
  }, [setWorktreeStatus]);

  function handleNewWorktree() {
    window.dispatchEvent(new CustomEvent("forktty-open-branch-picker"));
  }
  const totalUnread = useWorkspaceStore(selectTotalUnread);
  const CollapseIcon =
    sidebarPosition === "right"
      ? collapsed
        ? ChevronsLeft
        : ChevronsRight
      : collapsed
        ? ChevronsRight
        : ChevronsLeft;

  function handleSplitRight() {
    const state = useWorkspaceStore.getState();
    const ws = state.workspaces[state.activeWorkspaceId];
    if (ws) {
      splitPaneWithInheritedCwd(ws.focusedPaneId, "horizontal").catch(logError);
    }
  }

  function handleSplitDown() {
    const state = useWorkspaceStore.getState();
    const ws = state.workspaces[state.activeWorkspaceId];
    if (ws) {
      splitPaneWithInheritedCwd(ws.focusedPaneId, "vertical").catch(logError);
    }
  }

  if (collapsed) {
    return (
      <div className="sidebar sidebar-collapsed">
        <div className="sidebar-rail-actions">
          <button
            className="sidebar-icon-btn sidebar-rail-toggle"
            type="button"
            onClick={onToggleCollapsed}
            title="Expand sidebar"
            aria-label="Expand sidebar"
          >
            <CollapseIcon size={14} />
          </button>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={() => createWorkspaceWithInheritedCwd().catch(logError)}
            title="New workspace (Ctrl+N)"
            aria-label="New workspace"
          >
            <Plus size={14} />
          </button>
          <button
            className="sidebar-icon-btn sidebar-icon-btn-worktree"
            type="button"
            onClick={handleNewWorktree}
            title="New worktree (Ctrl+Shift+N)"
            aria-label="New worktree workspace"
          >
            <GitBranch size={14} />
          </button>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("forktty-open-command-palette"))
            }
            title="Command palette (Ctrl+Shift+P)"
            aria-label="Open command palette"
          >
            <Command size={14} />
          </button>
          <button
            className={`sidebar-icon-btn ${totalUnread > 0 ? "sidebar-icon-btn-unread" : ""} ${showNotificationPanel ? "sidebar-icon-btn-active" : ""}`}
            type="button"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("forktty-toggle-notifications"))
            }
            title="Notifications (Ctrl+Shift+I)"
            aria-label="Toggle notifications"
            aria-pressed={showNotificationPanel}
          >
            <Bell size={14} />
            {totalUnread > 0 && (
              <span className="sidebar-icon-badge">{totalUnread}</span>
            )}
          </button>
        </div>
        <div className="sidebar-rail-list">
          {workspaceOrder.map((id, index) => {
            const ws = workspaces[id];
            if (!ws) return null;
            return (
              <button
                key={id}
                type="button"
                className={`sidebar-rail-entry ${id === activeWorkspaceId ? "sidebar-rail-entry-active" : ""}`}
                onClick={() => switchWorkspace(id)}
                title={`${index + 1}. ${ws.name}${ws.unreadCount > 0 ? ` • ${ws.unreadCount} unread` : ""}`}
                aria-label={`${index + 1}. ${ws.name}${id === activeWorkspaceId ? ", active workspace" : ""}${ws.unreadCount > 0 ? `, ${ws.unreadCount} unread alerts` : ""}`}
              >
                <span className="sidebar-rail-index">{index + 1}</span>
                {ws.unreadCount > 0 && (
                  <span className="sidebar-rail-badge">{ws.unreadCount}</span>
                )}
              </button>
            );
          })}
        </div>
      </div>
    );
  }

  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-titlebar">
          <div className="sidebar-header-title-row">
            <div className="sidebar-header-title">ForkTTY</div>
            <div className="sidebar-header-count">{workspaceOrder.length}</div>
          </div>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={onToggleCollapsed}
            title="Collapse sidebar"
            aria-label="Collapse sidebar"
          >
            <CollapseIcon size={14} />
          </button>
        </div>
        <div className="sidebar-header-icons sidebar-secondary-actions">
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={() => createWorkspaceWithInheritedCwd().catch(logError)}
            title="New workspace (Ctrl+N)"
            aria-label="New workspace"
          >
            <Plus size={14} />
          </button>
          <button
            className="sidebar-icon-btn sidebar-icon-btn-worktree"
            type="button"
            onClick={handleNewWorktree}
            title="New worktree (Ctrl+Shift+N)"
            aria-label="New worktree workspace"
          >
            <GitBranch size={14} />
          </button>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("forktty-open-command-palette"))
            }
            title="Command palette (Ctrl+Shift+P)"
            aria-label="Open command palette"
          >
            <Command size={14} />
          </button>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={handleSplitRight}
            title="Split Right (Ctrl+D)"
            aria-label="Split right"
          >
            <Columns2 size={14} />
          </button>
          <button
            className="sidebar-icon-btn"
            type="button"
            onClick={handleSplitDown}
            title="Split Down (Ctrl+Shift+D)"
            aria-label="Split down"
          >
            <Rows2 size={14} />
          </button>
          <button
            className={`sidebar-icon-btn ${totalUnread > 0 ? "sidebar-icon-btn-unread" : ""} ${showNotificationPanel ? "sidebar-icon-btn-active" : ""}`}
            type="button"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("forktty-toggle-notifications"))
            }
            title="Notifications (Ctrl+Shift+I)"
            aria-label="Toggle notifications"
            aria-pressed={showNotificationPanel}
          >
            <Bell size={14} />
            {totalUnread > 0 && (
              <span className="sidebar-icon-badge">{totalUnread}</span>
            )}
          </button>
        </div>
      </div>
      <div className="sidebar-list">
        {workspaceOrder.map((id, index) => {
          const ws = workspaces[id];
          if (!ws) return null;
          return (
            <WorkspaceEntry
              key={id}
              workspace={ws}
              isActive={id === activeWorkspaceId}
              now={now}
              index={index}
              onGripMouseDown={handleGripMouseDown}
              onEntryMouseEnter={handleEntryMouseEnter}
              onEntryMouseUp={handleEntryMouseUp}
              isDragOver={dragOverIndex === index && dragFromIndex !== index}
              isDragging={dragFromIndex === index}
              onContextMenu={handleContextMenu}
            />
          );
        })}
      </div>
      {contextMenu &&
        createPortal(
          <ContextMenu menu={contextMenu} onClose={() => setContextMenu(null)} />,
          document.body,
        )}
    </div>
  );
}
