import { invoke, Channel } from "@tauri-apps/api/core";

/**
 * Spawn a new PTY and start streaming output.
 * Returns the PTY id. Calls onOutput with decoded binary data from the PTY.
 * Calls onExit when the shell process exits.
 */
export function spawnPty(
  onOutput: (data: Uint8Array) => void,
  onExit: () => void,
): Promise<number> {
  const onOutputChannel = new Channel<string>();

  onOutputChannel.onmessage = (message: string) => {
    if (message === "__EOF__") {
      onExit();
      return;
    }
    if (message.startsWith("__ERROR__:")) {
      console.error("PTY error:", message.slice(10));
      onExit();
      return;
    }
    // Decode base64 to binary
    const binary = atob(message);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    onOutput(bytes);
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
