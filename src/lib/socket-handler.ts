import {
  useWorkspaceStore,
  closeWorkspaceEnsuringOneRemains,
  type AppNotificationKind,
} from "../stores/workspace";
import { useMetadataStore } from "../stores/metadata";
import { writePty, socketRespond, logError } from "./pty-bridge";
import { dispatchWorkspaceNotification } from "./notification-dispatch";
import {
  createWorkspaceWithCwd,
  createWorkspaceWithInheritedCwd,
  splitPaneWithInheritedCwd,
} from "./workspace-launch";
import { readScreen } from "./terminal-registry";

type WorkspaceStoreState = ReturnType<typeof useWorkspaceStore.getState>;
const MAX_SOCKET_KEY_LENGTH = 128;
const MAX_SOCKET_LABEL_LENGTH = 160;
const MAX_SOCKET_NAME_LENGTH = 512;
const MAX_SOCKET_SHORT_TEXT_LENGTH = 200;
const MAX_SOCKET_BODY_LENGTH = 16_384;
const MAX_SOCKET_PATH_LENGTH = 4096;
const NOTIFICATION_KINDS: ReadonlySet<AppNotificationKind> = new Set([
  "prompt",
  "error",
  "info",
  "custom",
]);

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

function getDefinedParam(params: Record<string, unknown>, ...keys: string[]): unknown {
  for (const key of keys) {
    if (Object.prototype.hasOwnProperty.call(params, key)) {
      return params[key];
    }
  }
  return undefined;
}

function readOptionalStringParam(
  params: Record<string, unknown>,
  keys: string[],
  label: string,
  options: {
    allowEmpty?: boolean;
    trim?: boolean;
    maxLength?: number;
  } = {},
): string | undefined {
  const value = getDefinedParam(params, ...keys);
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== "string") {
    throw new Error(`Invalid parameter "${label}": expected string`);
  }

  const normalized = options.trim ? value.trim() : value;
  if (!options.allowEmpty && normalized.length === 0) {
    return undefined;
  }
  if (options.maxLength && normalized.length > options.maxLength) {
    throw new Error(
      `Invalid parameter "${label}": exceeds ${options.maxLength} characters`,
    );
  }
  return normalized;
}

function requireStringParam(
  params: Record<string, unknown>,
  keys: string[],
  label: string,
  options: {
    trim?: boolean;
    maxLength?: number;
  } = {},
): string {
  const value = readOptionalStringParam(params, keys, label, {
    trim: options.trim,
    maxLength: options.maxLength,
  });
  if (value === undefined) {
    throw new Error(`Missing required parameter: ${label}`);
  }
  return value;
}

function readOptionalFiniteNumberParam(
  params: Record<string, unknown>,
  keys: string[],
  label: string,
): number | undefined {
  const value = getDefinedParam(params, ...keys);
  if (value === undefined || value === null) {
    return undefined;
  }
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`Invalid parameter "${label}": expected finite number`);
  }
  return value;
}

function requireFiniteNumberParam(
  params: Record<string, unknown>,
  keys: string[],
  label: string,
): number {
  const value = readOptionalFiniteNumberParam(params, keys, label);
  if (value === undefined) {
    throw new Error(`Missing required parameter: ${label}`);
  }
  return value;
}

function readNotificationKindParam(
  params: Record<string, unknown>,
): AppNotificationKind | undefined {
  const kind = readOptionalStringParam(params, ["kind"], "kind", {
    trim: true,
    maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
  });
  if (!kind) {
    return undefined;
  }
  if (!NOTIFICATION_KINDS.has(kind as AppNotificationKind)) {
    throw new Error(
      `Invalid parameter "kind": expected one of ${Array.from(NOTIFICATION_KINDS).join(", ")}`,
    );
  }
  return kind as AppNotificationKind;
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
  return Object.values(workspace.surfaces).find((s) => s.ptyId != null)?.ptyId ?? null;
}

