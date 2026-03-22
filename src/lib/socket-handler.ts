import {
  useWorkspaceStore,
  closeWorkspaceEnsuringOneRemains,
} from "../stores/workspace";
import { useMetadataStore } from "../stores/metadata";
import { useConfigStore } from "../stores/config";
import {
  writePty,
  socketRespond,
  sendDesktopNotification,
  worktreeCreate,
  worktreeMerge,
  worktreeRemove,
  worktreeRunHook,
  logError,
} from "./pty-bridge";
import { readScreen } from "./terminal-registry";

type WorkspaceStoreState = ReturnType<typeof useWorkspaceStore.getState>;

interface WorkspaceSelector {
  workspaceId?: string | null;
  workspaceName?: string | null;
  worktreeName?: string | null;
  fallbackActive?: boolean;
}

function getStringParam(
  params: Record<string, unknown>,
  ...keys: string[]
): string | null {
  for (const key of keys) {
    const value = params[key];
    if (typeof value === "string" && value.length > 0) {
      return value;
    }
  }
  return null;
}

/** Resolve a workspace selector, preferring stable IDs over renameable labels. */
function resolveWorkspaceId(
  state: WorkspaceStoreState,
  {
    workspaceId,
    workspaceName,
    worktreeName,
    fallbackActive = false,
  }: WorkspaceSelector,
): { workspaceId: string | null; error?: string } {
  if (workspaceId) {
    if (state.workspaces[workspaceId]) {
      return { workspaceId };
    }
    return { workspaceId: null, error: `Workspace "${workspaceId}" not found` };
  }

  if (worktreeName) {
    const matches = state.workspaceOrder.filter(
      (wsId) => state.workspaces[wsId]?.worktreeName === worktreeName,
    );
    if (matches.length === 1) {
      return { workspaceId: matches[0]! };
    }
    if (matches.length > 1) {
      return {
        workspaceId: null,
        error: `Worktree "${worktreeName}" is ambiguous`,
      };
    }
  }

  if (workspaceName) {
    const matches = state.workspaceOrder.filter(
      (wsId) => state.workspaces[wsId]?.name === workspaceName,
    );
    if (matches.length === 1) {
      return { workspaceId: matches[0]! };
    }
    if (matches.length > 1) {
      return {
        workspaceId: null,
        error: `Workspace name "${workspaceName}" is ambiguous`,
      };
    }
    return {
      workspaceId: null,
      error: `Workspace "${workspaceName}" not found`,
    };
  }

  if (fallbackActive) {
    return { workspaceId: state.activeWorkspaceId };
  }

  return { workspaceId: null, error: "Workspace not found" };
}

function getWorkspacePtyId(workspaceId: string): number | null {
  const workspace = useWorkspaceStore.getState().workspaces[workspaceId];
  if (!workspace) return null;
  return (
    Object.values(workspace.surfaces).find((s) => s.ptyId != null)?.ptyId ??
    null
  );
}

function waitForWorkspacePty(
  workspaceId: string,
  timeoutMs = 5000,
): Promise<number> {
  const existingPtyId = getWorkspacePtyId(workspaceId);
  if (existingPtyId != null) {
    return Promise.resolve(existingPtyId);
  }

  return new Promise((resolve, reject) => {
    let unsubscribeFn: (() => void) | null = null;
    const timeoutId = window.setTimeout(() => {
      if (unsubscribeFn) unsubscribeFn();
      reject(new Error("Timed out waiting for workspace PTY"));
    }, timeoutMs);

    unsubscribeFn = useWorkspaceStore.subscribe(() => {
      const ptyId = getWorkspacePtyId(workspaceId);
      if (ptyId != null) {
        window.clearTimeout(timeoutId);
        if (unsubscribeFn) unsubscribeFn();
        resolve(ptyId);
      }
    });
  });
}

/**
 * Handle a socket API bridge request. Reads all state via .getState() at call time,
 * so this function is safe to call from a stale closure (e.g., a useEffect with empty deps).
 */
