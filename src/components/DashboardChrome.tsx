import { useMemo } from "react";
import type { ReactNode } from "react";
import {
  Activity,
  Bell,
  ChevronRight,
  Columns2,
  Command,
  FolderKanban,
  GitBranch,
  Plus,
  Rows2,
  Search,
  Settings2,
  Sparkles,
} from "lucide-react";
import { useWorkspaceStore } from "../stores/workspace";
import { useMetadataStore } from "../stores/metadata";
import type { MetadataState } from "../stores/metadata";
import {
  useActiveWorkspaceSummary,
  selectActiveWorkspaceId,
  selectWorkspaceCount,
  selectTotalUnread,
  selectTotalPaneCount,
  selectNotifications,
} from "../stores/selectors";
import { truncatePath } from "../lib/path-utils";

interface DashboardChromeProps {
  children: ReactNode;
  onCreateWorkspace: () => void;
  onOpenBranchPicker: () => void;
  onOpenCommandPalette: () => void;
  onOpenSettings: () => void;
  onToggleNotifications: () => void;
  onSplitRight: () => void;
  onSplitDown: () => void;
  showNotificationPanel: boolean;
}

function formatRelativeTime(timestamp: number): string {
  const diffMs = Date.now() - timestamp;
  if (diffMs < 60000) return "just now";
  const minutes = Math.round(diffMs / 60000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.round(hours / 24);
  return `${days}d ago`;
}

function EmptyRow({ label }: { label: string }) {
  return <div className="dashboard-empty-row">{label}</div>;
}

export default function DashboardChrome({
  children,
  onCreateWorkspace,
  onOpenBranchPicker,
  onOpenCommandPalette,
  onOpenSettings,
  onToggleNotifications,
  onSplitRight,
  onSplitDown,
  showNotificationPanel,
}: DashboardChromeProps) {
  const activeWs = useActiveWorkspaceSummary();
  const workspaceCount = useWorkspaceStore(selectWorkspaceCount);
  const totalUnread = useWorkspaceStore(selectTotalUnread);
  const totalPaneCount = useWorkspaceStore(selectTotalPaneCount);
  const allNotifications = useWorkspaceStore(selectNotifications);

  const activeWorkspaceId = useWorkspaceStore(selectActiveWorkspaceId);
  const metadataSelector = useMemo(
    () => (s: MetadataState) =>
      activeWorkspaceId ? s.metadata[activeWorkspaceId] : undefined,
    [activeWorkspaceId],
  );
  const activeMetadata = useMetadataStore(metadataSelector);

  const paneCount = activeWs?.surfaceCount ?? 0;
  const notifications = allNotifications.slice(0, 4);
  const recentStatuses = activeMetadata?.statuses.slice(0, 3) ?? [];
  const recentProgress = activeMetadata?.progress.slice(0, 3) ?? [];
  const recentLogs = activeMetadata?.logs.slice(0, 3) ?? [];
  const workspaceMode = activeWs?.worktreeDir ? "Worktree workspace" : "Workspace";

  const signalCards = useMemo(
    () => [
      {
        label: "Active workspace",
        value: activeWs?.name ?? "No workspace",
        meta: activeWs?.gitBranch ?? "Detached session",
      },
      {
        label: "Live panes",
        value: String(paneCount),
        meta: `${totalPaneCount} total`,
      },
      {
        label: "Unread alerts",
        value: String(totalUnread),
        meta: totalUnread > 0 ? "Needs review" : "All clear",
      },
      {
        label: "Workspaces",
        value: String(workspaceCount),
        meta: activeWs?.worktreeDir ? "Worktree-backed" : "Local shell",
      },
    ],
    [
      activeWs?.name,
      activeWs?.gitBranch,
      activeWs?.worktreeDir,
      paneCount,
      totalPaneCount,
      totalUnread,
      workspaceCount,
    ],
  );

  return (
    <div className="workspace-shell">
      <div className="dashboard-chrome">
        <div className="dashboard-command-row">
          <div className="dashboard-hero-copy">
            <div className="dashboard-hero-eyebrow">{workspaceMode}</div>
            <div className="dashboard-hero-title">
              {activeWs?.name ?? "Workspace overview"}
            </div>
            <div className="dashboard-hero-subtitle">
              {activeWs?.workingDir
                ? truncatePath(activeWs.workingDir, 64)
                : "Parallel terminals with isolated worktrees and fast context switches"}
            </div>
          </div>

          <button
            type="button"
            className="dashboard-search-trigger"
            onClick={onOpenCommandPalette}
            aria-label="Open command palette"
          >
            <Search size={17} />
            <span className="dashboard-search-copy">Type a command or search...</span>
            <span className="dashboard-search-shortcut">Ctrl+Shift+P</span>
          </button>

          <div className="dashboard-command-actions">
            <button
              type="button"
              className="dashboard-action-btn"
              onClick={onToggleNotifications}
              aria-pressed={showNotificationPanel}
            >
              <Bell size={15} />
              <span>Alerts</span>
              {totalUnread > 0 && (
                <span className="dashboard-action-pill">{totalUnread}</span>
              )}
            </button>
            <button
              type="button"
              className="dashboard-action-btn"
              onClick={onOpenSettings}
            >
              <Settings2 size={15} />
              <span>Settings</span>
            </button>
          </div>
        </div>

        <div className="dashboard-signal-strip">
          {signalCards.map((card) => (
            <div key={card.label} className="dashboard-signal-card">
              <span className="dashboard-signal-label">{card.label}</span>
              <strong className="dashboard-signal-value">{card.value}</strong>
              <span className="dashboard-signal-meta">{card.meta}</span>
            </div>
          ))}
        </div>
      </div>

      <div className="workspace-main-grid">
        <section className="workspace-stage">{children}</section>

        <aside className="workspace-inspector" aria-label="Workspace inspector">
          <div className="workspace-inspector-panel">
            <div className="workspace-inspector-header">
              <div>
                <div className="workspace-inspector-title">Quick Actions & Signals</div>
                <div className="workspace-inspector-subtitle">
                  Operational context for the active workspace
                </div>
              </div>
            </div>

            <div className="workspace-inspector-actions">
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onCreateWorkspace}
                aria-label="New workspace"
              >
                <Plus size={14} />
                <span>Workspace</span>
              </button>
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onOpenBranchPicker}
                aria-label="New worktree workspace"
              >
                <GitBranch size={14} />
                <span>Worktree</span>
              </button>
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onSplitRight}
                aria-label="Split pane right"
              >
                <Columns2 size={14} />
                <span>Split right</span>
              </button>
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onSplitDown}
                aria-label="Split pane down"
              >
                <Rows2 size={14} />
                <span>Split down</span>
              </button>
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onOpenCommandPalette}
                aria-label="Open command palette"
              >
                <Command size={14} />
                <span>Palette</span>
              </button>
              <button
                type="button"
                className="workspace-inspector-action"
                onClick={onOpenSettings}
                aria-label="Open settings"
              >
                <Settings2 size={14} />
                <span>Config</span>
              </button>
            </div>

            <div className="workspace-inspector-section">
              <div className="workspace-inspector-section-title">
                <Sparkles size={14} />
                <span>Live Workspace</span>
              </div>
              <div className="workspace-inspector-list">
                <div className="workspace-inspector-item">
                  <span className="workspace-inspector-item-label">Branch</span>
                  <span className="workspace-inspector-item-value">
                    {activeWs?.gitBranch ?? "No git branch"}
                  </span>
                </div>
                <div className="workspace-inspector-item">
                  <span className="workspace-inspector-item-label">Directory</span>
                  <span className="workspace-inspector-item-value">
                    {activeWs?.workingDir
                      ? truncatePath(activeWs.workingDir, 38)
                      : "Not resolved yet"}
                  </span>
                </div>
                <div className="workspace-inspector-item">
                  <span className="workspace-inspector-item-label">Pane layout</span>
                  <span className="workspace-inspector-item-value">
                    {paneCount} {paneCount === 1 ? "pane" : "panes"}
                  </span>
                </div>
                <div className="workspace-inspector-item">
                  <span className="workspace-inspector-item-label">Worktree</span>
                  <span className="workspace-inspector-item-value">
                    {activeWs?.worktreeDir
                      ? activeWs.worktreeStatus || "Attached"
                      : "Standard workspace"}
                  </span>
                </div>
              </div>
            </div>

            <div className="workspace-inspector-section">
              <div className="workspace-inspector-section-title">
                <Activity size={14} />
                <span>Signals</span>
              </div>
              <div className="workspace-inspector-stack">
                {recentStatuses.length === 0 ? (
                  <EmptyRow label="No live status entries for this workspace." />
                ) : (
                  recentStatuses.map((status) => (
                    <div key={status.key} className="workspace-signal-row">
                      <div className="workspace-signal-copy">
                        <span className="workspace-signal-name">{status.label}</span>
                        <span className="workspace-signal-value">{status.value}</span>
                      </div>
                      <ChevronRight size={14} />
                    </div>
                  ))
                )}
              </div>
            </div>

            <div className="workspace-inspector-section">
              <div className="workspace-inspector-section-title">
                <FolderKanban size={14} />
                <span>Background Tasks</span>
              </div>
              <div className="workspace-inspector-stack">
                {recentProgress.length === 0 ? (
                  <EmptyRow label="No tracked background tasks right now." />
                ) : (
                  recentProgress.map((entry) => {
                    const total = entry.total ?? 100;
                    const pct =
                      total > 0 ? Math.min((entry.value / total) * 100, 100) : 0;
                    return (
                      <div key={entry.key} className="workspace-progress-card">
                        <div className="workspace-progress-copy">
                          <span>{entry.label}</span>
                          <span>
                            {entry.total != null
                              ? `${entry.value}/${entry.total}`
                              : `${Math.round(pct)}%`}
                          </span>
                        </div>
                        <div className="workspace-progress-track">
                          <div
                            className="workspace-progress-fill"
                            style={{ width: `${pct}%` }}
                          />
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </div>

            <div className="workspace-inspector-section">
              <div className="workspace-inspector-section-title">
                <Bell size={14} />
                <span>Recent Alerts</span>
              </div>
              <div className="workspace-inspector-stack">
                {notifications.length === 0 ? (
                  <EmptyRow label="No recent alerts in this session." />
                ) : (
                  notifications.map((notification) => (
                    <div key={notification.id} className="workspace-alert-card">
                      <div className="workspace-alert-topline">
                        <span>{notification.workspaceName}</span>
                        <span>{formatRelativeTime(notification.timestamp)}</span>
                      </div>
                      <div className="workspace-alert-title">{notification.title}</div>
                      <div className="workspace-alert-body">{notification.body}</div>
                    </div>
                  ))
                )}
              </div>
            </div>

            {recentLogs.length > 0 && (
              <div className="workspace-inspector-section workspace-inspector-section-compact">
                <div className="workspace-inspector-section-title">
                  <Activity size={14} />
                  <span>Latest Log</span>
                </div>
                <div className="workspace-log-preview">{recentLogs[0]?.message}</div>
              </div>
            )}
          </div>
        </aside>
      </div>
    </div>
  );
}
