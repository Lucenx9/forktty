#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const HOOK_CONTINUE_RESPONSE = { continue: true, suppressOutput: false };
const HOOK_CONTINUE_JSON = `${JSON.stringify(HOOK_CONTINUE_RESPONSE)}\n`;
const HOOK_STATUS_TIMEOUT_MS = 5_000;
const SOCKET_TIMEOUT_MS = 5_000;
const HOOK_EVENT_ORDER_PARAM = "hook_event_order";
const HOOK_EVENT_CLOCK = "monotonic-ns";
const HOOK_TEST_STATUS_KEY_SUFFIX = "hook-test";
const SUPPORTED_HOOK_EVENTS = new Set([
  "notification",
  "post-tool",
  "pre-compact",
  "pre-tool",
  "prompt-submit",
  "session-end",
  "session-start",
  "stop",
  "stop-failure",
  "subagent-stop",
]);
const HOOK_TOOL_LABEL_MAX = 48;
const VALID_NOTIFICATION_KINDS = new Set(["prompt", "error", "info", "custom"]);
const VALID_STATUS_COLORS = new Set(["green", "yellow", "red", "blue", "muted"]);
const VALID_LOG_LEVELS = new Set(["info", "warn", "error"]);
const TARGET_SELECTOR_OPTION_NAMES = new Set([
  "workspace-id",
  "workspace-name",
  "worktree-name",
]);
const NOTIFICATION_OPTION_NAMES = new Set([
  ...TARGET_SELECTOR_OPTION_NAMES,
  "body",
  "kind",
  "title",
]);
const STATUS_OPTION_NAMES = new Set([
  ...TARGET_SELECTOR_OPTION_NAMES,
  "color",
  "key",
  "label",
  "value",
]);
const PROGRESS_OPTION_NAMES = new Set([
  ...TARGET_SELECTOR_OPTION_NAMES,
  "key",
  "label",
  "total",
  "value",
]);
const LOG_OPTION_NAMES = new Set([
  ...TARGET_SELECTOR_OPTION_NAMES,
  "level",
  "message",
]);
const HOOK_DEBUG_REDACT_KEYS = new Set([
  "body",
  "detail",
  "error",
  "last_assistant_message",
  "message",
  "prompt",
  "reason",
  "summary",
  "text",
]);
const CLEAR_METADATA_OPTION_NAMES = new Set([
  ...TARGET_SELECTOR_OPTION_NAMES,
  "key",
]);
const CREATE_WORKSPACE_OPTION_NAMES = new Set([
  "name",
  "working-dir",
]);
const SURFACE_ACTION_OPTION_NAMES = new Set(["surface-id"]);
const SURFACE_LIST_OPTION_NAMES = TARGET_SELECTOR_OPTION_NAMES;
const SURFACE_SPLIT_OPTION_NAMES = new Set(["axis", "surface-id"]);
const SEND_TEXT_OPTION_NAMES = new Set(["surface-id", "text"]);
const WORKTREE_COMMAND_OPTION_NAMES = new Set(["branch", "cwd", "name"]);
const WORKTREE_LIST_OPTION_NAMES = new Set(["cwd"]);
const WORKTREE_STATUS_OPTION_NAMES = new Set(["cwd", "path"]);
let fileNonceCounter = 0;

const AGENT_SPECS = {
  codex: {
    label: "Codex",
    disabledEnv: "FORKTTY_CODEX_HOOKS_DISABLED",
    configPath(env) {
      const codexHome =
        typeof env.CODEX_HOME === "string" && env.CODEX_HOME.trim().length > 0
          ? env.CODEX_HOME.trim()
          : path.join(homeDir(env), ".codex");
      return path.join(codexHome, "hooks.json");
    },
    hookEntries: [
      ["SessionStart", "session-start", 5000],
      ["UserPromptSubmit", "prompt-submit", 5000],
      ["PreToolUse", "pre-tool", 5000],
      ["PostToolUse", "post-tool", 5000],
      ["Stop", "stop", 5000],
    ],
  },
  claude: {
    label: "Claude",
    disabledEnv: "FORKTTY_CLAUDE_HOOKS_DISABLED",
    configPath(env) {
      const claudeDir =
        typeof env.CLAUDE_CONFIG_DIR === "string" && env.CLAUDE_CONFIG_DIR.trim().length > 0
          ? env.CLAUDE_CONFIG_DIR.trim()
          : path.join(homeDir(env), ".claude");
      return path.join(claudeDir, "settings.json");
    },
    hookEntries: [
      ["SessionStart", "session-start", 5],
      ["UserPromptSubmit", "prompt-submit", 5],
      ["PreToolUse", "pre-tool", 5],
      ["PostToolUse", "post-tool", 5],
      ["SubagentStop", "subagent-stop", 5],
      ["PreCompact", "pre-compact", 5],
      ["Stop", "stop", 5],
      ["Notification", "notification", 5],
      ["SessionEnd", "session-end", 5],
    ],
    matcher: "*",
  },
  gemini: {
    label: "Gemini",
    disabledEnv: "FORKTTY_GEMINI_HOOKS_DISABLED",
    configPath(env) {
      return path.join(homeDir(env), ".gemini", "settings.json");
    },
    hookEntries: [
      ["SessionStart", "session-start", 5000],
      ["BeforeAgent", "prompt-submit", 5000],
      ["BeforeTool", "pre-tool", 5000],
      ["AfterTool", "post-tool", 5000],
      ["AfterAgent", "stop", 5000],
      ["Notification", "notification", 5000],
      ["PreCompress", "pre-compact", 5000],
      ["SessionEnd", "session-end", 5000],
    ],
  },
};

