import { useEffect, useRef } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { CanvasAddon } from "@xterm/addon-canvas";
import { spawnPty, writePty, resizePty, killPty } from "../lib/pty-bridge";
import { useWorkspaceStore, updateSurfaceActivity } from "../stores/workspace";
import "@xterm/xterm/css/xterm.css";

interface TerminalPaneProps {
  paneId: string;
  isFocused: boolean;
  cwd: string;
}

export default function TerminalPane({
  paneId,
  isFocused,
  cwd,
}: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const lastActivityCallRef = useRef(0);
  const setFocusedPane = useWorkspaceStore((s) => s.setFocusedPane);
  const registerSurface = useWorkspaceStore((s) => s.registerSurface);
  const unregisterSurface = useWorkspaceStore((s) => s.unregisterSurface);

  // Focus the xterm instance when isFocused changes
  useEffect(() => {
    if (isFocused && termRef.current) {
      termRef.current.focus();
    }
  }, [isFocused]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    const term = new Terminal({
      cursorBlink: true,
      fontSize: 14,
      fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
      theme: {
        background: "#1e1e2e",
        foreground: "#cdd6f4",
        cursor: "#f5e0dc",
        selectionBackground: "#585b70",
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
      },
    });

    termRef.current = term;

    const fitAddon = new FitAddon();
    fitAddonRef.current = fitAddon;
    term.loadAddon(fitAddon);

    term.open(container);

    // Canvas renderer by default (WebGL has known bugs on WebKitGTK)
    term.loadAddon(new CanvasAddon());

    fitAddon.fit();

    // Spawn PTY and wire data flow
    let disposed = false;

    spawnPty(
      (data) => {
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
      () => {
        if (!disposed) {
          term.write("\r\n\x1b[90m[Process exited]\x1b[0m\r\n");
        }
      },
      cwd || undefined,
    )
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
      ref={containerRef}
      onClick={() => setFocusedPane(paneId)}
      style={{
        width: "100%",
        height: "100%",
        overflow: "hidden",
        border: isFocused ? "2px solid #89b4fa" : "2px solid transparent",
        boxSizing: "border-box",
      }}
    />
  );
}
