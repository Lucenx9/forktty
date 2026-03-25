import { create } from "zustand";
import type { AppConfig, TerminalTheme } from "../lib/pty-bridge";
import {
  getConfig,
  saveConfig as saveConfigApi,
  getTheme,
  writeLog,
  logError,
  hasTauriRuntime,
} from "../lib/pty-bridge";
import type { ITheme } from "@xterm/xterm";
import { toXtermTheme, applyThemeCssVars } from "../lib/ghostty-theme";
import { showToast } from "../components/ErrorToast";

interface ConfigState {
  config: AppConfig | null;
  theme: TerminalTheme | null;
  xtermTheme: ITheme | null;
  showSettings: boolean;
  loaded: boolean;
  fontSizeOffset: number;

  loadConfig: () => Promise<void>;
  saveConfig: (config: AppConfig) => Promise<void>;
  toggleSettings: () => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomReset: () => void;
}

function makeDefaultConfig(): AppConfig {
  return {
    general: {
      theme_source: "auto",
      shell: "/bin/bash",
      worktree_layout: "nested",
      notification_command: "",
    },
    appearance: {
      font_family: "",
      font_size: 14,
      sidebar_position: "left",
    },
    notifications: {
      desktop: true,
      sound: true,
    },
  };
}

function makePreviewTheme(config: AppConfig): TerminalTheme {
  return {
    background: "#1e1e2e",
    foreground: "#cdd6f4",
    cursor: "#f5e0dc",
    selection_background: "#585b70",
    selection_foreground: null,
    black: "#45475a",
    red: "#f38ba8",
    green: "#a6e3a1",
    yellow: "#f9e2af",
    blue: "#89b4fa",
    magenta: "#f5c2e7",
    cyan: "#94e2d5",
    white: "#bac2de",
    bright_black: "#585b70",
    bright_red: "#f38ba8",
    bright_green: "#a6e3a1",
    bright_yellow: "#f9e2af",
    bright_blue: "#89b4fa",
    bright_magenta: "#f5c2e7",
    bright_cyan: "#94e2d5",
    bright_white: "#a6adc8",
    font_family: config.appearance.font_family || null,
    font_size: config.appearance.font_size,
  };
}

const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  theme: null,
  xtermTheme: null,
  showSettings: false,
  loaded: false,
  fontSizeOffset: 0,

  loadConfig: async () => {
    if (!hasTauriRuntime()) {
      const config = makeDefaultConfig();
      const theme = makePreviewTheme(config);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme, loaded: true });
      return;
    }

    try {
      const [config, theme] = await Promise.all([getConfig(), getTheme()]);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme, loaded: true });
    } catch (err) {
      writeLog("ERROR", `Failed to load config: ${err}`).catch(logError);
      const config = makeDefaultConfig();
      const theme = makePreviewTheme(config);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme, loaded: true });
    }
  },

  saveConfig: async (config) => {
    if (!hasTauriRuntime()) {
      const theme = makePreviewTheme(config);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme });
      return;
    }

    try {
      await saveConfigApi(config);
      // Reload theme after saving (theme may have changed)
      const theme = await getTheme();
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme });
    } catch (err) {
      showToast(`Failed to save config: ${err}`, "error");
    }
  },

  toggleSettings: () => {
    set({ showSettings: !get().showSettings });
  },

  zoomIn: () => {
    const { fontSizeOffset, theme } = get();
    const base = theme?.font_size ?? 14;
    if (base + fontSizeOffset < 32) set({ fontSizeOffset: fontSizeOffset + 1 });
  },

  zoomOut: () => {
    const { fontSizeOffset, theme } = get();
    const base = theme?.font_size ?? 14;
    if (base + fontSizeOffset > 8) set({ fontSizeOffset: fontSizeOffset - 1 });
  },

  zoomReset: () => {
    set({ fontSizeOffset: 0 });
  },
}));

export { useConfigStore };
export type { ConfigState };
