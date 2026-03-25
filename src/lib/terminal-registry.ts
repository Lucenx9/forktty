import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import type { SearchAddon } from "@xterm/addon-search";

// --- Active terminal map (for readScreen / socket API) ---

const terminalMap = new Map<string, Terminal>();

export function registerTerminal(paneId: string, terminal: Terminal): void {
  terminalMap.set(paneId, terminal);
}

export function unregisterTerminal(paneId: string): void {
  terminalMap.delete(paneId);
}

export function readScreen(paneId?: string | null): string | null {
  if (!paneId) return null;

  const terminal = terminalMap.get(paneId);
  if (!terminal) return null;

  const buffer = terminal.buffer.active;
  const lines: string[] = [];
  for (let i = 0; i < buffer.length; i++) {
    const line = buffer.getLine(i);
    if (line) {
      lines.push(line.translateToString(true));
    }
  }

  while (lines.length > 0 && lines[lines.length - 1]!.trim() === "") {
    lines.pop();
  }

  return lines.join("\n");
}

// --- Saved terminal instances (survive React unmount during swap/split) ---

export interface SavedTerminalRuntime {
  ptyId: number | null;
  lastCols: number | null;
  lastRows: number | null;
}

export interface SavedTerminalInstance {
  terminal: Terminal;
  wrapper: HTMLDivElement;
  runtime: SavedTerminalRuntime;
  fitAddon: FitAddon;
  searchAddon: SearchAddon;
}

const savedInstances = new Map<string, SavedTerminalInstance>();

export function saveInstance(surfaceId: string, instance: SavedTerminalInstance): void {
  savedInstances.set(surfaceId, instance);
}

export function getSavedInstance(surfaceId: string): SavedTerminalInstance | undefined {
  return savedInstances.get(surfaceId);
}

export function removeSavedInstance(surfaceId: string): void {
  savedInstances.delete(surfaceId);
}

// --- Reconciliation: clean up orphaned instances ---

/**
 * Remove terminal and saved instances that no longer correspond to active
 * surface IDs.  Call periodically to prevent memory leaks when React cleanup
 * is missed (e.g. hot reload, crash recovery).
 */
export function reconcileInstances(activeSurfaceIds: Set<string>): number {
  let cleaned = 0;

  for (const id of savedInstances.keys()) {
    if (!activeSurfaceIds.has(id)) {
      const instance = savedInstances.get(id);
      if (instance) {
        instance.terminal.dispose();
        instance.wrapper.remove();
      }
      savedInstances.delete(id);
      cleaned++;
    }
  }

  for (const id of terminalMap.keys()) {
    if (!activeSurfaceIds.has(id)) {
      terminalMap.delete(id);
      cleaned++;
    }
  }

  return cleaned;
}

let reconcileTimer: ReturnType<typeof setInterval> | null = null;

/**
 * Start periodic reconciliation.  The `getActiveSurfaceIds` callback is
 * called each cycle to read the current set of surface IDs from the store.
 */
export function startReconciliation(
  getActiveSurfaceIds: () => Set<string>,
  intervalMs = 30_000,
): void {
  stopReconciliation();
  reconcileTimer = setInterval(() => {
    reconcileInstances(getActiveSurfaceIds());
  }, intervalMs);
}

export function stopReconciliation(): void {
  if (reconcileTimer !== null) {
    clearInterval(reconcileTimer);
    reconcileTimer = null;
  }
}