function homeDir(env = process.env) {
  return typeof env.HOME === "string" && env.HOME.trim().length > 0
    ? env.HOME.trim()
    : os.homedir();
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function shellQuote(value) {
  return `'${String(value).replace(/'/g, `'\"'\"'`)}'`;
}

function trimEnv(env, key) {
  return typeof env[key] === "string" && env[key].trim() ? env[key].trim() : "";
}

function sanitizeForTerminal(value) {
  return String(value).replace(/[\u0000-\u001f\u007f-\u009f]/g, (char) => {
    switch (char) {
      case "\n":
        return "\\n";
      case "\r":
        return "\\r";
      case "\t":
        return "\\t";
      default:
        return `\\x${char.codePointAt(0).toString(16).padStart(2, "0")}`;
    }
  });
}

function terminalLine(message) {
  return `${sanitizeForTerminal(message)}\n`;
}

function formatErrorForTerminal(error) {
  return sanitizeForTerminal(error instanceof Error ? error.message : String(error));
}

function nextHookEventOrder() {
  if (process.hrtime && typeof process.hrtime.bigint === "function") {
    return process.hrtime.bigint().toString();
  }
  return String(Date.now());
}

function incrementHookEventOrder(order) {
  try {
    return (BigInt(order) + 1n).toString();
  } catch {
    return String(Date.now());
  }
}

function shortSha256(value) {
  return crypto.createHash("sha256").update(String(value)).digest("hex").slice(0, 16);
}

function defaultSocketPath(env = process.env) {
  const socketEnvPath = socketPathFromEnv(env);
  if (socketEnvPath) {
    return socketEnvPath;
  }

  if (typeof env.XDG_RUNTIME_DIR === "string") {
    const runtimeDir = env.XDG_RUNTIME_DIR.trim();
    if (path.isAbsolute(runtimeDir)) {
      return path.join(runtimeDir, "forktty.sock");
    }
  }

  const uid =
    typeof process.getuid === "function" ? String(process.getuid()) : env.UID || "unknown";
  return path.join(os.tmpdir(), `forktty-${uid}`, "forktty.sock");
}

function socketPathFromEnv(env = process.env) {
  if (typeof env.FORKTTY_SOCKET_PATH !== "string") return null;
  const socketPath = env.FORKTTY_SOCKET_PATH.trim();
  return socketPath && path.isAbsolute(socketPath) ? socketPath : null;
}

function nextRequestId() {
  return `cli-${Date.now().toString(36)}-${Math.random().toString(16).slice(2, 8)}`;
}

function nextFileNonce() {
  fileNonceCounter = (fileNonceCounter % Number.MAX_SAFE_INTEGER) + 1;
  return `${Date.now()}-${process.pid}-${fileNonceCounter}`;
}

async function readStdinText() {
  if (process.stdin.isTTY) return "";
  const chunks = [];
  for await (const chunk of process.stdin) {
    chunks.push(typeof chunk === "string" ? Buffer.from(chunk) : chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function readOptionalStdinJson() {
  const text = await readStdinText();
  const trimmed = text.trim();
  if (!trimmed) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return { raw: trimmed };
  }
}

async function sendSocketRequest(socketPath, method, params, timeoutMs = SOCKET_TIMEOUT_MS) {
  return new Promise((resolve, reject) => {
    const requestId = nextRequestId();
    const request = JSON.stringify({
      id: requestId,
      method,
      params,
    });
    const socket = net.createConnection(socketPath);
    let settled = false;
    let buffer = "";

    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      socket.destroy();
      reject(new Error(`Timed out waiting for ${method} response from ${socketPath}`));
    }, timeoutMs);

    function finish(err, value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (!socket.destroyed) {
        if (err) {
          socket.destroy();
        } else {
          socket.end();
        }
      }
      if (err) {
        reject(err);
      } else {
        resolve(value);
      }
    }

    function consumeLine(line) {
      if (!line.trim()) return;
      let response;
      try {
        response = JSON.parse(line);
      } catch (error) {
        finish(new Error(`Invalid socket response from ${socketPath} for ${method}: ${error}`));
        return;
      }

      if (response?.id !== requestId && !isConnectionLevelSocketError(response)) {
        const actualId = Object.prototype.hasOwnProperty.call(response ?? {}, "id")
          ? response.id
          : null;
        finish(
          new Error(
            `Socket response id mismatch for ${method} at ${socketPath}: expected ${requestId}, got ${JSON.stringify(actualId)}`,
          ),
        );
        return;
      }

      if (response?.ok === true) {
        finish(null, response.result);
        return;
      }

      const errorMessage =
        response?.error?.message ||
        response?.error ||
        `Socket request failed for ${method}`;
      const errorCode =
        response?.error && typeof response.error.code === "string"
          ? response.error.code
          : "";
      const errorPrefix = errorCode ? `${errorCode}: ` : "";
      const error = new Error(
        `Socket request failed for ${method} at ${socketPath}: ${errorPrefix}${String(errorMessage)}`,
      );
      if (errorCode) {
        error.code = errorCode;
      }
      finish(error);
    }

    function isConnectionLevelSocketError(response) {
      const code = response?.error?.code;
      return (
        response?.id === null &&
        response?.ok === false &&
        (code === "parse_error" || code === "request_too_large")
      );
    }

    socket.setEncoding("utf8");

    socket.on("connect", () => {
      socket.write(`${request}\n`);
    });

    socket.on("data", (chunk) => {
      buffer += chunk;
      let newlineIndex = buffer.indexOf("\n");
      while (newlineIndex >= 0) {
        const line = buffer.slice(0, newlineIndex);
        buffer = buffer.slice(newlineIndex + 1);
        consumeLine(line);
        newlineIndex = settled ? -1 : buffer.indexOf("\n");
      }
    });

    socket.on("error", (error) => {
      finish(formatSocketConnectError(error, socketPath));
    });

    socket.on("end", () => {
      if (settled) return;
      if (buffer.trim()) {
        consumeLine(buffer);
      } else {
        finish(new Error(`Socket closed without response for ${method} at ${socketPath}`));
      }
    });
  });
}

function formatSocketConnectError(error, socketPath) {
  const code =
    error && typeof error === "object" && typeof error.code === "string" ? error.code : "";
  if (code === "ENOENT" || code === "ECONNREFUSED") {
    const wrapped = new Error(
      `Cannot reach ForkTTY at ${socketPath} (${code}). Start the app with: cargo run -p forktty-ui-gtk --features gtk-vte. If ForkTTY is already running, set FORKTTY_SOCKET_PATH to an absolute path or pass --socket <path>.`,
    );
    wrapped.cause = error;
    return wrapped;
  }
  if (code === "EACCES" || code === "EPERM") {
    const wrapped = new Error(
      `Cannot access ForkTTY socket at ${socketPath} (${code}). Check the socket owner/permissions, or pass --socket <path>.`,
    );
    wrapped.cause = error;
    return wrapped;
  }
  const detail = error instanceof Error && error.message ? `: ${error.message}` : "";
  const wrapped = new Error(
    `ForkTTY socket error at ${socketPath}${code ? ` (${code})` : ""}${detail}`,
  );
  if (error instanceof Error) {
    wrapped.cause = error;
  }
  return wrapped;
}

function parseFlags(args, booleanOptions = new Set()) {
  const options = {};
  const positionals = [];

  for (let index = 0; index < args.length; index += 1) {
    const token = args[index];
    if (token === "--") {
      positionals.push(...args.slice(index + 1));
      break;
    }
    if (!token.startsWith("--")) {
      positionals.push(token);
      continue;
    }

    const raw = token.slice(2);

    const eqIndex = raw.indexOf("=");
    if (eqIndex >= 0) {
      const key = raw.slice(0, eqIndex);
      options[key] = raw.slice(eqIndex + 1);
      continue;
    }

    if (booleanOptions.has(raw)) {
      options[raw] = true;
      continue;
    }

    const next = args[index + 1];
    if (next !== undefined && !next.startsWith("--")) {
      options[raw] = next;
      index += 1;
    } else {
      options[raw] = true;
    }
  }

  return { options, positionals };
}

function rejectUnknownOptions(options, allowedOptions, commandName) {
  for (const key of Object.keys(options)) {
    if (!allowedOptions.has(key)) {
      throw new Error(`${commandName}: unknown option --${key}`);
    }
  }
}

function requireNoCommandArgs(args, commandName) {
  if (args.length > 0) {
    const arg = args[0] ? ` ${args[0]}` : "";
    throw new Error(`${commandName}: unexpected argument${arg}`);
  }
}

function buildTargetParams(options, env = process.env, commandName = "workspace target") {
  const selectors = targetSelectorValues(options);
  if (selectors.length > 1) {
    throw new Error(
      `${commandName}: cannot combine ${formatOptionNames(selectors.map(({ option }) => option))}`,
    );
  }
  const envWorkspaceId =
    typeof env.FORKTTY_WORKSPACE_ID === "string" ? env.FORKTTY_WORKSPACE_ID.trim() : "";

  if (selectors.length === 1) {
    const selector = selectors[0];
    return { [selector.field]: selector.value };
  }
  return envWorkspaceId ? { workspace_id: envWorkspaceId } : {};
}

function requireTargetSelectorOptions(options) {
  requireNonBlankStringOption(options, "workspace-id");
  requireNonBlankStringOption(options, "workspace-name");
  requireNonBlankStringOption(options, "worktree-name");
}

function targetSelectorValues(options) {
  requireTargetSelectorOptions(options);
  return [
    {
      option: "workspace-id",
      field: "workspace_id",
      value:
        typeof options["workspace-id"] === "string" ? options["workspace-id"].trim() : "",
    },
    {
      option: "workspace-name",
      field: "workspace_name",
      value:
        typeof options["workspace-name"] === "string" ? options["workspace-name"].trim() : "",
    },
    {
      option: "worktree-name",
      field: "worktreeName",
      value:
        typeof options["worktree-name"] === "string" ? options["worktree-name"].trim() : "",
    },
  ].filter(({ value }) => value);
}

function formatOptionNames(options) {
  const formatted = options.map((option) => `--${option}`);
  if (formatted.length <= 1) {
    return formatted[0] || "";
  }
  if (formatted.length === 2) {
    return `${formatted[0]} and ${formatted[1]}`;
  }
  return `${formatted.slice(0, -1).join(", ")}, and ${formatted[formatted.length - 1]}`;
}

function parseFiniteNumber(value, optionName) {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${optionName} requires a value`);
  }
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) {
    throw new Error(`Invalid ${optionName}: expected finite number`);
  }
  return parsed;
}

function requireStringOption(options, key, optionName = `--${key}`) {
  if (options[key] !== undefined && typeof options[key] !== "string") {
    throw new Error(`${optionName} requires a value`);
  }
}

function requireNonBlankStringOption(options, key, optionName = `--${key}`) {
  if (
    options[key] !== undefined &&
    (typeof options[key] !== "string" || !options[key].trim())
  ) {
    throw new Error(`${optionName} requires a value`);
  }
}

function buildProgressParams(options, env = process.env) {
  rejectUnknownOptions(options, PROGRESS_OPTION_NAMES, "set-progress");
  requireStringOption(options, "label");
  requireStringOption(options, "value");
  requireStringOption(options, "total");
  const key = typeof options.key === "string" ? options.key.trim() : "";
  const label =
    typeof options.label === "string" && options.label.trim() ? options.label.trim() : key;

  if (!key) throw new Error("set-progress requires --key");
  if (typeof options.value !== "string") throw new Error("set-progress requires --value");

  const value = parseFiniteNumber(options.value, "--value");
  const params = {
    ...buildTargetParams(options, env, "set-progress"),
    key,
    label,
    value,
  };

  if (typeof options.total === "string") {
    const total = parseFiniteNumber(options.total, "--total");
    if (total <= 0) {
      throw new Error("Invalid --total: expected positive number");
    }
    params.total = total;
  }

  return params;
}

function buildLogParams(options, positionals, stdinText = "", env = process.env) {
  rejectUnknownOptions(options, LOG_OPTION_NAMES, "log");
  requireNonBlankStringOption(options, "level");
  requireStringOption(options, "message");
  const level =
    typeof options.level === "string" && options.level.trim() ? options.level.trim() : "info";
  const message =
    typeof options.message === "string"
      ? options.message
      : positionals.length > 0
        ? positionals.join(" ")
        : stdinText.trim();

  if (!VALID_LOG_LEVELS.has(level)) {
    throw new Error("Invalid --level: expected info, warn, or error");
  }
  if (!message.trim()) {
    throw new Error("log requires --message, a positional message, or stdin");
  }

  return {
    ...buildTargetParams(options, env, "log"),
    level,
    message,
  };
}

function buildClearMetadataParams(options, env = process.env) {
  rejectUnknownOptions(options, CLEAR_METADATA_OPTION_NAMES, "clear metadata");
  requireNonBlankStringOption(options, "key");
  return {
    ...buildTargetParams(options, env, "clear metadata"),
    ...(typeof options.key === "string" ? { key: options.key.trim() } : {}),
  };
}

function buildStatusParams(options, env = process.env) {
  rejectUnknownOptions(options, STATUS_OPTION_NAMES, "set-status");
  requireNonBlankStringOption(options, "key");
  requireNonBlankStringOption(options, "value");
  requireNonBlankStringOption(options, "label");
  requireNonBlankStringOption(options, "color");
  const key = typeof options.key === "string" ? options.key.trim() : "";
  const value = typeof options.value === "string" ? options.value.trim() : "";
  const label =
    typeof options.label === "string" && options.label.trim() ? options.label.trim() : key;
  const color =
    typeof options.color === "string" && options.color.trim() ? options.color.trim() : undefined;

  if (!key) throw new Error("set-status requires --key");
  if (!value) throw new Error("set-status requires --value");
  if (color && !VALID_STATUS_COLORS.has(color) && !color.startsWith("#")) {
    throw new Error(`Unsupported status color: ${color}`);
  }

  return {
    ...buildTargetParams(options, env, "set-status"),
    key,
    label,
    value,
    ...(color ? { color } : {}),
  };
}

function shouldReadCommandStdin(options, positionals, textOption) {
  return typeof options[textOption] !== "string" && positionals.length === 0;
}

function buildNotificationParams(options, positionals, stdinText = "", env = process.env) {
  rejectUnknownOptions(options, NOTIFICATION_OPTION_NAMES, "notify");
  requireStringOption(options, "body");
  requireNonBlankStringOption(options, "kind");
  requireNonBlankStringOption(options, "title");
  const body =
    typeof options.body === "string"
      ? options.body
      : positionals.length > 0
        ? positionals.join(" ")
        : stdinText.trim();
  const title = typeof options.title === "string" ? options.title : "ForkTTY";
  const kind =
    typeof options.kind === "string" && options.kind.trim() ? options.kind.trim() : "info";

  if (!VALID_NOTIFICATION_KINDS.has(kind)) {
    throw new Error(`Invalid kind: ${kind}`);
  }

  return {
    ...buildTargetParams(options, env, "notify"),
    title,
    body,
    kind,
  };
}

function printJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

const HELP_TEXT = `ForkTTY CLI

Usage:
  forktty list [--json]
  forktty create-workspace [--name <name>] [--working-dir <path>] [--json]
  forktty focus <selector>
  forktty focus --workspace-id <id>
  forktty close-workspace <selector>
  forktty notify [message] [--title <title>] [--kind <kind>]
  forktty surfaces [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  forktty split-surface [--surface-id <id>] [--axis horizontal|vertical]
  forktty focus-surface <surface-id>
  forktty close-surface <surface-id>
  forktty send-text <text> [--surface-id <id>]
  forktty worktree-list [--cwd <repo>]
  forktty worktree-status [--path <worktree>] [--cwd <worktree>]
  forktty worktree-create <branch> [--cwd <repo>]
  forktty worktree-attach <branch> [--cwd <repo>]
  forktty worktree-remove <branch-or-worktree> [--cwd <repo>]
  forktty worktree-merge <branch-or-worktree> [--cwd <repo>]
  forktty set-status --key <key> --value <value> [--label <label>] [--color <color>]
  forktty list-status [--workspace-id <id>]
  forktty clear-status [--key <key>]
  forktty set-progress --key <key> --value <number> [--label <label>] [--total <number>]
  forktty list-progress [--workspace-id <id>]
  forktty clear-progress [--key <key>]
  forktty log [message] [--message <message>] [--level info|warn|error]
  forktty logs [--workspace-id <id>]
  forktty clear-logs [--workspace-id <id>]
  forktty notifications [--json]
  forktty clear-notifications
  forktty hooks setup [codex] [claude] [gemini]
  forktty hooks doctor codex
  forktty hooks test codex
  forktty hooks <agent> <event>
  forktty doctor
  forktty ping

Selector flags:
  --workspace-id <id>
  --workspace-name <name>
  --worktree-name <name>

Notes:
  - The CLI defaults to an absolute FORKTTY_SOCKET_PATH, then the app default socket path.
  - Global --socket and --json flags may appear before or after the command.
  - Inside a ForkTTY terminal, FORKTTY_WORKSPACE_ID is used automatically for notify and metadata commands.
  - Worktree commands default --cwd to PWD, then the CLI process cwd, so repo operations follow the shell you run them from.
  - send-text targets --surface-id, then FORKTTY_SURFACE_ID, then the active workspace's focused surface.
  - Hook debug output uses stderr only. Enable it with --verbose or FORKTTY_HOOK_DEBUG=1.
  - Hook commands always return a continue JSON payload and never fail the agent hook pipeline.
`;

function printHelp() {
  process.stdout.write(HELP_TEXT);
}

function formatWorkspaceLine(workspace) {
  const parts = [];
  parts.push(workspace.active ? "*" : " ");
  parts.push(workspace.name);
  parts.push(`[${workspace.id}]`);
  const gitBranch = workspace.gitBranch || workspace.git_branch;
  const workingDir = workspace.workingDir || workspace.working_dir;
  const surfaceCount =
    typeof workspace.surfaces === "number"
      ? workspace.surfaces
      : workspace.pane_tree
        ? countPaneLeaves(workspace.pane_tree)
        : 0;
  if (gitBranch) {
    parts.push(gitBranch);
  }
  if (workingDir) {
    parts.push(workingDir);
  }
  parts.push(`${surfaceCount} surface${surfaceCount === 1 ? "" : "s"}`);
  return parts.join("  ");
}

function countPaneLeaves(node) {
  if (!node || typeof node !== "object") return 0;
  if (node.type === "leaf") return 1;
  if (Array.isArray(node.children)) {
    return node.children.reduce((total, child) => total + countPaneLeaves(child), 0);
  }
  return 0;
}

function buildCreateWorkspaceParams(options) {
  rejectUnknownOptions(options, CREATE_WORKSPACE_OPTION_NAMES, "create-workspace");
  requireNonBlankStringOption(options, "name");
  requireNonBlankStringOption(options, "working-dir");
  const params = {};

  if (typeof options.name === "string") {
    params.name = options.name.trim();
  }
  if (typeof options["working-dir"] === "string") {
    params.workingDir = options["working-dir"].trim();
  }
  return params;
}

async function handleList(context, args = []) {
  requireNoCommandArgs(args, "list");
  const workspaces = await sendSocketRequest(context.socketPath, "workspace.list", {});
  if (context.json) {
    printJson(workspaces);
    return;
  }

  for (const workspace of workspaces) {
    process.stdout.write(`${formatWorkspaceLine(workspace)}\n`);
  }
}

async function handleCreateWorkspace(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "create-workspace");
  const params = buildCreateWorkspaceParams(options);

  const result = await sendSocketRequest(context.socketPath, "workspace.create", params);

  if (context.json) {
    printJson(result);
    return;
  }

  process.stdout.write(
    `Created workspace ${result.id}${params.name ? ` (${params.name})` : ""}\n`,
  );
}

async function tryWorkspaceSelect(context, params) {
  return sendSocketRequest(context.socketPath, "workspace.select", params);
}

function isSocketNotFoundError(error) {
  return error && typeof error === "object" && error.code === "not_found";
}

async function handleFocus(context, args) {
  const { options, positionals } = parseFlags(args);
  const candidates = resolveSelectorParams(options, positionals, context.env, "focus");

  if (!candidates) {
    throw new Error(
      "focus requires a selector, --workspace-id, --workspace-name, or --worktree-name",
    );
  }

  let lastError;
  for (const params of candidates) {
    try {
      await tryWorkspaceSelect(context, params);
      lastError = undefined;
      break;
    } catch (error) {
      lastError = error;
      if (!isSocketNotFoundError(error)) {
        break;
      }
    }
  }
  if (lastError) throw lastError;

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Focused workspace\n");
  }
}

function resolveSelectorParams(
  options,
  positionals,
  env = process.env,
  commandName = "workspace selector",
) {
  rejectUnknownOptions(options, TARGET_SELECTOR_OPTION_NAMES, commandName);
  const selectors = targetSelectorValues(options);
  if (selectors.length > 1) {
    throw new Error(
      `${commandName}: cannot combine ${formatOptionNames(selectors.map(({ option }) => option))}`,
    );
  }
  if (positionals.length > 1) {
    throw new Error(`${commandName}: unexpected argument ${positionals[1]}`);
  }
  const selector = positionals.length > 0 ? positionals[0].trim() : "";
  if (positionals.length > 0 && !selector) {
    throw new Error("workspace selector requires a value");
  }
  if (selectors.length === 1 && selector) {
    throw new Error(
      `${commandName}: cannot combine a positional selector with --${selectors[0].option}`,
    );
  }

  if (selectors[0]?.field === "workspace_id") {
    return [{ id: selectors[0].value }];
  }
  if (selectors[0]?.field === "workspace_name") {
    return [{ name: selectors[0].value }];
  }
  if (selectors[0]?.field === "worktreeName") {
    return [{ worktreeName: selectors[0].value }];
  }
  if (selector) {
    // Workspace ids and names both routinely contain dashes (e.g. `workspace-1`
    // vs. `my-feature`), so we can't tell them apart by string shape. Try the
    // id first and fall back to the name, matching `handleFocus`.
    return [{ id: selector }, { name: selector }];
  }
  if (typeof env.FORKTTY_WORKSPACE_ID === "string" && env.FORKTTY_WORKSPACE_ID.trim()) {
    return [{ id: env.FORKTTY_WORKSPACE_ID.trim() }];
  }
  return null;
}

async function handleCloseWorkspace(context, args) {
  const { options, positionals } = parseFlags(args);
  const candidates = resolveSelectorParams(options, positionals, context.env, "close-workspace");
  if (!candidates) {
    throw new Error(
      "close-workspace requires a selector, --workspace-id, --workspace-name, or --worktree-name",
    );
  }

  let lastError;
  for (const params of candidates) {
    try {
      await sendSocketRequest(context.socketPath, "workspace.close", params);
      lastError = undefined;
      break;
    } catch (error) {
      lastError = error;
      if (!isSocketNotFoundError(error)) {
        break;
      }
    }
  }
  if (lastError) throw lastError;

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Closed workspace\n");
  }
}

async function handleNotify(context, args) {
  const { options, positionals } = parseFlags(args);
  const stdinText = shouldReadCommandStdin(options, positionals, "body")
    ? await readStdinText()
    : "";
  const params = buildNotificationParams(options, positionals, stdinText, context.env);

  await sendSocketRequest(context.socketPath, "notification.create", params);

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Sent ${params.kind} notification\n`);
  }
}

