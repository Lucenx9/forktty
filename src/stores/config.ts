import { create } from "zustand";
import type { AppConfig, TerminalTheme } from "../lib/pty-bridge";
import {
  getConfig,
  saveConfig as saveConfigApi,
  getTheme,
} from "../lib/pty-bridge";
import type { ITheme } from "@xterm/xterm";
import { toXtermTheme, applyThemeCssVars } from "../lib/ghostty-theme";

interface ConfigState {
  config: AppConfig | null;
  theme: TerminalTheme | null;
  xtermTheme: ITheme | null;
  showSettings: boolean;
  loaded: boolean;

  loadConfig: () => Promise<void>;
  saveConfig: (config: AppConfig) => Promise<void>;
  toggleSettings: () => void;
}

const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  theme: null,
  xtermTheme: null,
  showSettings: false,
  loaded: false,

  loadConfig: async () => {
    try {
      const [config, theme] = await Promise.all([getConfig(), getTheme()]);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme, loaded: true });
    } catch (err) {
      console.error("Failed to load config:", err);
      set({ loaded: true });
    }
  },

  saveConfig: async (config) => {
    try {
      await saveConfigApi(config);
      // Reload theme after saving (theme may have changed)
      const theme = await getTheme();
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme });
    } catch (err) {
      console.error("Failed to save config:", err);
    }
  },

  toggleSettings: () => {
    set({ showSettings: !get().showSettings });
  },
}));

export { useConfigStore };
export type { ConfigState };