function waitForWorkspacePty(workspaceId: string, timeoutMs = 5000): Promise<number> {
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
        const name = readOptionalStringParam(params, ["name"], "name", {
          trim: true,
          maxLength: MAX_SOCKET_NAME_LENGTH,
        });
        const prompt = readOptionalStringParam(params, ["prompt"], "prompt", {
          maxLength: MAX_SOCKET_BODY_LENGTH,
        });
        const worktreeDir =
          readOptionalStringParam(params, ["worktreeDir"], "worktreeDir", {
            trim: true,
            maxLength: MAX_SOCKET_PATH_LENGTH,
          }) ?? "";
        const worktreeName =
          readOptionalStringParam(params, ["worktreeName"], "worktreeName", {
            trim: true,
            maxLength: MAX_SOCKET_NAME_LENGTH,
          }) ?? "";
        const requestedWorkingDir = readOptionalStringParam(
          params,
          ["workingDir"],
          "workingDir",
          {
            trim: true,
            maxLength: MAX_SOCKET_PATH_LENGTH,
          },
        );
        const workingDir = requestedWorkingDir ?? worktreeDir;
        const gitBranch =
          readOptionalStringParam(params, ["gitBranch"], "gitBranch", {
            trim: true,
            maxLength: MAX_SOCKET_NAME_LENGTH,
          }) ?? "";
        const isWorktree = worktreeDir.length > 0;

        const wsId = isWorktree
          ? state.createWorktreeWorkspace(
              name ?? worktreeName,
              workingDir,
              gitBranch,
              worktreeDir,
              worktreeName || name || "",
            )
          : requestedWorkingDir
            ? await createWorkspaceWithCwd(name ?? undefined, requestedWorkingDir)
            : await createWorkspaceWithInheritedCwd(name ?? undefined);

        const response: Record<string, unknown> = { id: wsId };
        if (prompt) {
          try {
            const ptyId = await waitForWorkspacePty(wsId);
            response.pty_id = ptyId;
            await writePty(ptyId, prompt);
          } catch (err) {
            useWorkspaceStore.getState().closeWorkspace(wsId);
            throw err;
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
          if (latestState.workspaceOrder.length <= 1 && targetWorkspace?.worktreeName) {
            closeWorkspaceEnsuringOneRemains(
              target.workspaceId,
              getStringParam(params, "fallbackWorkingDir", "fallback_working_dir") ??
                undefined,
            );
            result = { result: true };
          } else if (latestState.workspaceOrder.length <= 1) {
            result = { error: "Cannot close the last workspace" };
          } else {
            latestState.closeWorkspace(target.workspaceId);
            result = { result: true };
          }
        } else {
          result = { error: target.error ?? "Workspace not found" };
        }
        break;
      }
      case "surface.list": {
        const listTarget = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          worktreeName: getStringParam(params, "worktreeName", "worktree_name"),
          fallbackActive: true,
        });
        const listWs = listTarget.workspaceId
          ? state.workspaces[listTarget.workspaceId]
          : null;
        if (listWs) {
          result = {
            result: Object.values(listWs.surfaces).map((s) => ({
              id: s.id,
              ptyId: s.ptyId,
              title: s.title,
            })),
          };
        } else {
          result = { error: listTarget.error ?? "Workspace not found" };
        }
        break;
      }
      case "surface.split": {
        const splitSurfaceId = getStringParam(params, "surface_id", "surfaceId");
        let splitPaneId: string | null = null;
        if (splitSurfaceId) {
          // Verify the surface exists in some workspace
          for (const ws of Object.values(state.workspaces)) {
            if (ws.surfaces[splitSurfaceId]) {
              splitPaneId = splitSurfaceId;
              break;
            }
          }
          if (!splitPaneId) {
            result = { error: `Surface "${splitSurfaceId}" not found` };
            break;
          }
        } else {
          const ws = state.workspaces[state.activeWorkspaceId];
          splitPaneId = ws?.focusedPaneId ?? null;
        }
        if (splitPaneId) {
          const direction =
            readOptionalStringParam(params, ["direction"], "direction", {
              trim: true,
              maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
            }) ?? "right";
          if (direction !== "right" && direction !== "down") {
            result = {
              error: 'Invalid parameter "direction": expected "right" or "down"',
            };
            break;
          }
          const dir = direction === "down" ? "vertical" : "horizontal";
          await splitPaneWithInheritedCwd(
            splitPaneId,
            dir as "horizontal" | "vertical",
          );
          result = { result: true };
        } else {
          result = { error: "No active workspace" };
        }
        break;
      }
      case "surface.close": {
        const closeSurfaceId = getStringParam(params, "surface_id", "surfaceId");
        if (closeSurfaceId) {
          // Find which workspace owns this surface
          const ownerWsId = Object.entries(state.workspaces).find(
            ([, ws]) => ws.surfaces[closeSurfaceId],
          )?.[0];
          if (!ownerWsId) {
            result = { error: `Surface "${closeSurfaceId}" not found` };
            break;
          }
          // closePane only works on the active workspace, so switch if needed
          const latestState = useWorkspaceStore.getState();
          if (ownerWsId !== latestState.activeWorkspaceId) {
            latestState.switchWorkspace(ownerWsId);
          }
          useWorkspaceStore.getState().closePane(closeSurfaceId);
          result = { result: true };
        } else {
          const ws = state.workspaces[state.activeWorkspaceId];
          if (ws) {
            state.closePane(ws.focusedPaneId);
            result = { result: true };
          } else {
            result = { error: "No active workspace" };
          }
        }
        break;
      }
      case "surface.send_text": {
        const surfaceId = readOptionalStringParam(
          params,
          ["surface_id", "surfaceId"],
          "surface_id",
          {
            trim: true,
            maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
          },
        );
        const text = requireStringParam(params, ["text"], "text", {
          maxLength: MAX_SOCKET_BODY_LENGTH,
        });
        if (surfaceId) {
          let targetPtyId: number | null = null;
          for (const ws of Object.values(state.workspaces)) {
            const surface = ws.surfaces[surfaceId];
            if (surface?.ptyId != null) {
              targetPtyId = surface.ptyId;
              break;
            }
          }
          if (targetPtyId != null) {
            await writePty(targetPtyId, text);
            result = { result: true };
          } else {
            result = { error: "Surface not found" };
          }
        } else if (getDefinedParam(params, "pty_id") !== undefined) {
          const ptyId = requireFiniteNumberParam(params, ["pty_id"], "pty_id");
          if (!Number.isInteger(ptyId) || ptyId < 0) {
            result = {
              error: 'Invalid parameter "pty_id": expected non-negative integer',
            };
            break;
          }
          await writePty(ptyId, text);
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
        const notifTarget = resolveWorkspaceId(state, {
          workspaceId: getStringParam(params, "workspace_id", "workspaceId"),
          workspaceName: getStringParam(params, "workspace_name", "workspaceName"),
          worktreeName: getStringParam(params, "worktreeName", "worktree_name"),
          fallbackActive: true,
        });
        if (!notifTarget.workspaceId) {
          result = { error: notifTarget.error ?? "Workspace not found" };
          break;
        }
        const title =
          readOptionalStringParam(params, ["title"], "title", {
            trim: true,
            allowEmpty: true,
            maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
          }) || "ForkTTY";
        const body =
          readOptionalStringParam(params, ["body"], "body", {
            allowEmpty: true,
            maxLength: MAX_SOCKET_BODY_LENGTH,
          }) ?? "";
        const kind = readNotificationKindParam(params) ?? "info";
        dispatchWorkspaceNotification({
          workspaceId: notifTarget.workspaceId,
          title,
          body,
          kind,
        });
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
          key: requireStringParam(params, ["key"], "key", {
            trim: true,
            maxLength: MAX_SOCKET_KEY_LENGTH,
          }),
          label: requireStringParam(params, ["label"], "label", {
            trim: true,
            maxLength: MAX_SOCKET_LABEL_LENGTH,
          }),
          value: requireStringParam(params, ["value"], "value", {
            maxLength: MAX_SOCKET_BODY_LENGTH,
          }),
          color: readOptionalStringParam(params, ["color"], "color", {
            trim: true,
            maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
          }),
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
        useMetadataStore.getState().clearStatus(
          target.workspaceId,
          readOptionalStringParam(params, ["key"], "key", {
            trim: true,
            maxLength: MAX_SOCKET_KEY_LENGTH,
          }),
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
        const progressValue = Math.max(
          0,
          requireFiniteNumberParam(params, ["value"], "value"),
        );
        const progressTotal = readOptionalFiniteNumberParam(params, ["total"], "total");
        if (progressTotal !== undefined && progressTotal <= 0) {
          result = { error: 'Invalid parameter "total": expected positive number' };
          break;
        }
        useMetadataStore.getState().setProgress(target.workspaceId, {
          key: requireStringParam(params, ["key"], "key", {
            trim: true,
            maxLength: MAX_SOCKET_KEY_LENGTH,
          }),
          label: requireStringParam(params, ["label"], "label", {
            trim: true,
            maxLength: MAX_SOCKET_LABEL_LENGTH,
          }),
          value:
            progressTotal !== undefined
              ? Math.min(progressValue, progressTotal)
              : progressValue,
          total: progressTotal,
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
        useMetadataStore.getState().clearProgress(
          target.workspaceId,
          readOptionalStringParam(params, ["key"], "key", {
            trim: true,
            maxLength: MAX_SOCKET_KEY_LENGTH,
          }),
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
        const level =
          readOptionalStringParam(params, ["level"], "level", {
            trim: true,
            maxLength: MAX_SOCKET_SHORT_TEXT_LENGTH,
          }) ?? "info";
        if (level !== "info" && level !== "warn" && level !== "error") {
          result = {
            error: 'Invalid parameter "level": expected "info", "warn", or "error"',
          };
          break;
        }
        useMetadataStore.getState().appendLog(target.workspaceId, {
          level,
          message: requireStringParam(params, ["message"], "message", {
            maxLength: MAX_SOCKET_BODY_LENGTH,
          }),
        });
        result = { result: true };
        break;
      }
      case "system.ping": {
        result = { result: "pong" };
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
