// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { spawnPty } from "./pty-bridge";

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: unknown;
};

interface MockChannel<T> {
  onmessage: ((message: T) => void) | null;
}

type PtyEofEvent = {
  kind: "Eof";
};

vi.mock("@tauri-apps/api/core", () => {
  return {
    invoke: vi.fn(),
    Channel: class MockTauriChannel<T> implements MockChannel<T> {
      onmessage: ((message: T) => void) | null = null;
    },
  };
});

describe("pty-bridge", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    (window as TauriWindow).__TAURI_INTERNALS__ = {};
  });

  afterEach(() => {
    vi.restoreAllMocks();
    delete (window as TauriWindow).__TAURI_INTERNALS__;
  });

  describe("spawnPty", () => {
    it("calls invoke with the expected pty_spawn payload", async () => {
      vi.mocked(invoke).mockResolvedValueOnce(42);

      const result = await spawnPty({
        cwd: "/tmp",
        workspaceId: "workspace-1",
        surfaceId: "surface-1",
        rows: 24,
        cols: 80,
        onOutput: vi.fn(),
        onExit: vi.fn(),
      });

      expect(invoke).toHaveBeenCalledWith(
        "pty_spawn",
        expect.objectContaining({
          cwd: "/tmp",
          workspaceId: "workspace-1",
          surfaceId: "surface-1",
          rows: 24,
          cols: 80,
          onOutput: expect.objectContaining({ onmessage: expect.any(Function) }),
        }),
      );
      expect(result).toBe(42);
    });

    it("calls onExit when the output channel receives EOF", async () => {
      vi.mocked(invoke).mockResolvedValueOnce(42);
      const onExit = vi.fn();

      await spawnPty({
        onOutput: vi.fn(),
        onExit,
        rows: 24,
        cols: 80,
      });

      const payload = vi.mocked(invoke).mock.calls[0]?.[1] as
        | { onOutput: MockChannel<PtyEofEvent> }
        | undefined;

      payload?.onOutput.onmessage?.({ kind: "Eof" });

      expect(onExit).toHaveBeenCalledOnce();
    });

    it("passes null for optional arguments when omitted", async () => {
      vi.mocked(invoke).mockResolvedValueOnce(42);

      await spawnPty({
        onOutput: vi.fn(),
        onExit: vi.fn(),
        rows: 24,
        cols: 80,
      });

      expect(invoke).toHaveBeenCalledWith(
        "pty_spawn",
        expect.objectContaining({
          cwd: null,
          workspaceId: null,
          surfaceId: null,
          rows: 24,
          cols: 80,
        }),
      );
    });

    it("propagates generic errors directly from invoke", async () => {
      const error = new Error("IPC failure");
      vi.mocked(invoke).mockRejectedValueOnce(error);

      await expect(
        spawnPty({
          onOutput: vi.fn(),
          onExit: vi.fn(),
          rows: 24,
          cols: 80,
        }),
      ).rejects.toThrow(error);
    });

    it("rejects before invoking when the Tauri runtime is unavailable", async () => {
      delete (window as TauriWindow).__TAURI_INTERNALS__;

      await expect(
        spawnPty({
          onOutput: vi.fn(),
          onExit: vi.fn(),
          rows: 24,
          cols: 80,
        }),
      ).rejects.toThrow("PTY spawn is only available inside the Tauri app");
      expect(invoke).not.toHaveBeenCalled();
    });

    it("normalizes missing Tauri runtime errors thrown by invoke", async () => {
      const tauriError = new Error("window is not defined");
      vi.mocked(invoke).mockRejectedValueOnce(tauriError);

      await expect(
        spawnPty({
          onOutput: vi.fn(),
          onExit: vi.fn(),
          rows: 24,
          cols: 80,
        }),
      ).rejects.toThrow("PTY spawn is only available inside the Tauri app");
    });
  });
});