async function handleSendText(context, args) {
  const { options, positionals } = parseFlags(args);
  rejectUnknownOptions(options, SEND_TEXT_OPTION_NAMES, "send-text");
  const stdinText = shouldReadCommandStdin(options, positionals, "text")
    ? await readStdinText()
    : "";
  const text =
    typeof options.text === "string"
      ? options.text
      : positionals.length > 0
        ? positionals.join(" ")
        : stdinText;
  const surfaceId = await resolveSendTextSurfaceId(context, options);

  if (!surfaceId) {
    throw new Error(
      "send-text requires --surface-id, FORKTTY_SURFACE_ID, or an active workspace surface",
    );
  }
  if (!text) {
    throw new Error("send-text requires text or stdin");
  }

  await sendSocketRequest(context.socketPath, "surface.send_text", {
    surface_id: surfaceId,
    text,
  });

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Sent text\n");
  }
}

async function resolveSendTextSurfaceId(context, options) {
  requireNonBlankStringOption(options, "surface-id");
  if (typeof options["surface-id"] === "string" && options["surface-id"].trim()) {
    return options["surface-id"].trim();
  }
  if (
    typeof context.env.FORKTTY_SURFACE_ID === "string" &&
    context.env.FORKTTY_SURFACE_ID.trim()
  ) {
    return context.env.FORKTTY_SURFACE_ID.trim();
  }
  return resolveFocusedSurfaceId(context);
}

async function resolveFocusedSurfaceId(context) {
  const workspaces = await sendSocketRequest(context.socketPath, "workspace.list", {});
  return surfaceIdFromWorkspaceList(workspaces);
}

function surfaceIdFromWorkspaceList(workspaces) {
  if (!Array.isArray(workspaces) || workspaces.length === 0) {
    return "";
  }
  const workspace =
    workspaces.find((candidate) => candidate.active) ||
    workspaces.find((candidate) => candidate?.focused_surface_id || candidate?.focusedSurfaceId);
  return workspace?.focused_surface_id || workspace?.focusedSurfaceId || "";
}

function surfaceIdFromArgs(options, positionals, env = process.env, command = "surface") {
  requireNonBlankStringOption(options, "surface-id");
  const optionSurfaceId =
    typeof options["surface-id"] === "string" && options["surface-id"].trim()
      ? options["surface-id"].trim()
      : "";
  const positionalSurfaceId = positionals.length > 0 ? positionals[0].trim() : "";
  if (positionals.length > 0 && !positionalSurfaceId) {
    throw new Error("surface id requires a value");
  }
  if (optionSurfaceId && positionalSurfaceId) {
    throw new Error(`${command}: cannot combine --surface-id with a positional surface id`);
  }
  if (optionSurfaceId) {
    return optionSurfaceId;
  }
  if (positionalSurfaceId) {
    return positionalSurfaceId;
  }
  if (typeof env.FORKTTY_SURFACE_ID === "string" && env.FORKTTY_SURFACE_ID.trim()) {
    return env.FORKTTY_SURFACE_ID.trim();
  }
  return "";
}

function buildSurfaceActionParams(options, positionals, env = process.env, command = "surface") {
  rejectUnknownOptions(options, SURFACE_ACTION_OPTION_NAMES, command);
  if (positionals.length > 1) {
    throw new Error(`${command}: unexpected argument ${positionals[1]}`);
  }
  const surfaceId = surfaceIdFromArgs(options, positionals, env, command);
  if (!surfaceId) {
    throw new Error(`${command} requires --surface-id, a surface id, or FORKTTY_SURFACE_ID`);
  }
  return { surface_id: surfaceId };
}

function buildSurfaceSplitParams(options, positionals, env = process.env) {
  rejectUnknownOptions(options, SURFACE_SPLIT_OPTION_NAMES, "split-surface");
  if (positionals.length > 1) {
    throw new Error(`split-surface: unexpected argument ${positionals[1]}`);
  }
  requireNonBlankStringOption(options, "axis");
  const axis =
    typeof options.axis === "string" && options.axis.trim() ? options.axis.trim() : "horizontal";
  if (axis !== "horizontal" && axis !== "vertical") {
    throw new Error("Invalid --axis: expected horizontal or vertical");
  }
  const params = { axis };
  const surfaceId = surfaceIdFromArgs(options, positionals, env, "split-surface");
  if (surfaceId) {
    params.surface_id = surfaceId;
  }
  return params;
}

function formatSurfaceLine(surface) {
  const state = surface.unread || surface.needs_attention ? "unread" : "read";
  const title = surface.title ? ` ${surface.title}` : "";
  const cwd = surface.cwd ? ` ${surface.cwd}` : "";
  return `${surface.id} [${surface.workspace_id}] ${state}${title}${cwd}`;
}

async function handleSurfaces(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "surfaces");
  rejectUnknownOptions(options, SURFACE_LIST_OPTION_NAMES, "surfaces");
  const result = await sendSocketRequest(context.socketPath, "surface.list", {
    ...buildTargetParams(options, context.env, "surfaces"),
  });

  if (context.json) {
    printJson(result);
    return;
  }

  if (!Array.isArray(result) || result.length === 0) {
    process.stdout.write("No surfaces\n");
    return;
  }

  for (const surface of result) {
    process.stdout.write(`${formatSurfaceLine(surface)}\n`);
  }
}

async function handleSplitSurface(context, args) {
  const { options, positionals } = parseFlags(args);
  const params = buildSurfaceSplitParams(options, positionals, context.env);
  if (!params.surface_id) {
    params.surface_id = await resolveFocusedSurfaceId(context);
  }
  if (!params.surface_id) {
    throw new Error(
      "split-surface requires --surface-id, a surface id, FORKTTY_SURFACE_ID, or an active workspace surface",
    );
  }
  const result = await sendSocketRequest(
    context.socketPath,
    "surface.split",
    params,
  );

  if (context.json) {
    printJson(result);
  } else {
    process.stdout.write(`Created surface ${result.id}\n`);
  }
}

async function handleFocusSurface(context, args) {
  const { options, positionals } = parseFlags(args);
  await sendSocketRequest(
    context.socketPath,
    "surface.focus",
    buildSurfaceActionParams(options, positionals, context.env, "focus-surface"),
  );

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Focused surface\n");
  }
}

async function handleCloseSurface(context, args) {
  const { options, positionals } = parseFlags(args);
  await sendSocketRequest(
    context.socketPath,
    "surface.close",
    buildSurfaceActionParams(options, positionals, context.env, "close-surface"),
  );

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Closed surface\n");
  }
}

