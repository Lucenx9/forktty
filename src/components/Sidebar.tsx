import { useEffect, useRef, useState } from "react";
import { useWorkspaceStore, getLastActivity } from "../stores/workspace";
import type { Workspace } from "../stores/workspace";
import {
  getCwd,
  getGitBranch,
  worktreeCreate,
  worktreeMerge,
  worktreeRemove,
  worktreeRunHook,
  worktreeStatus,
} from "../lib/pty-bridge";
import { showToast } from "./ErrorToast";
import { useConfigStore } from "../stores/config";

const ACTIVITY_THRESHOLD_MS = 3000;

function worktreeStatusWarning(status: string): string {
  if (status === "dirty") return " (has uncommitted changes!)";
  if (status === "conflicts") return " (has merge conflicts!)";
  return "";
}

function truncatePath(path: string, maxLen: number): string {
  if (path.length <= maxLen) return path;
  const home = path.replace(/^\/home\/[^/]+/, "~");
  if (home.length <= maxLen) return home;
  return "..." + home.slice(home.length - maxLen + 3);
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
  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const markWorkspaceRead = useWorkspaceStore((s) => s.markWorkspaceRead);
  const reorderWorkspaces = useWorkspaceStore((s) => s.reorderWorkspaces);

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

  // Clamp menu position so it doesn't overflow the viewport
  useEffect(() => {
    if (!menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    const el = menuRef.current;
    if (rect.right > window.innerWidth) {
      el.style.left = `${window.innerWidth - rect.width - 4}px`;
    }
    if (rect.bottom > window.innerHeight) {
      el.style.top = `${window.innerHeight - rect.height - 4}px`;
    }
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
      if (
        !window.confirm(
          `Close workspace "${ws!.name}" with ${paneCount} panes?`,
        )
      ) {
        onClose();
        return;
      }
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
    if (wsIndex < workspaceOrder.length - 1)
      reorderWorkspaces(wsIndex, wsIndex + 1);
    onClose();
  }

  function handleMoveToTop() {
    if (wsIndex > 0) reorderWorkspaces(wsIndex, 0);
    onClose();
  }

  function handleSplitRight() {
    splitPane(ws!.focusedPaneId, "horizontal");
    onClose();
  }

  function handleSplitDown() {
    splitPane(ws!.focusedPaneId, "vertical");
    onClose();
  }

  function handleMarkRead() {
    markWorkspaceRead(menu.workspaceId);
    onClose();
  }

  function handleMerge() {
    worktreeMerge(ws!.worktreeName)
      .then((msg) => showToast(String(msg), "info"))
      .catch((err) => showToast(`Merge failed: ${err}`, "error"));
    onClose();
  }

  function handleRemoveWorktree() {
    const warning = worktreeStatusWarning(ws!.worktreeStatus);
    if (
      !window.confirm(
        `Remove worktree "${ws!.worktreeName}" and delete branch?${warning}`,
      )
    ) {
      onClose();
      return;
    }
    worktreeRemove(ws!.worktreeName)
      .then(() => closeWorkspace(menu.workspaceId))
      .catch((err) => showToast(`Remove failed: ${err}`, "error"));
    onClose();
  }

  return (
    <div
      ref={menuRef}
      className="context-menu"
      style={{ left: menu.x, top: menu.y }}
    >
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
        <button
          className="context-menu-item context-menu-item-disabled"
          disabled
        >
          <span>Mark as Read</span>
        </button>
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
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  isDragOver,
  onContextMenu,
}: {
  workspace: Workspace;
  isActive: boolean;
  now: number;
  index: number;
  onDragStart: (index: number) => void;
  onDragOver: (e: React.DragEvent, index: number) => void;
  onDrop: (index: number) => void;
  onDragEnd: () => void;
  isDragOver: boolean;
  onContextMenu: (e: React.MouseEvent, workspaceId: string) => void;
}) {
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const renameWorkspace = useWorkspaceStore((s) => s.renameWorkspace);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(workspace.name);
  const inputRef = useRef<HTMLInputElement>(null);
  const canClose = workspaceOrder.length > 1;

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

  const statusColor = hasActivity ? "#a6e3a1" : "#585b70";

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

  function handleMerge(e: React.MouseEvent) {
    e.stopPropagation();
    worktreeMerge(workspace.worktreeName)
      .then((msg) => {
        showToast(String(msg), "info");
      })
      .catch((err) => {
        showToast(`Merge failed: ${err}`, "error");
      });
  }

  function handleRemove(e: React.MouseEvent) {
    e.stopPropagation();
    const warning = worktreeStatusWarning(workspace.worktreeStatus);
    if (
      !window.confirm(
        `Remove worktree "${workspace.worktreeName}" and delete branch?${warning}`,
      )
    ) {
      return;
    }
    worktreeRemove(workspace.worktreeName)
      .then(() => {
        closeWorkspace(workspace.id);
      })
      .catch((err) => {
        showToast(`Remove failed: ${err}`, "error");
      });
  }

  function handleClose(e: React.MouseEvent) {
    e.stopPropagation();
    const paneCount = Object.keys(workspace.surfaces).length;
    if (paneCount > 1) {
      if (
        !window.confirm(
          `Close workspace "${workspace.name}" with ${paneCount} panes?`,
        )
      ) {
        return;
      }
    }
    closeWorkspace(workspace.id);
  }

  const isWorktree = workspace.worktreeDir !== "";

  return (
    <>
      {isDragOver && <div className="sidebar-drop-indicator" />}
      <div
        className={`sidebar-entry ${isActive ? "sidebar-entry-active" : ""}`}
        draggable
        onDragStart={() => onDragStart(index)}
        onDragOver={(e) => onDragOver(e, index)}
        onDrop={() => onDrop(index)}
        onDragEnd={onDragEnd}
        onClick={() => switchWorkspace(workspace.id)}
        onContextMenu={(e) => onContextMenu(e, workspace.id)}
      >
        <div className="sidebar-entry-header">
          <span
            className="sidebar-status-dot"
            style={{ backgroundColor: statusColor }}
          />
          {workspace.unreadCount > 0 && (
            <span className="sidebar-unread-badge">
              {workspace.unreadCount}
            </span>
          )}
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
            <span
              className="sidebar-entry-name"
              onDoubleClick={handleDoubleClick}
            >
              {workspace.name}
            </span>
          )}
          {canClose && (
            <button
              className="sidebar-close-btn"
              onClick={handleClose}
              title="Close workspace"
            >
              x
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
          </div>
        )}
        {workspace.workingDir && (
          <div className="sidebar-entry-meta">
            <span className="sidebar-cwd">
              {truncatePath(workspace.workingDir, 28)}
            </span>
          </div>
        )}
        {!isActive && workspace.lastNotificationText && (
          <div className="sidebar-notification-preview">
            {workspace.lastNotificationText}
          </div>
        )}
        {isWorktree && isActive && (
          <div className="sidebar-entry-actions">
            <button
              className="sidebar-action-btn"
              onClick={handleMerge}
              title="Merge branch into main"
            >
              merge
            </button>
            <button
              className="sidebar-action-btn sidebar-action-danger"
              onClick={handleRemove}
              title="Remove worktree and delete branch"
            >
              remove
            </button>
          </div>
        )}
      </div>
    </>
  );
}

