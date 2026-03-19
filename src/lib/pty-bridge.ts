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
 * Optional cwd sets the working directory for the shell.
 */
export function spawnPty(
  onOutput: (data: Uint8Array) => void,
  onExit: () => void,
  cwd?: string,
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
 * Returns empty string if not a git repo.
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

// --- Worktree commands ---

export interface WorktreeInfo {
  name: string;
  path: string;
  branch: string;
}

/**
 * Create a new git worktree with a branch.
 * Layout: "nested" (.worktrees/<name>), "sibling", "outer-nested".
 */
export function worktreeCreate(
  name: string,
  layout?: string,
): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>("worktree_create", {
    name,
    layout: layout ?? null,
  });
}

/**
 * List all git worktrees.
 */
export function worktreeList(): Promise<WorktreeInfo[]> {
  return invoke<WorktreeInfo[]>("worktree_list");
}

/**
 * Remove a git worktree and delete its branch.
 * Runs .forktty/teardown hook if present.
 */
export function worktreeRemove(name: string): Promise<void> {
  return invoke("worktree_remove", { name });
}

/**
 * Merge a worktree's branch into the main checkout's current branch.
 */
export function worktreeMerge(name: string): Promise<string> {
  return invoke<string>("worktree_merge", { name });
}

/**
 * Get worktree status: "clean", "dirty", or "conflicts".
 */
export function worktreeStatus(path: string): Promise<string> {
  return invoke<string>("worktree_status", { path });
}

/**
 * Run a hook (.forktty/setup or .forktty/teardown) in a worktree.
 * Returns exit code or null if hook doesn't exist.
 */
export function worktreeRunHook(
  worktreePath: string,
  hookName: string,
): Promise<number | null> {
  return invoke<number | null>("worktree_run_hook", {
    worktreePath,
    hookName,
  });
}
