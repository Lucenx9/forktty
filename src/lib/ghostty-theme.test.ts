// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { applyThemeCssVars } from "./ghostty-theme";
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

    // Active and hover-active are just {blue} + alpha (26 or 33)
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
