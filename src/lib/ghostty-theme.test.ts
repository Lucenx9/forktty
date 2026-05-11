// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyThemeCssVars, toXtermTheme } from "./ghostty-theme";
import type { TerminalTheme } from "./pty-bridge";

describe("applyThemeCssVars", () => {
  let originalStyle: string;
  let originalDatasetTheme: string | undefined;

  beforeEach(() => {
    // Save original state to ensure test isolation
    originalStyle = document.documentElement.style.cssText;
    originalDatasetTheme = document.documentElement.dataset.theme;
  });

  afterEach(() => {
    // Restore original state
    document.documentElement.style.cssText = originalStyle;
    if (originalDatasetTheme === undefined) {
      delete document.documentElement.dataset.theme;
    } else {
      document.documentElement.dataset.theme = originalDatasetTheme;
    }
  });

  it("applies theme dataset to root element", () => {
    const theme = {
      background: "#1e1e2e",
      foreground: "#cdd6f4",
    } as TerminalTheme;

    applyThemeCssVars(theme);

    expect(document.documentElement.dataset.theme).toBe("applied");
  });

  it("sets css properties for defined theme variables", () => {
    const theme = {
      background: "#111111",
      foreground: "#222222",
      cursor: "#333333",
      selection_background: "#444444",
      black: "#555555",
      red: "#666666",
      green: "#777777",
      yellow: "#888888",
      blue: "#999999",
      magenta: "#aaaaaa",
      cyan: "#bbbbbb",
      white: "#cccccc",
      bright_black: "#dddddd",
      bright_red: "#eeeeee",
      bright_green: "#ffffff",
      bright_yellow: "#000000",
      bright_blue: "#111111",
      bright_magenta: "#222222",
      bright_cyan: "#333333",
      bright_white: "#444444",
    } as TerminalTheme;

    applyThemeCssVars(theme);

    const style = document.documentElement.style;
    expect(style.getPropertyValue("--theme-bg")).toBe("#111111");
    expect(style.getPropertyValue("--theme-fg")).toBe("#222222");
    expect(style.getPropertyValue("--theme-cursor")).toBe("#333333");
    expect(style.getPropertyValue("--theme-selection")).toBe("#444444");
    expect(style.getPropertyValue("--theme-black")).toBe("#555555");
    expect(style.getPropertyValue("--theme-red")).toBe("#666666");
    expect(style.getPropertyValue("--theme-green")).toBe("#777777");
    expect(style.getPropertyValue("--theme-yellow")).toBe("#888888");
    expect(style.getPropertyValue("--theme-blue")).toBe("#999999");
    expect(style.getPropertyValue("--theme-magenta")).toBe("#aaaaaa");
    expect(style.getPropertyValue("--theme-cyan")).toBe("#bbbbbb");
    expect(style.getPropertyValue("--theme-white")).toBe("#cccccc");
    expect(style.getPropertyValue("--theme-bright-black")).toBe("#dddddd");
    expect(style.getPropertyValue("--theme-bright-red")).toBe("#eeeeee");
    expect(style.getPropertyValue("--theme-bright-green")).toBe("#ffffff");
    expect(style.getPropertyValue("--theme-bright-yellow")).toBe("#000000");
    expect(style.getPropertyValue("--theme-bright-blue")).toBe("#111111");
    expect(style.getPropertyValue("--theme-bright-magenta")).toBe("#222222");
    expect(style.getPropertyValue("--theme-bright-cyan")).toBe("#333333");
    expect(style.getPropertyValue("--theme-bright-white")).toBe("#444444");
  });

  it("does not set css properties for undefined theme variables", () => {
    const theme = {
      background: "#1e1e2e",
      foreground: "#cdd6f4",
    } as TerminalTheme;

    applyThemeCssVars(theme);

    const style = document.documentElement.style;
    // Check that we set the ones we defined
    expect(style.getPropertyValue("--theme-bg")).toBe("#1e1e2e");
    expect(style.getPropertyValue("--theme-fg")).toBe("#cdd6f4");

    // Check that undefined ones are not set
    expect(style.getPropertyValue("--theme-cursor")).toBe("");
    expect(style.getPropertyValue("--theme-selection")).toBe("");
  });

  it("calculates derived sidebar colors", () => {
    // We provide specific test hex values where we can easily predict output or verify it
    const theme = {
      background: "#1e1e2e", // RGB: 30, 30, 46
      blue: "#89b4fa",
    } as TerminalTheme;

    applyThemeCssVars(theme);

    const style = document.documentElement.style;
    // We don't want to perfectly duplicate the inner logic (darken/lighten)
    // but we can assert they are set to *something* not empty, and derived correctly.
    expect(style.getPropertyValue("--sidebar-bg")).not.toBe("");
    expect(style.getPropertyValue("--sidebar-border")).not.toBe("");
    expect(style.getPropertyValue("--sidebar-hover")).not.toBe("");

    expect(style.getPropertyValue("--sidebar-active")).toBe("#89b4fa26");
    expect(style.getPropertyValue("--sidebar-active-hover")).toBe("#89b4fa33");
  });

  it("calculates derived sidebar colors with fallback values if background/blue are undefined", () => {
    const theme = {} as TerminalTheme;

    applyThemeCssVars(theme);

    const style = document.documentElement.style;
    // It should use #1e1e2e for bg fallback and #89b4fa for blue fallback
    expect(style.getPropertyValue("--sidebar-active")).toBe("#89b4fa26");
    expect(style.getPropertyValue("--sidebar-active-hover")).toBe("#89b4fa33");
  });
});

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
