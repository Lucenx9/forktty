import { useWorkspaceStore } from "../stores/workspace";
import { X } from "lucide-react";

function formatTime(timestamp: number): string {
  const diff = Math.floor((Date.now() - timestamp) / 1000);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  return `${Math.floor(diff / 3600)}h ago`;
}

export default function NotificationPanel() {
  const notifications = useWorkspaceStore((s) => s.notifications);
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const markWorkspaceRead = useWorkspaceStore((s) => s.markWorkspaceRead);
  const clearNotifications = useWorkspaceStore((s) => s.clearNotifications);
  const toggleNotificationPanel = useWorkspaceStore((s) => s.toggleNotificationPanel);

  function handleClick(workspaceId: string) {
    switchWorkspace(workspaceId);
    markWorkspaceRead(workspaceId);
    toggleNotificationPanel();
  }

  return (
    <div className="notification-panel">
      <div className="notification-panel-header">
        <div className="notification-panel-heading">
          <span className="notification-panel-title">Notifications</span>
          <span className="notification-panel-subtitle">
            Background prompts and agent events
          </span>
        </div>
        <div className="notification-panel-actions">
          {notifications.length > 0 && (
            <button
              type="button"
              className="notification-clear-btn"
              onClick={clearNotifications}
            >
              Clear all
            </button>
          )}
          <button
            type="button"
            className="notification-close-btn"
            onClick={toggleNotificationPanel}
            aria-label="Close notifications"
          >
            <X size={14} />
          </button>
        </div>
      </div>
      <div className="notification-list">
        {notifications.length === 0 ? (
          <div className="notification-empty">
            <div className="notification-empty-title">You&apos;re caught up</div>
            <div className="notification-empty-body">
              Alerts from background workspaces will land here.
            </div>
          </div>
        ) : (
          notifications.map((n) => (
            <button
              key={n.id}
              type="button"
              className={`notification-item ${n.read ? "notification-read" : ""}`}
              onClick={() => handleClick(n.workspaceId)}
            >
              <div className="notification-item-header">
                {!n.read && <span className="notification-dot" />}
                <span className="notification-workspace">{n.workspaceName}</span>
                <span className="notification-time">{formatTime(n.timestamp)}</span>
              </div>
              <div className="notification-title">{n.title}</div>
              {n.body && n.body !== n.title && (
                <div className="notification-body">{n.body}</div>
              )}
            </button>
          ))
        )}
      </div>
    </div>
  );
}
