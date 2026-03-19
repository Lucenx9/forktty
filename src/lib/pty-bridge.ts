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
export function spawnPty(opts: {
  onOutput: (data: Uint8Array) => void;
  onExit: () => void;
  cwd?: string;
  workspaceId?: string;
  surfaceId?: string;
  onScanEvent?: (event: ScanEventData) => void;
}): Promise<number> {
  const onOutputChannel = new Channel<PtyEvent>();

  onOutputChannel.onmessage = (event: PtyEvent) => {
    switch (event.kind) {
      case "Output": {
        const binary = atob(event.data);
        const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
        opts.onOutput(bytes);
        break;
      }
      case "Eof":
        opts.onExit();
        break;
      case "Error":
        console.error("PTY error:", event.data);
        opts.onExit();
        break;
      case "Scan":
        if (opts.onScanEvent) {
          opts.onScanEvent(event.data);
        }
        break;
    }
  };

  return invoke<number>("pty_spawn", {
    onOutput: onOutputChannel,
    cwd: opts.cwd ?? null,
    workspaceId: opts.workspaceId ?? null,
    surfaceId: opts.surfaceId ?? null,
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

// --- Socket bridge ---

/**
 * Respond to a socket API request bridged from the backend.
 */
export function socketRespond(id: string, result: unknown): Promise<void> {
  return invoke("socket_respond", { id, result });
}

/**
 * Get the socket path used by the app.
 */
export function getSocketPath(): Promise<string> {
  return invoke<string>("get_socket_path");
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

// --- Config commands ---

export interface AppConfig {
  general: {
    theme: string;
    shell: string;
    worktree_layout: string;
    notification_command: string;
  };
  appearance: {
    font_family: string;
    font_size: number;
    sidebar_position: string;
  };
  notifications: {
    desktop: boolean;
    sound: boolean;
    idle_threshold_ms: number;
  };
}

export interface TerminalTheme {
  background: string | null;
  foreground: string | null;
  cursor: string | null;
  selection_background: string | null;
  selection_foreground: string | null;
  black: string | null;
  red: string | null;
  green: string | null;
  yellow: string | null;
  blue: string | null;
  magenta: string | null;
  cyan: string | null;
  white: string | null;
  bright_black: string | null;
  bright_red: string | null;
  bright_green: string | null;
  bright_yellow: string | null;
  bright_blue: string | null;
  bright_magenta: string | null;
  bright_cyan: string | null;
  bright_white: string | null;
  font_family: string | null;
  font_size: number | null;
}

export function getConfig(): Promise<AppConfig> {
  return invoke<AppConfig>("get_config");
}

export function saveConfig(configData: AppConfig): Promise<void> {
  return invoke("save_config", { configData });
}

export function getTheme(): Promise<TerminalTheme> {
  return invoke<TerminalTheme>("get_theme");
}

// --- Session commands ---

export interface PaneTreeSnapshot {
  type: "leaf" | "horizontal" | "vertical";
  children?: PaneTreeSnapshot[];
  sizes?: number[];
}

export interface WorkspaceSnapshot {
  name: string;
  working_dir: string;
  git_branch: string;
  worktree_dir: string;
  worktree_name: string;
  pane_tree: PaneTreeSnapshot;
}

export interface SessionData {
  workspaces: WorkspaceSnapshot[];
  active_workspace_index: number;
}

export function saveSession(data: SessionData): Promise<void> {
  return invoke("save_session", { data });
}

export function loadSession(): Promise<SessionData | null> {
  return invoke<SessionData | null>("load_session");
}

// --- Logging ---

export function writeLog(level: string, message: string): Promise<void> {
  return invoke("write_log", { level, message });
}

export type { ScanEventData };
