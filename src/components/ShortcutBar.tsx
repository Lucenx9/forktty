import { memo } from "react";
import { useWorkspaceStore } from "../stores/workspace";

const ShortcutBar = memo(function ShortcutBar() {
  const activeWorkspace = useWorkspaceStore((s) => s.workspaces[s.activeWorkspaceId]);
  const totalUnread = useWorkspaceStore((s) =>
    Object.values(s.workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0),
  );
  const paneCount = activeWorkspace ? Object.keys(activeWorkspace.surfaces).length : 0;

  return (
    <div className="shortcut-bar">
      <span className="shortcut-status">
        <span className="shortcut-status-label">
          {activeWorkspace?.name ?? "No workspace"}
        </span>
        {activeWorkspace?.gitBranch && (
          <span className="shortcut-status-pill">{activeWorkspace.gitBranch}</span>
        )}
        <span className="shortcut-status-meta">
          {paneCount} {paneCount === 1 ? "pane" : "panes"}
        </span>
        {totalUnread > 0 && (
          <span className="shortcut-status-pill shortcut-status-pill-alert">
            {totalUnread} unread
          </span>
        )}
      </span>
      <span className="shortcut-divider" aria-hidden="true" />
      <span className="shortcut-hint">
        <kbd>Ctrl+D</kbd> Split
      </span>
      <span className="shortcut-hint">
        <kbd>Alt+Arrow</kbd> Navigate
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+W</kbd> Close Pane
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+F</kbd> Find
      </span>
      <span className="shortcut-hint">
        <kbd>Ctrl+Shift+P</kbd> Palette
      </span>
    </div>
  );
});

export default ShortcutBar;
