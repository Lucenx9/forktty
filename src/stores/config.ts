import { create } from "zustand";
import type { AppConfig, TerminalTheme } from "../lib/pty-bridge";
import {
  getConfig,
  saveConfig as saveConfigApi,
  getTheme,
  writeLog,
  logError,
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

const useConfigStore = create<ConfigState>((set, get) => ({
  config: null,
  theme: null,
  xtermTheme: null,
  showSettings: false,
  loaded: false,
  fontSizeOffset: 0,

  loadConfig: async () => {
    try {
      const [config, theme] = await Promise.all([getConfig(), getTheme()]);
      const xtermTheme = toXtermTheme(theme);
      applyThemeCssVars(theme);
      set({ config, theme, xtermTheme, loaded: true });
    } catch (err) {
      writeLog("ERROR", `Failed to load config: ${err}`).catch(logError);
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
