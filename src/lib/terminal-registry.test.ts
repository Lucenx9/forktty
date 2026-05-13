import { afterEach, describe, expect, it, vi } from "vitest";
import { killPty } from "./pty-bridge";
import {
  readScreen,
  reconcileInstances,
  registerTerminal,
  saveInstance,
  unregisterTerminal,
} from "./terminal-registry";
import type { Terminal, IBufferLine } from "@xterm/xterm";

vi.mock("./pty-bridge", () => ({
  killPty: vi.fn().mockResolvedValue(undefined),
  logError: vi.fn(),
}));

function createMockTerminal(lines: (string | null)[] = []): Terminal {
  const buffer = {
    active: {
      length: lines.length,
      getLine: (index: number): IBufferLine | undefined => {
        const lineContent = lines[index];
        if (lineContent === null) return undefined;
        return {
          translateToString: (_trimRight?: boolean) => lineContent,
        } as IBufferLine;
      },
    },
  };

  return {
    buffer,
    dispose: vi.fn(),
  } as unknown as Terminal;
}

describe("terminal-registry: readScreen", () => {
  afterEach(() => {
    // Unregister terminals used in tests
    unregisterTerminal("test-pane-1");
    vi.clearAllMocks();
  });

  it("returns null if paneId is falsy", () => {
    expect(readScreen(null)).toBeNull();
    expect(readScreen(undefined)).toBeNull();
    expect(readScreen("")).toBeNull();
  });

  it("returns null if paneId is not registered", () => {
    expect(readScreen("non-existent-pane")).toBeNull();
  });

  it("returns joined lines from the terminal buffer", () => {
    const mockTerminal = createMockTerminal(["Line 1", "Line 2", "Line 3"]);
    registerTerminal("test-pane-1", mockTerminal);

    expect(readScreen("test-pane-1")).toBe("Line 1\nLine 2\nLine 3");
  });

  it("trims trailing empty lines", () => {
    const mockTerminal = createMockTerminal(["Line 1", "Line 2", "", "   ", ""]);
    registerTerminal("test-pane-1", mockTerminal);

    expect(readScreen("test-pane-1")).toBe("Line 1\nLine 2");
  });

  it("keeps leading or middle empty lines", () => {
    const mockTerminal = createMockTerminal(["", "Line 1", "", "Line 2", ""]);
    registerTerminal("test-pane-1", mockTerminal);

    expect(readScreen("test-pane-1")).toBe("\nLine 1\n\nLine 2");
  });

  it("handles missing lines gracefully", () => {
    const mockTerminal = createMockTerminal(["Line 1", null, "Line 3"]);
    registerTerminal("test-pane-1", mockTerminal);

    expect(readScreen("test-pane-1")).toBe("Line 1\nLine 3");
  });

  it("kills PTYs for orphaned saved instances during reconciliation", async () => {
    const wrapper = {
      remove: vi.fn(),
    } as unknown as HTMLDivElement;
    saveInstance("orphan-pane", {
      terminal: createMockTerminal(),
      wrapper,
      runtime: {
        ptyId: 42,
        lastCols: null,
        lastRows: null,
        disposed: false,
      },
      fitAddon: {} as never,
      canvasAddon: null,
      searchAddon: {} as never,
    });

    expect(reconcileInstances(new Set())).toBe(1);
    await Promise.resolve();

    expect(killPty).toHaveBeenCalledWith(42);
    expect(wrapper.remove).toHaveBeenCalled();
  });
});
