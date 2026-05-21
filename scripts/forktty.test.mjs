import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  buildHookActions,
  buildHookShellCommand,
  buildLogParams,
  buildProgressParams,
  buildSurfaceActionParams,
  buildSurfaceSplitParams,
  buildWorktreeStatusParams,
  defaultSocketPath,
  formatNotificationLine,
  mergeHookConfig,
  resolveSelectorParams,
  surfaceIdFromWorkspaceList,
} from "./forktty.mjs";

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
});