function worktreeParams(
  options,
  positionals,
  requireName = false,
  env = process.env,
  fallbackCwd,
  commandName = "worktree command",
  allowedOptions = WORKTREE_COMMAND_OPTION_NAMES,
) {
  rejectUnknownOptions(options, allowedOptions, commandName);
  if (positionals.length > 1) {
    throw new Error(`${commandName}: unexpected argument ${positionals[1]}`);
  }
  requireNonBlankStringOption(options, "branch");
  requireNonBlankStringOption(options, "cwd");
  requireNonBlankStringOption(options, "name");
  const params = {};
  const positionalName = positionals.length > 0 ? positionals[0].trim() : "";
  if (positionals.length > 0 && !positionalName) {
    throw new Error("worktree command requires a branch or worktree name");
  }
  const optionName = typeof options.name === "string" && options.name.trim()
    ? options.name.trim()
    : "";
  const optionBranch = typeof options.branch === "string" && options.branch.trim()
    ? options.branch.trim()
    : "";
  if (positionalName && (optionName || optionBranch)) {
    throw new Error(`${commandName}: cannot combine a positional name with --name or --branch`);
  }
  if (optionName && optionBranch) {
    throw new Error(`${commandName}: cannot combine --name and --branch`);
  }
  const name = positionalName || optionName || optionBranch;
  if (requireName && (!name || typeof name !== "string")) {
    throw new Error("worktree command requires a branch or worktree name");
  }
  if (typeof name === "string" && name.trim()) {
    params.name = name.trim();
  }
  if (typeof options.cwd === "string" && options.cwd.trim()) {
    params.cwd = options.cwd.trim();
  } else {
    const cwd = callerCwd(env, fallbackCwd);
    if (cwd) {
      params.cwd = cwd;
    } else {
      throw new Error("worktree command requires --cwd, PWD, or the current directory");
    }
  }
  return params;
}

function buildWorktreeStatusParams(
  options,
  positionals,
  env = process.env,
  fallbackCwd,
) {
  rejectUnknownOptions(options, WORKTREE_STATUS_OPTION_NAMES, "worktree-status");
  if (positionals.length > 1) {
    throw new Error(`worktree-status: unexpected argument ${positionals[1]}`);
  }
  requireNonBlankStringOption(options, "cwd");
  requireNonBlankStringOption(options, "path");
  const pathValue =
    typeof options.path === "string" && options.path.trim()
      ? options.path.trim()
      : typeof options.cwd === "string" && options.cwd.trim()
        ? options.cwd.trim()
      : positionals[0]
        ? positionals[0].trim()
        : callerCwd(env, fallbackCwd);

  if (!pathValue) {
    throw new Error("worktree-status requires --path, --cwd, a path, PWD, or the current directory");
  }
  return { path: pathValue };
}

function safeProcessCwd() {
  try {
    return process.cwd();
  } catch {
    return "";
  }
}

function callerCwd(env = process.env, fallbackCwd) {
  if (typeof env.PWD === "string" && env.PWD.trim()) {
    return env.PWD.trim();
  }
  const resolvedFallback =
    typeof fallbackCwd === "string" && fallbackCwd.trim() ? fallbackCwd.trim() : safeProcessCwd();
  return typeof resolvedFallback === "string" && resolvedFallback.trim() ? resolvedFallback.trim() : "";
}

function formatWorktreeLine(worktree) {
  const status = worktree.status ? ` ${worktree.status}` : "";
  return `${worktree.branch || worktree.name} [${worktree.worktree_name}] ${worktree.path}${status}`;
}

async function handleWorktreeList(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "worktree-list");
  const result = await sendSocketRequest(
    context.socketPath,
    "worktree.list",
    worktreeParams(
      options,
      [],
      false,
      context.env,
      undefined,
      "worktree-list",
      WORKTREE_LIST_OPTION_NAMES,
    ),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  for (const worktree of result) {
    process.stdout.write(`${formatWorktreeLine(worktree)}\n`);
  }
}

async function handleWorktreeStatus(context, args) {
  const { options, positionals } = parseFlags(args);
  const result = await sendSocketRequest(
    context.socketPath,
    "worktree.status",
    buildWorktreeStatusParams(options, positionals, context.env),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  process.stdout.write(`${result.status || "unknown"}\n`);
}

async function handleWorktreeCreate(context, args, method) {
  const { options, positionals } = parseFlags(args);
  const commandName = method === "worktree.create" ? "worktree-create" : "worktree-attach";
  const result = await sendSocketRequest(
    context.socketPath,
    method,
    worktreeParams(options, positionals, true, context.env, undefined, commandName),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  process.stdout.write(`Opened worktree ${result.branch || result.name} at ${result.path}\n`);
}

async function handleWorktreeRemove(context, args) {
  const { options, positionals } = parseFlags(args);
  const result = await sendSocketRequest(
    context.socketPath,
    "worktree.remove",
    worktreeParams(options, positionals, true, context.env, undefined, "worktree-remove"),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  process.stdout.write(`Removed worktree ${result.removed}\n`);
}

async function handleWorktreeMerge(context, args) {
  const { options, positionals } = parseFlags(args);
  const result = await sendSocketRequest(
    context.socketPath,
    "worktree.merge",
    worktreeParams(options, positionals, true, context.env, undefined, "worktree-merge"),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  process.stdout.write(`${result}\n`);
}

async function handleSetStatus(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "set-status");
  const params = buildStatusParams(options, context.env);
  await sendSocketRequest(context.socketPath, "metadata.set_status", params);

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Updated status ${params.key}\n`);
  }
}

function formatStatusLine(status) {
  const color = status.color ? ` (${status.color})` : "";
  return `${status.label}: ${status.value}${color}`;
}

async function handleListStatus(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "list-status");
  rejectUnknownOptions(options, TARGET_SELECTOR_OPTION_NAMES, "list-status");
  const result = await sendSocketRequest(context.socketPath, "metadata.list_status", {
    ...buildTargetParams(options, context.env, "list-status"),
  });

  if (context.json) {
    printJson(result);
    return;
  }

  if (!Array.isArray(result) || result.length === 0) {
    process.stdout.write("No status entries\n");
    return;
  }

  for (const status of result) {
    process.stdout.write(`${formatStatusLine(status)}\n`);
  }
}

async function handleClearStatus(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "clear-status");
  await sendSocketRequest(
    context.socketPath,
    "metadata.clear_status",
    buildClearMetadataParams(options, context.env),
  );

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared status\n");
  }
}

async function handleSetProgress(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "set-progress");
  const params = buildProgressParams(options, context.env);
  await sendSocketRequest(context.socketPath, "metadata.set_progress", params);

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Updated progress ${params.key}\n`);
  }
}

function formatProgressLine(progress) {
  const label = progress.label || progress.key;
  if (typeof progress.total === "number") {
    return `${label}: ${progress.value}/${progress.total}`;
  }
  return `${label}: ${progress.value}`;
}

async function handleListProgress(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "list-progress");
  rejectUnknownOptions(options, TARGET_SELECTOR_OPTION_NAMES, "list-progress");
  const result = await sendSocketRequest(context.socketPath, "metadata.list_progress", {
    ...buildTargetParams(options, context.env, "list-progress"),
  });

  if (context.json) {
    printJson(result);
    return;
  }

  if (!Array.isArray(result) || result.length === 0) {
    process.stdout.write("No progress entries\n");
    return;
  }

  for (const progress of result) {
    process.stdout.write(`${formatProgressLine(progress)}\n`);
  }
}

async function handleClearProgress(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "clear-progress");
  await sendSocketRequest(
    context.socketPath,
    "metadata.clear_progress",
    buildClearMetadataParams(options, context.env),
  );

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared progress\n");
  }
}

function formatLogLine(log) {
  return `[${log.level || "info"}] ${log.message || ""}`;
}

