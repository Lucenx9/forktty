import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";

import {
  atomicWriteFile,
  buildClearMetadataParams,
  buildCreateWorkspaceParams,
  buildHookActions,
  buildHookShellCommand,
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
  handleHooksSetup,
  main,
  mergeHookConfig,
  parseGlobalArgs,
  parseFlags,
  readAgentConfig,
  resolveSelectorParams,
  sendSocketRequest,
  surfaceIdFromWorkspaceList,
  shouldSendHookActions,
  shouldReadCommandStdin,
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
      },
    );
    assert.deepEqual(
      parseGlobalArgs(["ping", "--socket", " /tmp/trimmed.sock "], {}),
      {
        args: ["ping"],
        json: false,
        socketPath: "/tmp/trimmed.sock",
        socketExplicit: true,
        help: false,
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
      },
    );
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
      () => buildSurfaceSplitParams({ axs: "vertical" }, ["surface-2"]),
      /split-surface: unknown option --axs/,
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
      resolveSelectorParams({ "workspace-name": " release " }, ["ignored"], {}),
      [{ name: "release" }],
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
      () => buildWorktreeStatusParams({}, [], {}, ""),
      /worktree-status requires --path, --cwd, a path, PWD, or the current directory/,
    );
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
    assert.throws(
      () => worktreeParams({}, ["feature/x"], true, {}, ""),
      /worktree command requires --cwd, PWD, or the current directory/,
    );
    assert.throws(
      () => worktreeParams({ branch: "" }, [], true, { PWD: "/repo/current" }),
      /--branch requires a value/,
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
