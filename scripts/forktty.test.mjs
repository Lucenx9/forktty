import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { Readable } from "node:stream";
import { afterEach, beforeEach, describe, it } from "node:test";

import {
  atomicWriteFile,
  buildClearMetadataParams,
  buildCreateWorkspaceParams,
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
  HELP_TEXT,
  HOOK_CONTINUE_RESPONSE,
  handleHooksDoctor,
  handleHooksSetup,
  handleHooksTest,
  main,
  mergeHookConfig,
  parseGlobalArgs,
  parseFlags,
  readAgentConfig,
  resolveHookLauncherPath,
  resolveHookNodePath,
  resolveSelectorParams,
  sendSocketRequest,
  sanitizeForTerminal,
  surfaceIdFromWorkspaceList,
  shouldSendHookActions,
  shouldReadCommandStdin,
  summarizeHookAction,
  worktreeParams,
} from "./forktty.mjs";

async function withSocketServer(handler, callback) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-socket-test-"));
  const socketPath = path.join(dir, "forktty.sock");
  const sockets = new Set();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    handler(socket);
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, () => {
      server.off("error", reject);
      resolve();
    });
  });
  try {
    return await callback(socketPath);
  } finally {
    for (const socket of sockets) {
      socket.destroy();
    }
    await new Promise((resolve) => server.close(resolve));
    await fs.rm(dir, { recursive: true, force: true });
  }
}

