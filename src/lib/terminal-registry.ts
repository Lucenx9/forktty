import type { Terminal } from "@xterm/xterm";
import type { FitAddon } from "@xterm/addon-fit";
import type { CanvasAddon } from "@xterm/addon-canvas";
import type { SearchAddon } from "@xterm/addon-search";

// --- Active terminal map (for readScreen / socket API) ---

const terminalMap = new Map<string, Terminal>();

export function registerTerminal(paneId: string, terminal: Terminal): void {
  terminalMap.set(paneId, terminal);
}

export function unregisterTerminal(paneId: string): void {
  terminalMap.delete(paneId);
}

export function readScreen(paneId?: string): string | null {
  const terminal = paneId
    ? terminalMap.get(paneId)
    : terminalMap.values().next().value;
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

export interface SavedTerminalInstance {
  terminal: Terminal;
  wrapper: HTMLDivElement;
  ptyId: number | null;
  fitAddon: FitAddon;
  canvasAddon: CanvasAddon | null;
  searchAddon: SearchAddon;
}

const savedInstances = new Map<string, SavedTerminalInstance>();

export function saveInstance(
  surfaceId: string,
  instance: SavedTerminalInstance,
): void {
  savedInstances.set(surfaceId, instance);
}

export function getSavedInstance(
  surfaceId: string,
): SavedTerminalInstance | undefined {
  return savedInstances.get(surfaceId);
}

export function removeSavedInstance(surfaceId: string): void {
  savedInstances.delete(surfaceId);
}
