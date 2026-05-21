#!/usr/bin/env node

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
const VALID_NOTIFICATION_KINDS = new Set(["prompt", "error", "info", "custom"]);
const VALID_STATUS_COLORS = new Set(["green", "yellow", "red", "blue", "muted"]);
const VALID_LOG_LEVELS = new Set(["info", "warn", "error"]);
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
      ["AfterAgent", "stop", 5000],
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

function defaultSocketPath(env = process.env) {
  if (typeof env.FORKTTY_SOCKET_PATH === "string" && env.FORKTTY_SOCKET_PATH.trim()) {
    return env.FORKTTY_SOCKET_PATH.trim();
  }

  if (typeof env.XDG_RUNTIME_DIR === "string" && env.XDG_RUNTIME_DIR.startsWith("/")) {
    return path.join(env.XDG_RUNTIME_DIR, "forktty.sock");
  }

  const uid =
    typeof process.getuid === "function" ? String(process.getuid()) : env.UID || "unknown";
  return path.join(os.tmpdir(), `forktty-${uid}`, "forktty.sock");
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
          ? `${response.error.code}: `
          : "";
      finish(
        new Error(
          `Socket request failed for ${method} at ${socketPath}: ${errorCode}${String(errorMessage)}`,
        ),
      );
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
      `Cannot reach ForkTTY at ${socketPath} (${code}). Start the app with: cargo run -p forktty-ui-gtk --features gtk-vte. If ForkTTY is already running, set FORKTTY_SOCKET_PATH or pass --socket <path>.`,
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
  return error;
}