describe("forktty CLI helpers", () => {
  it("builds the default socket path from XDG runtime dir", () => {
    assert.equal(
      defaultSocketPath({
        XDG_RUNTIME_DIR: "/run/user/1000",
      }),
      "/run/user/1000/forktty.sock",
    );
    assert.equal(
      defaultSocketPath({
        XDG_RUNTIME_DIR: " /run/user/1000 ",
      }),
      "/run/user/1000/forktty.sock",
    );
    assert.equal(
      defaultSocketPath({
        FORKTTY_SOCKET_PATH: "relative.sock",
        XDG_RUNTIME_DIR: "/run/user/1000",
      }),
      "/run/user/1000/forktty.sock",
    );
  });

  it("falls back to a user temp socket path", () => {
    const socketPath = defaultSocketPath({});
    assert.equal(socketPath.startsWith(os.tmpdir()), true);
    assert.equal(socketPath.endsWith("forktty.sock"), true);
  });

  it("explains how to recover when the app socket is missing", () => {
    const raw = Object.assign(new Error("connect ENOENT"), { code: "ENOENT" });
    const error = formatSocketConnectError(raw, "/tmp/forktty.sock");

    assert.match(error.message, /Cannot reach ForkTTY at \/tmp\/forktty\.sock/);
    assert.match(error.message, /cargo run -p forktty-ui-gtk --features gtk-vte/);
    assert.match(error.message, /FORKTTY_SOCKET_PATH to an absolute path/);
    assert.equal(error.cause, raw);
  });

  it("documents that env socket overrides must be absolute", () => {
    assert.match(HELP_TEXT, /absolute FORKTTY_SOCKET_PATH/);
  });

  it("explains socket permission failures without hiding the path", () => {
    const raw = Object.assign(new Error("connect EACCES"), { code: "EACCES" });
    const error = formatSocketConnectError(raw, "/run/user/1000/forktty.sock");

    assert.match(error.message, /Cannot access ForkTTY socket/);
    assert.match(error.message, /\/run\/user\/1000\/forktty\.sock/);
    assert.equal(error.cause, raw);
  });

  it("keeps path and code on unexpected socket errors", () => {
    const raw = Object.assign(new Error("read ECONNRESET"), { code: "ECONNRESET" });
    const error = formatSocketConnectError(raw, "/tmp/forktty.sock");

    assert.match(error.message, /ForkTTY socket error at \/tmp\/forktty\.sock/);
    assert.match(error.message, /ECONNRESET/);
    assert.match(error.message, /read ECONNRESET/);
    assert.equal(error.cause, raw);
  });

  it("keeps socket path and error code in failed socket responses", async () => {
    await withSocketServer(
      (socket) => {
        socket.once("data", (chunk) => {
          const request = JSON.parse(String(chunk).trim());
          socket.end(
            `${JSON.stringify({
              id: request.id,
              ok: false,
              error: { code: "not_found", message: "Workspace not found" },
            })}\n`,
          );
        });
      },
      async (socketPath) => {
        await assert.rejects(
          sendSocketRequest(socketPath, "workspace.select", { id: "missing" }),
          (error) =>
            error.code === "not_found" &&
            error.message.includes(socketPath) &&
            /workspace\.select/.test(error.message) &&
            /not_found: Workspace not found/.test(error.message),
        );
      },
    );
  });

  it("does not fall back to workspace name after a non-not-found focus failure", async () => {
    let requestCount = 0;
    await withSocketServer(
      (socket) => {
        socket.once("data", (chunk) => {
          requestCount += 1;
          const request = JSON.parse(String(chunk).trim());
          socket.end(
            `${JSON.stringify({
              id: request.id,
              ok: false,
              error: { code: "error", message: "spawn failed" },
            })}\n`,
          );
        });
      },
      async (socketPath) => {
        await assert.rejects(
          main(["focus", "workspace-1", "--socket", socketPath], {}),
          /spawn failed/,
        );
      },
    );

    assert.equal(requestCount, 1);
  });

  it("rejects socket responses with the wrong request id", async () => {
    await withSocketServer(
      (socket) => {
        socket.once("data", () => {
          socket.end(
            `${JSON.stringify({
              id: "stale-response",
              ok: true,
              result: "pong",
            })}\n`,
          );
        });
      },
      async (socketPath) => {
        await assert.rejects(
          sendSocketRequest(socketPath, "system.ping", {}),
          (error) =>
            error.message.includes(socketPath) &&
            /system\.ping/.test(error.message) &&
            /response id mismatch/.test(error.message) &&
            /stale-response/.test(error.message),
        );
      },
    );
  });

  it("keeps id-null connection-level socket error codes", async () => {
    await withSocketServer(
      (socket) => {
        socket.once("data", () => {
          socket.end(
            `${JSON.stringify({
              id: null,
              ok: false,
              error: { code: "request_too_large", message: "Request exceeds 1 MiB" },
            })}\n`,
          );
        });
      },
      async (socketPath) => {
        await assert.rejects(
          sendSocketRequest(socketPath, "surface.send_text", { text: "x" }),
          (error) =>
            error.message.includes(socketPath) &&
            /surface\.send_text/.test(error.message) &&
            /request_too_large: Request exceeds 1 MiB/.test(error.message) &&
            !/response id mismatch/.test(error.message),
        );
      },
    );
  });

  it("keeps socket path in invalid socket response diagnostics", async () => {
    await withSocketServer(
      (socket) => {
        socket.end("not json\n");
      },
      async (socketPath) => {
        await assert.rejects(
          sendSocketRequest(socketPath, "system.ping", {}),
          (error) =>
            error.message.includes(socketPath) &&
            /system\.ping/.test(error.message) &&
            /Invalid socket response/.test(error.message),
        );
      },
    );
  });

  it("formats global notifications without leaking undefined workspace text", () => {
    assert.equal(
      formatNotificationLine({
        read: false,
        kind: "info",
        title: "Smoke",
        body: "GTK",
      }),
      "[unread] global · info · Smoke — GTK",
    );
  });

  it("accepts global socket and json flags after the command", () => {
    assert.deepEqual(
      parseGlobalArgs(["ping", "--socket", "/tmp/stub.sock", "--json"], {}),
      {
        args: ["ping"],
        json: true,
        socketPath: "/tmp/stub.sock",
        socketExplicit: true,
        help: false,
        verbose: false,
      },
    );
    assert.deepEqual(
      parseGlobalArgs(
        ["worktree-create", "feature/x", "--cwd", "/repo", "--socket=/tmp/forktty.sock"],
        {},
      ),
      {
        args: ["worktree-create", "feature/x", "--cwd", "/repo"],
        json: false,
        socketPath: "/tmp/forktty.sock",
        socketExplicit: true,
        help: false,
        verbose: false,
      },
    );
    assert.deepEqual(
      parseGlobalArgs(["ping", "--socket", " /tmp/trimmed.sock ", "--verbose"], {}),
      {
        args: ["ping"],
        json: false,
        socketPath: "/tmp/trimmed.sock",
        socketExplicit: true,
        help: false,
        verbose: true,
      },
    );
    assert.throws(
      () => parseGlobalArgs(["ping", "--socket="], {}),
      /--socket requires a value/,
    );
    assert.throws(
      () => parseGlobalArgs(["ping", "--socket", "--json"], {}),
      /--socket requires a value/,
    );
  });

  it("does not parse global flags after a command option terminator", () => {
    assert.deepEqual(
      parseGlobalArgs(["send-text", "--", "--socket", "literal", "--json"], {
        XDG_RUNTIME_DIR: "/run/user/1000",
      }),
      {
        args: ["send-text", "--", "--socket", "literal", "--json"],
        json: false,
        socketPath: "/run/user/1000/forktty.sock",
        socketExplicit: false,
        help: false,
        verbose: false,
      },
    );
  });

  it("exposes a minimal doctor command from the Node CLI", async () => {
    const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-doctor-"));
    const stdout = [];
    const originalStdout = process.stdout.write.bind(process.stdout);
    process.stdout.write = (chunk) => {
      stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await main(["doctor"], { HOME: tmpDir, XDG_RUNTIME_DIR: tmpDir });
    } finally {
      process.stdout.write = originalStdout;
      await fs.rm(tmpDir, { recursive: true, force: true });
    }

    const output = stdout.join("");
    assert.match(output, /ForkTTY doctor/);
    assert.match(output, /hook configs:/);
    assert.match(output, /codex:/);
  });

  it("exposes hooks doctor codex without hook protocol stdout", async () => {
    const tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-hooks-doctor-"));
    const stdout = [];
    const stderr = [];
    const originalStdout = process.stdout.write.bind(process.stdout);
    const originalStderr = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk) => {
      stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    process.stderr.write = (chunk) => {
      stderr.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await handleHooksDoctor(
        {
          env: { HOME: tmpDir, XDG_RUNTIME_DIR: tmpDir },
          json: false,
          socketPath: path.join(tmpDir, "forktty.sock"),
          socketExplicit: false,
        },
        ["codex"],
      );
    } finally {
      process.stdout.write = originalStdout;
      process.stderr.write = originalStderr;
      await fs.rm(tmpDir, { recursive: true, force: true });
    }

    assert.equal(stdout.join(""), "");
    assert.match(stderr.join(""), /ForkTTY Codex hook doctor/);
    assert.match(stderr.join(""), /codex hook config/);
  });

  it("rejects unexpected args for commands that take none", async () => {
    for (const [argv, message] of [
      [["ping", "--wat"], /ping: unexpected argument --wat/],
      [["list", "workspace-1"], /list: unexpected argument workspace-1/],
      [["create-workspace", "project"], /create-workspace: unexpected argument project/],
      [
        ["notifications", "--workspace-id", "main"],
        /notifications: unexpected argument --workspace-id/,
      ],
      [
        ["clear-notifications", "--workspace-id", "main"],
        /clear-notifications: unexpected argument --workspace-id/,
      ],
      [["surfaces", "surface-1"], /surfaces: unexpected argument surface-1/],
      [["surfaces", "--workspace", "main"], /surfaces: unknown option --workspace/],
      [["send-text", "--txt", "hello"], /send-text: unknown option --txt/],
      [
        ["set-status", "qa", "--key", "qa", "--value", "ok"],
        /set-status: unexpected argument qa/,
      ],
      [["list-status", "--workspace", "main"], /list-status: unknown option --workspace/],
      [["list-status", "qa"], /list-status: unexpected argument qa/],
      [["clear-status", "qa"], /clear-status: unexpected argument qa/],
      [
        ["set-progress", "build", "--key", "build", "--value", "1"],
        /set-progress: unexpected argument build/,
      ],
      [["list-progress", "--workspace", "main"], /list-progress: unknown option --workspace/],
      [["list-progress", "build"], /list-progress: unexpected argument build/],
      [["clear-progress", "build"], /clear-progress: unexpected argument build/],
      [["logs", "--workspace", "main"], /logs: unknown option --workspace/],
      [["logs", "build"], /logs: unexpected argument build/],
      [["clear-logs", "--workspace", "main"], /clear-logs: unknown option --workspace/],
      [["clear-logs", "build"], /clear-logs: unexpected argument build/],
      [["worktree-list", "feature/x"], /worktree-list: unexpected argument feature\/x/],
    ]) {
      await assert.rejects(main(argv, { XDG_RUNTIME_DIR: "/tmp" }), message);
    }
  });

  it("only skips hook socket sends when no socket target was supplied", () => {
    assert.equal(shouldSendHookActions({ env: {}, socketExplicit: false }), false);
    assert.equal(
      shouldSendHookActions({
        env: { FORKTTY_SOCKET_PATH: " /tmp/forktty.sock " },
        socketExplicit: false,
      }),
      true,
    );
    assert.equal(
      shouldSendHookActions({
        env: { FORKTTY_SOCKET_PATH: "relative.sock" },
        socketExplicit: false,
      }),
      false,
    );
    assert.equal(shouldSendHookActions({ env: {}, socketExplicit: true }), true);
  });

  it("does not let boolean flags consume following positionals", () => {
    assert.deepEqual(parseFlags(["--dry-run", "codex"], new Set(["dry-run"])), {
      options: { "dry-run": true },
      positionals: ["codex"],
    });
  });

  it("builds create workspace params without ignoring bad explicit options", () => {
    assert.deepEqual(
      buildCreateWorkspaceParams({
        name: " feature ",
        "working-dir": " /repo/feature ",
      }),
      {
        name: "feature",
        workingDir: "/repo/feature",
      },
    );
    assert.deepEqual(buildCreateWorkspaceParams({}), {});
    assert.throws(
      () => buildCreateWorkspaceParams({ name: "" }),
      /--name requires a value/,
    );
    assert.throws(
      () => buildCreateWorkspaceParams({ "working-dir": true }),
      /--working-dir requires a value/,
    );
    assert.throws(
      () => buildCreateWorkspaceParams({ workingdir: "/repo/feature" }),
      /create-workspace: unknown option --workingdir/,
    );
  });

  it("honors -- as the end of command options", () => {
    assert.deepEqual(parseFlags(["--", "--help", "--literal"]), {
      options: {},
      positionals: ["--help", "--literal"],
    });
    assert.deepEqual(parseFlags(["--title", "Heads up", "--", "--body"]), {
      options: { title: "Heads up" },
      positionals: ["--body"],
    });
  });

  it("only reads command stdin when no explicit text was provided", () => {
    assert.equal(shouldReadCommandStdin({}, [], "text"), true);
    assert.equal(shouldReadCommandStdin({ text: "echo ok" }, [], "text"), false);
    assert.equal(shouldReadCommandStdin({}, ["echo", "ok"], "text"), false);
    assert.equal(shouldReadCommandStdin({ body: "" }, [], "body"), false);
  });

  it("shell-quotes the generated hook command", () => {
    const scriptPath = "/tmp/ForkTTY Repo/scripts/forktty.mjs";
    const command = buildHookShellCommand(
      scriptPath,
      "codex",
      "session-start",
      "/opt/Node Bin/node",
    );
    assert.match(command, /FORKTTY_CODEX_HOOKS_DISABLED/);
    assert.ok(
      command.includes(
        "'/opt/Node Bin/node' '/tmp/ForkTTY Repo/scripts/forktty.mjs' hooks codex session-start",
      ),
    );
  });

  it("can generate hook commands through the forktty launcher", () => {
    const command = buildHookShellCommand(
      "/tmp/ForkTTY Repo/scripts/forktty.mjs",
      "codex",
      "session-start",
      "/opt/Node Bin/node",
      "/home/me/ForkTTY/forktty.AppImage",
    );

    assert.match(command, /FORKTTY_CODEX_HOOKS_DISABLED/);
    assert.ok(
      command.includes(
        "FORKTTY_NODE='/opt/Node Bin/node' '/home/me/ForkTTY/forktty.AppImage' hooks codex session-start",
      ),
    );
    assert.ok(!command.includes("scripts/forktty.mjs' hooks codex session-start"));
  });

  it("validates hook launcher bridge paths before writing configs", () => {
    assert.equal(
      resolveHookLauncherPath({
        FORKTTY_HOOK_LAUNCHER: " /opt/forktty/forktty ",
      }),
      "/opt/forktty/forktty",
    );
    assert.throws(
      () => resolveHookLauncherPath({ FORKTTY_HOOK_LAUNCHER: "forktty" }),
      /FORKTTY_HOOK_LAUNCHER must be an absolute path/,
    );
    assert.equal(
      resolveHookNodePath({ FORKTTY_HOOK_NODE: " /usr/bin/node " }),
      "/usr/bin/node",
    );
    assert.throws(
      () => resolveHookNodePath({ FORKTTY_HOOK_NODE: "node" }),
      /FORKTTY_HOOK_NODE must be an absolute path/,
    );
  });

  it("merges hook config without duplicating existing commands", () => {
    const scriptPath = "/tmp/forktty/scripts/forktty.mjs";
    const existing = {
      hooks: {
        SessionStart: [
          {
            hooks: [
              {
                type: "command",
                command: buildHookShellCommand(scriptPath, "codex", "session-start"),
                timeout: 5000,
              },
            ],
          },
        ],
      },
      custom: true,
    };

    const { changed, config } = mergeHookConfig(existing, "codex", scriptPath);

    assert.equal(changed, true);
    assert.equal(config.custom, true);
    assert.equal(config.hooks.SessionStart.length, 1);
    assert.equal(config.hooks.UserPromptSubmit.length, 1);
    assert.equal(config.hooks.Stop.length, 1);
  });

  it("merges hook config using the forktty launcher when available", () => {
    const { changed, config } = mergeHookConfig(
      {},
      "codex",
      "/tmp/forktty/scripts/forktty.mjs",
      "/usr/bin/node",
      "/opt/forktty/forktty.AppImage",
    );

    assert.equal(changed, true);
    const command = config.hooks.SessionStart[0].hooks[0].command;
    assert.ok(command.includes("FORKTTY_NODE='/usr/bin/node'"));
    assert.ok(command.includes("'/opt/forktty/forktty.AppImage' hooks codex session-start"));
    assert.ok(!command.includes("scripts/forktty.mjs"));
  });


  it("replaces legacy bare-node hook commands instead of appending pinned duplicates", () => {
    const scriptPath = "/tmp/forktty/scripts/forktty.mjs";
    const legacyCommand =
      `[ "\${FORKTTY_CODEX_HOOKS_DISABLED:-}" != "1" ] && node '${scriptPath}' hooks codex session-start || echo '{"hookSpecificOutput":{"hookEventName":"SessionStart","permissionDecision":"allow","permissionDecisionReason":"ForkTTY hook disabled"}}'`;
    const existing = {
      hooks: {
        SessionStart: [
          {
            hooks: [
              {
                type: "command",
                command: legacyCommand,
                timeout: 5000,
              },
            ],
          },
        ],
      },
    };

    const { changed, config } = mergeHookConfig(existing, "codex", scriptPath, "/usr/bin/node");

    assert.equal(changed, true);
    assert.equal(config.hooks.SessionStart.length, 1);
    assert.ok(config.hooks.SessionStart[0].hooks[0].command.includes("'/usr/bin/node'"));
    assert.ok(!config.hooks.SessionStart[0].hooks[0].command.includes(" && node "));
  });

  it("replaces legacy untagged hook commands from an old repo path", () => {
    const oldScriptPath = "/old/forktty/scripts/forktty.mjs";
    const newScriptPath = "/usr/lib/forktty/forktty.mjs";
    const legacyCommand =
      `[ "\${FORKTTY_CODEX_HOOKS_DISABLED:-}" != "1" ] && node '${oldScriptPath}' hooks codex session-start || echo '{"continue":true,"suppressOutput":false}'`;
    const existing = {
      hooks: {
        SessionStart: [
          {
            hooks: [
              {
                type: "command",
                command: legacyCommand,
                timeout: 5000,
              },
            ],
          },
        ],
      },
    };

    const { config } = mergeHookConfig(existing, "codex", newScriptPath, "/usr/bin/node");

    assert.equal(config.hooks.SessionStart.length, 1);
    assert.ok(config.hooks.SessionStart[0].hooks[0].command.includes(newScriptPath));
    assert.ok(!config.hooks.SessionStart[0].hooks[0].command.includes(oldScriptPath));
  });

  it("strips prior forktty-tagged hook entries when reinstalling from a new script path", () => {
    const oldScriptPath = "/old/forktty/scripts/forktty.mjs";
    const newScriptPath = "/new/forktty/scripts/forktty.mjs";
    const installed = mergeHookConfig({}, "codex", oldScriptPath).config;
    assert.equal(installed.hooks.SessionStart.length, 1);

    const { changed, config } = mergeHookConfig(installed, "codex", newScriptPath);

    assert.equal(changed, true);
    // Each event should still have exactly one ForkTTY entry — no stale
    // command from the old script path remains alongside the new one.
    assert.equal(config.hooks.SessionStart.length, 1);
    assert.equal(config.hooks.UserPromptSubmit.length, 1);
    assert.equal(config.hooks.Stop.length, 1);
    assert.ok(
      config.hooks.SessionStart[0].hooks[0].command.includes(newScriptPath),
    );
    assert.ok(
      !config.hooks.SessionStart[0].hooks[0].command.includes(oldScriptPath),
    );
  });

  it("preserves foreign hook entries when uninstalling ForkTTY hooks would leave them alone", () => {
    const scriptPath = "/tmp/forktty/scripts/forktty.mjs";
    const foreignEntry = {
      hooks: [
        {
          type: "command",
          command: "/usr/local/bin/other-tool",
          timeout: 1000,
        },
      ],
    };
    const existing = {
      hooks: { SessionStart: [foreignEntry] },
    };

    const { config } = mergeHookConfig(existing, "codex", scriptPath);

    assert.equal(config.hooks.SessionStart.length, 2);
    assert.deepEqual(config.hooks.SessionStart[0], foreignEntry);
  });

  it("builds notification params and rejects missing option values", () => {
    assert.deepEqual(
      buildNotificationParams(
        {
          kind: " prompt ",
          title: "Input",
          "workspace-name": " main ",
        },
        ["Review", "needed"],
        "",
        {},
      ),
      {
        workspace_name: "main",
        title: "Input",
        body: "Review needed",
        kind: "prompt",
      },
    );
    assert.throws(
      () => buildNotificationParams({ kind: true }, ["Review needed"]),
      /--kind requires a value/,
    );
    assert.throws(
      () => buildNotificationParams({ kind: "" }, ["Review needed"]),
      /--kind requires a value/,
    );
    assert.throws(
      () => buildNotificationParams({ title: true }, ["Review needed"]),
      /--title requires a value/,
    );
    assert.throws(
      () => buildNotificationParams({ title: "" }, ["Review needed"]),
      /--title requires a value/,
    );
    assert.throws(
      () => buildNotificationParams({ body: true }, []),
      /--body requires a value/,
    );
    assert.throws(
      () => buildNotificationParams({ knd: "prompt" }, ["Review needed"]),
      /notify: unknown option --knd/,
    );
  });

  it("maps notification events to status and prompt notifications", () => {
    const actions = buildHookActions(
      "claude",
      "notification",
      { message: "Review needed" },
      { FORKTTY_WORKSPACE_ID: "ws-1" },
    );

    assert.deepEqual(actions, [
      {
        method: "metadata.log",
        params: {
          workspace_id: "ws-1",
          level: "warn",
          message: "Review needed",
        },
      },
      {
        method: "metadata.set_status",
        params: {
          workspace_id: "ws-1",
          key: "agent:claude",
          label: "Claude",
          value: "Needs input",
          color: "yellow",
        },
      },
      {
        method: "notification.create",
        params: {
          workspace_id: "ws-1",
          title: "Claude needs input",
          body: "Review needed",
          kind: "prompt",
        },
      },
    ]);
  });

  it("attaches hook workspace and surface targets when both env vars are present", () => {
    assert.deepEqual(
      buildHookTargetParams({
        FORKTTY_WORKSPACE_ID: " ws-1 ",
        FORKTTY_SURFACE_ID: " surface-1\n",
      }),
      {
        workspace_id: "ws-1",
        surface_id: "surface-1",
      },
    );

    const actions = buildHookActions(
      "codex",
      "prompt-submit",
      null,
      {
        FORKTTY_WORKSPACE_ID: "ws-1",
        FORKTTY_SURFACE_ID: "surface-1",
      },
      12345,
    );

    assert.deepEqual(actions[0].params, {
      workspace_id: "ws-1",
      surface_id: "surface-1",
      level: "info",
      message: "Codex prompt submitted",
    });
    assert.deepEqual(actions[1].params, {
      workspace_id: "ws-1",
      surface_id: "surface-1",
      key: "agent:codex",
      label: "Codex",
      value: "Running",
      color: "blue",
      hook_event_order: 12345,
      hook_event_clock: "monotonic-ns",
      hook_event_name: "prompt-submit",
    });
  });

  it("adds stable hook turn ids without exposing prompt text", () => {
    const actions = buildHookActions(
      "codex",
      "prompt-submit",
      { prompt: "ship the secret feature" },
      { FORKTTY_WORKSPACE_ID: "ws-1" },
      "12345",
    );

    assert.match(actions[1].params.hook_turn_id, /^prompt:[a-f0-9]{16}$/);
    assert.doesNotMatch(actions[1].params.hook_turn_id, /secret/);
  });

  it("escapes control characters before hook messages reach terminal-visible output", () => {
    assert.equal(sanitizeForTerminal("bad\u001b[31m\nnext"), "bad\\x1b[31m\\nnext");

    const actions = buildHookActions(
      "claude",
      "notification",
      { message: "review\u001b[31m\nneeded" },
      { FORKTTY_WORKSPACE_ID: "ws-1" },
      99,
    );

    assert.equal(actions[0].params.message, "review\\x1b[31m\\nneeded");
    assert.equal(actions[2].params.body, "review\\x1b[31m\\nneeded");
  });

  it("redacts sensitive fields from hook debug summaries", () => {
    const summary = summarizeHookAction({
      method: "metadata.log",
      params: {
        workspace_id: "ws-1",
        level: "info",
        message: "secret prompt text",
        nested: { body: "secret body" },
      },
    });

    assert.match(summary, /metadata\.log/);
    assert.match(summary, /<redacted:/);
    assert.doesNotMatch(summary, /secret prompt text/);
    assert.doesNotMatch(summary, /secret body/);
  });

  it("clears agent status on session end", () => {
    const actions = buildHookActions(
      "gemini",
      "session-end",
      null,
      { FORKTTY_WORKSPACE_ID: "ws-2" },
    );

    assert.deepEqual(actions, [
      {
        method: "metadata.log",
        params: {
          workspace_id: "ws-2",
          level: "info",
          message: "Gemini session ended",
        },
      },
      {
        method: "metadata.clear_status",
        params: {
          workspace_id: "ws-2",
          key: "agent:gemini",
        },
      },
    ]);
  });

  it("maps pre-tool events to a status with the tool name", () => {
    const actions = buildHookActions(
      "claude",
      "pre-tool",
      { tool_name: "Bash", tool_input: { command: "ls" } },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
      77,
    );

    assert.deepEqual(actions, [
      {
        method: "metadata.log",
        params: {
          workspace_id: "ws-3",
          level: "info",
          message: "Claude running Bash",
        },
      },
      {
        method: "metadata.set_status",
        params: {
          workspace_id: "ws-3",
          key: "agent:claude",
          label: "Claude",
          value: "Running Bash",
          color: "blue",
          hook_event_order: 77,
          hook_event_clock: "monotonic-ns",
          hook_event_name: "pre-tool",
        },
      },
    ]);
  });

  it("falls back to a generic pre-tool status when no tool name is present", () => {
    const actions = buildHookActions(
      "claude",
      "pre-tool",
      {},
      { FORKTTY_WORKSPACE_ID: "ws-3" },
    );

    assert.equal(actions[1].params.value, "Running tool");
    assert.equal(actions[0].params.message, "Claude running tool");
  });

  it("truncates very long tool names in the pre-tool status", () => {
    const longName = "a".repeat(120);
    const actions = buildHookActions(
      "claude",
      "pre-tool",
      { tool_name: longName },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
    );

    assert.ok(actions[1].params.value.startsWith("Running "));
    assert.ok(actions[1].params.value.length <= "Running ".length + 48);
    assert.ok(actions[1].params.value.endsWith("…"));
  });

  it("sanitizes control characters in tool names before exposing them", () => {
    const action = extractHookToolName({ tool_name: "Bash[31m" });
    assert.equal(action, "Bash\\x1b[31m");
  });

  it("emits only a log on post-tool, leaving status untouched", () => {
    const actions = buildHookActions(
      "claude",
      "post-tool",
      { tool_name: "Edit" },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
    );

    assert.equal(actions.length, 1);
    assert.equal(actions[0].method, "metadata.log");
    assert.equal(actions[0].params.message, "Claude finished Edit");
  });

  it("creates an error notification when a post-tool payload reports failure", () => {
    const actions = buildHookActions(
      "claude",
      "post-tool",
      { tool_name: "Bash", tool_response: { is_error: true, output: "boom" } },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
    );

    assert.equal(actions.length, 2);
    assert.equal(actions[0].params.level, "error");
    assert.match(actions[0].params.message, /Bash reported an error/);
    assert.equal(actions[1].method, "notification.create");
    assert.equal(actions[1].params.kind, "error");
    assert.equal(actions[1].params.title, "Claude tool error");
    assert.match(actions[1].params.body, /Bash/);
  });

  it("detects tool errors at any payload depth", () => {
    assert.equal(extractHookToolError({ tool_response: { is_error: true } }), true);
    assert.equal(extractHookToolError({ result: { error: { message: "bad" } } }), true);
    assert.equal(extractHookToolError({ tool_response: { is_error: false } }), false);
    assert.equal(extractHookToolError({}), false);
    assert.equal(extractHookToolError(null), false);
  });

  it("resets status to Running when a subagent stops", () => {
    const actions = buildHookActions(
      "claude",
      "subagent-stop",
      null,
      { FORKTTY_WORKSPACE_ID: "ws-3" },
      42,
    );

    assert.equal(actions.length, 2);
    assert.equal(actions[0].method, "metadata.log");
    assert.equal(actions[0].params.message, "Claude subagent finished");
    assert.equal(actions[1].method, "metadata.set_status");
    assert.equal(actions[1].params.value, "Running");
    assert.equal(actions[1].params.color, "blue");
  });

  it("marks Compacting status and creates an info notification on pre-compact", () => {
    const actions = buildHookActions(
      "claude",
      "pre-compact",
      { trigger: "auto" },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
      55,
    );

    assert.equal(actions.length, 3);
    assert.equal(actions[0].params.level, "warn");
    assert.match(actions[0].params.message, /context compacting \(auto\)/);
    assert.equal(actions[1].params.value, "Compacting");
    assert.equal(actions[1].params.color, "yellow");
    assert.equal(actions[2].method, "notification.create");
    assert.equal(actions[2].params.kind, "info");
    assert.match(actions[2].params.body, /auto/);
  });

  it("tags session-start log with the source variant", () => {
    const actions = buildHookActions(
      "claude",
      "session-start",
      { source: "resume" },
      { FORKTTY_WORKSPACE_ID: "ws-3" },
    );

    assert.equal(actions[0].params.message, "Claude session started (resume)");
    assert.equal(extractHookSource({ source: "compact" }), "compact");
    assert.equal(extractHookSource({}), "");
  });

  it("extracts compact triggers from common payload shapes", () => {
    assert.equal(extractHookCompactTrigger({ trigger: "auto" }), "auto");
    assert.equal(extractHookCompactTrigger({ compactTrigger: "manual" }), "manual");
    assert.equal(extractHookCompactTrigger({ reason: "context" }), "context");
    assert.equal(extractHookCompactTrigger({}), "");
  });

  it("filters pending notifications by workspace and read state", () => {
    const list = [
      { id: "1", read: false, workspace_id: "ws-A", title: "a" },
      { id: "2", read: true, workspace_id: "ws-A", title: "b" },
      { id: "3", read: false, workspace_id: "ws-B", title: "c" },
      { id: "4", read: false, workspace_id: "", title: "d" },
    ];
    const filtered = filterPendingNotificationsForTarget(list, {
      FORKTTY_WORKSPACE_ID: "ws-A",
    });
    assert.deepEqual(
      filtered.map((entry) => entry.id),
      ["1", "4"],
    );

    const noFilter = filterPendingNotificationsForTarget(list, {});
    assert.deepEqual(
      noFilter.map((entry) => entry.id),
      ["1", "3", "4"],
    );

    assert.deepEqual(filterPendingNotificationsForTarget(null, {}), []);
  });

  it("formats pending notification blocks with a trailing more line when truncated", () => {
    const block = formatPendingNotificationsBlock([
      { kind: "error", title: "Build broke", body: "exit 1" },
      { kind: "info", title: "Heads up", body: "" },
    ]);
    assert.match(block, /ForkTTY pending notifications:/);
    assert.match(block, /\[error\] Build broke — exit 1/);
    assert.match(block, /\[info\] Heads up$/m);

    const many = Array.from({ length: 12 }, (_, i) => ({
      kind: "info",
      title: `n${i}`,
      body: "",
    }));
    const truncated = formatPendingNotificationsBlock(many);
    assert.match(truncated, /\.\.\.and 2 more/);

    assert.equal(formatPendingNotificationsBlock([]), "");
    assert.equal(formatPendingNotificationsBlock(null), "");
  });

  it("formats token usage with totals derived from cache and input counts", () => {
    const block = formatTokenUsageBlock({
      input: 1000,
      cacheRead: 4000,
      cacheCreation: 500,
      output: 250,
    });
    assert.match(block, /5,500 \/ 200,000 input tokens/);
    assert.match(block, /\(3% — input=1000/);
    assert.match(block, /cache_read=4000/);
    assert.match(block, /output=250/);
    assert.equal(formatTokenUsageBlock(null), "");
  });

  it("honors FORKTTY_HOOK_TOKEN_CEILING in the usage block", () => {
    const block = formatTokenUsageBlock(
      { input: 1000, cacheRead: 9000, cacheCreation: 0, output: 0 },
      { FORKTTY_HOOK_TOKEN_CEILING: "50000" },
    );
    assert.match(block, /10,000 \/ 50,000 input tokens/);
    assert.match(block, /\(20% —/);
  });

  it("resolves the token ceiling with a sane fallback", () => {
    assert.equal(resolveTokenCeiling({}), 200_000);
    assert.equal(
      resolveTokenCeiling({ FORKTTY_HOOK_TOKEN_CEILING: "1000000" }),
      1_000_000,
    );
    assert.equal(
      resolveTokenCeiling({ FORKTTY_HOOK_TOKEN_CEILING: "0" }),
      200_000,
    );
    assert.equal(
      resolveTokenCeiling({ FORKTTY_HOOK_TOKEN_CEILING: "junk" }),
      200_000,
    );
  });

  it("flags agent self-feedback notifications by exact title match", () => {
    assert.equal(
      isAgentSelfFeedbackNotification({ kind: "prompt", title: "Claude needs input" }),
      true,
    );
    assert.equal(
      isAgentSelfFeedbackNotification({ kind: "prompt", title: "Codex needs input" }),
      true,
    );
    assert.equal(
      isAgentSelfFeedbackNotification({ kind: "info", title: "Claude needs input" }),
      false,
    );
    assert.equal(
      isAgentSelfFeedbackNotification({ kind: "prompt", title: "Custom prompt" }),
      false,
    );
    assert.equal(isAgentSelfFeedbackNotification(null), false);
  });

  it("filters self-feedback prompts out of pending notifications", () => {
    const list = [
      { id: "self", kind: "prompt", title: "Claude needs input", read: false, workspace_id: "ws-A" },
      { id: "user", kind: "info", title: "Build broke", read: false, workspace_id: "ws-A" },
    ];
    const filtered = filterPendingNotificationsForTarget(list, {
      FORKTTY_WORKSPACE_ID: "ws-A",
    });
    assert.deepEqual(
      filtered.map((entry) => entry.id),
      ["user"],
    );
  });

  it("buildTokenProgressAction picks up FORKTTY_HOOK_TOKEN_CEILING for the total", () => {
    const action = buildTokenProgressAction(
      "claude",
      { input: 100, cacheRead: 200, cacheCreation: 50, output: 10 },
      { FORKTTY_HOOK_TOKEN_CEILING: "12345" },
      1,
      "prompt-submit",
    );
    assert.equal(action.params.total, 12345);
  });

  it("builds a token progress action only when usage totals are positive", () => {
    const action = buildTokenProgressAction(
      "claude",
      { input: 100, cacheRead: 200, cacheCreation: 50, output: 10 },
      { FORKTTY_WORKSPACE_ID: "ws-A" },
      77,
      "prompt-submit",
    );
    assert.equal(action.method, "metadata.set_progress");
    assert.equal(action.params.key, "agent:claude:tokens");
    assert.equal(action.params.label, "Claude input tokens");
    assert.equal(action.params.value, 350);
    assert.equal(action.params.total, 200_000);
    assert.equal(action.params.workspace_id, "ws-A");
    assert.equal(action.params.hook_event_order, 77);
    assert.equal(action.params.hook_event_name, "prompt-submit");

    assert.equal(
      buildTokenProgressAction("claude", null, {}, 1, "prompt-submit"),
      null,
    );
    assert.equal(
      buildTokenProgressAction(
        "claude",
        { input: 0, cacheRead: 0, cacheCreation: 0, output: 0 },
        {},
        1,
        "prompt-submit",
      ),
      null,
    );
  });

  it("reads the latest usage object from a transcript jsonl file", async () => {
    const tmp = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-tx-"));
    const file = path.join(tmp, "transcript.jsonl");
    const lines = [
      JSON.stringify({ type: "user", message: { content: "hi" } }),
      JSON.stringify({
        type: "assistant",
        message: {
          usage: {
            input_tokens: 1200,
            output_tokens: 80,
            cache_read_input_tokens: 4500,
            cache_creation_input_tokens: 300,
          },
        },
      }),
      JSON.stringify({ type: "tool_use" }),
    ];
    await fs.writeFile(file, `${lines.join("\n")}\n`);
    try {
      const usage = await readTokenUsageFromTranscript(file);
      assert.deepEqual(usage, {
        input: 1200,
        output: 80,
        cacheRead: 4500,
        cacheCreation: 300,
      });

      const missing = await readTokenUsageFromTranscript(
        path.join(tmp, "does-not-exist.jsonl"),
      );
      assert.equal(missing, null);
      assert.equal(await readTokenUsageFromTranscript(""), null);
    } finally {
      await fs.rm(tmp, { recursive: true, force: true });
    }
  });

  it("includes pending notifications and token usage in prompt-submit responses", () => {
    const response = buildHookResponse(
      "claude",
      "prompt-submit",
      { FORKTTY_WORKSPACE_ID: "ws-A" },
      {
        pendingNotifications: [
          { kind: "error", title: "Build broke", body: "exit 1" },
        ],
        tokenUsage: {
          input: 500,
          cacheRead: 1000,
          cacheCreation: 0,
          output: 50,
        },
      },
    );
    assert.equal(response.hookSpecificOutput.hookEventName, "UserPromptSubmit");
    const ctx = response.hookSpecificOutput.additionalContext;
    assert.match(ctx, /Build broke/);
    assert.match(ctx, /1,500 \/ 200,000 input tokens/);
  });

  it("returns plain continue for prompt-submit when no extras present", () => {
    assert.deepEqual(
      buildHookResponse("claude", "prompt-submit", {}, {}),
      HOOK_CONTINUE_RESPONSE,
    );
    assert.deepEqual(
      buildHookResponse("codex", "prompt-submit", {}, {
        pendingNotifications: [{ kind: "info", title: "x" }],
      }),
      HOOK_CONTINUE_RESPONSE,
    );
  });

  it("session-start response appends pending notifications block when present", () => {
    const response = buildHookResponse(
      "claude",
      "session-start",
      {},
      {
        pendingNotifications: [{ kind: "info", title: "Hello" }],
      },
    );
    assert.match(response.hookSpecificOutput.additionalContext, /Hello/);
  });

  it("returns additionalContext for claude session-start responses", () => {
    const response = buildHookResponse("claude", "session-start", {
      FORKTTY_WORKSPACE_ID: "ws-4",
      FORKTTY_SURFACE_ID: "surface-9",
      FORKTTY_SOCKET_PATH: "/tmp/forktty.sock",
    });

    assert.equal(response.continue, true);
    assert.equal(response.suppressOutput, false);
    assert.equal(response.hookSpecificOutput.hookEventName, "SessionStart");
    assert.match(response.hookSpecificOutput.additionalContext, /ForkTTY/);
    assert.match(response.hookSpecificOutput.additionalContext, /ws-4/);
    assert.match(response.hookSpecificOutput.additionalContext, /surface-9/);
    assert.match(response.hookSpecificOutput.additionalContext, /forktty\.sock/);
    assert.match(response.hookSpecificOutput.additionalContext, /PreToolUse/);
  });

  it("falls back to plain continue for non-session-start hook responses", () => {
    assert.deepEqual(
      buildHookResponse("claude", "prompt-submit", {}),
      HOOK_CONTINUE_RESPONSE,
    );
    assert.deepEqual(
      buildHookResponse("codex", "session-start", {}),
      HOOK_CONTINUE_RESPONSE,
    );
  });

  it("warns and continues for unsupported hook events", async () => {
    const stdout = [];
    const stderr = [];
    const originalStdout = process.stdout.write.bind(process.stdout);
    const originalStderr = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk) => {
      stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    process.stderr.write = (chunk) => {
      stderr.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await main(["hooks", "codex", "sesion-start"], {});
    } finally {
      process.stdout.write = originalStdout;
      process.stderr.write = originalStderr;
    }

    assert.deepEqual(JSON.parse(stdout.join("")), HOOK_CONTINUE_RESPONSE);
    assert.match(stderr.join(""), /Unsupported hook event for codex: sesion-start/);
  });

  it("warns and continues for extra hook event arguments", async () => {
    const stdout = [];
    const stderr = [];
    const originalStdout = process.stdout.write.bind(process.stdout);
    const originalStderr = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk) => {
      stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    process.stderr.write = (chunk) => {
      stderr.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await main(["hooks", "codex", "session-start", "extra"], {});
    } finally {
      process.stdout.write = originalStdout;
      process.stderr.write = originalStderr;
    }

    assert.deepEqual(JSON.parse(stdout.join("")), HOOK_CONTINUE_RESPONSE);
    assert.match(stderr.join(""), /Unexpected hook argument for codex session-start: extra/);
  });

  it("emits token progress and additionalContext on claude prompt-submit", async () => {
    const tmp = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-prompt-submit-"));
    const transcript = path.join(tmp, "tx.jsonl");
    await fs.writeFile(
      transcript,
      `${JSON.stringify({
        type: "assistant",
        message: {
          usage: {
            input_tokens: 200,
            output_tokens: 50,
            cache_read_input_tokens: 800,
            cache_creation_input_tokens: 0,
          },
        },
      })}\n`,
    );

    const requests = [];
    const pendingList = [
      { id: "u1", kind: "info", title: "Build broke", body: "exit 1", read: false, workspace_id: "ws-1" },
      { id: "self", kind: "prompt", title: "Claude needs input", body: "x", read: false, workspace_id: "ws-1" },
    ];

    try {
      await withSocketServer(
        (socket) => {
          socket.once("data", (chunk) => {
            const request = JSON.parse(String(chunk).trim());
            requests.push(request);
            let result;
            switch (request.method) {
              case "metadata.log":
                result = { id: "log-1", level: request.params.level, message: request.params.message };
                break;
              case "metadata.set_status":
                result = { key: request.params.key, value: request.params.value };
                break;
              case "notification.list":
                result = pendingList;
                break;
              case "metadata.set_progress":
                result = { key: request.params.key, value: request.params.value, total: request.params.total };
                break;
              default:
                result = null;
            }
            socket.end(`${JSON.stringify({ id: request.id, ok: true, result })}\n`);
          });
        },
        async (socketPath) => {
          const stdout = [];
          const originalStdout = process.stdout.write.bind(process.stdout);
          process.stdout.write = (chunk) => {
            stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
            return true;
          };
          const payload = `${JSON.stringify({
            hook_event_name: "UserPromptSubmit",
            transcript_path: transcript,
            prompt: "hello",
          })}\n`;
          const stdinStream = Readable.from([Buffer.from(payload)]);
          stdinStream.isTTY = false;
          const originalStdin = process.stdin;
          Object.defineProperty(process, "stdin", {
            value: stdinStream,
            configurable: true,
          });
          try {
            await main(["hooks", "claude", "prompt-submit", "--socket", socketPath], {
              FORKTTY_WORKSPACE_ID: "ws-1",
            });
          } finally {
            process.stdout.write = originalStdout;
            Object.defineProperty(process, "stdin", {
              value: originalStdin,
              configurable: true,
            });
          }

          const response = JSON.parse(stdout.join(""));
          assert.equal(response.hookSpecificOutput.hookEventName, "UserPromptSubmit");
          const ctx = response.hookSpecificOutput.additionalContext;
          assert.match(ctx, /Build broke/);
          assert.doesNotMatch(ctx, /Claude needs input/);
          assert.match(ctx, /1,000 \/ 200,000 input tokens/);
        },
      );
    } finally {
      await fs.rm(tmp, { recursive: true, force: true });
    }

    const methods = requests.map((r) => r.method);
    assert.ok(methods.includes("metadata.log"));
    assert.ok(methods.includes("metadata.set_status"));
    assert.ok(methods.includes("notification.list"));
    const progress = requests.find((r) => r.method === "metadata.set_progress");
    assert.ok(progress, "progress request emitted");
    assert.equal(progress.params.key, "agent:claude:tokens");
    assert.equal(progress.params.value, 1000);
    assert.equal(progress.params.total, 200_000);
  });

  it("keeps hook stdout as JSON while verbose details go to stderr", async () => {
    const requests = [];
    await withSocketServer(
      (socket) => {
        socket.once("data", (chunk) => {
          const request = JSON.parse(String(chunk).trim());
          requests.push(request);
          const result =
            request.method === "metadata.log"
              ? { id: "log-1", level: request.params.level, message: request.params.message }
              : {
                  key: request.params.key,
                  label: request.params.label,
                  value: request.params.value,
                  color: request.params.color,
                };
          socket.end(`${JSON.stringify({ id: request.id, ok: true, result })}\n`);
        });
      },
      async (socketPath) => {
        const stdout = [];
        const stderr = [];
        const originalStdout = process.stdout.write.bind(process.stdout);
        const originalStderr = process.stderr.write.bind(process.stderr);
        const originalStdinIsTTY = process.stdin.isTTY;
        process.stdout.write = (chunk) => {
          stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
          return true;
        };
        process.stderr.write = (chunk) => {
          stderr.push(typeof chunk === "string" ? chunk : chunk.toString());
          return true;
        };
        process.stdin.isTTY = true;
        try {
          await main(
            ["--verbose", "hooks", "codex", "prompt-submit", "--socket", socketPath],
            {
              FORKTTY_WORKSPACE_ID: "ws-1",
              FORKTTY_SURFACE_ID: "surface-1",
            },
          );
        } finally {
          process.stdout.write = originalStdout;
          process.stderr.write = originalStderr;
          if (originalStdinIsTTY === undefined) {
            delete process.stdin.isTTY;
          } else {
            process.stdin.isTTY = originalStdinIsTTY;
          }
        }

        assert.deepEqual(JSON.parse(stdout.join("")), HOOK_CONTINUE_RESPONSE);
        assert.match(stderr.join(""), /ForkTTY hook debug: codex prompt-submit/);
      },
    );

    assert.equal(requests.length, 2);
    assert.equal(requests[1].method, "metadata.set_status");
    assert.equal(requests[1].params.surface_id, "surface-1");
    assert.equal(typeof requests[1].params.hook_event_order, "string");
  });

  it("runs hooks test codex as a socket roundtrip without stdout noise", async () => {
    const requests = [];
    let notificationCreated = null;
    await withSocketServer(
      (socket) => {
        socket.once("data", (chunk) => {
          const request = JSON.parse(String(chunk).trim());
          requests.push(request);
          let result;
          switch (request.method) {
            case "system.ping":
              result = "pong";
              break;
            case "metadata.set_status":
              result = {
                key: request.params.key,
                label: request.params.label,
                value: request.params.value,
                color: request.params.color,
              };
              break;
            case "metadata.log":
              result = {
                id: "log-1",
                level: request.params.level,
                message: request.params.message,
              };
              break;
            case "notification.list":
              result = notificationCreated ? [notificationCreated] : [];
              break;
            case "notification.create":
              notificationCreated = {
                id: "notification-1",
                title: request.params.title,
                body: request.params.body,
                kind: request.params.kind,
                workspace_id: request.params.workspace_id,
                surface_id: request.params.surface_id,
              };
              result = notificationCreated;
              break;
            case "metadata.clear_status":
              result = { cleared: true };
              break;
            case "notification.clear":
              notificationCreated = null;
              result = { cleared: true };
              break;
            default:
              result = {};
          }
          socket.end(`${JSON.stringify({ id: request.id, ok: true, result })}\n`);
        });
      },
      async (socketPath) => {
        const stdout = [];
        const stderr = [];
        const originalStdout = process.stdout.write.bind(process.stdout);
        const originalStderr = process.stderr.write.bind(process.stderr);
        process.stdout.write = (chunk) => {
          stdout.push(typeof chunk === "string" ? chunk : chunk.toString());
          return true;
        };
        process.stderr.write = (chunk) => {
          stderr.push(typeof chunk === "string" ? chunk : chunk.toString());
          return true;
        };
        try {
          await handleHooksTest(
            {
              env: {
                FORKTTY_WORKSPACE_ID: "ws-1",
                FORKTTY_SURFACE_ID: "surface-1",
              },
              json: false,
              socketPath,
              socketExplicit: true,
            },
            ["codex"],
          );
        } finally {
          process.stdout.write = originalStdout;
          process.stderr.write = originalStderr;
        }

        assert.equal(stdout.join(""), "");
        assert.match(stderr.join(""), /ForkTTY Codex hook test: ok/);
      },
    );

    assert.deepEqual(
      requests.map((request) => request.method),
      [
        "system.ping",
        "metadata.set_status",
        "metadata.log",
        "notification.list",
        "notification.create",
        "metadata.clear_status",
        "notification.list",
        "notification.clear",
      ],
    );
    assert.equal(requests[1].params.key, "agent:codex:hook-test");
    assert.equal(requests[1].params.surface_id, "surface-1");
    assert.equal(requests[2].params.surface_id, "surface-1");
    assert.equal(requests[4].params.surface_id, "surface-1");
    assert.equal(notificationCreated, null);
  });

  it("builds progress metadata params with workspace targeting", () => {
    assert.deepEqual(
      buildProgressParams(
        {
          key: "build",
          label: "Build",
          value: "12.5",
          total: "100",
        },
        { FORKTTY_WORKSPACE_ID: "ws-1" },
      ),
      {
        workspace_id: "ws-1",
        key: "build",
        label: "Build",
        value: 12.5,
        total: 100,
      },
    );
    assert.deepEqual(
      buildProgressParams(
        {
          key: "build",
          value: "1",
          "workspace-name": " main ",
        },
        { FORKTTY_WORKSPACE_ID: " ws-env " },
      ),
      {
        workspace_name: "main",
        key: "build",
        label: "build",
        value: 1,
      },
    );
    assert.deepEqual(
      buildProgressParams(
        {
          key: "build",
          value: "1",
        },
        { FORKTTY_WORKSPACE_ID: " ws-env \n" },
      ),
      {
        workspace_id: "ws-env",
        key: "build",
        label: "build",
        value: 1,
      },
    );
    assert.throws(
      () =>
        buildProgressParams(
          {
            key: "build",
            value: "1",
            "workspace-id": true,
          },
          { FORKTTY_WORKSPACE_ID: "ws-env" },
        ),
      /--workspace-id requires a value/,
    );
    assert.throws(
      () =>
        buildProgressParams(
          {
            key: "build",
            value: "1",
            "workspace-id": "",
          },
          { FORKTTY_WORKSPACE_ID: "ws-env" },
        ),
      /--workspace-id requires a value/,
    );
    assert.throws(
      () => buildProgressParams({ key: "build", value: "1", totl: "100" }, {}),
      /set-progress: unknown option --totl/,
    );
    assert.throws(
      () =>
        buildProgressParams(
          {
            key: "build",
            value: "1",
            "workspace-id": "ws-1",
            "workspace-name": "main",
          },
          {},
        ),
      /set-progress: cannot combine --workspace-id and --workspace-name/,
    );
  });

  it("rejects invalid progress values", () => {
    assert.throws(
      () => buildProgressParams({ key: "build", value: "1", total: true }),
      /--total requires a value/,
    );
    assert.throws(
      () => buildProgressParams({ key: "build", value: "nan" }),
      /Invalid --value/,
    );
    assert.throws(
      () => buildProgressParams({ key: "build", value: "1", total: "0" }),
      /Invalid --total/,
    );
  });

  it("builds log metadata params from positional text", () => {
    assert.deepEqual(
      buildLogParams(
        { level: "warn" },
        ["waiting", "for", "input"],
        "",
        { FORKTTY_WORKSPACE_ID: "ws-2" },
      ),
      {
        workspace_id: "ws-2",
        level: "warn",
        message: "waiting for input",
      },
    );
  });

  it("rejects invalid log levels", () => {
    assert.throws(
      () => buildLogParams({ level: true }, ["hello"]),
      /--level requires a value/,
    );
    assert.throws(
      () => buildLogParams({ level: "" }, ["hello"]),
      /--level requires a value/,
    );
    assert.throws(
      () => buildLogParams({ message: true }, []),
      /--message requires a value/,
    );
    assert.throws(
      () => buildLogParams({ levl: "warn" }, ["hello"]),
      /log: unknown option --levl/,
    );
    assert.throws(() => buildLogParams({ level: "debug" }, ["hello"]), /Invalid --level/);
  });

  it("builds clear metadata params without treating bad keys as clear-all", () => {
    assert.deepEqual(
      buildClearMetadataParams({ key: " build " }, { FORKTTY_WORKSPACE_ID: " ws-1 " }),
      {
        workspace_id: "ws-1",
        key: "build",
      },
    );
    assert.deepEqual(
      buildClearMetadataParams({}, { FORKTTY_WORKSPACE_ID: " ws-1 " }),
      {
        workspace_id: "ws-1",
      },
    );
    assert.throws(
      () => buildClearMetadataParams({ key: true }, { FORKTTY_WORKSPACE_ID: "ws-1" }),
      /--key requires a value/,
    );
    assert.throws(
      () => buildClearMetadataParams({ key: "" }, { FORKTTY_WORKSPACE_ID: "ws-1" }),
      /--key requires a value/,
    );
    assert.throws(
      () => buildClearMetadataParams({ kee: "build" }, { FORKTTY_WORKSPACE_ID: "ws-1" }),
      /clear metadata: unknown option --kee/,
    );
  });

  it("builds status metadata params with explicit option validation", () => {
    assert.deepEqual(
      buildStatusParams(
        {
          key: " qa ",
          value: " ok ",
          label: " QA ",
          color: " green ",
        },
        { FORKTTY_WORKSPACE_ID: " ws-1 " },
      ),
      {
        workspace_id: "ws-1",
        key: "qa",
        label: "QA",
        value: "ok",
        color: "green",
      },
    );
    assert.deepEqual(buildStatusParams({ key: "qa", value: "ok" }, {}), {
      key: "qa",
      label: "qa",
      value: "ok",
    });
    assert.throws(
      () => buildStatusParams({ key: true, value: "ok" }, {}),
      /--key requires a value/,
    );
    assert.throws(
      () => buildStatusParams({ key: "qa", value: "" }, {}),
      /--value requires a value/,
    );
    assert.throws(
      () => buildStatusParams({ key: "qa", value: "ok", label: "" }, {}),
      /--label requires a value/,
    );
    assert.throws(
      () => buildStatusParams({ key: "qa", value: "ok", color: true }, {}),
      /--color requires a value/,
    );
    assert.throws(
      () => buildStatusParams({ key: "qa", value: "ok", colour: "red" }, {}),
      /set-status: unknown option --colour/,
    );
  });

  it("builds surface action params from env fallback", () => {
    assert.deepEqual(
      buildSurfaceActionParams({}, [], { FORKTTY_SURFACE_ID: "surface-1" }, "focus-surface"),
      {
        surface_id: "surface-1",
      },
    );
    assert.throws(
      () => buildSurfaceActionParams({ "surface-id": true }, [], {}, "focus-surface"),
      /--surface-id requires a value/,
    );
    assert.throws(
      () => buildSurfaceActionParams({ surface: "surface-1" }, [], {}, "focus-surface"),
      /focus-surface: unknown option --surface/,
    );
    assert.throws(
      () =>
        buildSurfaceActionParams(
          { "surface-id": "" },
          [],
          { FORKTTY_SURFACE_ID: "surface-env" },
          "focus-surface",
        ),
      /--surface-id requires a value/,
    );
    assert.throws(
      () => buildSurfaceActionParams({}, ["   "], { FORKTTY_SURFACE_ID: "surface-env" }),
      /surface id requires a value/,
    );
    assert.throws(
      () =>
        buildSurfaceActionParams(
          { "surface-id": "surface-2" },
          ["surface-1"],
          {},
          "focus-surface",
        ),
      /focus-surface: cannot combine --surface-id with a positional surface id/,
    );
    assert.throws(
      () => buildSurfaceActionParams({}, ["surface-1", "extra"], {}, "close-surface"),
      /close-surface: unexpected argument extra/,
    );
  });

  it("resolves send-text fallback from the active workspace surface", () => {
    assert.equal(
      surfaceIdFromWorkspaceList([
        { id: "workspace-1", active: false, focused_surface_id: "surface-1" },
        { id: "workspace-2", active: true, focused_surface_id: "surface-2" },
      ]),
      "surface-2",
    );
    assert.equal(
      surfaceIdFromWorkspaceList([{ id: "workspace-1", focusedSurfaceId: "surface-1" }]),
      "surface-1",
    );
    assert.equal(surfaceIdFromWorkspaceList([]), "");
  });

  it("builds surface split params with axis validation", () => {
    assert.deepEqual(buildSurfaceSplitParams({ axis: "vertical" }, ["surface-2"]), {
      surface_id: "surface-2",
      axis: "vertical",
    });
    assert.deepEqual(buildSurfaceSplitParams({}, [], {}), {
      axis: "horizontal",
    });
    assert.throws(
      () => buildSurfaceSplitParams({ axis: "diagonal" }, ["surface-2"]),
      /Invalid --axis/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({ axis: true }, ["surface-2"]),
      /--axis requires a value/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({ axis: "" }, ["surface-2"]),
      /--axis requires a value/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({ "surface-id": true }, [], {}),
      /--surface-id requires a value/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({ "surface-id": "surface-2" }, ["surface-1"], {}),
      /split-surface: cannot combine --surface-id with a positional surface id/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({ axs: "vertical" }, ["surface-2"]),
      /split-surface: unknown option --axs/,
    );
    assert.throws(
      () => buildSurfaceSplitParams({}, ["surface-2", "extra"]),
      /split-surface: unexpected argument extra/,
    );
  });

  it("resolves a positional selector to both id and name candidates", () => {
    assert.deepEqual(
      resolveSelectorParams({}, ["my-feature"], {}),
      [{ id: "my-feature" }, { name: "my-feature" }],
    );
    assert.deepEqual(
      resolveSelectorParams({}, [" my-feature "], {}),
      [{ id: "my-feature" }, { name: "my-feature" }],
    );
    assert.deepEqual(
      resolveSelectorParams({ "workspace-name": " release " }, [], {}),
      [{ name: "release" }],
    );
    assert.throws(
      () => resolveSelectorParams({ "workspace-name": " release " }, ["ignored"], {}, "focus"),
      /focus: cannot combine a positional selector with --workspace-name/,
    );
    assert.throws(
      () =>
        resolveSelectorParams(
          { "workspace-id": "workspace-1", "workspace-name": "release" },
          [],
          {},
          "focus",
        ),
      /focus: cannot combine --workspace-id and --workspace-name/,
    );
    assert.throws(
      () => resolveSelectorParams({}, ["workspace-1", "extra"], {}, "focus"),
      /focus: unexpected argument extra/,
    );
    assert.throws(
      () => resolveSelectorParams({ "worktree-name": true }, ["ignored"], {}),
      /--worktree-name requires a value/,
    );
    assert.throws(
      () => resolveSelectorParams({ "workspace-name": "   " }, ["ignored"], {}),
      /--workspace-name requires a value/,
    );
    assert.throws(
      () => resolveSelectorParams({ workspace: "main" }, [], {}, "focus"),
      /focus: unknown option --workspace/,
    );
    assert.throws(
      () => resolveSelectorParams({}, ["   "], { FORKTTY_WORKSPACE_ID: "ws-env" }),
      /workspace selector requires a value/,
    );
    assert.equal(resolveSelectorParams({}, [], {}), null);
  });

  it("builds worktree status params from path options and env fallback", () => {
    assert.deepEqual(buildWorktreeStatusParams({ path: "/repo/wt" }, [], {}), {
      path: "/repo/wt",
    });
    assert.deepEqual(buildWorktreeStatusParams({}, [], { PWD: "/repo/current" }), {
      path: "/repo/current",
    });
    assert.deepEqual(buildWorktreeStatusParams({}, [], {}, "/repo/process-cwd"), {
      path: "/repo/process-cwd",
    });
    assert.throws(
      () => buildWorktreeStatusParams({ path: true }, [], { PWD: "/repo/current" }),
      /--path requires a value/,
    );
    assert.throws(
      () => buildWorktreeStatusParams({ cwd: "" }, [], { PWD: "/repo/current" }),
      /--cwd requires a value/,
    );
    assert.throws(
      () => buildWorktreeStatusParams({ pth: "/repo/wt" }, [], { PWD: "/repo/current" }),
      /worktree-status: unknown option --pth/,
    );
    assert.throws(
      () => buildWorktreeStatusParams({}, ["/repo/wt", "extra"], {}),
      /worktree-status: unexpected argument extra/,
    );

    const originalCwd = process.cwd;
    process.cwd = () => {
      throw new Error("cwd boom");
    };
    try {
      assert.deepEqual(buildWorktreeStatusParams({ path: "/repo/wt" }, [], {}), {
        path: "/repo/wt",
      });
      assert.deepEqual(worktreeParams({ cwd: "/repo/explicit" }, ["feature/x"], true, {}), {
        name: "feature/x",
        cwd: "/repo/explicit",
      });
      assert.throws(
        () => buildWorktreeStatusParams({}, [], {}),
        /worktree-status requires --path, --cwd, a path, PWD, or the current directory/,
      );
    } finally {
      process.cwd = originalCwd;
    }
  });

  it("defaults worktree command cwd to the caller PWD", () => {
    assert.deepEqual(worktreeParams({}, ["feature/x"], true, { PWD: "/repo/current" }), {
      name: "feature/x",
      cwd: "/repo/current",
    });
    assert.deepEqual(worktreeParams({}, ["feature/x"], true, {}, "/repo/process-cwd"), {
      name: "feature/x",
      cwd: "/repo/process-cwd",
    });
    assert.deepEqual(
      worktreeParams({ cwd: "/repo/explicit" }, ["feature/x"], true, {
        PWD: "/repo/current",
      }),
      {
        name: "feature/x",
        cwd: "/repo/explicit",
      },
    );
    assert.throws(
      () => worktreeParams({ cwd: true }, ["feature/x"], true, { PWD: "/repo/current" }),
      /--cwd requires a value/,
    );
    {
      const originalCwd = process.cwd;
      process.cwd = () => "";
      try {
        assert.throws(
          () => worktreeParams({}, ["feature/x"], true, {}),
          /worktree command requires --cwd, PWD, or the current directory/,
        );
      } finally {
        process.cwd = originalCwd;
      }
    }
    assert.throws(
      () => worktreeParams({ branch: "" }, [], true, { PWD: "/repo/current" }),
      /--branch requires a value/,
    );
    assert.throws(
      () =>
        worktreeParams(
          { branch: "feature/y" },
          ["feature/x"],
          true,
          { PWD: "/repo/current" },
          process.cwd(),
          "worktree-create",
        ),
      /worktree-create: cannot combine a positional name with --name or --branch/,
    );
    assert.throws(
      () =>
        worktreeParams(
          { name: "feature/x", branch: "feature/y" },
          [],
          true,
          { PWD: "/repo/current" },
          process.cwd(),
          "worktree-create",
        ),
      /worktree-create: cannot combine --name and --branch/,
    );
    assert.throws(
      () => worktreeParams({ cw: "/repo/current" }, ["feature/x"], true, {}),
      /worktree command: unknown option --cw/,
    );
    assert.throws(
      () => worktreeParams({}, ["feature/x", "extra"], true, { PWD: "/repo/current" }),
      /worktree command: unexpected argument extra/,
    );
    assert.throws(
      () => worktreeParams({ name: "feature/x" }, ["   "], true, { PWD: "/repo/current" }),
      /worktree command requires a branch or worktree name/,
    );
  });
});

describe("hook installer", () => {
  let tmpDir;

  beforeEach(async () => {
    tmpDir = await fs.mkdtemp(path.join(os.tmpdir(), "forktty-hooks-"));
  });

  afterEach(async () => {
    await fs.rm(tmpDir, { recursive: true, force: true });
  });

  function makeContext(env = {}) {
    return {
      env: {
        CODEX_HOME: path.join(tmpDir, "codex"),
        CLAUDE_CONFIG_DIR: path.join(tmpDir, "claude"),
        HOME: tmpDir,
        ...env,
      },
      json: true,
      socketPath: "/tmp/forktty.sock",
    };
  }

  it("creates a fresh hook config when the file does not exist", async () => {
    const context = makeContext();
    const printed = [];
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (chunk) => {
      printed.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await handleHooksSetup(context, ["codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const written = await fs.readFile(
      path.join(context.env.CODEX_HOME, "hooks.json"),
      "utf8",
    );
    const parsed = JSON.parse(written);
    assert.ok(parsed.hooks?.SessionStart?.length > 0, "SessionStart hook installed");
    assert.match(printed.join(""), /"changed":\s*true/);
  });

  it("writes launcher-based hook commands when run through forktty", async () => {
    const context = makeContext({
      FORKTTY_HOOK_LAUNCHER: "/opt/forktty/forktty.AppImage",
      FORKTTY_HOOK_NODE: "/usr/bin/node",
    });
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = () => true;
    try {
      await handleHooksSetup(context, ["codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const written = JSON.parse(
      await fs.readFile(path.join(context.env.CODEX_HOME, "hooks.json"), "utf8"),
    );
    const command = written.hooks.SessionStart[0].hooks[0].command;
    assert.ok(command.includes("FORKTTY_NODE='/usr/bin/node'"));
    assert.ok(command.includes("'/opt/forktty/forktty.AppImage' hooks codex session-start"));
    assert.ok(!command.includes("scripts/forktty.mjs"));
  });

  it("installs PreToolUse and PostToolUse entries for claude", async () => {
    const context = makeContext();
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["claude"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const written = JSON.parse(
      await fs.readFile(
        path.join(context.env.CLAUDE_CONFIG_DIR, "settings.json"),
        "utf8",
      ),
    );
    assert.ok(written.hooks?.PreToolUse?.length, "PreToolUse hook installed");
    assert.ok(written.hooks?.PostToolUse?.length, "PostToolUse hook installed");
    assert.ok(written.hooks?.SubagentStop?.length, "SubagentStop hook installed");
    assert.ok(written.hooks?.PreCompact?.length, "PreCompact hook installed");
    const preCommand = written.hooks.PreToolUse[0].hooks[0].command;
    assert.match(preCommand, /hooks claude pre-tool/);
    const postCommand = written.hooks.PostToolUse[0].hooks[0].command;
    assert.match(postCommand, /hooks claude post-tool/);
    const subagentCommand = written.hooks.SubagentStop[0].hooks[0].command;
    assert.match(subagentCommand, /hooks claude subagent-stop/);
    const compactCommand = written.hooks.PreCompact[0].hooks[0].command;
    assert.match(compactCommand, /hooks claude pre-compact/);
  });

  it("installs current codex tool hook entries", async () => {
    const context = makeContext();
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = () => true;
    try {
      await handleHooksSetup(context, ["codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const written = JSON.parse(
      await fs.readFile(path.join(context.env.CODEX_HOME, "hooks.json"), "utf8"),
    );
    assert.ok(written.hooks?.PreToolUse?.length, "PreToolUse hook installed");
    assert.ok(written.hooks?.PostToolUse?.length, "PostToolUse hook installed");
    assert.match(written.hooks.PreToolUse[0].hooks[0].command, /hooks codex pre-tool/);
    assert.match(written.hooks.PostToolUse[0].hooks[0].command, /hooks codex post-tool/);
  });

  it("installs current gemini observability hook entries", async () => {
    const context = makeContext();
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = () => true;
    try {
      await handleHooksSetup(context, ["gemini"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const written = JSON.parse(
      await fs.readFile(path.join(context.env.HOME, ".gemini/settings.json"), "utf8"),
    );
    for (const event of [
      "BeforeTool",
      "AfterTool",
      "Notification",
      "PreCompress",
    ]) {
      assert.ok(written.hooks?.[event]?.length, `${event} hook installed`);
    }
    assert.match(written.hooks.BeforeTool[0].hooks[0].command, /hooks gemini pre-tool/);
    assert.match(written.hooks.AfterTool[0].hooks[0].command, /hooks gemini post-tool/);
    assert.match(written.hooks.Notification[0].hooks[0].command, /hooks gemini notification/);
    assert.match(written.hooks.PreCompress[0].hooks[0].command, /hooks gemini pre-compact/);
  });

  it("deduplicates repeated hook setup agent names", async () => {
    const context = makeContext();
    const printed = [];
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = (chunk) => {
      printed.push(typeof chunk === "string" ? chunk : chunk.toString());
      return true;
    };
    try {
      await handleHooksSetup(context, ["codex", "codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const summaries = JSON.parse(printed.join(""));
    assert.equal(summaries.length, 1);
    assert.equal(summaries[0].agent, "codex");
    assert.ok(
      JSON.parse(await fs.readFile(path.join(context.env.CODEX_HOME, "hooks.json"), "utf8")),
    );
  });

  it("is idempotent on a second run", async () => {
    const context = makeContext();
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex"]);
      const firstStat = await fs.stat(
        path.join(context.env.CODEX_HOME, "hooks.json"),
      );
      await handleHooksSetup(context, ["codex"]);
      const secondStat = await fs.stat(
        path.join(context.env.CODEX_HOME, "hooks.json"),
      );
      // Second run should not have rewritten the file, so mtime is unchanged.
      assert.equal(firstStat.mtimeMs, secondStat.mtimeMs);
    } finally {
      process.stdout.write = originalWrite;
    }
  });

  it("does not write anything in --dry-run mode", async () => {
    const context = makeContext();
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex", "--dry-run"]);
    } finally {
      process.stdout.write = originalWrite;
    }
    const codexConfig = path.join(context.env.CODEX_HOME, "hooks.json");
    await assert.rejects(fs.access(codexConfig), /ENOENT/);
  });

  it("keeps --dry-run safe before agent names", async () => {
    const context = makeContext();
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["--dry-run", "codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }
    const codexConfig = path.join(context.env.CODEX_HOME, "hooks.json");
    await assert.rejects(fs.access(codexConfig), /ENOENT/);
  });

  it("rejects invalid --dry-run values before writing hook configs", async () => {
    const context = makeContext();
    await assert.rejects(
      handleHooksSetup(context, ["--dry-run=yes", "codex"]),
      /hooks setup: --dry-run must be true or false/,
    );

    const codexConfig = path.join(context.env.CODEX_HOME, "hooks.json");
    await assert.rejects(fs.access(codexConfig), /ENOENT/);
  });

  it("rejects unknown setup options before writing hook configs", async () => {
    const context = makeContext();
    await assert.rejects(
      handleHooksSetup(context, ["--dryrun", "codex"]),
      /hooks setup: unknown option --dryrun/,
    );

    const codexConfig = path.join(context.env.CODEX_HOME, "hooks.json");
    await assert.rejects(fs.access(codexConfig), /ENOENT/);
  });

  it("uses HOME for default hook config locations", async () => {
    const context = makeContext({
      CODEX_HOME: "",
      CLAUDE_CONFIG_DIR: "",
      HOME: path.join(tmpDir, "isolated-home"),
    });
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex", "claude", "gemini"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const home = context.env.HOME;
    assert.ok(JSON.parse(await fs.readFile(path.join(home, ".codex/hooks.json"), "utf8")));
    assert.ok(JSON.parse(await fs.readFile(path.join(home, ".claude/settings.json"), "utf8")));
    assert.ok(JSON.parse(await fs.readFile(path.join(home, ".gemini/settings.json"), "utf8")));
  });


  it("supports hook setup paths containing spaces", async () => {
    const context = makeContext({
      CODEX_HOME: path.join(tmpDir, "codex home"),
      CLAUDE_CONFIG_DIR: path.join(tmpDir, "claude config"),
      HOME: path.join(tmpDir, "home dir"),
    });
    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex", "claude"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    assert.ok(JSON.parse(await fs.readFile(path.join(context.env.CODEX_HOME, "hooks.json"), "utf8")));
    assert.ok(JSON.parse(await fs.readFile(path.join(context.env.CLAUDE_CONFIG_DIR, "settings.json"), "utf8")));
  });

  it("preserves unrelated keys in an existing config", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    await fs.writeFile(
      configPath,
      JSON.stringify({ customKey: { keepMe: true } }, null, 2),
    );

    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    const parsed = JSON.parse(await fs.readFile(configPath, "utf8"));
    assert.deepEqual(parsed.customKey, { keepMe: true });
    assert.ok(parsed.hooks?.SessionStart?.length > 0);
  });

  it("updates symlinked hook config targets without replacing the symlink", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    const managedDir = path.join(tmpDir, "managed-codex");
    await fs.mkdir(codexDir, { recursive: true });
    await fs.mkdir(managedDir, { recursive: true });
    const targetPath = path.join(managedDir, "hooks.json");
    const configPath = path.join(codexDir, "hooks.json");
    await fs.writeFile(targetPath, `${JSON.stringify({ customKey: "managed" })}\n`);
    await fs.symlink(targetPath, configPath);

    const swallow = () => true;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = swallow;
    try {
      await handleHooksSetup(context, ["codex"]);
    } finally {
      process.stdout.write = originalWrite;
    }

    assert.ok((await fs.lstat(configPath)).isSymbolicLink());
    assert.equal(await fs.readlink(configPath), targetPath);
    const parsed = JSON.parse(await fs.readFile(targetPath, "utf8"));
    assert.equal(parsed.customKey, "managed");
    assert.ok(parsed.hooks?.SessionStart?.length > 0);
    const backups = (await fs.readdir(managedDir)).filter((name) =>
      name.startsWith("hooks.json.bak-"),
    );
    assert.equal(backups.length, 1);
    assert.deepEqual(await fs.readdir(codexDir), ["hooks.json"]);
  });

  it("creates distinct backups when changed setups share the same clock tick", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    const originalNow = Date.now;
    const originalWrite = process.stdout.write.bind(process.stdout);
    process.stdout.write = () => true;
    Date.now = () => 1716246123456;
    try {
      await fs.writeFile(configPath, `${JSON.stringify({ customKey: "first" })}\n`);
      await handleHooksSetup(context, ["codex"]);
      await fs.writeFile(configPath, `${JSON.stringify({ customKey: "second" })}\n`);
      await handleHooksSetup(context, ["codex"]);
    } finally {
      Date.now = originalNow;
      process.stdout.write = originalWrite;
    }

    const backups = (await fs.readdir(codexDir))
      .filter((name) => name.startsWith("hooks.json.bak-"))
      .sort();
    assert.equal(backups.length, 2);
    assert.notEqual(backups[0], backups[1]);
    const backupContents = await Promise.all(
      backups.map((name) => fs.readFile(path.join(codexDir, name), "utf8")),
    );
    assert.ok(backupContents.some((content) => content.includes("first")));
    assert.ok(backupContents.some((content) => content.includes("second")));
  });

  it("surfaces agent + path context when the config is malformed JSON", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    await fs.writeFile(configPath, "{ not json ::: ");

    await assert.rejects(
      handleHooksSetup(context, ["codex"]),
      (error) =>
        /codex/.test(error.message) &&
        error.message.includes(configPath) &&
        /JSON/i.test(error.message),
    );
  });

  it("preflights all requested configs before writing any hook config", async () => {
    const context = makeContext();
    const claudeDir = context.env.CLAUDE_CONFIG_DIR;
    await fs.mkdir(claudeDir, { recursive: true });
    const claudeConfig = path.join(claudeDir, "settings.json");
    await fs.writeFile(claudeConfig, "{ not json ::: ");

    await assert.rejects(
      handleHooksSetup(context, ["codex", "claude"]),
      (error) =>
        /claude/.test(error.message) &&
        error.message.includes(claudeConfig) &&
        /JSON/i.test(error.message),
    );

    await assert.rejects(
      fs.access(path.join(context.env.CODEX_HOME, "hooks.json")),
      /ENOENT/,
    );
  });

  it("rejects non-object hook configs without overwriting them", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    await fs.writeFile(configPath, "[]\n");

    await assert.rejects(
      handleHooksSetup(context, ["codex"]),
      (error) =>
        /codex/.test(error.message) &&
        error.message.includes(configPath) &&
        /JSON object/.test(error.message),
    );
    assert.equal(await fs.readFile(configPath, "utf8"), "[]\n");
  });

  it("rejects hook config paths that are not regular files", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    await fs.mkdir(configPath);

    await assert.rejects(
      handleHooksSetup(context, ["codex"]),
      (error) =>
        /codex/.test(error.message) &&
        error.message.includes(configPath) &&
        /not a regular file/.test(error.message),
    );
    assert.ok((await fs.stat(configPath)).isDirectory());
    assert.deepEqual(await fs.readdir(codexDir), ["hooks.json"]);
  });

  it("rejects broken hook config symlinks without replacing them", async () => {
    const context = makeContext();
    const codexDir = context.env.CODEX_HOME;
    await fs.mkdir(codexDir, { recursive: true });
    const configPath = path.join(codexDir, "hooks.json");
    const missingTarget = path.join(codexDir, "missing-hooks.json");
    await fs.symlink(missingTarget, configPath);

    await assert.rejects(
      handleHooksSetup(context, ["codex"]),
      (error) =>
        /codex/.test(error.message) &&
        error.message.includes(configPath) &&
        /broken symlink/.test(error.message),
    );
    assert.equal(await fs.readlink(configPath), missingTarget);
    assert.deepEqual(await fs.readdir(codexDir), ["hooks.json"]);
  });

  it("readAgentConfig returns {} for whitespace-only files without throwing", async () => {
    const configPath = path.join(tmpDir, "whitespace.json");
    await fs.writeFile(configPath, "   \n\t  \n");
    const value = await readAgentConfig("codex", configPath);
    assert.deepEqual(value, {});
  });

  it("atomicWriteFile replaces target and leaves no temp behind on success", async () => {
    const target = path.join(tmpDir, "atomic.json");
    await atomicWriteFile(target, "first\n");
    await atomicWriteFile(target, "second\n");
    const content = await fs.readFile(target, "utf8");
    assert.equal(content, "second\n");

    const siblings = await fs.readdir(tmpDir);
    const leftover = siblings.filter((name) => name.includes(".tmp-"));
    assert.deepEqual(leftover, [], `unexpected temp files: ${leftover.join(", ")}`);
  });

  it("atomicWriteFile removes temp file when temp write fails", async () => {
    const target = path.join(tmpDir, "atomic-write-fail.json");

    await assert.rejects(atomicWriteFile(target, { not: "text" }), TypeError);

    await assert.rejects(fs.access(target), /ENOENT/);
    const siblings = await fs.readdir(tmpDir);
    const leftover = siblings.filter((name) => name.includes(".tmp-"));
    assert.deepEqual(leftover, [], `unexpected temp files: ${leftover.join(", ")}`);
  });
});
