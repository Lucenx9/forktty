import { describe, expect, it, afterEach } from "vitest";
import { registerTerminal, unregisterTerminal, readScreen } from "./terminal-registry";
import type { Terminal, IBufferLine } from "@xterm/xterm";

function createMockTerminal(lines: (string | null)[]): Terminal {
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
  } as unknown as Terminal;
}

describe("terminal-registry: readScreen", () => {
  afterEach(() => {
    // Unregister terminals used in tests
    unregisterTerminal("test-pane-1");
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
});
