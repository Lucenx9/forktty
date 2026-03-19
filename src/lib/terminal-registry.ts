import type { Terminal } from "@xterm/xterm";

const terminalMap = new Map<string, Terminal>();

export function registerTerminal(paneId: string, terminal: Terminal): void {
  terminalMap.set(paneId, terminal);
}

export function unregisterTerminal(paneId: string): void {
  terminalMap.delete(paneId);
}

export function readScreen(paneId?: string): string | null {
  // If paneId specified, read that terminal; otherwise read the first one found
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

  // Trim trailing empty lines
  while (lines.length > 0 && lines[lines.length - 1]!.trim() === "") {
    lines.pop();
  }

  return lines.join("\n");
}
