import { useEffect, useRef, useState, useCallback } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { CanvasAddon } from "@xterm/addon-canvas";
import { SearchAddon } from "@xterm/addon-search";
import FindBar from "./FindBar";
import {
  spawnPty,
  writePty,
  resizePty,
  killPty,
  sendDesktopNotification,
} from "../lib/pty-bridge";
import type { ScanEventData } from "../lib/pty-bridge";
import { useWorkspaceStore, updateSurfaceActivity } from "../stores/workspace";
import { useConfigStore } from "../stores/config";
import "@xterm/xterm/css/xterm.css";

interface TerminalPaneProps {
  paneId: string;
  isFocused: boolean;
  cwd: string;
  workspaceId: string;
}

export default function TerminalPane({
  paneId,
  isFocused,
  cwd,
  workspaceId,
}: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const lastActivityCallRef = useRef(0);
  const [showFind, setShowFind] = useState(false);
  const setFocusedPane = useWorkspaceStore((s) => s.setFocusedPane);
  const registerSurface = useWorkspaceStore((s) => s.registerSurface);
  const unregisterSurface = useWorkspaceStore((s) => s.unregisterSurface);
  const xtermTheme = useConfigStore((s) => s.xtermTheme);
  const configTheme = useConfigStore((s) => s.theme);

  const handleFind = useCallback(
    (term: string, opts: { caseSensitive: boolean }) => {
      searchAddonRef.current?.findNext(term, {
        caseSensitive: opts.caseSensitive,
      });
    },
    [],
  );

  const handleFindNext = useCallback(() => {
    searchAddonRef.current?.findNext("");
  }, []);

  const handleFindPrevious = useCallback(() => {
    searchAddonRef.current?.findPrevious("");
  }, []);

  const handleFindClose = useCallback(() => {
    searchAddonRef.current?.clearDecorations();
    setShowFind(false);
    termRef.current?.focus();
  }, []);

  // Focus the xterm instance when isFocused changes
  useEffect(() => {
    if (isFocused && termRef.current && !showFind) {
      termRef.current.focus();
    }
  }, [isFocused, showFind]);

  // Update terminal theme when config changes
  useEffect(() => {
    if (termRef.current && xtermTheme) {
      termRef.current.options.theme = xtermTheme;
    }
    if (termRef.current && configTheme) {
      const fontFamily = configTheme.font_family ?? "JetBrains Mono";
      const fontSize = configTheme.font_size ?? 14;
      termRef.current.options.fontFamily = `'${fontFamily}', 'Fira Code', 'Cascadia Code', monospace`;
      termRef.current.options.fontSize = fontSize;
      fitAddonRef.current?.fit();
    }
  }, [xtermTheme, configTheme]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const cfgStore = useConfigStore.getState();
    const fontFamily = cfgStore.theme?.font_family ?? "JetBrains Mono";
    const fontSize = cfgStore.theme?.font_size ?? 14;

    const term = new Terminal({
      cursorBlink: true,
      fontSize,
      fontFamily: `'${fontFamily}', 'Fira Code', 'Cascadia Code', monospace`,
      theme: cfgStore.xtermTheme ?? undefined,
    });

    termRef.current = term;

    const fitAddon = new FitAddon();
    fitAddonRef.current = fitAddon;
    term.loadAddon(fitAddon);

    term.open(container);

    // Canvas renderer by default (WebGL has known bugs on WebKitGTK)
    term.loadAddon(new CanvasAddon());

    // Search addon for Ctrl+F
    const searchAddon = new SearchAddon();
    searchAddonRef.current = searchAddon;
    term.loadAddon(searchAddon);

    fitAddon.fit();

    // Spawn PTY and wire data flow
    let disposed = false;

    // Debounce notifications: at most one per 5 seconds per pane
    let lastNotifyTime = 0;

    function handleScanEvent(event: ScanEventData) {
      if (disposed) return;
      if (event.event_type !== "prompt_detected") return;

      // Check if workspace containing this pane is unfocused
      const state = useWorkspaceStore.getState();
      const wsId = Object.entries(state.workspaces).find(([, ws]) =>
        Object.prototype.hasOwnProperty.call(ws.surfaces, paneId),
      )?.[0];
      if (!wsId || wsId === state.activeWorkspaceId) return;

      // Debounce
      const now = Date.now();
      if (now - lastNotifyTime < 5000) return;
      lastNotifyTime = now;

      const ws = state.workspaces[wsId];
      const title = "Prompt waiting";
      const body = `${ws?.name ?? "Workspace"} needs attention`;

      state.addNotification(wsId, title, body);
      sendDesktopNotification("ForkTTY", body).catch(console.error);
    }

    spawnPty({
      onOutput: (data) => {
        if (!disposed) {
          term.write(data);

          // Throttled activity tracking (at most once per second)
          const now = Date.now();
          if (now - lastActivityCallRef.current > 1000) {
            lastActivityCallRef.current = now;
            updateSurfaceActivity(paneId);
          }
        }
      },
      onExit: () => {
        if (!disposed) {
          term.write("\r\n\x1b[90m[Process exited]\x1b[0m\r\n");
        }
      },
      cwd: cwd || undefined,
      workspaceId,
      surfaceId: paneId,
      onScanEvent: handleScanEvent,
    })
      .then((id) => {
        if (disposed) {
          // React StrictMode double-mount: kill the orphaned PTY
          killPty(id).catch(console.error);
          return;
        }
        ptyIdRef.current = id;
        registerSurface(paneId, id);

        // Send initial resize based on actual terminal dimensions
        const { cols, rows } = term;
        resizePty(id, cols, rows).catch(console.error);
      })
      .catch((err) => {
        console.error("Failed to spawn PTY:", err);
        term.write(`\r\n\x1b[31mFailed to spawn PTY: ${err}\x1b[0m\r\n`);
      });

    // Wire keyboard input to PTY
    const dataDisposable = term.onData((data) => {
      const id = ptyIdRef.current;
      if (id !== null) {
        writePty(id, data).catch(console.error);
      }
    });

    // Debounced resize handling via ResizeObserver
    let resizeRaf: number | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeRaf !== null) return;
      resizeRaf = requestAnimationFrame(() => {
        resizeRaf = null;
        if (fitAddonRef.current) {
          fitAddonRef.current.fit();
        }
      });
    });
    resizeObserver.observe(container);

    const resizeDisposable = term.onResize(({ cols, rows }) => {
      const id = ptyIdRef.current;
      if (id !== null) {
        resizePty(id, cols, rows).catch(console.error);
      }
    });

    return () => {
      disposed = true;
      if (resizeRaf !== null) {
        cancelAnimationFrame(resizeRaf);
      }
      resizeObserver.disconnect();
      dataDisposable.dispose();
      resizeDisposable.dispose();
      term.dispose();

      // Kill PTY process on cleanup
      const id = ptyIdRef.current;
      if (id !== null) {
        killPty(id).catch(console.error);
        ptyIdRef.current = null;
      }
      unregisterSurface(paneId);
    };
    // paneId is stable for the lifetime of this component
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div
      style={{
        width: "100%",
        height: "100%",
        position: "relative",
        overflow: "hidden",
        border: isFocused
          ? "2px solid var(--theme-blue, #89b4fa)"
          : "2px solid transparent",
        boxSizing: "border-box",
      }}
      onKeyDown={(e) => {
        // Ctrl+F: find in terminal
        if (e.ctrlKey && !e.shiftKey && e.key === "f") {
          e.preventDefault();
          e.stopPropagation();
          setShowFind(true);
          return;
        }
        // Ctrl+Shift+C: copy selection
        if (e.ctrlKey && e.shiftKey && e.key === "C") {
          e.preventDefault();
          e.stopPropagation();
          const sel = termRef.current?.getSelection();
          if (sel) {
            navigator.clipboard.writeText(sel).catch(console.error);
          }
        }
      }}
    >
      {showFind && (
        <FindBar
          onFind={handleFind}
          onFindNext={handleFindNext}
          onFindPrevious={handleFindPrevious}
          onClose={handleFindClose}
        />
      )}
      <div
        ref={containerRef}
        onClick={() => setFocusedPane(paneId)}
        style={{
          width: "100%",
          height: "100%",
        }}
      />
    </div>
  );
}
