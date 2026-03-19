import { useEffect, useRef, useState } from "react";
import { useWorkspaceStore, getLastActivity } from "../stores/workspace";
import type { Workspace } from "../stores/workspace";
import { getCwd, getGitBranch } from "../lib/pty-bridge";

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
        </div>
      )}
      {workspace.workingDir && (
        <div className="sidebar-entry-meta">
          <span className="sidebar-cwd">
            {truncatePath(workspace.workingDir, 28)}
          </span>
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
  const setWorkspaceGitBranch = useWorkspaceStore(
    (s) => s.setWorkspaceGitBranch,
  );
  const setWorkspaceWorkingDir = useWorkspaceStore(
    (s) => s.setWorkspaceWorkingDir,
  );

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
      <button
        className="sidebar-new-btn"
        onClick={() => createWorkspace()}
        title="New workspace (Ctrl+N)"
      >
        + New
      </button>
    </div>
  );
}
