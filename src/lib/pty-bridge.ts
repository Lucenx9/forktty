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

interface PtyEventScan {
  kind: "Scan";
  data: ScanEventData;
}

interface ScanEventPrompt {
  event_type: "prompt_detected";
}

interface ScanEventCommandStarted {
  event_type: "command_started";
}

interface ScanEventCommandFinished {
  event_type: "command_finished";
  exit_code: number | null;
}

type ScanEventData =
  | ScanEventPrompt
  | ScanEventCommandStarted
  | ScanEventCommandFinished;

type PtyEvent = PtyEventOutput | PtyEventEof | PtyEventError | PtyEventScan;

/**
 * Spawn a new PTY and start streaming output.
 * Returns the PTY id. Calls onOutput with decoded binary data from the PTY.
 * Calls onExit when the shell process exits.
 * Calls onScanEvent when the output scanner detects a prompt or command event.
 * Optional cwd sets the working directory for the shell.
 */
export function spawnPty(
  onOutput: (data: Uint8Array) => void,
  onExit: () => void,
  cwd?: string,
  onScanEvent?: (event: ScanEventData) => void,
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
      case "Scan":
        if (onScanEvent) {
          onScanEvent(event.data);
        }
        break;
    }
  };

  return invoke<number>("pty_spawn", {
    onOutput: onOutputChannel,
    cwd: cwd ?? null,
  });
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

/**
 * Get the current git branch for a directory.
 */
export function getGitBranch(cwd: string): Promise<string> {
  return invoke<string>("get_git_branch", { cwd });
}

/**
 * Get the app's current working directory.
 */
export function getCwd(): Promise<string> {
  return invoke<string>("get_cwd");
}

// --- Notification commands ---

/**
 * Send a desktop notification via notify-rust (XDG/D-Bus).
 */
export function sendDesktopNotification(
  title: string,
  body: string,
): Promise<void> {
  return invoke("send_desktop_notification", { title, body });
}

/**
 * Run a custom notification command with env vars.
 */
export function sendCustomNotification(
  command: string,
  title: string,
  body: string,
): Promise<void> {
  return invoke("send_custom_notification", { command, title, body });
}

// --- Worktree commands ---

export interface WorktreeInfo {
  name: string;
  path: string;
  branch: string;
}

export function worktreeCreate(
  name: string,
  layout?: string,
): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>("worktree_create", {
    name,
    layout: layout ?? null,
  });
}

export function worktreeList(): Promise<WorktreeInfo[]> {
  return invoke<WorktreeInfo[]>("worktree_list");
}

export function worktreeRemove(name: string): Promise<void> {
  return invoke("worktree_remove", { name });
}

export function worktreeMerge(name: string): Promise<string> {
  return invoke<string>("worktree_merge", { name });
}

export function worktreeStatus(path: string): Promise<string> {
  return invoke<string>("worktree_status", { path });
}

export function worktreeRunHook(
  worktreePath: string,
  hookName: string,
): Promise<number | null> {
  return invoke<number | null>("worktree_run_hook", {
    worktreePath,
    hookName,
  });
}

export type { ScanEventData };
