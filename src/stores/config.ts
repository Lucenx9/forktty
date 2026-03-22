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
      theme: "ghostty",
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
      idle_threshold_ms: 2000,
    },
  };
}

function makePreviewTheme(config: AppConfig): TerminalTheme {
  return {
    background: null,
    foreground: null,
    cursor: null,
    selection_background: null,
    selection_foreground: null,
    black: null,
    red: null,
    green: null,
    yellow: null,
    blue: null,
    magenta: null,
    cyan: null,
    white: null,
    bright_black: null,
    bright_red: null,
    bright_green: null,
    bright_yellow: null,
    bright_blue: null,
    bright_magenta: null,
    bright_cyan: null,
    bright_white: null,
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
