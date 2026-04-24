import { describe, expect, it } from "vitest";
import { buildTerminalFontFamily } from "./terminal-fonts";

describe("buildTerminalFontFamily", () => {
  it("prefers concrete fallbacks before generic monospace", () => {
    expect(buildTerminalFontFamily("monospace")).toBe(
      '"DejaVu Sans Mono", "Liberation Mono", "Noto Mono", "FreeMono", monospace, "Symbols Nerd Font Mono"',
    );
  });

  it("normalizes a quoted single family", () => {
    expect(buildTerminalFontFamily('"JetBrains Mono"')).toContain('"JetBrains Mono"');
    expect(buildTerminalFontFamily('"JetBrains Mono"')).not.toContain(
      '"\\"JetBrains Mono\\""',
    );
  });

  it("preserves a comma-separated family list", () => {
    expect(buildTerminalFontFamily('"JetBrains Mono", "Fira Code"')).toMatch(
      /^"JetBrains Mono", "Fira Code", /,
    );
  });

  it("deduplicates repeated fallbacks", () => {
    const fontStack = buildTerminalFontFamily("monospace, Liberation Mono");
    expect(fontStack.startsWith('"Liberation Mono", ')).toBe(true);
    expect(fontStack.match(/monospace/g)?.length).toBe(1);
    expect(fontStack.match(/"Liberation Mono"/g)?.length).toBe(1);
  });

  it("falls back to a sane monospace stack when empty", () => {
    expect(buildTerminalFontFamily("")).toBe(
      '"DejaVu Sans Mono", "Liberation Mono", "Noto Mono", "FreeMono", monospace, "Symbols Nerd Font Mono"',
    );
  });
});