export async function handleSocketRequest(
  id: string,
  method: string,
  params: Record<string, unknown>,
): Promise<void> {
  const state = useWorkspaceStore.getState();
  const config = useConfigStore.getState().config;
  let result: unknown;

  try {
    switch (method) {
      case "workspace.list": {
        const list = state.workspaceOrder.map((wsId) => {
          const ws = state.workspaces[wsId];
          return ws
            ? {
                id: ws.id,
                name: ws.name,
                gitBranch: ws.gitBranch,
                workingDir: ws.workingDir,
                surfaces: Object.keys(ws.surfaces).length,
                active: wsId === state.activeWorkspaceId,
              }
            : null;
        });
        result = { result: list.filter(Boolean) };
        break;
      }
      case "workspace.create": {
        const name = params.name as string | undefined;
        const prompt =
          typeof params.prompt === "string" && params.prompt.length > 0
            ? params.prompt
            : undefined;
        const worktreeDir =
          typeof params.worktreeDir === "string" ? params.worktreeDir : "";
        const worktreeName =
          typeof params.worktreeName === "string" ? params.worktreeName : "";
        const workingDir =
          typeof params.workingDir === "string"
            ? params.workingDir
            : worktreeDir;
        const gitBranch =
          typeof params.gitBranch === "string" ? params.gitBranch : "";
        const isWorktree = worktreeDir.length > 0;

        const wsId = isWorktree
          ? state.createWorktreeWorkspace(
              name ?? worktreeName,
              workingDir,
              gitBranch,
              worktreeDir,
              worktreeName || name || "",
            )
          : state.createWorkspace(name ?? undefined);

        const response: Record<string, unknown> = { id: wsId };
        if (prompt) {
          const ptyId = await waitForWorkspacePty(wsId);
          response.pty_id = ptyId;

          if (!isWorktree) {
            await writePty(ptyId, prompt);
          }
        }

        result = { result: response };
        break;
      }
      case "workspace.select": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "id", "workspaceId"),
          workspaceName: getStringParam(params, "name"),
          worktreeName: getStringParam(params, "worktreeName", "worktree_name"),
        });
        if (target.workspaceId) {
          state.switchWorkspace(target.workspaceId);
          result = { result: true };
        } else {
          result = { error: target.error ?? "Workspace not found" };
        }
        break;
      }
      case "workspace.close": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "id", "workspaceId"),
          workspaceName: getStringParam(params, "name"),
          worktreeName: getStringParam(params, "worktreeName", "worktree_name"),
        });
        if (target.workspaceId) {
          const latestState = useWorkspaceStore.getState();
          const targetWorkspace = latestState.workspaces[target.workspaceId];
          if (
            latestState.workspaceOrder.length <= 1 &&
            targetWorkspace?.worktreeName
          ) {
            closeWorkspaceEnsuringOneRemains(target.workspaceId);
          } else {
            latestState.closeWorkspace(target.workspaceId);
          }
          result = { result: true };
        } else {
          result = { error: target.error ?? "Workspace not found" };
        }
        break;
      }
      case "surface.list": {
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) {
          result = {
            result: Object.values(ws.surfaces).map((s) => ({
              id: s.id,
              ptyId: s.ptyId,
              title: s.title,
            })),
          };
        } else {
          result = { result: [] };
        }
        break;
      }
      case "surface.split": {
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) {
          const dir =
            (params.direction as string) === "down" ? "vertical" : "horizontal";
          state.splitPane(ws.focusedPaneId, dir as "horizontal" | "vertical");
          result = { result: true };
        } else {
          result = { error: "No active workspace" };
        }
        break;
      }
      case "surface.close": {
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) {
          state.closePane(ws.focusedPaneId);
          result = { result: true };
        } else {
          result = { error: "No active workspace" };
        }
        break;
      }
      case "surface.send_text": {
        const surfaceId = params.surface_id as string | undefined;
        const text = params.text as string;
        if (surfaceId) {
          for (const ws of Object.values(state.workspaces)) {
            const surface = ws.surfaces[surfaceId];
            if (surface?.ptyId != null) {
              writePty(surface.ptyId, text).catch(logError);
              result = { result: true };
              break;
            }
          }
          if (!result) result = { error: "Surface not found" };
        } else if (params.pty_id != null) {
          await writePty(params.pty_id as number, text);
          result = { result: true };
        } else {
          result = { error: "Missing surface_id or pty_id" };
        }
        break;
      }
      case "surface.read_screen": {
        const surfaceId = params.surface_id as string | undefined;
        const activeSurfaceId =
          state.workspaces[state.activeWorkspaceId]?.focusedPaneId ?? null;
        const targetSurfaceId = surfaceId ?? activeSurfaceId;
        const content = readScreen(targetSurfaceId);
        if (content === null) {
          result = {
            error: surfaceId
              ? `Surface "${surfaceId}" not found`
              : activeSurfaceId
                ? `Surface "${activeSurfaceId}" not found`
                : "No terminals available",
          };
        } else {
          result = { result: content };
        }
        break;
      }
      case "notification.create": {
        const title = (params.title as string) || "ForkTTY";
        const body = (params.body as string) || "";
        state.addNotification(state.activeWorkspaceId, title, body);
        if (config?.notifications.desktop ?? true) {
          sendDesktopNotification(title, body).catch(logError);
        }
        result = { result: true };
        break;
      }
      case "notification.list": {
        result = {
          result: state.notifications.map((n) => ({
            id: n.id,
            workspaceName: n.workspaceName,
            title: n.title,
            body: n.body,
            timestamp: n.timestamp,
            read: n.read,
          })),
        };
        break;
      }
      case "notification.clear": {
        state.clearNotifications();
        result = { result: true };
        break;
      }
      case "metadata.set_status": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        useMetadataStore.getState().setStatus(target.workspaceId, {
          key: params.key as string,
          label: params.label as string,
          value: params.value as string,
          color: (params.color as string | undefined) ?? undefined,
        });
        result = { result: true };
        break;
      }
      case "metadata.list_status": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        result = {
          result: useMetadataStore.getState().listStatus(target.workspaceId),
        };
        break;
      }
      case "metadata.clear_status": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        useMetadataStore
          .getState()
          .clearStatus(
            target.workspaceId,
            (params.key as string | undefined) ?? undefined,
          );
        result = { result: true };
        break;
      }
      case "metadata.set_progress": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        useMetadataStore.getState().setProgress(target.workspaceId, {
          key: params.key as string,
          label: params.label as string,
          value: params.value as number,
          total: (params.total as number | undefined) ?? undefined,
        });
        result = { result: true };
        break;
      }
      case "metadata.clear_progress": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        useMetadataStore
          .getState()
          .clearProgress(
            target.workspaceId,
            (params.key as string | undefined) ?? undefined,
          );
        result = { result: true };
        break;
      }
      case "metadata.log": {
        const target = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          fallbackActive: true,
        });
        if (!target.workspaceId) {
          result = { error: target.error ?? "Workspace not found" };
          break;
        }
        useMetadataStore.getState().appendLog(target.workspaceId, {
          level: ((params.level as string) || "info") as
            | "info"
            | "warn"
            | "error",
          message: params.message as string,
        });
        result = { result: true };
        break;
      }
      case "system.ping": {
        result = { result: "pong" };
        break;
      }
      case "worktree.create": {
        const name = params.name as string;
        const layout =
          (params.layout as string) ||
          config?.general.worktree_layout ||
          undefined;
        try {
          const info = await worktreeCreate(name, layout);
          const wsId = state.createWorktreeWorkspace(
            info.name,
            info.path,
            info.branch,
            info.path,
            info.name,
          );
          worktreeRunHook(info.path, "setup").catch(logError);
          result = { result: { id: wsId, ...info } };
        } catch (err) {
          result = { error: String(err) };
        }
        break;
      }
      case "worktree.merge": {
        const name = params.name as string;
        try {
          const msg = await worktreeMerge(name);
          result = { result: msg };
        } catch (err) {
          result = { error: String(err) };
        }
        break;
      }
      case "worktree.remove": {
        const name = params.name as string;
        try {
          await worktreeRemove(name);
          // Close the workspace associated with this worktree
          const latestState = useWorkspaceStore.getState();
          const target = latestState.workspaceOrder.find(
            (wsId) => latestState.workspaces[wsId]?.worktreeName === name,
          );
          if (target) closeWorkspaceEnsuringOneRemains(target);
          result = { result: true };
        } catch (err) {
          result = { error: String(err) };
        }
        break;
      }
      default:
        result = { error: `Unknown method: ${method}` };
    }
  } catch (err) {
    result = { error: String(err) };
  }

  socketRespond(id, result).catch(logError);
}
