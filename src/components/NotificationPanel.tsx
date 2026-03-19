import { useWorkspaceStore } from "../stores/workspace";

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
  const toggleNotificationPanel = useWorkspaceStore(
    (s) => s.toggleNotificationPanel,
  );

  function handleClick(workspaceId: string) {
    switchWorkspace(workspaceId);
    markWorkspaceRead(workspaceId);
    toggleNotificationPanel();
  }

  return (
    <div className="notification-panel">
      <div className="notification-panel-header">
        <span className="notification-panel-title">Notifications</span>
        <div className="notification-panel-actions">
          {notifications.length > 0 && (
            <button
              className="notification-clear-btn"
              onClick={clearNotifications}
            >
              Clear all
            </button>
          )}
          <button
            className="notification-close-btn"
            onClick={toggleNotificationPanel}
          >
            x
          </button>
        </div>
      </div>
      <div className="notification-list">
        {notifications.length === 0 ? (
          <div className="notification-empty">No notifications</div>
        ) : (
          notifications.map((n) => (
            <div
              key={n.id}
              className={`notification-item ${n.read ? "notification-read" : ""}`}
              onClick={() => handleClick(n.workspaceId)}
            >
              <div className="notification-item-header">
                {!n.read && <span className="notification-dot" />}
                <span className="notification-workspace">
                  {n.workspaceName}
                </span>
                <span className="notification-time">
                  {formatTime(n.timestamp)}
                </span>
              </div>
              <div className="notification-body">{n.body}</div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
