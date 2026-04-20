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

interface ScanEventNotification {
  event_type: "notification";
  title: string;
  body: string;
}

type ScanEventData =
  | ScanEventPrompt
  | ScanEventCommandStarted
  | ScanEventCommandFinished
  | ScanEventNotification;

type PtyEvent = PtyEventOutput | PtyEventEof | PtyEventError | PtyEventScan;

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: unknown;
};

export function hasTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  return typeof (window as TauriWindow).__TAURI_INTERNALS__ !== "undefined";
}

function isMissingTauriRuntimeError(err: unknown): boolean {
  const message = String(err);
  return (
    message.includes("__TAURI_INTERNALS__") ||
    message.includes("reading 'invoke'") ||
    message.includes('reading "invoke"') ||
    message.includes("window is not defined")
  );
}

function browserFallbackCwd(): string {
  if (typeof window === "undefined") return "";
  return "";
}

/**
 * Spawn a new PTY and start streaming output.
 * Returns the PTY id. Calls onOutput with decoded binary data from the PTY.
 * Calls onExit when the shell process exits.
 * Calls onScanEvent when the output scanner detects a prompt or command event.
 * Optional cwd sets the working directory for the shell.
 */
export function spawnPty(opts: {
  onOutput: (data: string | Uint8Array) => void;
  onExit: () => void;
  cwd?: string;
  workspaceId?: string;
  surfaceId?: string;
  cols?: number;
  rows?: number;
  onScanEvent?: (event: ScanEventData) => void;
}): Promise<number> {
  const onOutputChannel = new Channel<PtyEvent>();

  onOutputChannel.onmessage = (event: PtyEvent) => {
    switch (event.kind) {
      case "Output": {
        const binary = atob(event.data);
        const bytes = new Uint8Array(binary.length);
        for (let i = 0; i < binary.length; i++) {
          bytes[i] = binary.charCodeAt(i);
        }
        opts.onOutput(bytes);
        break;
      }
      case "Eof":
        opts.onExit();
        break;
      case "Error":
        writeLog("ERROR", `PTY error: ${event.data}`).catch(logError);
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
    cols: opts.cols ?? null,
    rows: opts.rows ?? null,
  }).catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      throw new Error("PTY spawn is only available inside the Tauri app");
    }
    throw err;
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
export function resizePty(id: number, cols: number, rows: number): Promise<void> {
  return invoke("pty_resize", { id, cols, rows });
}

/**
 * Kill a PTY process.
 */
export function killPty(id: number): Promise<void> {
  return invoke("pty_kill", { id });
}

/**
 * Get the current working directory of a PTY shell process.
 */
export function getPtyCwd(id: number): Promise<string> {
  return invoke<string>("pty_get_cwd", { id });
}

/**
 * Get the current git branch for a directory.
 */
export function getGitBranch(cwd: string): Promise<string> {
  return invoke<string>("get_git_branch", { cwd }).catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      return "";
    }
    throw err;
  });
}

/**
 * Get the app's current working directory.
 */
export function getCwd(): Promise<string> {
  return invoke<string>("get_cwd").catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      return browserFallbackCwd();
    }
    throw err;
  });
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

export function signalFrontendReady(): Promise<void> {
  return invoke("socket_frontend_ready");
}

// --- Notification commands ---

/**
 * Send a desktop notification via notify-rust (XDG/D-Bus).
 */
export function sendDesktopNotification(
  title: string,
  body: string,
  playSound = true,
): Promise<void> {
  return invoke("send_desktop_notification", { title, body, playSound });
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
  worktree_name: string;
}

export interface BranchInfo {
  name: string;
  is_head: boolean;
  last_commit_time: number;
  last_commit_summary: string;
}

export function gitListBranches(cwd?: string): Promise<BranchInfo[]> {
  return invoke<BranchInfo[]>("git_list_branches", { cwd: cwd ?? null });
}

export function worktreeAttach(
  branchName: string,
  layout?: string,
  cwd?: string,
): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>("worktree_attach", {
    branchName,
    layout: layout ?? null,
    cwd: cwd ?? null,
  });
}

export function worktreeCreate(
  name: string,
  layout?: string,
  cwd?: string,
): Promise<WorktreeInfo> {
  return invoke<WorktreeInfo>("worktree_create", {
    name,
    layout: layout ?? null,
    cwd: cwd ?? null,
  });
}

export function worktreeList(cwd?: string): Promise<WorktreeInfo[]> {
  return invoke<WorktreeInfo[]>("worktree_list", { cwd: cwd ?? null });
}

export function worktreeRemove(name: string, cwd?: string): Promise<string> {
  return invoke<string>("worktree_remove", { name, cwd: cwd ?? null });
}

export function worktreeMerge(name: string, cwd?: string): Promise<string> {
  return invoke<string>("worktree_merge", { name, cwd: cwd ?? null });
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
    theme_source: string;
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
  focused_leaf_index: number;
}

export interface SessionData {
  version?: number;
  workspaces: WorkspaceSnapshot[];
  active_workspace_index: number;
}

export function saveSession(data: SessionData): Promise<void> {
  return invoke<void>("save_session", { data }).catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      return;
    }
    throw err;
  });
}

export function loadSession(): Promise<SessionData | null> {
  return invoke<SessionData | null>("load_session").catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      return null;
    }
    throw err;
  });
}

// --- Logging ---

export function writeLog(level: string, message: string): Promise<void> {
  return invoke<void>("write_log", { level, message }).catch((err) => {
    if (isMissingTauriRuntimeError(err)) {
      const prefix = `[ForkTTY/${level}]`;
      if (level === "ERROR") {
        console.error(prefix, message);
      } else {
        console.log(prefix, message);
      }
      return;
    }
    throw err;
  });
}

/** Drop-in replacement for console.error in .catch() chains */
export function logError(err: unknown): void {
  writeLog("ERROR", String(err)).catch(() => {});
}

// --- Tray commands ---

export function updateTrayTooltip(count: number): Promise<void> {
  return invoke("update_tray_tooltip", { count });
}

export type { ScanEventData };
