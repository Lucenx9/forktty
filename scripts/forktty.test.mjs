import fs from "node:fs/promises";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { afterEach, beforeEach, describe, it } from "node:test";

import {
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
  readAgentConfig,
  resolveSelectorParams,
  sendSocketRequest,
  surfaceIdFromWorkspaceList,
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
    assert.match(error.message, /FORKTTY_SOCKET_PATH/);
    assert.equal(error.cause, raw);
  });

  it("explains socket permission failures without hiding the path", () => {
    const raw = Object.assign(new Error("connect EACCES"), { code: "EACCES" });
    const error = formatSocketConnectError(raw, "/run/user/1000/forktty.sock");

    assert.match(error.message, /Cannot access ForkTTY socket/);
    assert.match(error.message, /\/run\/user\/1000\/forktty\.sock/);
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
            error.message.includes(socketPath) &&
            /workspace\.select/.test(error.message) &&
            /not_found: Workspace not found/.test(error.message),
        );
      },
    );
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

  it("shell-quotes the generated hook command", () => {
    const scriptPath = "/tmp/ForkTTY Repo/scripts/forktty.mjs";
    const command = buildHookShellCommand(scriptPath, "codex", "session-start");
    assert.match(command, /FORKTTY_CODEX_HOOKS_DISABLED/);
    assert.ok(
      command.includes(
        "node '/tmp/ForkTTY Repo/scripts/forktty.mjs' hooks codex session-start",
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
  });

  it("rejects invalid progress values", () => {
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
    assert.throws(() => buildLogParams({ level: "debug" }, ["hello"]), /Invalid --level/);
  });

  it("builds surface action params from env fallback", () => {
    assert.deepEqual(
      buildSurfaceActionParams({}, [], { FORKTTY_SURFACE_ID: "surface-1" }, "focus-surface"),
      {
        surface_id: "surface-1",
      },
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
  });

  it("resolves a positional selector to both id and name candidates", () => {
    assert.deepEqual(
      resolveSelectorParams({}, ["my-feature"], {}),
      [{ id: "my-feature" }, { name: "my-feature" }],
    );
    assert.deepEqual(
      resolveSelectorParams({ "workspace-name": "release" }, ["ignored"], {}),
      [{ name: "release" }],
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
  });

  it("defaults worktree command cwd to the caller PWD", () => {
    assert.deepEqual(worktreeParams({}, ["feature/x"], true, { PWD: "/repo/current" }), {
      name: "feature/x",
      cwd: "/repo/current",
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
