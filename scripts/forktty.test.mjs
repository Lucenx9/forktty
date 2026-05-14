import os from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";

import {
  buildHookActions,
  buildHookShellCommand,
  buildLogParams,
  buildProgressParams,
  buildSurfaceActionParams,
  buildSurfaceSplitParams,
  buildWorktreeStatusParams,
  defaultSocketPath,
  mergeHookConfig,
  surfaceIdFromWorkspaceList,
} from "./forktty.mjs";

describe("forktty CLI helpers", () => {
  it("builds the default socket path from XDG runtime dir", () => {
    expect(
      defaultSocketPath({
        XDG_RUNTIME_DIR: "/run/user/1000",
      }),
    ).toBe("/run/user/1000/forktty.sock");
  });

  it("falls back to a user temp socket path", () => {
    const socketPath = defaultSocketPath({});
    expect(socketPath.startsWith(os.tmpdir())).toBe(true);
    expect(socketPath.endsWith("forktty.sock")).toBe(true);
  });

  it("shell-quotes the generated hook command", () => {
    const scriptPath = "/tmp/ForkTTY Repo/scripts/forktty.mjs";
    const command = buildHookShellCommand(scriptPath, "codex", "session-start");
    expect(command).toContain("FORKTTY_CODEX_HOOKS_DISABLED");
    expect(command).toContain(
      "node '/tmp/ForkTTY Repo/scripts/forktty.mjs' hooks codex session-start",
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

    expect(changed).toBe(true);
    expect(config.custom).toBe(true);
    expect(config.hooks.SessionStart).toHaveLength(1);
    expect(config.hooks.UserPromptSubmit).toHaveLength(1);
    expect(config.hooks.Stop).toHaveLength(1);
  });

  it("maps notification events to status and prompt notifications", () => {
    const actions = buildHookActions(
      "claude",
      "notification",
      { message: "Review needed" },
      { FORKTTY_WORKSPACE_ID: "ws-1" },
    );

    expect(actions).toEqual([
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

    expect(actions).toEqual([
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
    expect(
      buildProgressParams(
        {
          key: "build",
          label: "Build",
          value: "12.5",
          total: "100",
        },
        { FORKTTY_WORKSPACE_ID: "ws-1" },
      ),
    ).toEqual({
      workspace_id: "ws-1",
      key: "build",
      label: "Build",
      value: 12.5,
      total: 100,
    });
  });

  it("rejects invalid progress values", () => {
    expect(() => buildProgressParams({ key: "build", value: "nan" })).toThrow(
      "Invalid --value",
    );
    expect(() => buildProgressParams({ key: "build", value: "1", total: "0" })).toThrow(
      "Invalid --total",
    );
  });

  it("builds log metadata params from positional text", () => {
    expect(
      buildLogParams(
        { level: "warn" },
        ["waiting", "for", "input"],
        "",
        { FORKTTY_WORKSPACE_ID: "ws-2" },
      ),
    ).toEqual({
      workspace_id: "ws-2",
      level: "warn",
      message: "waiting for input",
    });
  });

  it("rejects invalid log levels", () => {
    expect(() => buildLogParams({ level: "debug" }, ["hello"])).toThrow("Invalid --level");
  });

  it("builds surface action params from env fallback", () => {
    expect(
      buildSurfaceActionParams({}, [], { FORKTTY_SURFACE_ID: "surface-1" }, "focus-surface"),
    ).toEqual({
      surface_id: "surface-1",
    });
  });

  it("resolves send-text fallback from the active workspace surface", () => {
    expect(
      surfaceIdFromWorkspaceList([
        { id: "workspace-1", active: false, focused_surface_id: "surface-1" },
        { id: "workspace-2", active: true, focused_surface_id: "surface-2" },
      ]),
    ).toBe("surface-2");
    expect(surfaceIdFromWorkspaceList([{ id: "workspace-1", focusedSurfaceId: "surface-1" }])).toBe(
      "surface-1",
    );
    expect(surfaceIdFromWorkspaceList([])).toBe("");
  });

  it("builds surface split params with axis validation", () => {
    expect(buildSurfaceSplitParams({ axis: "vertical" }, ["surface-2"])).toEqual({
      surface_id: "surface-2",
      axis: "vertical",
    });
    expect(buildSurfaceSplitParams({}, [], {})).toEqual({
      axis: "horizontal",
    });
    expect(() => buildSurfaceSplitParams({ axis: "diagonal" }, ["surface-2"])).toThrow(
      "Invalid --axis",
    );
  });

  it("builds worktree status params from path options and env fallback", () => {
    expect(buildWorktreeStatusParams({ path: "/repo/wt" }, [], {})).toEqual({
      path: "/repo/wt",
    });
    expect(buildWorktreeStatusParams({}, [], { PWD: "/repo/current" })).toEqual({
      path: "/repo/current",
    });
  });
});
