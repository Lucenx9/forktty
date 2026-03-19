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

const ACTIVITY_THRESHOLD_MS = 3000;

function truncatePath(path: string, maxLen: number): string {
  if (path.length <= maxLen) return path;
  const home = path.replace(/^\/home\/[^/]+/, "~");
  if (home.length <= maxLen) return home;
  return "..." + home.slice(home.length - maxLen + 3);
}

function WorkspaceEntry({
  workspace,
  isActive,
  now,
}: {
  workspace: Workspace;
  isActive: boolean;
  now: number;
}) {
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const renameWorkspace = useWorkspaceStore((s) => s.renameWorkspace);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const [editing, setEditing] = useState(false);
  const [editValue, setEditValue] = useState(workspace.name);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing]);

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
        showToast(String(err), "error");
      });
  }

  function handleRemove(e: React.MouseEvent) {
    e.stopPropagation();
    const statusWarning =
      workspace.worktreeStatus === "dirty"
        ? " (has uncommitted changes!)"
        : workspace.worktreeStatus === "conflicts"
          ? " (has merge conflicts!)"
          : "";
    if (
      !window.confirm(
        `Remove worktree "${workspace.worktreeName}" and delete branch?${statusWarning}`,
      )
    ) {
      return;
    }
    worktreeRemove(workspace.worktreeName)
      .then(() => {
        closeWorkspace(workspace.id);
      })
      .catch((err) => {
        showToast(String(err), "error");
      });
  }

  const isWorktree = workspace.worktreeDir !== "";

  return (
    <div
      className={`sidebar-entry ${isActive ? "sidebar-entry-active" : ""}`}
      onClick={() => switchWorkspace(workspace.id)}
    >
      <div className="sidebar-entry-header">
        <span
          className="sidebar-status-dot"
          style={{ backgroundColor: statusColor }}
        />
        {workspace.unreadCount > 0 && (
          <span className="sidebar-unread-badge">{workspace.unreadCount}</span>
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
  );
}

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
            .catch(() => {});
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

    worktreeCreate(trimmed)
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
        console.error("Failed to create worktree:", err);
      });
  }

  return (
    <div className="sidebar">
      <div className="sidebar-header">Workspaces</div>
      <div className="sidebar-list">
        {workspaceOrder.map((id) => {
          const ws = workspaces[id];
          if (!ws) return null;
          return (
            <WorkspaceEntry
              key={id}
              workspace={ws}
              isActive={id === activeWorkspaceId}
              now={now}
            />
          );
        })}
      </div>
      <div className="sidebar-buttons">
        <button
          className="sidebar-new-btn"
          onClick={() => createWorkspace()}
          title="New workspace (Ctrl+N)"
        >
          + New
        </button>
        <button
          className="sidebar-new-btn sidebar-worktree-btn"
          onClick={handleNewWorktree}
          title="New worktree workspace (Ctrl+Shift+N)"
        >
          + Worktree
        </button>
      </div>
    </div>
  );
}
