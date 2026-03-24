import { useMetadataStore } from "../stores/metadata";
import type { StatusEntry, ProgressEntry, LogEntry } from "../stores/metadata";

interface WorkspaceMetadataViewProps {
  workspaceId: string;
  isActive: boolean;
}

const COLOR_MAP: Record<string, string> = {
  green: "var(--theme-green)",
  yellow: "var(--theme-yellow)",
  red: "var(--theme-red)",
  blue: "var(--theme-blue)",
  muted: "var(--muted)",
};

function resolveColor(color: string | undefined): string | undefined {
  if (!color) return undefined;
  return COLOR_MAP[color] ?? color;
}

function StatusPills({ statuses }: { statuses: StatusEntry[] }) {
  if (statuses.length === 0) return null;
  return (
    <div className="sidebar-meta-pills">
      {statuses.map((s) => (
        <span
          key={s.key}
          className="sidebar-meta-pill"
          style={s.color ? { borderColor: resolveColor(s.color) } : undefined}
          title={`${s.label}: ${s.value}`}
        >
          {s.label}: {s.value}
        </span>
      ))}
    </div>
  );
}

function ProgressBars({ entries }: { entries: ProgressEntry[] }) {
  if (entries.length === 0) return null;
  return (
    <div className="sidebar-meta-progress">
      {entries.map((p) => {
        const total = p.total ?? 100;
        const pct = total > 0 ? Math.min((p.value / total) * 100, 100) : 0;
        return (
          <div key={p.key} className="sidebar-meta-progress-row">
            <span className="sidebar-meta-progress-label">
              {p.label}
              {p.total != null ? ` ${p.value}/${p.total}` : ` ${Math.round(pct)}%`}
            </span>
            <div className="sidebar-progress-bar">
              <div className="sidebar-progress-fill" style={{ width: `${pct}%` }} />
            </div>
          </div>
        );
      })}
    </div>
  );
}

const LEVEL_CLASS: Record<string, string> = {
  info: "sidebar-log-level-info",
  warn: "sidebar-log-level-warn",
  error: "sidebar-log-level-error",
};

function LogPreview({ logs }: { logs: LogEntry[] }) {
  if (logs.length === 0) return null;
  const preview = logs.slice(0, 3);
  return (
    <div className="sidebar-meta-logs">
      {preview.map((entry) => (
        <div
          key={entry.id}
          className={`sidebar-log-entry ${LEVEL_CLASS[entry.level] ?? ""}`}
        >
          {entry.message}
        </div>
      ))}
    </div>
  );
}

export default function WorkspaceMetadataView({
  workspaceId,
  isActive,
}: WorkspaceMetadataViewProps) {
  const wsMeta = useMetadataStore((s) => s.metadata[workspaceId]);

  if (!wsMeta) return null;

  const hasContent =
    wsMeta.statuses.length > 0 || wsMeta.progress.length > 0 || wsMeta.logs.length > 0;

  if (!hasContent) return null;

  return (
    <div className="sidebar-meta-view">
      <StatusPills statuses={wsMeta.statuses} />
      <ProgressBars entries={wsMeta.progress} />
      {!isActive && <LogPreview logs={wsMeta.logs} />}
    </div>
  );
}