// --- Help button ---

function HelpButton() {
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const btnRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    function handleClick(e: MouseEvent) {
      if (
        menuRef.current &&
        !menuRef.current.contains(e.target as Node) &&
        btnRef.current &&
        !btnRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", handleClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [open]);

  return (
    <div className="sidebar-help-wrapper">
      {open && (
        <div ref={menuRef} className="context-menu sidebar-help-menu">
          <div className="context-menu-header">ForkTTY</div>
          <button
            className="context-menu-item"
            onClick={() => {
              setOpen(false);
              window.dispatchEvent(
                new KeyboardEvent("keydown", {
                  key: "P",
                  ctrlKey: true,
                  shiftKey: true,
                  bubbles: true,
                }),
              );
            }}
          >
            <span>Command Palette</span>
            <span className="context-menu-shortcut">Ctrl+Shift+P</span>
          </button>
          <button
            className="context-menu-item"
            onClick={() => {
              setOpen(false);
              window.dispatchEvent(
                new KeyboardEvent("keydown", {
                  key: ",",
                  ctrlKey: true,
                  bubbles: true,
                }),
              );
            }}
          >
            <span>Settings</span>
            <span className="context-menu-shortcut">Ctrl+,</span>
          </button>
          <div className="context-menu-separator" />
          <div className="context-menu-header">Keyboard Shortcuts</div>
          <div className="help-shortcut-list">
            <div className="help-shortcut-row">
              <span>New Workspace</span>
              <span className="context-menu-shortcut">Ctrl+N</span>
            </div>
            <div className="help-shortcut-row">
              <span>Close Workspace</span>
              <span className="context-menu-shortcut">Ctrl+Shift+W</span>
            </div>
            <div className="help-shortcut-row">
              <span>Split Right</span>
              <span className="context-menu-shortcut">Ctrl+D</span>
            </div>
            <div className="help-shortcut-row">
              <span>Split Down</span>
              <span className="context-menu-shortcut">Ctrl+Shift+D</span>
            </div>
            <div className="help-shortcut-row">
              <span>Close Pane</span>
              <span className="context-menu-shortcut">Ctrl+W</span>
            </div>
            <div className="help-shortcut-row">
              <span>Navigate Panes</span>
              <span className="context-menu-shortcut">Alt+Arrow</span>
            </div>
            <div className="help-shortcut-row">
              <span>Find in Terminal</span>
              <span className="context-menu-shortcut">Ctrl+F</span>
            </div>
            <div className="help-shortcut-row">
              <span>Jump to Unread</span>
              <span className="context-menu-shortcut">Ctrl+Shift+U</span>
            </div>
            <div className="help-shortcut-row">
              <span>Switch Workspace</span>
              <span className="context-menu-shortcut">Ctrl+1..9</span>
            </div>
          </div>
        </div>
      )}
      <button
        ref={btnRef}
        className="sidebar-icon-btn sidebar-help-btn"
        onClick={() => setOpen((v) => !v)}
        title="Help & Shortcuts"
      >
        <svg width="14" height="14" viewBox="0 0 14 14">
          <circle
            cx="7"
            cy="7"
            r="6"
            fill="none"
            stroke="currentColor"
            strokeWidth="1"
          />
          <text
            x="7"
            y="10.5"
            textAnchor="middle"
            fill="currentColor"
            fontSize="9"
            fontWeight="600"
            fontFamily="inherit"
          >
            ?
          </text>
        </svg>
      </button>
    </div>
  );
}

// --- Sidebar ---

export default function Sidebar() {
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);
  const createWorkspace = useWorkspaceStore((s) => s.createWorkspace);
  const createWorktreeWorkspace = useWorkspaceStore(
    (s) => s.createWorktreeWorkspace,
  );
  const setWorkspaceGitBranch = useWorkspaceStore(
    (s) => s.setWorkspaceGitBranch,
  );
  const setWorkspaceWorkingDir = useWorkspaceStore(
    (s) => s.setWorkspaceWorkingDir,
  );
  const setWorktreeStatus = useWorkspaceStore((s) => s.setWorktreeStatus);
  const reorderWorkspaces = useWorkspaceStore((s) => s.reorderWorkspaces);
  const worktreeLayout = useConfigStore(
    (s) => s.config?.general.worktree_layout ?? "nested",
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

  function handleDragStart(index: number) {
    setDragFromIndex(index);
  }

  function handleDragOver(e: React.DragEvent, index: number) {
    e.preventDefault();
    setDragOverIndex(index);
  }

  function handleDrop(toIndex: number) {
    if (dragFromIndex !== null && dragFromIndex !== toIndex) {
      reorderWorkspaces(dragFromIndex, toIndex);
    }
    setDragFromIndex(null);
    setDragOverIndex(null);
  }

  function handleDragEnd() {
    setDragFromIndex(null);
    setDragOverIndex(null);
  }

  // 1-second tick for status dot re-evaluation
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, []);

  // Fetch CWD and git branch for new workspaces (those without workingDir)
  useEffect(() => {
    const { workspaces: current } = useWorkspaceStore.getState();
    for (const id of workspaceOrder) {
      const ws = current[id];
      if (ws && !ws.workingDir) {
        getCwd()
          .then((cwd) => {
            setWorkspaceWorkingDir(id, cwd);
            return getGitBranch(cwd);
          })
          .then((branch) => {
            if (branch) {
              setWorkspaceGitBranch(id, branch);
            }
          })
          .catch(console.error);
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
        .catch(console.error);
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
    const name = window.prompt("Worktree name (becomes branch name):");
    if (!name || !name.trim()) return;
    const trimmed = name.trim();

    worktreeCreate(trimmed, worktreeLayout)
      .then((info) => {
        const wsId = createWorktreeWorkspace(
          trimmed,
          info.path,
          info.branch,
          info.path,
          info.name,
        );
        // Run setup hook
        worktreeRunHook(info.path, "setup").catch(console.error);
        return wsId;
      })
      .catch((err) => {
        showToast(`Failed to create worktree: ${err}`, "error");
      });
  }

  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const toggleNotificationPanel = useWorkspaceStore(
    (s) => s.toggleNotificationPanel,
  );
  const totalUnread = useWorkspaceStore((s) =>
    Object.values(s.workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0),
  );

  function handleSplitRight() {
    const state = useWorkspaceStore.getState();
    const ws = state.workspaces[state.activeWorkspaceId];
    if (ws) splitPane(ws.focusedPaneId, "horizontal");
  }

  function handleSplitDown() {
    const state = useWorkspaceStore.getState();
    const ws = state.workspaces[state.activeWorkspaceId];
    if (ws) splitPane(ws.focusedPaneId, "vertical");
  }

  return (
    <div className="sidebar">
      <div className="sidebar-header">
        <div className="sidebar-header-icons">
          <button
            className="sidebar-icon-btn"
            onClick={() => createWorkspace()}
            title="New workspace (Ctrl+N)"
          >
            <svg width="14" height="14" viewBox="0 0 14 14">
              <line
                x1="7"
                y1="2"
                x2="7"
                y2="12"
                stroke="currentColor"
                strokeWidth="1.5"
              />
              <line
                x1="2"
                y1="7"
                x2="12"
                y2="7"
                stroke="currentColor"
                strokeWidth="1.5"
              />
            </svg>
          </button>
          <button
            className="sidebar-icon-btn sidebar-icon-btn-worktree"
            onClick={handleNewWorktree}
            title="New worktree (Ctrl+Shift+N)"
          >
            <svg width="14" height="14" viewBox="0 0 14 14">
              <circle
                cx="7"
                cy="3"
                r="1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
              <circle
                cx="3"
                cy="11"
                r="1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
              <circle
                cx="11"
                cy="11"
                r="1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
              <line
                x1="7"
                y1="4.5"
                x2="3"
                y2="9.5"
                stroke="currentColor"
                strokeWidth="1"
              />
              <line
                x1="7"
                y1="4.5"
                x2="11"
                y2="9.5"
                stroke="currentColor"
                strokeWidth="1"
              />
            </svg>
          </button>
          <button
            className="sidebar-icon-btn"
            onClick={handleSplitRight}
            title="Split Right (Ctrl+D)"
          >
            <svg width="14" height="14" viewBox="0 0 14 14">
              <rect
                x="1"
                y="1"
                width="12"
                height="12"
                rx="1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
              <line
                x1="7"
                y1="1"
                x2="7"
                y2="13"
                stroke="currentColor"
                strokeWidth="1"
              />
            </svg>
          </button>
          <button
            className="sidebar-icon-btn"
            onClick={handleSplitDown}
            title="Split Down (Ctrl+Shift+D)"
          >
            <svg width="14" height="14" viewBox="0 0 14 14">
              <rect
                x="1"
                y="1"
                width="12"
                height="12"
                rx="1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
              <line
                x1="1"
                y1="7"
                x2="13"
                y2="7"
                stroke="currentColor"
                strokeWidth="1"
              />
            </svg>
          </button>
          <button
            className={`sidebar-icon-btn ${totalUnread > 0 ? "sidebar-icon-btn-unread" : ""}`}
            onClick={toggleNotificationPanel}
            title="Notifications (Ctrl+Shift+I)"
          >
            <svg width="14" height="14" viewBox="0 0 14 14">
              <path
                d="M7 1C4.8 1 3 2.8 3 5v2.5L1.5 9.5v1h11v-1L11 7.5V5c0-2.2-1.8-4-4-4z"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
                strokeLinejoin="round"
              />
              <path
                d="M5.5 11.5c0 .8.7 1.5 1.5 1.5s1.5-.7 1.5-1.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1"
              />
            </svg>
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
              onDragStart={handleDragStart}
              onDragOver={handleDragOver}
              onDrop={handleDrop}
              onDragEnd={handleDragEnd}
              isDragOver={dragOverIndex === index && dragFromIndex !== index}
              onContextMenu={handleContextMenu}
            />
          );
        })}
      </div>
      <div className="sidebar-footer">
        <HelpButton />
      </div>
      {contextMenu && (
        <ContextMenu menu={contextMenu} onClose={() => setContextMenu(null)} />
      )}
    </div>
  );
}
