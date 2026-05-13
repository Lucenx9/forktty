import { CircleAlert, Info, Terminal, X } from "lucide-react";
import { useMemo } from "react";
import { useWorkspaceStore } from "../stores/workspace";

function formatTime(timestamp: number): string {
  const diff = Math.floor((Date.now() - timestamp) / 1000);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 172800) return "yesterday";
  if (diff < 604800) return `${Math.floor(diff / 86400)}d ago`;
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
  }).format(new Date(timestamp));
}

function sectionLabel(timestamp: number): string {
  const date = new Date(timestamp);
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const target = new Date(
    date.getFullYear(),
    date.getMonth(),
    date.getDate(),
  ).getTime();
  const diffDays = Math.round((today - target) / 86400000);

  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) {
    return new Intl.DateTimeFormat(undefined, { weekday: "long" }).format(date);
  }

  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: date.getFullYear() === now.getFullYear() ? undefined : "numeric",
  }).format(date);
}

export default function NotificationPanel() {
  const notifications = useWorkspaceStore((s) => s.notifications);
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const markWorkspaceRead = useWorkspaceStore((s) => s.markWorkspaceRead);
  const markAllNotificationsRead = useWorkspaceStore((s) => s.markAllNotificationsRead);
  const clearNotifications = useWorkspaceStore((s) => s.clearNotifications);
  const toggleNotificationPanel = useWorkspaceStore((s) => s.toggleNotificationPanel);
  const unreadCount = notifications.filter((n) => !n.read).length;
  const workspaceCount = new Set(notifications.map((n) => n.workspaceId)).size;
  const sections = useMemo(() => {
    const grouped = new Map<string, typeof notifications>();
    for (const notification of notifications) {
      const label = sectionLabel(notification.timestamp);
      if (!grouped.has(label)) {
        grouped.set(label, []);
      }
      grouped.get(label)!.push(notification);
    }
    return Array.from(grouped.entries());
  }, [notifications]);

  const subtitle =
    notifications.length === 0
      ? "Background prompts and agent events will land here"
      : unreadCount > 0
        ? `${unreadCount} unread event${unreadCount === 1 ? "" : "s"} across ${workspaceCount} workspace${workspaceCount === 1 ? "" : "s"}`
        : `${notifications.length} recent event${notifications.length === 1 ? "" : "s"}, all caught up`;

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
          <span className="notification-panel-subtitle">{subtitle}</span>
        </div>
        <div className="notification-panel-actions">
          {unreadCount > 0 && (
            <button
              type="button"
              className="notification-clear-btn"
              onClick={markAllNotificationsRead}
            >
              Mark all read
            </button>
          )}
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
          sections.map(([label, items]) => (
            <section key={label} className="notification-section">
              <div className="notification-section-title">{label}</div>
              {items.map((n) => {
                const kind = n.kind === "custom" ? "info" : n.kind;
                return (
                  <button
                    key={n.id}
                    type="button"
                    className={`notification-item notification-item-${kind} ${n.read ? "notification-read" : "notification-unread"}`}
                    onClick={() => handleClick(n.workspaceId)}
                  >
                    <div className="notification-item-header">
                      <span className="notification-item-icon" aria-hidden="true">
                        {kind === "error" ? (
                          <CircleAlert size={14} />
                        ) : kind === "prompt" ? (
                          <Terminal size={14} />
                        ) : (
                          <Info size={14} />
                        )}
                      </span>
                      <span className="notification-type-badge">
                        {kind === "error"
                          ? "Error"
                          : kind === "prompt"
                            ? "Prompt waiting"
                            : "Agent update"}
                      </span>
                      <span className="notification-workspace">{n.workspaceName}</span>
                      <span className="notification-time">
                        {formatTime(n.timestamp)}
                      </span>
                    </div>
                    <div className="notification-title">{n.title}</div>
                    {n.body && n.body !== n.title && (
                      <div className="notification-body">{n.body}</div>
                    )}
                  </button>
                );
              })}
            </section>
          ))
        )}
      </div>
    </div>
  );
}
