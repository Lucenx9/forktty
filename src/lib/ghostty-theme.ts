import type { ITheme } from "@xterm/xterm";
import type { TerminalTheme } from "./pty-bridge";

/**
 * Convert a backend TerminalTheme to xterm.js ITheme.
 * All fields should be pre-filled by the backend's resolve_theme().
 */
export function toXtermTheme(theme: TerminalTheme): ITheme {
  return {
    background: theme.background ?? "#1e1e2e",
    foreground: theme.foreground ?? "#cdd6f4",
    cursor: theme.cursor ?? "#f5e0dc",
    selectionBackground: addAlpha(theme.selection_background ?? "#585b70", 0.5),
    selectionForeground: theme.selection_foreground ?? undefined,
    black: theme.black ?? "#45475a",
    red: theme.red ?? "#f38ba8",
    green: theme.green ?? "#a6e3a1",
    yellow: theme.yellow ?? "#f9e2af",
    blue: theme.blue ?? "#89b4fa",
    magenta: theme.magenta ?? "#f5c2e7",
    cyan: theme.cyan ?? "#94e2d5",
    white: theme.white ?? "#bac2de",
    brightBlack: theme.bright_black ?? "#585b70",
    brightRed: theme.bright_red ?? "#f38ba8",
    brightGreen: theme.bright_green ?? "#a6e3a1",
    brightYellow: theme.bright_yellow ?? "#f9e2af",
    brightBlue: theme.bright_blue ?? "#89b4fa",
    brightMagenta: theme.bright_magenta ?? "#f5c2e7",
    brightCyan: theme.bright_cyan ?? "#94e2d5",
    brightWhite: theme.bright_white ?? "#a6adc8",
  };
}

/**
 * Apply theme colors as CSS custom properties on :root for sidebar/app theming.
 */
export function applyThemeCssVars(theme: TerminalTheme): void {
  const root = document.documentElement;
  root.dataset.theme = "applied";
  const set = (name: string, value: string | null) => {
    if (value) root.style.setProperty(name, value);
  };

  set("--theme-bg", theme.background);
  set("--theme-fg", theme.foreground);
  set("--theme-cursor", theme.cursor);
  set("--theme-selection", theme.selection_background);
  set("--theme-black", theme.black);
  set("--theme-red", theme.red);
  set("--theme-green", theme.green);
  set("--theme-yellow", theme.yellow);
  set("--theme-blue", theme.blue);
  set("--theme-magenta", theme.magenta);
  set("--theme-cyan", theme.cyan);
  set("--theme-white", theme.white);
  set("--theme-bright-black", theme.bright_black);
  set("--theme-bright-red", theme.bright_red);
  set("--theme-bright-green", theme.bright_green);
  set("--theme-bright-yellow", theme.bright_yellow);
  set("--theme-bright-blue", theme.bright_blue);
  set("--theme-bright-magenta", theme.bright_magenta);
  set("--theme-bright-cyan", theme.bright_cyan);
  set("--theme-bright-white", theme.bright_white);

  // Derived sidebar colors
  const bg = theme.background ?? "#1e1e2e";
  root.style.setProperty("--sidebar-bg", darken(bg, 0.12));
  root.style.setProperty("--sidebar-border", lighten(bg, 0.08));
  root.style.setProperty("--sidebar-hover", lighten(bg, 0.06));
  root.style.setProperty("--sidebar-active", `${theme.blue ?? "#89b4fa"}26`);
  root.style.setProperty("--sidebar-active-hover", `${theme.blue ?? "#89b4fa"}33`);
}

/** Darken a hex color by a fraction (0..1). */
function darken(hex: string, amount: number): string {
  const rgb = hexToRgb(hex);
  if (!rgb) return hex;
  return rgbToHex(
    Math.round(rgb.r * (1 - amount)),
    Math.round(rgb.g * (1 - amount)),
    Math.round(rgb.b * (1 - amount)),
  );
}

/** Lighten a hex color by a fraction (0..1). */
function lighten(hex: string, amount: number): string {
  const rgb = hexToRgb(hex);
  if (!rgb) return hex;
  return rgbToHex(
    Math.min(255, Math.round(rgb.r + (255 - rgb.r) * amount)),
    Math.min(255, Math.round(rgb.g + (255 - rgb.g) * amount)),
    Math.min(255, Math.round(rgb.b + (255 - rgb.b) * amount)),
  );
}

/** Convert a hex color to #RRGGBBAA format with the given alpha (0..1). */
function addAlpha(hex: string, alpha: number): string {
  // Already has alpha channel — return as-is
  if (/^#[a-f\d]{8}$/i.test(hex)) return hex;
  const rgb = hexToRgb(hex);
  if (!rgb) return hex;
  const a = Math.round(alpha * 255)
    .toString(16)
    .padStart(2, "0");
  return `${rgbToHex(rgb.r, rgb.g, rgb.b)}${a}`;
}

function hexToRgb(hex: string): { r: number; g: number; b: number } | null {
  // Handle 3-digit shorthand (#abc -> #aabbcc)
  const short = /^#?([a-f\d])([a-f\d])([a-f\d])$/i.exec(hex);
  if (short) {
    return {
      r: parseInt(short[1]! + short[1]!, 16),
      g: parseInt(short[2]! + short[2]!, 16),
      b: parseInt(short[3]! + short[3]!, 16),
    };
  }
  const match = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (!match) return null;
  return {
    r: parseInt(match[1]!, 16),
    g: parseInt(match[2]!, 16),
    b: parseInt(match[3]!, 16),
  };
}

function rgbToHex(r: number, g: number, b: number): string {
  return `#${((1 << 24) | (r << 16) | (g << 8) | b).toString(16).slice(1)}`;
}