async function handleLog(context, args) {
  const { options, positionals } = parseFlags(args);
  const stdinText = shouldReadCommandStdin(options, positionals, "message")
    ? await readStdinText()
    : "";
  const params = buildLogParams(options, positionals, stdinText, context.env);
  await sendSocketRequest(context.socketPath, "metadata.log", params);

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Appended ${params.level} log\n`);
  }
}

async function handleLogs(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "logs");
  rejectUnknownOptions(options, TARGET_SELECTOR_OPTION_NAMES, "logs");
  const result = await sendSocketRequest(context.socketPath, "metadata.list_logs", {
    ...buildTargetParams(options, context.env, "logs"),
  });

  if (context.json) {
    printJson(result);
    return;
  }

  if (!Array.isArray(result) || result.length === 0) {
    process.stdout.write("No logs\n");
    return;
  }

  for (const log of result) {
    process.stdout.write(`${formatLogLine(log)}\n`);
  }
}

async function handleClearLogs(context, args) {
  const { options, positionals } = parseFlags(args);
  requireNoCommandArgs(positionals, "clear-logs");
  rejectUnknownOptions(options, TARGET_SELECTOR_OPTION_NAMES, "clear-logs");
  await sendSocketRequest(context.socketPath, "metadata.clear_logs", {
    ...buildTargetParams(options, context.env, "clear-logs"),
  });

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared logs\n");
  }
}

function formatNotificationLine(notification) {
  const state = notification.read ? "read" : "unread";
  const title = notification.title || "ForkTTY";
  const body = notification.body ? ` — ${notification.body}` : "";
  const workspace = notification.workspaceName || notification.workspace_id || "global";
  return `[${state}] ${workspace} · ${notification.kind} · ${title}${body}`;
}

async function handleNotifications(context, args = []) {
  requireNoCommandArgs(args, "notifications");
  const result = await sendSocketRequest(context.socketPath, "notification.list", {});

  if (context.json) {
    printJson(result);
    return;
  }

  if (!Array.isArray(result) || result.length === 0) {
    process.stdout.write("No notifications\n");
    return;
  }

  for (const notification of result) {
    process.stdout.write(`${formatNotificationLine(notification)}\n`);
  }
}

async function handleClearNotifications(context, args = []) {
  requireNoCommandArgs(args, "clear-notifications");
  await sendSocketRequest(context.socketPath, "notification.clear", {});

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared notifications\n");
  }
}

function buildHookShellCommand(
  scriptPath,
  agent,
  event,
  nodePath = process.execPath,
  launcherPath = "",
) {
  const spec = AGENT_SPECS[agent];
  const disabledGuard = `[ "\${${spec.disabledEnv}:-}" != "1" ]`;
  const command = launcherPath
    ? `FORKTTY_NODE=${shellQuote(nodePath)} ${shellQuote(launcherPath)} hooks ${agent} ${event}`
    : `${shellQuote(nodePath)} ${shellQuote(scriptPath)} hooks ${agent} ${event}`;
  return `${disabledGuard} && ${command} || echo '${HOOK_CONTINUE_JSON.trimEnd()}'`;
}

const FORKTTY_HOOK_TAG = "forktty";

function buildHookEntry(command, statusMessage, timeout, matcher) {
  const entry = {
    hooks: [
      {
        type: "command",
        command,
        statusMessage,
        timeout,
      },
    ],
    forkttySource: FORKTTY_HOOK_TAG,
  };
  if (matcher) {
    entry.matcher = matcher;
  }
  return entry;
}

function isForkttyManagedEntry(entry) {
  return isObject(entry) && entry.forkttySource === FORKTTY_HOOK_TAG;
}

function isLegacyForkttyHookCommand(command, scriptPath, agent, event, launcherPath = "") {
  if (typeof command !== "string") return false;
  const eventSuffix = ` hooks ${agent} ${event}`;
  if (!command.includes(eventSuffix)) return false;
  const scriptRef = shellQuote(scriptPath);
  if (command.includes(` ${scriptRef}${eventSuffix}`)) return true;
  if (launcherPath) {
    const launcherRef = shellQuote(launcherPath);
    if (command.includes(` ${launcherRef}${eventSuffix}`)) return true;
  }
  const spec = AGENT_SPECS[agent];
  return command.includes("forktty.mjs") && command.includes(spec.disabledEnv);
}

function deepCloneJson(value) {
  return value === undefined ? {} : JSON.parse(JSON.stringify(value));
}

function mergeHookConfig(
  existingConfig,
  agent,
  scriptPath,
  nodePath = process.execPath,
  launcherPath = "",
) {
  const spec = AGENT_SPECS[agent];
  if (!spec) {
    throw new Error(`Unsupported agent: ${agent}`);
  }

  const nextConfig = isObject(existingConfig) ? deepCloneJson(existingConfig) : {};
  const hooks = isObject(nextConfig.hooks) ? { ...nextConfig.hooks } : {};
  let changed = !isObject(nextConfig.hooks);

  for (const [eventName, hookEventName, timeout] of spec.hookEntries) {
    const statusMessage = `ForkTTY ${spec.label} hooks`;
    const command = buildHookShellCommand(
      scriptPath,
      agent,
      hookEventName,
      nodePath,
      launcherPath,
    );
    const nextEntry = buildHookEntry(command, statusMessage, timeout, spec.matcher);
    const existingEntries = Array.isArray(hooks[eventName]) ? [...hooks[eventName]] : [];
    // Strip any previously-installed ForkTTY entries before appending so a
    // moved repo, an upgraded node binary, or a tweaked shell guard does not
    // leave stale duplicates behind. Recognise both tagged entries (new
    // installs) and entries whose command exactly matches the one we're
    // about to write (legacy untagged installs).
    const filtered = existingEntries.filter((entry) => {
      if (isForkttyManagedEntry(entry)) return false;
      if (isObject(entry) && Array.isArray(entry.hooks)) {
        const hasMatchingCommand = entry.hooks.some((hook) => {
          if (hook?.type !== "command") return false;
          return (
            hook.command === command ||
            isLegacyForkttyHookCommand(
              hook.command,
              scriptPath,
              agent,
              hookEventName,
              launcherPath,
            )
          );
        });
        if (hasMatchingCommand) {
          return false;
        }
      }
      return true;
    });
    filtered.push(nextEntry);
    if (JSON.stringify(filtered) !== JSON.stringify(existingEntries)) {
      changed = true;
    }
    hooks[eventName] = filtered;
  }

  nextConfig.hooks = hooks;
  return { changed, config: nextConfig };
}

function resolveHookLauncherPath(env = process.env) {
  const launcherPath = trimEnv(env, "FORKTTY_HOOK_LAUNCHER");
  if (!launcherPath) return "";
  if (!path.isAbsolute(launcherPath)) {
    throw new Error("FORKTTY_HOOK_LAUNCHER must be an absolute path");
  }
  return launcherPath;
}

function resolveHookNodePath(env = process.env) {
  const nodePath = trimEnv(env, "FORKTTY_HOOK_NODE") || process.execPath;
  if (!path.isAbsolute(nodePath)) {
    throw new Error("FORKTTY_HOOK_NODE must be an absolute path");
  }
  return nodePath;
}

async function readJsonFile(filePath) {
  let stat;
  try {
    const linkStat = await fs.lstat(filePath);
    if (linkStat.isSymbolicLink()) {
      try {
        stat = await fs.stat(filePath);
      } catch (error) {
        if (error && typeof error === "object" && error.code === "ENOENT") {
          throw new Error("path is a broken symlink");
        }
        throw error;
      }
    } else {
      stat = linkStat;
    }
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return {};
    }
    throw error;
  }
  if (!stat.isFile()) {
    throw new Error("path exists but is not a regular file");
  }
  const text = await fs.readFile(filePath, "utf8");
  if (!text.trim()) return {};
  return JSON.parse(text);
}

async function readAgentConfig(agent, filePath) {
  try {
    const config = await readJsonFile(filePath);
    if (!isObject(config)) {
      throw new Error("expected a JSON object at the top level");
    }
    return config;
  } catch (error) {
    // Re-throw with agent + path context. The bare error from JSON.parse only
    // says e.g. "Unexpected token } in JSON at position 12" which leaves the
    // user guessing which agent's file is broken.
    const detail = error instanceof Error ? error.message : String(error);
    const wrapped = new Error(
      `failed to read ${agent} hook config at ${filePath}: ${detail}`,
    );
    wrapped.cause = error;
    throw wrapped;
  }
}

async function ensureParentDir(filePath) {
  await fs.mkdir(path.dirname(filePath), { recursive: true });
}

async function hookConfigWritePath(filePath) {
  try {
    const stat = await fs.lstat(filePath);
    if (stat.isSymbolicLink()) {
      return await fs.realpath(filePath);
    }
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return filePath;
    }
    throw error;
  }
  return filePath;
}

async function backupFile(filePath) {
  try {
    await fs.access(filePath);
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return null;
    }
    throw error;
  }

  for (;;) {
    const backupPath = `${filePath}.bak-${nextFileNonce()}`;
    try {
      await fs.copyFile(filePath, backupPath, fsConstants.COPYFILE_EXCL);
      return backupPath;
    } catch (error) {
      if (error && typeof error === "object" && error.code === "EEXIST") {
        continue;
      }
      throw error;
    }
  }
}

async function atomicWriteFile(filePath, content) {
  // Write to a sibling temp file, fsync it, then rename. Protects against
  // truncation+partial-write windows on SIGKILL or power loss; the user's
  // existing config either stays intact or is replaced atomically.
  const dir = path.dirname(filePath);
  const base = path.basename(filePath);
  const tmpPath = path.join(dir, `.${base}.tmp-${nextFileNonce()}`);
  let handle;
  let tempCreated = false;
  try {
    handle = await fs.open(tmpPath, "wx", 0o600);
    tempCreated = true;
    await handle.writeFile(content, "utf8");
    await handle.sync();
    await handle.close();
    handle = null;
    await fs.rename(tmpPath, filePath);
  } catch (error) {
    if (handle) {
      await handle.close().catch(() => {});
    }
    if (tempCreated) {
      await fs.unlink(tmpPath).catch(() => {});
    }
    throw error;
  }
}

function supportedAgents(positionals) {
  if (positionals.length === 0) {
    return Object.keys(AGENT_SPECS);
  }
  const seen = new Set();
  const agents = [];
  for (const name of positionals) {
    const normalized = name.toLowerCase();
    if (!AGENT_SPECS[normalized]) {
      throw new Error(`Unsupported agent: ${name}`);
    }
    if (!seen.has(normalized)) {
      seen.add(normalized);
      agents.push(normalized);
    }
  }
  return agents;
}

async function handleHooksSetup(context, args) {
  const allowedOptions = new Set(["dry-run"]);
  const { options, positionals } = parseFlags(args, allowedOptions);
  rejectUnknownOptions(options, allowedOptions, "hooks setup");
  const dryRunOption = options["dry-run"];
  if (
    dryRunOption !== undefined &&
    dryRunOption !== true &&
    dryRunOption !== "true" &&
    dryRunOption !== "false"
  ) {
    throw new Error("hooks setup: --dry-run must be true or false");
  }
  const dryRun = dryRunOption === true || dryRunOption === "true";
  const agentNames = supportedAgents(positionals);
  const scriptPath = fileURLToPath(import.meta.url);
  const launcherPath = resolveHookLauncherPath(context.env);
  const nodePath = resolveHookNodePath(context.env);

  const plans = [];
  for (const agent of agentNames) {
    const spec = AGENT_SPECS[agent];
    const configPath = spec.configPath(context.env);
    const existing = await readAgentConfig(agent, configPath);
    const { changed, config } = mergeHookConfig(
      existing,
      agent,
      scriptPath,
      nodePath,
      launcherPath,
    );
    plans.push({
      agent,
      configPath,
      changed,
      config,
    });
  }

  const summaries = [];
  for (const plan of plans) {
    let backupPath = null;
    if (plan.changed && !dryRun) {
      const writePath = await hookConfigWritePath(plan.configPath);
      await ensureParentDir(writePath);
      backupPath = await backupFile(writePath);
      await atomicWriteFile(writePath, `${JSON.stringify(plan.config, null, 2)}\n`);
    }

    summaries.push({
      agent: plan.agent,
      configPath: plan.configPath,
      changed: plan.changed,
      backupPath,
      dryRun,
    });
  }

  if (context.json) {
    printJson(summaries);
    return;
  }

  for (const summary of summaries) {
    const verb = summary.changed
      ? summary.dryRun
        ? "would update"
        : "updated"
      : "already configured";
    process.stdout.write(`${summary.agent}: ${verb} at ${summary.configPath}\n`);
    if (summary.backupPath) {
      process.stdout.write(`  backup: ${summary.backupPath}\n`);
    }
  }
}

function extractHookMessage(payload) {
  if (!payload || typeof payload !== "object") return "";

  const queue = [payload];
  const seen = new Set();
  const keys = [
    "message",
    "body",
    "reason",
    "error",
    "summary",
    "detail",
    "title",
    "text",
    "last_assistant_message",
  ];

  while (queue.length > 0) {
    const current = queue.shift();
    if (!isObject(current) || seen.has(current)) continue;
    seen.add(current);

    for (const key of keys) {
      const value = current[key];
      if (typeof value === "string" && value.trim()) {
        return value.trim();
      }
    }

    for (const value of Object.values(current)) {
      if (isObject(value)) {
        queue.push(value);
      }
    }
  }

  return "";
}

function extractFirstStringLikeValue(payload, keys) {
  if (!payload || typeof payload !== "object") return "";

  const queue = [payload];
  const seen = new Set();
  while (queue.length > 0) {
    const current = queue.shift();
    if (!isObject(current) || seen.has(current)) continue;
    seen.add(current);

    for (const key of keys) {
      const value = current[key];
      if (typeof value === "string" && value.trim()) {
        return value.trim();
      }
      if (typeof value === "number" || typeof value === "boolean") {
        return String(value);
      }
    }

    for (const value of Object.values(current)) {
      if (isObject(value)) {
        queue.push(value);
      }
    }
  }

  return "";
}

function extractHookSource(payload) {
  const raw = extractFirstStringLikeValue(payload, ["source", "trigger", "reason"]);
  if (!raw) return "";
  return sanitizeForTerminal(raw).slice(0, 32);
}

function extractHookCompactTrigger(payload) {
  const raw = extractFirstStringLikeValue(payload, [
    "trigger",
    "compact_trigger",
    "compactTrigger",
    "reason",
  ]);
  if (!raw) return "";
  return sanitizeForTerminal(raw).slice(0, 32);
}

function extractHookToolError(payload) {
  if (!payload || typeof payload !== "object") return false;
  const queue = [payload];
  const seen = new Set();
  while (queue.length > 0) {
    const current = queue.shift();
    if (!current || typeof current !== "object" || seen.has(current)) continue;
    seen.add(current);
    for (const key of ["is_error", "isError", "error"]) {
      const value = current[key];
      if (value === true) return true;
      if (typeof value === "string" && value.trim().length > 0) return true;
      if (isObject(value) && (value.message || value.type || value.code)) return true;
    }
    for (const value of Object.values(current)) {
      if (isObject(value)) queue.push(value);
    }
  }
  return false;
}

function extractHookToolName(payload) {
  const raw = extractFirstStringLikeValue(payload, [
    "tool_name",
    "toolName",
    "tool",
    "name",
  ]);
  if (!raw) return "";
  const sanitized = sanitizeForTerminal(raw);
  if (sanitized.length <= HOOK_TOOL_LABEL_MAX) return sanitized;
  return `${sanitized.slice(0, HOOK_TOOL_LABEL_MAX - 1)}…`;
}

function extractHookTurnId(eventName, payload) {
  const explicit = extractFirstStringLikeValue(payload, [
    "turn_id",
    "turnId",
    "prompt_id",
    "promptId",
    "request_id",
    "requestId",
    "message_id",
    "messageId",
    "event_id",
    "eventId",
    "sequence",
    "seq",
  ]);
  if (explicit) {
    return `id:${shortSha256(explicit)}`;
  }

  if (eventName !== "prompt-submit") {
    return "";
  }

  const prompt = extractFirstStringLikeValue(payload, [
    "prompt",
    "message",
    "text",
    "body",
  ]);
  return prompt ? `prompt:${shortSha256(prompt)}` : "";
}

function buildHookTargetParams(env = process.env) {
  const workspaceId = trimEnv(env, "FORKTTY_WORKSPACE_ID");
  const surfaceId = trimEnv(env, "FORKTTY_SURFACE_ID");
  return {
    ...(workspaceId ? { workspace_id: workspaceId } : {}),
    ...(surfaceId ? { surface_id: surfaceId } : {}),
  };
}

function addHookEventMetadata(params, eventName, payload, hookEventOrder) {
  if (hookEventOrder === undefined || hookEventOrder === null) {
    return params;
  }
  const turnId = extractHookTurnId(eventName, payload);
  return {
    ...params,
    [HOOK_EVENT_ORDER_PARAM]: hookEventOrder,
    hook_event_clock: HOOK_EVENT_CLOCK,
    hook_event_name: eventName,
    ...(turnId ? { hook_turn_id: turnId } : {}),
  };
}

function buildHookActions(
  agent,
  eventName,
  payload,
  env = process.env,
  hookEventOrder,
) {
  const spec = AGENT_SPECS[agent];
  if (!spec) {
    throw new Error(`Unsupported agent: ${agent}`);
  }

  const target = buildHookTargetParams(env);
  const key = `agent:${agent}`;
  const message = sanitizeForTerminal(extractHookMessage(payload));
  const log = (level, logMessage) => ({
    method: "metadata.log",
    params: {
      ...target,
      level,
      message: logMessage,
    },
  });

  switch (eventName) {
    case "session-start": {
      const source = extractHookSource(payload);
      const sourceMsg = source ? ` (${source})` : "";
      return [
        log("info", `${spec.label} session started${sourceMsg}`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Ready",
              color: "green",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    }
    case "prompt-submit":
      return [
        log("info", `${spec.label} prompt submitted`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Running",
              color: "blue",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    case "notification":
      return [
        log("warn", message || `${spec.label} requested attention`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Needs input",
              color: "yellow",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
        {
          method: "notification.create",
          params: {
            ...target,
            title: `${spec.label} needs input`,
            body: message || `${spec.label} reported a prompt or attention event.`,
            kind: "prompt",
          },
        },
      ];
    case "stop-failure":
      return [
        log("error", message || `${spec.label} reported a failure`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Error",
              color: "red",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
        {
          method: "notification.create",
          params: {
            ...target,
            title: `${spec.label} error`,
            body: message || `${spec.label} reported a failure.`,
            kind: "error",
          },
        },
      ];
    case "pre-tool": {
      const toolName = extractHookToolName(payload);
      const value = toolName ? `Running ${toolName}` : "Running tool";
      return [
        log("info", toolName ? `${spec.label} running ${toolName}` : `${spec.label} running tool`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value,
              color: "blue",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    }
    case "post-tool": {
      const toolName = extractHookToolName(payload);
      const isError = extractHookToolError(payload);
      const labelText = toolName || "tool";
      const actions = [
        log(
          isError ? "error" : "info",
          isError
            ? `${spec.label} ${labelText} reported an error`
            : `${spec.label} finished ${labelText}`,
        ),
      ];
      if (isError) {
        actions.push({
          method: "notification.create",
          params: {
            ...target,
            title: `${spec.label} tool error`,
            body: `${labelText} returned an error response.`,
            kind: "error",
          },
        });
      }
      return actions;
    }
    case "subagent-stop":
      return [
        log("info", message || `${spec.label} subagent finished`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Running",
              color: "blue",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    case "pre-compact": {
      const trigger = extractHookCompactTrigger(payload);
      const triggerMsg = trigger ? ` (${trigger})` : "";
      return [
        log("warn", `${spec.label} context compacting${triggerMsg}`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Compacting",
              color: "yellow",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
        {
          method: "notification.create",
          params: {
            ...target,
            title: `${spec.label} compacting context`,
            body: trigger
              ? `Context compaction triggered: ${trigger}.`
              : "Context compaction in progress.",
            kind: "info",
          },
        },
      ];
    }
    case "stop":
      return [
        log("info", message || `${spec.label} stopped`),
        {
          method: "metadata.set_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
              label: spec.label,
              value: "Ready",
              color: "green",
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    case "session-end":
      return [
        log("info", `${spec.label} session ended`),
        {
          method: "metadata.clear_status",
          params: addHookEventMetadata(
            {
              ...target,
              key,
            },
            eventName,
            payload,
            hookEventOrder,
          ),
        },
      ];
    default:
      return [];
  }
}

function isTruthyEnv(value) {
  if (typeof value !== "string") return false;
  const normalized = value.trim().toLowerCase();
  return normalized === "1" || normalized === "true" || normalized === "yes";
}

function isHookDebugEnabled(context) {
  return Boolean(context.verbose) || isTruthyEnv(context.env.FORKTTY_HOOK_DEBUG);
}

function hookDebug(context, message) {
  if (!isHookDebugEnabled(context)) return;
  process.stderr.write(terminalLine(`ForkTTY hook debug: ${message}`));
}

function redactHookDebugValue(value, key = "", depth = 0) {
  if (HOOK_DEBUG_REDACT_KEYS.has(key)) {
    const text = typeof value === "string" ? value : JSON.stringify(value);
    return `<redacted:${text ? text.length : 0}:sha256:${shortSha256(text || "")}>`;
  }
  if (depth >= 4) {
    return "<redacted:depth>";
  }
  if (Array.isArray(value)) {
    return value.slice(0, 20).map((item) => redactHookDebugValue(item, "", depth + 1));
  }
  if (isObject(value)) {
    const out = {};
    for (const [childKey, childValue] of Object.entries(value)) {
      out[childKey] = redactHookDebugValue(childValue, childKey, depth + 1);
    }
    return out;
  }
  return value;
}

function summarizeHookAction(action) {
  const params = JSON.stringify(redactHookDebugValue(action.params));
  return `${action.method} ${params.length > 500 ? `${params.slice(0, 500)}...` : params}`;
}

function assertObjectResult(method, result) {
  if (!isObject(result)) {
    throw new Error(`${method} returned ${JSON.stringify(result)}, expected an object`);
  }
  return result;
}

function assertFieldEquals(method, result, field, expected) {
  if (result[field] !== expected) {
    throw new Error(
      `${method} returned ${field}=${JSON.stringify(result[field])}, expected ${JSON.stringify(expected)}`,
    );
  }
}

function hookTestStatusKey(agent) {
  return `agent:${agent}:${HOOK_TEST_STATUS_KEY_SUFFIX}`;
}

async function handleHooksTest(context, args) {
  const [agentName, ...extraArgs] = args;
  const agent = typeof agentName === "string" ? agentName.toLowerCase() : "";
  if (!agent) {
    throw new Error("hooks test requires an agent");
  }
  if (!AGENT_SPECS[agent]) {
    throw new Error(`Unsupported hook test agent: ${agentName}`);
  }
  requireNoCommandArgs(extraArgs, `hooks test ${agent}`);

  const spec = AGENT_SPECS[agent];
  const target = buildHookTargetParams(context.env);
  const statusKey = hookTestStatusKey(agent);
  const order = nextHookEventOrder();
  let statusSet = false;
  let notificationsBefore = null;
  let createdNotification = null;

  const note = (message) => process.stderr.write(terminalLine(message));
  note(`ForkTTY ${spec.label} hook test`);
  note(`socket: ${context.socketPath}`);
  note(`workspace: ${target.workspace_id || "(active workspace fallback)"}`);
  note(`surface: ${target.surface_id || "(none)"}`);

  try {
    const ping = await sendSocketRequest(context.socketPath, "system.ping", {});
    if (ping !== "pong") {
      throw new Error(`system.ping returned ${JSON.stringify(ping)}, expected "pong"`);
    }
    note("system.ping: ok");

    const statusParams = {
      ...target,
      key: statusKey,
      label: `${spec.label} hook test`,
      value: "Running",
      color: "blue",
      [HOOK_EVENT_ORDER_PARAM]: order,
    };
    const status = assertObjectResult(
      "metadata.set_status",
      await sendSocketRequest(context.socketPath, "metadata.set_status", statusParams),
    );
    assertFieldEquals("metadata.set_status", status, "key", statusKey);
    assertFieldEquals("metadata.set_status", status, "value", "Running");
    statusSet = true;
    note("metadata.set_status: ok");

    const log = assertObjectResult(
      "metadata.log",
      await sendSocketRequest(context.socketPath, "metadata.log", {
        ...target,
        level: "info",
        message: `${spec.label} hook roundtrip test`,
      }),
    );
    assertFieldEquals("metadata.log", log, "level", "info");
    note("metadata.log: ok");

    notificationsBefore = await sendSocketRequest(
      context.socketPath,
      "notification.list",
      {},
    );
    if (!Array.isArray(notificationsBefore)) {
      throw new Error("notification.list returned a non-array response before test notification");
    }

    createdNotification = assertObjectResult(
      "notification.create",
      await sendSocketRequest(context.socketPath, "notification.create", {
        ...target,
        title: `${spec.label} hook test`,
        body: "Roundtrip validation",
        kind: "prompt",
      }),
    );
    assertFieldEquals("notification.create", createdNotification, "title", `${spec.label} hook test`);
    assertFieldEquals("notification.create", createdNotification, "kind", "prompt");
    if (target.surface_id) {
      assertFieldEquals("notification.create", createdNotification, "surface_id", target.surface_id);
    }
    note("notification.create: ok");
  } finally {
    if (statusSet) {
      try {
        await sendSocketRequest(context.socketPath, "metadata.clear_status", {
          ...target,
          key: statusKey,
          [HOOK_EVENT_ORDER_PARAM]: incrementHookEventOrder(order),
        });
        note("metadata.clear_status: ok");
        statusSet = false;
      } catch (error) {
        process.stderr.write(
          terminalLine(`metadata.clear_status cleanup warning: ${formatErrorForTerminal(error)}`),
        );
      }
    }

    if (
      Array.isArray(notificationsBefore) &&
      notificationsBefore.length === 0 &&
      createdNotification?.id
    ) {
      try {
        const notificationsAfter = await sendSocketRequest(
          context.socketPath,
          "notification.list",
          {},
        );
        const onlyTestNotification =
          Array.isArray(notificationsAfter) &&
          notificationsAfter.length === 1 &&
          notificationsAfter[0]?.id === createdNotification.id;
        if (onlyTestNotification) {
          await sendSocketRequest(context.socketPath, "notification.clear", {});
          note("notification.clear: ok");
        } else {
          note("notification.clear: skipped because other notifications are present");
        }
      } catch (error) {
        process.stderr.write(
          terminalLine(`notification cleanup warning: ${formatErrorForTerminal(error)}`),
        );
      }
    } else if (createdNotification?.id) {
      note(
        `notification.clear: skipped; test notification remains as ${createdNotification.id}`,
      );
    }
  }

  note(`ForkTTY ${spec.label} hook test: ok`);
}

async function handleHooksDoctor(context, args) {
  const [agentName, ...extraArgs] = args;
  const agent = typeof agentName === "string" ? agentName.toLowerCase() : "";
  if (!agent) {
    throw new Error("hooks doctor requires an agent");
  }
  if (!AGENT_SPECS[agent]) {
    throw new Error(`Unsupported hook doctor agent: ${agentName}`);
  }
  requireNoCommandArgs(extraArgs, `hooks doctor ${agent}`);

  const spec = AGENT_SPECS[agent];
  const scriptPath = fileURLToPath(import.meta.url);
  const report = {
    agent,
    socket: {
      path: context.socketPath,
      source: socketPathSource(context),
      inspect: await inspectPath(context.socketPath, fsConstants.R_OK | fsConstants.W_OK),
    },
    env: {
      FORKTTY_SOCKET_PATH: trimEnv(context.env, "FORKTTY_SOCKET_PATH") || null,
      FORKTTY_WORKSPACE_ID: trimEnv(context.env, "FORKTTY_WORKSPACE_ID") || null,
      FORKTTY_SURFACE_ID: trimEnv(context.env, "FORKTTY_SURFACE_ID") || null,
      CODEX_HOME: trimEnv(context.env, "CODEX_HOME") || null,
      HOME: trimEnv(context.env, "HOME") || null,
    },
    executable: {
      node: await inspectPath(process.execPath, fsConstants.X_OK),
      script: await inspectPath(scriptPath, fsConstants.R_OK),
    },
    hookConfig: await inspectPath(spec.configPath(context.env), fsConstants.R_OK),
  };

  if (context.json) {
    printJson(report);
    return;
  }

  const note = (message) => process.stderr.write(terminalLine(message));
  note(`ForkTTY ${spec.label} hook doctor`);
  note(`socket source: ${report.socket.source}`);
  note(formatDoctorPath("socket", report.socket.inspect));
  note(formatDoctorPath("node", report.executable.node));
  note(formatDoctorPath("script", report.executable.script));
  note("environment:");
  for (const [key, value] of Object.entries(report.env)) {
    note(`  ${key}=${value || "(unset)"}`);
  }
  note(formatDoctorPath(`${agent} hook config`, report.hookConfig));
}

async function handleHookEvent(context, args) {
  const [agentName, eventName] = args;
  const agent = typeof agentName === "string" ? agentName.toLowerCase() : "";
  const event = typeof eventName === "string" ? eventName.toLowerCase() : "";
  const extraArgs = args.slice(2);

  if (!AGENT_SPECS[agent]) {
    process.stderr.write(terminalLine(`Unsupported hook agent: ${agentName}`));
    process.stdout.write(HOOK_CONTINUE_JSON);
    return;
  }
  if (!SUPPORTED_HOOK_EVENTS.has(event)) {
    const label = eventName === undefined ? "(missing)" : eventName;
    process.stderr.write(terminalLine(`Unsupported hook event for ${agent}: ${label}`));
    process.stdout.write(HOOK_CONTINUE_JSON);
    return;
  }
  if (extraArgs.length > 0) {
    process.stderr.write(
      terminalLine(`Unexpected hook argument for ${agent} ${event}: ${extraArgs[0]}`),
    );
    process.stdout.write(HOOK_CONTINUE_JSON);
    return;
  }

  const hookEventOrder = nextHookEventOrder();
  const payload = await readOptionalStdinJson();
  const actions = buildHookActions(agent, event, payload, context.env, hookEventOrder);
  hookDebug(
    context,
    `${agent} ${event} order=${hookEventOrder} actions=${actions.length} socket=${shouldSendHookActions(context) ? context.socketPath : "(disabled)"}`,
  );

  if (shouldSendHookActions(context)) {
    for (const action of actions) {
      try {
        hookDebug(context, summarizeHookAction(action));
        await sendSocketRequest(
          context.socketPath,
          action.method,
          action.params,
          HOOK_STATUS_TIMEOUT_MS,
        );
      } catch (error) {
        process.stderr.write(
          terminalLine(`ForkTTY hook warning: ${formatErrorForTerminal(error)}`),
        );
        break;
      }
    }
  }

  const enrichments = await gatherHookEnrichments(agent, event, payload, context);
  const tokenAction = buildTokenProgressAction(
    agent,
    enrichments.tokenUsage,
    context.env,
    hookEventOrder,
    event,
  );
  if (tokenAction && shouldSendHookActions(context)) {
    try {
      hookDebug(context, summarizeHookAction(tokenAction));
      await sendSocketRequest(
        context.socketPath,
        tokenAction.method,
        tokenAction.params,
        HOOK_STATUS_TIMEOUT_MS,
      );
    } catch (error) {
      hookDebug(
        context,
        `token progress send failed: ${formatErrorForTerminal(error)}`,
      );
    }
  }

  const response = buildHookResponse(agent, event, context.env, enrichments);
  process.stdout.write(`${JSON.stringify(response)}\n`);
}

const HOOK_TOKEN_CEILING_DEFAULT = 200_000;
const HOOK_PENDING_NOTIFICATION_LIMIT = 10;

function formatPendingNotificationLine(notification) {
  const kind = typeof notification?.kind === "string" ? notification.kind : "info";
  const title = typeof notification?.title === "string" ? notification.title : "(no title)";
  const body = typeof notification?.body === "string" && notification.body.trim()
    ? ` — ${notification.body.trim()}`
    : "";
  return `  [${kind}] ${title}${body}`;
}

function formatPendingNotificationsBlock(notifications) {
  if (!Array.isArray(notifications) || notifications.length === 0) return "";
  const lines = notifications
    .slice(0, HOOK_PENDING_NOTIFICATION_LIMIT)
    .map(formatPendingNotificationLine);
  const more =
    notifications.length > HOOK_PENDING_NOTIFICATION_LIMIT
      ? `  ...and ${notifications.length - HOOK_PENDING_NOTIFICATION_LIMIT} more (use \`forktty notifications\`).`
      : "";
  const tail = more ? [more] : [];
  return ["ForkTTY pending notifications:", ...lines, ...tail].join("\n");
}

function formatThousands(n) {
  return String(Math.trunc(n)).replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

function resolveTokenCeiling(env = process.env) {
  const raw = trimEnv(env, "FORKTTY_HOOK_TOKEN_CEILING");
  if (!raw) return HOOK_TOKEN_CEILING_DEFAULT;
  const parsed = Number(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) return HOOK_TOKEN_CEILING_DEFAULT;
  return Math.floor(parsed);
}

function formatTokenUsageBlock(usage, env = process.env) {
  if (!usage || typeof usage !== "object") return "";
  const input = Number(usage.input) || 0;
  const cacheRead = Number(usage.cacheRead) || 0;
  const cacheCreation = Number(usage.cacheCreation) || 0;
  const output = Number(usage.output) || 0;
  const total = input + cacheRead + cacheCreation;
  const ceiling = resolveTokenCeiling(env);
  const pct = ceiling > 0 ? Math.min(100, Math.round((total / ceiling) * 100)) : 0;
  return (
    `ForkTTY token estimate (latest assistant turn): ~${formatThousands(total)} / ${formatThousands(ceiling)} input tokens ` +
    `(${pct}% — input=${input}, cache_read=${cacheRead}, cache_creation=${cacheCreation}, output=${output}).`
  );
}

function buildHookResponse(agent, eventName, env = process.env, extras = {}) {
  const pending = Array.isArray(extras.pendingNotifications)
    ? extras.pendingNotifications
    : [];
  const usage = extras.tokenUsage || null;

  if (agent === "claude" && eventName === "session-start") {
    const workspaceId = trimEnv(env, "FORKTTY_WORKSPACE_ID") || "(none)";
    const surfaceId = trimEnv(env, "FORKTTY_SURFACE_ID") || "(none)";
    const socketPath = trimEnv(env, "FORKTTY_SOCKET_PATH") || "(default)";
    const sections = [
      `Running inside the ForkTTY terminal. ` +
        `workspace_id=${workspaceId} surface_id=${surfaceId} socket=${socketPath}. ` +
        `ForkTTY hooks publish status, logs, and notifications to the app for SessionStart, ` +
        `UserPromptSubmit, PreToolUse, PostToolUse, SubagentStop, PreCompact, Stop, Notification, and SessionEnd. ` +
        `Inspect state via the \`forktty\` CLI (notifications, status, workspaces, surfaces, worktrees).`,
    ];
    const pendingBlock = formatPendingNotificationsBlock(pending);
    if (pendingBlock) sections.push(pendingBlock);
    return {
      continue: true,
      suppressOutput: false,
      hookSpecificOutput: {
        hookEventName: "SessionStart",
        additionalContext: sections.join("\n\n"),
      },
    };
  }

  if (agent === "claude" && eventName === "prompt-submit") {
    const sections = [];
    const pendingBlock = formatPendingNotificationsBlock(pending);
    if (pendingBlock) sections.push(pendingBlock);
    const usageBlock = formatTokenUsageBlock(usage, env);
    if (usageBlock) sections.push(usageBlock);
    if (sections.length === 0) return HOOK_CONTINUE_RESPONSE;
    return {
      continue: true,
      suppressOutput: false,
      hookSpecificOutput: {
        hookEventName: "UserPromptSubmit",
        additionalContext: sections.join("\n\n"),
      },
    };
  }

  return HOOK_CONTINUE_RESPONSE;
}

const AGENT_SELF_NOTIFICATION_TITLES = new Set(
  Object.values(AGENT_SPECS).map((spec) => `${spec.label} needs input`),
);

function isAgentSelfFeedbackNotification(notification) {
  if (!notification || typeof notification !== "object") return false;
  if (notification.kind !== "prompt") return false;
  if (typeof notification.title !== "string") return false;
  return AGENT_SELF_NOTIFICATION_TITLES.has(notification.title.trim());
}

function filterPendingNotificationsForTarget(list, env = process.env) {
  if (!Array.isArray(list)) return [];
  const workspaceId = trimEnv(env, "FORKTTY_WORKSPACE_ID");
  return list.filter((notification) => {
    if (!notification || typeof notification !== "object") return false;
    if (notification.read === true) return false;
    if (isAgentSelfFeedbackNotification(notification)) return false;
    if (workspaceId && typeof notification.workspace_id === "string" && notification.workspace_id) {
      if (notification.workspace_id !== workspaceId) return false;
    }
    return true;
  });
}

async function readTokenUsageFromTranscript(filePath) {
  if (typeof filePath !== "string" || !filePath.trim()) return null;
  let fileHandle;
  try {
    const stat = await fs.stat(filePath);
    if (!stat.isFile()) return null;
    const size = stat.size;
    if (size === 0) return null;
    const chunkSize = Math.min(size, 64 * 1024);
    const offset = size - chunkSize;
    const buffer = Buffer.alloc(chunkSize);
    fileHandle = await fs.open(filePath, "r");
    await fileHandle.read(buffer, 0, chunkSize, offset);
    const text = buffer.toString("utf8");
    const lines = text.split("\n");
    for (let i = lines.length - 1; i >= 0; i--) {
      const raw = lines[i].trim();
      if (!raw) continue;
      let entry;
      try {
        entry = JSON.parse(raw);
      } catch {
        continue;
      }
      const usage = entry?.message?.usage || entry?.usage;
      if (usage && typeof usage === "object") {
        return {
          input: Number(usage.input_tokens) || 0,
          output: Number(usage.output_tokens) || 0,
          cacheRead: Number(usage.cache_read_input_tokens) || 0,
          cacheCreation: Number(usage.cache_creation_input_tokens) || 0,
        };
      }
    }
    return null;
  } catch {
    return null;
  } finally {
    if (fileHandle) {
      try {
        await fileHandle.close();
      } catch {
        // ignore close errors — best effort
      }
    }
  }
}

function buildTokenProgressAction(agent, usage, env, hookEventOrder, eventName) {
  if (!usage) return null;
  const spec = AGENT_SPECS[agent];
  if (!spec) return null;
  const total = (Number(usage.input) || 0) +
    (Number(usage.cacheRead) || 0) +
    (Number(usage.cacheCreation) || 0);
  if (total <= 0) return null;
  return {
    method: "metadata.set_progress",
    params: addHookEventMetadata(
      {
        ...buildHookTargetParams(env),
        key: `agent:${agent}:tokens`,
        label: `${spec.label} input tokens`,
        value: total,
        total: resolveTokenCeiling(env),
      },
      eventName,
      null,
      hookEventOrder,
    ),
  };
}

async function gatherHookEnrichments(agent, eventName, payload, context) {
  const enrichments = { pendingNotifications: [], tokenUsage: null };
  if (agent !== "claude") return enrichments;

  const wantsPending = eventName === "session-start" || eventName === "prompt-submit";
  if (wantsPending && shouldSendHookActions(context)) {
    try {
      const list = await sendSocketRequest(
        context.socketPath,
        "notification.list",
        {},
        HOOK_STATUS_TIMEOUT_MS,
      );
      enrichments.pendingNotifications = filterPendingNotificationsForTarget(list, context.env);
    } catch (error) {
      hookDebug(
        context,
        `notification.list failed: ${formatErrorForTerminal(error)}`,
      );
    }
  }

  if (eventName === "prompt-submit") {
    const transcriptPath = extractFirstStringLikeValue(payload, [
      "transcript_path",
      "transcriptPath",
    ]);
    if (transcriptPath) {
      enrichments.tokenUsage = await readTokenUsageFromTranscript(transcriptPath);
    }
  }

  return enrichments;
}

function shouldSendHookActions(context) {
  return (
    context.socketExplicit ||
    Boolean(socketPathFromEnv(context.env))
  );
}

async function inspectPath(filePath, accessMode = fsConstants.R_OK) {
  const result = {
    path: filePath,
    exists: false,
    kind: "missing",
    readable: false,
    writable: false,
    executable: false,
    mode: null,
    error: null,
  };
  try {
    const stat = await fs.lstat(filePath);
    result.exists = true;
    result.mode = `0${(stat.mode & 0o777).toString(8)}`;
    if (stat.isSymbolicLink()) {
      result.kind = "symlink";
    } else if (stat.isFile()) {
      result.kind = "file";
    } else if (stat.isDirectory()) {
      result.kind = "directory";
    } else if (typeof stat.isSocket === "function" && stat.isSocket()) {
      result.kind = "socket";
    } else {
      result.kind = "other";
    }
    await fs.access(filePath, fsConstants.R_OK).then(
      () => {
        result.readable = true;
      },
      () => {},
    );
    await fs.access(filePath, fsConstants.W_OK).then(
      () => {
        result.writable = true;
      },
      () => {},
    );
    await fs.access(filePath, fsConstants.X_OK).then(
      () => {
        result.executable = true;
      },
      () => {},
    );
    if (accessMode) {
      await fs.access(filePath, accessMode);
    }
  } catch (error) {
    result.error = error instanceof Error ? error.message : String(error);
  }
  return result;
}

function socketPathSource(context) {
  if (context.socketExplicit) return "argument";
  return socketPathFromEnv(context.env) ? "FORKTTY_SOCKET_PATH" : "default";
}

async function buildDoctorReport(context) {
  const scriptPath = fileURLToPath(import.meta.url);
  const hookConfigs = {};
  for (const [agent, spec] of Object.entries(AGENT_SPECS)) {
    hookConfigs[agent] = await inspectPath(spec.configPath(context.env), fsConstants.R_OK);
  }
  return {
    socket: {
      path: context.socketPath,
      source: socketPathSource(context),
      inspect: await inspectPath(context.socketPath, fsConstants.R_OK | fsConstants.W_OK),
    },
    env: {
      FORKTTY_SOCKET_PATH: trimEnv(context.env, "FORKTTY_SOCKET_PATH") || null,
      FORKTTY_WORKSPACE_ID: trimEnv(context.env, "FORKTTY_WORKSPACE_ID") || null,
      FORKTTY_SURFACE_ID: trimEnv(context.env, "FORKTTY_SURFACE_ID") || null,
      CODEX_HOME: trimEnv(context.env, "CODEX_HOME") || null,
      HOME: trimEnv(context.env, "HOME") || null,
    },
    executable: {
      node: await inspectPath(process.execPath, fsConstants.X_OK),
      script: await inspectPath(scriptPath, fsConstants.R_OK),
    },
    hookConfigs,
  };
}

function formatDoctorPath(label, info) {
  const details = [
    info.kind,
    info.exists ? "exists" : "missing",
    info.readable ? "readable" : "not readable",
    info.writable ? "writable" : "not writable",
    info.executable ? "executable" : "not executable",
  ];
  if (info.mode) details.push(info.mode);
  const error = info.error ? ` (${info.error})` : "";
  return `${label}: ${info.path} [${details.join(", ")}]${error}`;
}

async function handleDoctor(context, args = []) {
  requireNoCommandArgs(args, "doctor");
  const report = await buildDoctorReport(context);
  if (context.json) {
    printJson(report);
    return;
  }

  process.stdout.write(terminalLine("ForkTTY doctor"));
  process.stdout.write(terminalLine(`socket source: ${report.socket.source}`));
  process.stdout.write(terminalLine(formatDoctorPath("socket", report.socket.inspect)));
  process.stdout.write(terminalLine(formatDoctorPath("node", report.executable.node)));
  process.stdout.write(terminalLine(formatDoctorPath("script", report.executable.script)));
  process.stdout.write(terminalLine("environment:"));
  for (const [key, value] of Object.entries(report.env)) {
    process.stdout.write(terminalLine(`  ${key}=${value || "(unset)"}`));
  }
  process.stdout.write(terminalLine("hook configs:"));
  for (const [agent, info] of Object.entries(report.hookConfigs)) {
    process.stdout.write(terminalLine(`  ${formatDoctorPath(agent, info)}`));
  }
}

async function handlePing(context, args = []) {
  requireNoCommandArgs(args, "ping");
  const result = await sendSocketRequest(context.socketPath, "system.ping", {});
  if (context.json) {
    printJson({ result });
  } else {
    process.stdout.write(`${result}\n`);
  }
}

function parseGlobalArgs(argv, env = process.env) {
  const args = [];
  let json = false;
  let socketPath = defaultSocketPath(env);
  let socketExplicit = false;
  let help = false;
  let verbose = false;
  let stopGlobalParsing = false;

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];
    if (!stopGlobalParsing && token === "--" && args.length > 0) {
      stopGlobalParsing = true;
      args.push(token);
      continue;
    }
    if (!stopGlobalParsing && token === "--json") {
      json = true;
      continue;
    }
    if (!stopGlobalParsing && (token === "--verbose" || token === "--debug")) {
      verbose = true;
      continue;
    }
    if (!stopGlobalParsing && token === "--socket") {
      const next = argv[index + 1];
      if (typeof next !== "string" || !next.trim() || next.startsWith("--")) {
        throw new Error("--socket requires a value");
      }
      socketPath = next.trim();
      socketExplicit = true;
      index += 1;
      continue;
    }
    if (!stopGlobalParsing && typeof token === "string" && token.startsWith("--socket=")) {
      const explicitSocketPath = token.slice("--socket=".length).trim();
      if (!explicitSocketPath) throw new Error("--socket requires a value");
      socketPath = explicitSocketPath;
      socketExplicit = true;
      continue;
    }
    if (!stopGlobalParsing && token === "--help" && args.length === 0) {
      help = true;
      continue;
    }
    if (
      !stopGlobalParsing &&
      args.length === 0 &&
      typeof token === "string" &&
      token.startsWith("--")
    ) {
      throw new Error(`Unknown option: ${token}`);
    }
    args.push(token);
  }

  return { args, json, socketPath, socketExplicit, help, verbose };
}

