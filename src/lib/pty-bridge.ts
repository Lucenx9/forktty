import { invoke, Channel } from "@tauri-apps/api/core";

interface PtyEventOutput {
  kind: "Output";
  data: string;
}

interface PtyEventEof {
  kind: "Eof";
}

interface PtyEventError {
  kind: "Error";
  data: string;
}

type PtyEvent = PtyEventOutput | PtyEventEof | PtyEventError;

/**
 * Spawn a new PTY and start streaming output.
 * Returns the PTY id. Calls onOutput with decoded binary data from the PTY.
 * Calls onExit when the shell process exits.
 */
export function spawnPty(
  onOutput: (data: Uint8Array) => void,
  onExit: () => void,
): Promise<number> {
  const onOutputChannel = new Channel<PtyEvent>();

  onOutputChannel.onmessage = (event: PtyEvent) => {
    switch (event.kind) {
      case "Output": {
        const binary = atob(event.data);
        const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
        onOutput(bytes);
        break;
      }
      case "Eof":
        onExit();
        break;
      case "Error":
        console.error("PTY error:", event.data);
        onExit();
        break;
    }
  };

  return invoke<number>("pty_spawn", { onOutput: onOutputChannel });
}

/**
 * Write user input to a PTY.
 */
export function writePty(id: number, data: string): Promise<void> {
  return invoke("pty_write", { id, data });
}

/**
 * Resize a PTY to new dimensions.
 */
export function resizePty(
  id: number,
  cols: number,
  rows: number,
): Promise<void> {
  return invoke("pty_resize", { id, cols, rows });
}

/**
 * Kill a PTY process.
 */
export function killPty(id: number): Promise<void> {
  return invoke("pty_kill", { id });
}
