import { create } from "zustand";

interface StatusEntry {
  key: string;
  label: string;
  value: string;
  color?: string;
}

interface ProgressEntry {
  key: string;
  label: string;
  value: number;
  total?: number;
}

interface LogEntry {
  id: string;
  timestamp: number;
  level: "info" | "warn" | "error";
  message: string;
}

interface WorkspaceMetadata {
  statuses: StatusEntry[];
  progress: ProgressEntry[];
  logs: LogEntry[];
}

interface MetadataState {
  metadata: Record<string, WorkspaceMetadata>;
  setStatus(workspaceId: string, entry: StatusEntry): void;
  listStatus(workspaceId: string): StatusEntry[];
  clearStatus(workspaceId: string, key?: string): void;
  setProgress(workspaceId: string, entry: ProgressEntry): void;
  clearProgress(workspaceId: string, key?: string): void;
  appendLog(
    workspaceId: string,
    entry: Omit<LogEntry, "id" | "timestamp">,
  ): void;
  clearLogs(workspaceId: string): void;
  pruneWorkspace(workspaceId: string): void;
}

const MAX_LOG_ENTRIES = 200;

const VALID_LEVELS: ReadonlySet<string> = new Set(["info", "warn", "error"]);

function normalizeLevel(level: string): "info" | "warn" | "error" {
  return VALID_LEVELS.has(level)
    ? (level as "info" | "warn" | "error")
    : "info";
}

function emptyMetadata(): WorkspaceMetadata {
  return { statuses: [], progress: [], logs: [] };
}

function ensureMetadata(
  metadata: Record<string, WorkspaceMetadata>,
  workspaceId: string,
): WorkspaceMetadata {
  return metadata[workspaceId] ?? emptyMetadata();
}

export const useMetadataStore = create<MetadataState>((set, get) => ({
  metadata: {},

  setStatus(workspaceId, entry) {
    const { metadata } = get();
    const wsMeta = ensureMetadata(metadata, workspaceId);
    const idx = wsMeta.statuses.findIndex((s) => s.key === entry.key);
    const newStatuses = [...wsMeta.statuses];
    if (idx >= 0) {
      newStatuses[idx] = entry;
    } else {
      newStatuses.push(entry);
    }
    set({
      metadata: {
        ...metadata,
        [workspaceId]: { ...wsMeta, statuses: newStatuses },
      },
    });
  },

  listStatus(workspaceId) {
    const wsMeta = get().metadata[workspaceId];
    return wsMeta ? wsMeta.statuses : [];
  },

  clearStatus(workspaceId, key?) {
    const { metadata } = get();
    const wsMeta = metadata[workspaceId];
    if (!wsMeta) return;
    if (key === undefined) {
      set({
        metadata: {
          ...metadata,
          [workspaceId]: { ...wsMeta, statuses: [] },
        },
      });
    } else {
      set({
        metadata: {
          ...metadata,
          [workspaceId]: {
            ...wsMeta,
            statuses: wsMeta.statuses.filter((s) => s.key !== key),
          },
        },
      });
    }
  },

  setProgress(workspaceId, entry) {
    const { metadata } = get();
    const wsMeta = ensureMetadata(metadata, workspaceId);
    const idx = wsMeta.progress.findIndex((p) => p.key === entry.key);
    const newProgress = [...wsMeta.progress];
    if (idx >= 0) {
      newProgress[idx] = entry;
    } else {
      newProgress.push(entry);
    }
    set({
      metadata: {
        ...metadata,
        [workspaceId]: { ...wsMeta, progress: newProgress },
      },
    });
  },

  clearProgress(workspaceId, key?) {
    const { metadata } = get();
    const wsMeta = metadata[workspaceId];
    if (!wsMeta) return;
    if (key === undefined) {
      set({
        metadata: {
          ...metadata,
          [workspaceId]: { ...wsMeta, progress: [] },
        },
      });
    } else {
      set({
        metadata: {
          ...metadata,
          [workspaceId]: {
            ...wsMeta,
            progress: wsMeta.progress.filter((p) => p.key !== key),
          },
        },
      });
    }
  },

  appendLog(workspaceId, entry) {
    const { metadata } = get();
    const wsMeta = ensureMetadata(metadata, workspaceId);
    const logEntry: LogEntry = {
      id: crypto.randomUUID(),
      timestamp: Date.now(),
      level: normalizeLevel(entry.level),
      message: entry.message,
    };
    const newLogs = [logEntry, ...wsMeta.logs].slice(0, MAX_LOG_ENTRIES);
    set({
      metadata: {
        ...metadata,
        [workspaceId]: { ...wsMeta, logs: newLogs },
      },
    });
  },

  clearLogs(workspaceId) {
    const { metadata } = get();
    const wsMeta = metadata[workspaceId];
    if (!wsMeta) return;
    set({
      metadata: {
        ...metadata,
        [workspaceId]: { ...wsMeta, logs: [] },
      },
    });
  },

  pruneWorkspace(workspaceId) {
    const { metadata } = get();
    if (!metadata[workspaceId]) return;
    const newMetadata = { ...metadata };
    delete newMetadata[workspaceId];
    set({ metadata: newMetadata });
  },
}));

export type {
  StatusEntry,
  ProgressEntry,
  LogEntry,
  WorkspaceMetadata,
  MetadataState,
};