async function main(argv = process.argv.slice(2), env = process.env) {
  const parsed = parseGlobalArgs(argv, env);
  const args = [...parsed.args];

  const command = args.shift();
  if (parsed.help || !command) {
    printHelp();
    return;
  }

  const context = {
    env,
    json: parsed.json,
    socketPath: parsed.socketPath,
    socketExplicit: parsed.socketExplicit,
    verbose: parsed.verbose,
  };

  switch (command) {
    case "list":
      await handleList(context, args);
      return;
    case "create-workspace":
      await handleCreateWorkspace(context, args);
      return;
    case "focus":
      await handleFocus(context, args);
      return;
    case "close-workspace":
      await handleCloseWorkspace(context, args);
      return;
    case "notify":
      await handleNotify(context, args);
      return;
    case "surfaces":
    case "surface-list":
    case "surface:list":
      await handleSurfaces(context, args);
      return;
    case "split-surface":
    case "surface-split":
    case "surface:split":
      await handleSplitSurface(context, args);
      return;
    case "focus-surface":
    case "surface-focus":
    case "surface:focus":
      await handleFocusSurface(context, args);
      return;
    case "close-surface":
    case "surface-close":
    case "surface:close":
      await handleCloseSurface(context, args);
      return;
    case "send-text":
    case "send_text":
      await handleSendText(context, args);
      return;
    case "worktree-list":
    case "worktree:list":
      await handleWorktreeList(context, args);
      return;
    case "worktree-status":
    case "worktree:status":
      await handleWorktreeStatus(context, args);
      return;
    case "worktree-create":
    case "worktree:create":
      await handleWorktreeCreate(context, args, "worktree.create");
      return;
    case "worktree-attach":
    case "worktree:attach":
      await handleWorktreeCreate(context, args, "worktree.attach");
      return;
    case "worktree-remove":
    case "worktree:remove":
      await handleWorktreeRemove(context, args);
      return;
    case "worktree-merge":
    case "worktree:merge":
      await handleWorktreeMerge(context, args);
      return;
    case "set-status":
      await handleSetStatus(context, args);
      return;
    case "list-status":
      await handleListStatus(context, args);
      return;
    case "clear-status":
      await handleClearStatus(context, args);
      return;
    case "set-progress":
      await handleSetProgress(context, args);
      return;
    case "list-progress":
      await handleListProgress(context, args);
      return;
    case "clear-progress":
      await handleClearProgress(context, args);
      return;
    case "log":
      await handleLog(context, args);
      return;
    case "logs":
    case "list-logs":
      await handleLogs(context, args);
      return;
    case "clear-logs":
      await handleClearLogs(context, args);
      return;
    case "notifications":
      await handleNotifications(context, args);
      return;
    case "clear-notifications":
    case "notifications-clear":
    case "notification:clear":
      await handleClearNotifications(context, args);
      return;
    case "hooks":
      if (args[0] === "setup") {
        await handleHooksSetup(context, args.slice(1));
      } else if (args[0] === "doctor") {
        await handleHooksDoctor(context, args.slice(1));
      } else if (args[0] === "test") {
        await handleHooksTest(context, args.slice(1));
      } else {
        await handleHookEvent(context, args);
      }
      return;
    case "doctor":
      await handleDoctor(context, args);
      return;
    case "ping":
      await handlePing(context, args);
      return;
    case "help":
      printHelp();
      return;
    default:
      throw new Error(`Unknown command: ${command}`);
  }
}