function parseFlags(args) {
  const options = {};
  const positionals = [];

  for (let index = 0; index < args.length; index += 1) {
    const token = args[index];
    if (!token.startsWith("--")) {
      positionals.push(token);
      continue;
    }

    const raw = token.slice(2);
    if (!raw) continue;

    const eqIndex = raw.indexOf("=");
    if (eqIndex >= 0) {
      const key = raw.slice(0, eqIndex);
      options[key] = raw.slice(eqIndex + 1);
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

function buildTargetParams(options, env = process.env) {
  const params = {};
  if (typeof options["workspace-id"] === "string") {
    params.workspace_id = options["workspace-id"];
  } else if (typeof options["workspace-name"] === "string") {
    params.workspace_name = options["workspace-name"];
  } else if (typeof options["worktree-name"] === "string") {
    params.worktreeName = options["worktree-name"];
  } else if (typeof env.FORKTTY_WORKSPACE_ID === "string" && env.FORKTTY_WORKSPACE_ID) {
    params.workspace_id = env.FORKTTY_WORKSPACE_ID;
  }
  return params;
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

function buildProgressParams(options, env = process.env) {
  const key = typeof options.key === "string" ? options.key.trim() : "";
  const label =
    typeof options.label === "string" && options.label.trim() ? options.label.trim() : key;

  if (!key) throw new Error("set-progress requires --key");
  if (typeof options.value !== "string") throw new Error("set-progress requires --value");

  const value = parseFiniteNumber(options.value, "--value");
  const params = {
    ...buildTargetParams(options, env),
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
    ...buildTargetParams(options, env),
    level,
    message,
  };
}

function printJson(value) {
  process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
}

function printHelp() {
  process.stdout.write(`ForkTTY CLI

Usage:
  ./scripts/forktty.mjs list [--json]
  ./scripts/forktty.mjs create-workspace [--name <name>] [--working-dir <path>] [--json]
  ./scripts/forktty.mjs focus <selector>
  ./scripts/forktty.mjs focus --workspace-id <id>
  ./scripts/forktty.mjs close-workspace <selector>
  ./scripts/forktty.mjs notify [message] [--title <title>] [--kind <kind>]
  ./scripts/forktty.mjs surfaces [--workspace-id <id>|--workspace-name <name>|--worktree-name <name>] [--json]
  ./scripts/forktty.mjs split-surface [--surface-id <id>] [--axis horizontal|vertical]
  ./scripts/forktty.mjs focus-surface <surface-id>
  ./scripts/forktty.mjs close-surface <surface-id>
  ./scripts/forktty.mjs send-text <text> [--surface-id <id>]
  ./scripts/forktty.mjs worktree-list [--cwd <repo>]
  ./scripts/forktty.mjs worktree-status [--path <worktree>] [--cwd <worktree>]
  ./scripts/forktty.mjs worktree-create <branch> [--cwd <repo>]
  ./scripts/forktty.mjs worktree-attach <branch> [--cwd <repo>]
  ./scripts/forktty.mjs worktree-remove <branch-or-worktree> [--cwd <repo>]
  ./scripts/forktty.mjs worktree-merge <branch-or-worktree> [--cwd <repo>]
  ./scripts/forktty.mjs set-status --key <key> --value <value> [--label <label>] [--color <color>]
  ./scripts/forktty.mjs list-status [--workspace-id <id>]
  ./scripts/forktty.mjs clear-status [--key <key>]
  ./scripts/forktty.mjs set-progress --key <key> --value <number> [--label <label>] [--total <number>]
  ./scripts/forktty.mjs list-progress [--workspace-id <id>]
  ./scripts/forktty.mjs clear-progress [--key <key>]
  ./scripts/forktty.mjs log [message] [--message <message>] [--level info|warn|error]
  ./scripts/forktty.mjs logs [--workspace-id <id>]
  ./scripts/forktty.mjs clear-logs [--workspace-id <id>]
  ./scripts/forktty.mjs notifications [--json]
  ./scripts/forktty.mjs clear-notifications
  ./scripts/forktty.mjs hooks setup [codex] [claude] [gemini]
  ./scripts/forktty.mjs hooks <agent> <event>
  ./scripts/forktty.mjs ping

Selector flags:
  --workspace-id <id>
  --workspace-name <name>
  --worktree-name <name>

Notes:
  - The CLI defaults to FORKTTY_SOCKET_PATH when present, then the app default socket path.
  - Inside a ForkTTY terminal, FORKTTY_WORKSPACE_ID is used automatically for notify and metadata commands.
  - Worktree commands default --cwd to PWD so repo operations follow the shell you run them from.
  - send-text targets --surface-id, then FORKTTY_SURFACE_ID, then the active workspace's focused surface.
  - Hook commands always return a continue JSON payload and never fail the agent hook pipeline.
`);
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

async function handleList(context) {
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
  const { options } = parseFlags(args);
  const params = {};

  if (typeof options.name === "string" && options.name.trim()) {
    params.name = options.name.trim();
  }
  if (typeof options["working-dir"] === "string" && options["working-dir"].trim()) {
    params.workingDir = options["working-dir"].trim();
  }

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

async function handleFocus(context, args) {
  const { options, positionals } = parseFlags(args);

  if (typeof options["workspace-id"] === "string") {
    await tryWorkspaceSelect(context, { id: options["workspace-id"] });
  } else if (typeof options["workspace-name"] === "string") {
    await tryWorkspaceSelect(context, { name: options["workspace-name"] });
  } else if (typeof options["worktree-name"] === "string") {
    await tryWorkspaceSelect(context, { worktreeName: options["worktree-name"] });
  } else if (positionals[0]) {
    const selector = positionals[0];
    try {
      await tryWorkspaceSelect(context, { id: selector });
    } catch {
      await tryWorkspaceSelect(context, { name: selector });
    }
  } else {
    throw new Error("focus requires a selector or --workspace-id/--workspace-name");
  }

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Focused workspace\n");
  }
}

function resolveSelectorParams(options, positionals, env = process.env) {
  if (typeof options["workspace-id"] === "string") {
    return [{ id: options["workspace-id"] }];
  }
  if (typeof options["workspace-name"] === "string") {
    return [{ name: options["workspace-name"] }];
  }
  if (typeof options["worktree-name"] === "string") {
    return [{ worktreeName: options["worktree-name"] }];
  }
  if (positionals[0]) {
    const selector = positionals[0];
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
  const candidates = resolveSelectorParams(options, positionals, context.env);
  if (!candidates) {
    throw new Error("close-workspace requires a selector or --workspace-id");
  }

  let lastError;
  for (const params of candidates) {
    try {
      await sendSocketRequest(context.socketPath, "workspace.close", params);
      lastError = undefined;
      break;
    } catch (error) {
      lastError = error;
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
  const stdinText = await readStdinText();
  const body =
    typeof options.body === "string"
      ? options.body
      : positionals.length > 0
        ? positionals.join(" ")
        : stdinText.trim();
  const title = typeof options.title === "string" ? options.title : "ForkTTY";
  const kind = typeof options.kind === "string" ? options.kind : "info";

  if (!VALID_NOTIFICATION_KINDS.has(kind)) {
    throw new Error(`Invalid kind: ${kind}`);
  }

  await sendSocketRequest(context.socketPath, "notification.create", {
    ...buildTargetParams(options, context.env),
    title,
    body,
    kind,
  });

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Sent ${kind} notification\n`);
  }
}

async function handleSendText(context, args) {
  const { options, positionals } = parseFlags(args);
  const stdinText = await readStdinText();
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

function surfaceIdFromArgs(options, positionals, env = process.env) {
  if (typeof options["surface-id"] === "string" && options["surface-id"].trim()) {
    return options["surface-id"].trim();
  }
  if (positionals[0]) {
    return positionals[0].trim();
  }
  if (typeof env.FORKTTY_SURFACE_ID === "string" && env.FORKTTY_SURFACE_ID.trim()) {
    return env.FORKTTY_SURFACE_ID.trim();
  }
  return "";
}

function buildSurfaceActionParams(options, positionals, env = process.env, command = "surface") {
  const surfaceId = surfaceIdFromArgs(options, positionals, env);
  if (!surfaceId) {
    throw new Error(`${command} requires --surface-id, a surface id, or FORKTTY_SURFACE_ID`);
  }
  return { surface_id: surfaceId };
}

function buildSurfaceSplitParams(options, positionals, env = process.env) {
  const axis =
    typeof options.axis === "string" && options.axis.trim() ? options.axis.trim() : "horizontal";
  if (axis !== "horizontal" && axis !== "vertical") {
    throw new Error("Invalid --axis: expected horizontal or vertical");
  }
  const params = { axis };
  const surfaceId = surfaceIdFromArgs(options, positionals, env);
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
  const { options } = parseFlags(args);
  const result = await sendSocketRequest(context.socketPath, "surface.list", {
    ...buildTargetParams(options, context.env),
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

function worktreeParams(options, positionals, requireName = false, env = process.env) {
  const params = {};
  const name = positionals[0] || options.name || options.branch;
  if (requireName && (!name || typeof name !== "string")) {
    throw new Error("worktree command requires a branch or worktree name");
  }
  if (typeof name === "string" && name.trim()) {
    params.name = name.trim();
  }
  if (typeof options.cwd === "string" && options.cwd.trim()) {
    params.cwd = options.cwd.trim();
  } else if (typeof env.PWD === "string" && env.PWD.trim()) {
    params.cwd = env.PWD.trim();
  }
  return params;
}

function buildWorktreeStatusParams(options, positionals, env = process.env) {
  const pathValue =
    typeof options.path === "string" && options.path.trim()
      ? options.path.trim()
      : typeof options.cwd === "string" && options.cwd.trim()
        ? options.cwd.trim()
        : positionals[0]
          ? positionals[0].trim()
          : typeof env.PWD === "string" && env.PWD.trim()
            ? env.PWD.trim()
            : "";

  if (!pathValue) {
    throw new Error("worktree-status requires --path, --cwd, a path, or PWD");
  }
  return { path: pathValue };
}

function formatWorktreeLine(worktree) {
  const status = worktree.status ? ` ${worktree.status}` : "";
  return `${worktree.branch || worktree.name} [${worktree.worktree_name}] ${worktree.path}${status}`;
}

async function handleWorktreeList(context, args) {
  const { options } = parseFlags(args);
  const result = await sendSocketRequest(
    context.socketPath,
    "worktree.list",
    worktreeParams(options, [], false, context.env),
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
  const result = await sendSocketRequest(
    context.socketPath,
    method,
    worktreeParams(options, positionals, true, context.env),
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
    worktreeParams(options, positionals, true, context.env),
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
    worktreeParams(options, positionals, true, context.env),
  );
  if (context.json) {
    printJson(result);
    return;
  }
  process.stdout.write(`${result}\n`);
}

async function handleSetStatus(context, args) {
  const { options } = parseFlags(args);
  const key = typeof options.key === "string" ? options.key : "";
  const value = typeof options.value === "string" ? options.value : "";
  const label =
    typeof options.label === "string" && options.label.trim() ? options.label : key;
  const color = typeof options.color === "string" ? options.color : undefined;

  if (!key) throw new Error("set-status requires --key");
  if (!value) throw new Error("set-status requires --value");
  if (color && !VALID_STATUS_COLORS.has(color) && !color.startsWith("#")) {
    throw new Error(`Unsupported status color: ${color}`);
  }

  await sendSocketRequest(context.socketPath, "metadata.set_status", {
    ...buildTargetParams(options, context.env),
    key,
    label,
    value,
    color,
  });

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Updated status ${key}\n`);
  }
}

function formatStatusLine(status) {
  const color = status.color ? ` (${status.color})` : "";
  return `${status.label}: ${status.value}${color}`;
}

async function handleListStatus(context, args) {
  const { options } = parseFlags(args);
  const result = await sendSocketRequest(context.socketPath, "metadata.list_status", {
    ...buildTargetParams(options, context.env),
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
  const { options } = parseFlags(args);
  await sendSocketRequest(context.socketPath, "metadata.clear_status", {
    ...buildTargetParams(options, context.env),
    ...(typeof options.key === "string" ? { key: options.key } : {}),
  });

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared status\n");
  }
}

async function handleSetProgress(context, args) {
  const { options } = parseFlags(args);
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
  const { options } = parseFlags(args);
  const result = await sendSocketRequest(context.socketPath, "metadata.list_progress", {
    ...buildTargetParams(options, context.env),
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
  const { options } = parseFlags(args);
  await sendSocketRequest(context.socketPath, "metadata.clear_progress", {
    ...buildTargetParams(options, context.env),
    ...(typeof options.key === "string" ? { key: options.key } : {}),
  });

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
  const stdinText = await readStdinText();
  const params = buildLogParams(options, positionals, stdinText, context.env);
  await sendSocketRequest(context.socketPath, "metadata.log", params);

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write(`Appended ${params.level} log\n`);
  }
}

async function handleLogs(context, args) {
  const { options } = parseFlags(args);
  const result = await sendSocketRequest(context.socketPath, "metadata.list_logs", {
    ...buildTargetParams(options, context.env),
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
  const { options } = parseFlags(args);
  await sendSocketRequest(context.socketPath, "metadata.clear_logs", {
    ...buildTargetParams(options, context.env),
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

async function handleNotifications(context) {
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

async function handleClearNotifications(context) {
  await sendSocketRequest(context.socketPath, "notification.clear", {});

  if (context.json) {
    printJson({ result: true });
  } else {
    process.stdout.write("Cleared notifications\n");
  }
}

function buildHookShellCommand(scriptPath, agent, event) {
  const spec = AGENT_SPECS[agent];
  const disabledGuard = `[ "\${${spec.disabledEnv}:-}" != "1" ]`;
  const command = `node ${shellQuote(scriptPath)} hooks ${agent} ${event}`;
  return `${disabledGuard} && ${command} || echo '${HOOK_CONTINUE_JSON.trimEnd()}'`;
}

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
  };
  if (matcher) {
    entry.matcher = matcher;
  }
  return entry;
}

function deepCloneJson(value) {
  return value === undefined ? {} : JSON.parse(JSON.stringify(value));
}

function mergeHookConfig(existingConfig, agent, scriptPath) {
  const spec = AGENT_SPECS[agent];
  if (!spec) {
    throw new Error(`Unsupported agent: ${agent}`);
  }

  const nextConfig = isObject(existingConfig) ? deepCloneJson(existingConfig) : {};
  const hooks = isObject(nextConfig.hooks) ? { ...nextConfig.hooks } : {};
  let changed = !isObject(nextConfig.hooks);

  for (const [eventName, hookEventName, timeout] of spec.hookEntries) {
    const statusMessage = `ForkTTY ${spec.label} hooks`;
    const command = buildHookShellCommand(scriptPath, agent, hookEventName);
    const nextEntry = buildHookEntry(command, statusMessage, timeout, spec.matcher);
    const existingEntries = Array.isArray(hooks[eventName]) ? [...hooks[eventName]] : [];
    const alreadyPresent = existingEntries.some((entry) =>
      Array.isArray(entry?.hooks)
        ? entry.hooks.some((hook) => hook?.type === "command" && hook.command === command)
        : false,
    );
    if (!alreadyPresent) {
      existingEntries.push(nextEntry);
      hooks[eventName] = existingEntries;
      changed = true;
    }
  }

  nextConfig.hooks = hooks;
  return { changed, config: nextConfig };
}

async function readJsonFile(filePath) {
  try {
    const text = await fs.readFile(filePath, "utf8");
    if (!text.trim()) return {};
    return JSON.parse(text);
  } catch (error) {
    if (error && typeof error === "object" && error.code === "ENOENT") {
      return {};
    }
    throw error;
  }
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
  return positionals.map((name) => {
    const normalized = name.toLowerCase();
    if (!AGENT_SPECS[normalized]) {
      throw new Error(`Unsupported agent: ${name}`);
    }
    return normalized;
  });
}

async function handleHooksSetup(context, args) {
  const { options, positionals } = parseFlags(args);
  const dryRun = options["dry-run"] === true || options["dry-run"] === "true";
  const agentNames = supportedAgents(positionals);
  const scriptPath = fileURLToPath(import.meta.url);

  const summaries = [];
  for (const agent of agentNames) {
    const spec = AGENT_SPECS[agent];
    const configPath = spec.configPath(context.env);
    const existing = await readAgentConfig(agent, configPath);
    const { changed, config } = mergeHookConfig(existing, agent, scriptPath);

    let backupPath = null;
    if (changed && !dryRun) {
      await ensureParentDir(configPath);
      backupPath = await backupFile(configPath);
      await atomicWriteFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
    }

    summaries.push({
      agent,
      configPath,
      changed,
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

function buildHookActions(agent, eventName, payload, env = process.env) {
  const spec = AGENT_SPECS[agent];
  if (!spec) {
    throw new Error(`Unsupported agent: ${agent}`);
  }

  const workspaceId =
    typeof env.FORKTTY_WORKSPACE_ID === "string" && env.FORKTTY_WORKSPACE_ID.trim()
      ? env.FORKTTY_WORKSPACE_ID.trim()
      : "";
  const target = workspaceId ? { workspace_id: workspaceId } : {};
  const key = `agent:${agent}`;
  const message = extractHookMessage(payload);
  const log = (level, logMessage) => ({
    method: "metadata.log",
    params: {
      ...target,
      level,
      message: logMessage,
    },
  });

  switch (eventName) {
    case "session-start":
      return [
        log("info", `${spec.label} session started`),
        {
          method: "metadata.set_status",
          params: {
            ...target,
            key,
            label: spec.label,
            value: "Ready",
            color: "green",
          },
        },
      ];
    case "prompt-submit":
      return [
        log("info", `${spec.label} prompt submitted`),
        {
          method: "metadata.set_status",
          params: {
            ...target,
            key,
            label: spec.label,
            value: "Running",
            color: "blue",
          },
        },
      ];
    case "notification":
      return [
        log("warn", message || `${spec.label} requested attention`),
        {
          method: "metadata.set_status",
          params: {
            ...target,
            key,
            label: spec.label,
            value: "Needs input",
            color: "yellow",
          },
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
          params: {
            ...target,
            key,
            label: spec.label,
            value: "Error",
            color: "red",
          },
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
    case "stop":
      return [
        log("info", message || `${spec.label} stopped`),
        {
          method: "metadata.set_status",
          params: {
            ...target,
            key,
            label: spec.label,
            value: "Ready",
            color: "green",
          },
        },
      ];
    case "session-end":
      return [
        log("info", `${spec.label} session ended`),
        {
          method: "metadata.clear_status",
          params: {
            ...target,
            key,
          },
        },
      ];
    default:
      return [];
  }
}

async function handleHookEvent(context, args) {
  const [agentName, eventName] = args;
  const agent = typeof agentName === "string" ? agentName.toLowerCase() : "";
  const event = typeof eventName === "string" ? eventName.toLowerCase() : "";

  if (!AGENT_SPECS[agent]) {
    process.stderr.write(`Unsupported hook agent: ${agentName}\n`);
    process.stdout.write(HOOK_CONTINUE_JSON);
    return;
  }

  const payload = await readOptionalStdinJson();
  const actions = buildHookActions(agent, event, payload, context.env);

  const hasSocketPath =
    typeof context.env.FORKTTY_SOCKET_PATH === "string" && context.env.FORKTTY_SOCKET_PATH.trim();

  if (hasSocketPath) {
    for (const action of actions) {
      try {
        await sendSocketRequest(
          context.socketPath,
          action.method,
          action.params,
          HOOK_STATUS_TIMEOUT_MS,
        );
      } catch (error) {
        process.stderr.write(`ForkTTY hook warning: ${error}\n`);
        break;
      }
    }
  }

  process.stdout.write(HOOK_CONTINUE_JSON);
}

async function handlePing(context) {
  const result = await sendSocketRequest(context.socketPath, "system.ping", {});
  if (context.json) {
    printJson({ result });
  } else {
    process.stdout.write(`${result}\n`);
  }
}

async function main(argv = process.argv.slice(2), env = process.env) {
  const args = [...argv];
  let json = false;
  let socketPath = defaultSocketPath(env);

  while (args[0]?.startsWith("--")) {
    const token = args.shift();
    if (token === "--json") {
      json = true;
      continue;
    }
    if (token === "--help") {
      printHelp();
      return;
    }
    if (token === "--socket") {
      const next = args.shift();
      if (!next) throw new Error("--socket requires a value");
      socketPath = next;
      continue;
    }
    if (token.startsWith("--socket=")) {
      socketPath = token.slice("--socket=".length);
      continue;
    }
    throw new Error(`Unknown option: ${token}`);
  }

  const command = args.shift();
  if (!command) {
    printHelp();
    return;
  }

  const context = {
    env,
    json,
    socketPath,
  };

  switch (command) {
    case "list":
      await handleList(context);
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
      await handleNotifications(context);
      return;
    case "clear-notifications":
    case "notifications-clear":
    case "notification:clear":
      await handleClearNotifications(context);
      return;
    case "hooks":
      if (args[0] === "setup") {
        await handleHooksSetup(context, args.slice(1));
      } else {
        await handleHookEvent(context, args);
      }
      return;
    case "ping":
      await handlePing(context);
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
  HOOK_CONTINUE_RESPONSE,
  atomicWriteFile,
  buildHookActions,
  buildHookShellCommand,
  buildLogParams,
  buildProgressParams,
  buildSurfaceActionParams,
  buildSurfaceSplitParams,
  buildWorktreeStatusParams,
  defaultSocketPath,
  formatSocketConnectError,
  formatNotificationLine,
  handleHooksSetup,
  mergeHookConfig,
  parseFlags,
  readAgentConfig,
  resolveSelectorParams,
  sendSocketRequest,
  shellQuote,
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
