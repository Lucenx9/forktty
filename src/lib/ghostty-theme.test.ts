import { describe, expect, it } from "vitest";
import { toXtermTheme } from "./ghostty-theme";
import type { TerminalTheme } from "./pty-bridge";

describe("toXtermTheme", () => {
  it("applies default values when theme is empty", () => {
    const emptyTheme = {} as TerminalTheme;
    const result = toXtermTheme(emptyTheme);

    expect(result).toEqual({
      background: "#1e1e2e",
      foreground: "#cdd6f4",
      cursor: "#f5e0dc",
      // default selection is "#585b70" with 0.5 alpha.
      // 0.5 * 255 = 127.5, rounded to 128 -> "80" in hex
      selectionBackground: "#585b7080",
      selectionForeground: undefined,
      black: "#45475a",
      red: "#f38ba8",
      green: "#a6e3a1",
      yellow: "#f9e2af",
      blue: "#89b4fa",
      magenta: "#f5c2e7",
      cyan: "#94e2d5",
      white: "#bac2de",
      brightBlack: "#585b70",
      brightRed: "#f38ba8",
      brightGreen: "#a6e3a1",
      brightYellow: "#f9e2af",
      brightBlue: "#89b4fa",
      brightMagenta: "#f5c2e7",
      brightCyan: "#94e2d5",
      brightWhite: "#a6adc8",
    });
  });

  it("maps provided theme values correctly", () => {
    const customTheme = {
      background: "#000000",
      foreground: "#ffffff",
      cursor: "#ff0000",
      selection_background: "#00ff00",
      selection_foreground: "#0000ff",
      black: "#111111",
      red: "#222222",
      green: "#333333",
      yellow: "#444444",
      blue: "#555555",
      magenta: "#666666",
      cyan: "#777777",
      white: "#888888",
      bright_black: "#999999",
      bright_red: "#aaaaaa",
      bright_green: "#bbbbbb",
      bright_yellow: "#cccccc",
      bright_blue: "#dddddd",
      bright_magenta: "#eeeeee",
      bright_cyan: "#ffffff",
      bright_white: "#000000",
    } as TerminalTheme;

    const result = toXtermTheme(customTheme);

    expect(result).toEqual({
      background: "#000000",
      foreground: "#ffffff",
      cursor: "#ff0000",
      // "#00ff00" with 0.5 alpha -> "#00ff0080"
      selectionBackground: "#00ff0080",
      selectionForeground: "#0000ff",
      black: "#111111",
      red: "#222222",
      green: "#333333",
      yellow: "#444444",
      blue: "#555555",
      magenta: "#666666",
      cyan: "#777777",
      white: "#888888",
      brightBlack: "#999999",
      brightRed: "#aaaaaa",
      brightGreen: "#bbbbbb",
      brightYellow: "#cccccc",
      brightBlue: "#dddddd",
      brightMagenta: "#eeeeee",
      brightCyan: "#ffffff",
      brightWhite: "#000000",
    });
  });

  it("preserves alpha channel if already present in selection_background", () => {
    const themeWithAlpha = {
      // 8-character hex, already has alpha channel
      selection_background: "#11223344",
    } as TerminalTheme;

    const result = toXtermTheme(themeWithAlpha);
    expect(result.selectionBackground).toBe("#11223344");
  });

  it("handles 3-digit hex colors in selection_background", () => {
    const themeWithShortHex = {
      // #abc is 3 digits, equivalent to #aabbcc
      selection_background: "#abc",
    } as TerminalTheme;

    const result = toXtermTheme(themeWithShortHex);
    expect(result.selectionBackground).toBe("#aabbcc80");
  });
});