export {
  AGENT_SPECS,
  HELP_TEXT,
  HOOK_CONTINUE_RESPONSE,
  atomicWriteFile,
  buildCreateWorkspaceParams,
  buildClearMetadataParams,
  buildHookActions,
  buildHookResponse,
  buildHookShellCommand,
  buildHookTargetParams,
  buildTokenProgressAction,
  extractHookCompactTrigger,
  extractHookSource,
  extractHookToolError,
  extractHookToolName,
  filterPendingNotificationsForTarget,
  formatPendingNotificationsBlock,
  formatTokenUsageBlock,
  isAgentSelfFeedbackNotification,
  readTokenUsageFromTranscript,
  resolveTokenCeiling,
  buildLogParams,
  buildNotificationParams,
  buildProgressParams,
  buildStatusParams,
  buildSurfaceActionParams,
  buildSurfaceSplitParams,
  buildWorktreeStatusParams,
  defaultSocketPath,
  formatSocketConnectError,
  formatNotificationLine,
  handleHooksSetup,
  handleHooksDoctor,
  handleHooksTest,
  mergeHookConfig,
  main,
  parseGlobalArgs,
  parseFlags,
  readAgentConfig,
  resolveHookLauncherPath,
  resolveHookNodePath,
  resolveSelectorParams,
  sendSocketRequest,
  shellQuote,
  shouldSendHookActions,
  shouldReadCommandStdin,
  summarizeHookAction,
  sanitizeForTerminal,
  surfaceIdFromWorkspaceList,
  worktreeParams,
};

const isMainModule =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMainModule) {
  main().catch((error) => {
    process.stderr.write(`forktty: ${error.message}\n`);
    process.exitCode = 1;
  });
}
