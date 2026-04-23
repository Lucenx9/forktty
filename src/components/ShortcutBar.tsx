import { memo } from "react";
import { useWorkspaceStore } from "../stores/workspace";
import {
  useActiveWorkspaceSummary,
  selectWorkspaceCount,
  selectTotalUnread,
} from "../stores/selectors";
import { truncatePath } from "../lib/path-utils";

const ShortcutBar = memo(function ShortcutBar() {
  const activeWs = useActiveWorkspaceSummary();
  const workspaceCount = useWorkspaceStore(selectWorkspaceCount);
  const totalUnread = useWorkspaceStore(selectTotalUnread);
  const paneCount = activeWs?.surfaceCount ?? 0;

  return (
    <div className="shortcut-bar">
      <span className="shortcut-status">
        <span className="shortcut-status-label">
          {activeWs?.name ?? "No workspace"}
        </span>
        {activeWs?.gitBranch && (
          <span className="shortcut-status-pill">{activeWs.gitBranch}</span>
        )}
        <span className="shortcut-status-meta">
          {paneCount} {paneCount === 1 ? "pane" : "panes"}
        </span>
        <span className="shortcut-status-meta">
          {workspaceCount} {workspaceCount === 1 ? "workspace" : "workspaces"}
        </span>
        {totalUnread > 0 && (
          <span className="shortcut-status-pill shortcut-status-pill-alert">
            {totalUnread} unread
          </span>
        )}
        {activeWs?.workingDir && (
          <span className="shortcut-status-path" title={activeWs.workingDir}>
            {truncatePath(activeWs.workingDir, 44)}
          </span>
        )}
      </span>
      <span className="shortcut-divider" aria-hidden="true" />
      <span className="shortcut-hints" aria-label="Common shortcuts">
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
      </span>
    </div>
  );
});

export default ShortcutBar;
