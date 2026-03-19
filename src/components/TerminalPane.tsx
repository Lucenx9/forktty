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
  sendCustomNotification,
} from "../lib/pty-bridge";
import { registerTerminal, unregisterTerminal } from "../lib/terminal-registry";
import type { ScanEventData } from "../lib/pty-bridge";
import {
  useWorkspaceStore,
  updateSurfaceActivity,
  getLastWorkspaceSwitchTime,
} from "../stores/workspace";
import { useConfigStore } from "../stores/config";
import "@xterm/xterm/css/xterm.css";

interface TerminalPaneProps {
  paneId: string;
  isFocused: boolean;
  cwd: string;
  workspaceId: string;
}

interface PaneContextMenuState {
  x: number;
  y: number;
}

function PaneContextMenu({
  menu,
  paneId,
  onClose,
  termRef,
}: {
  menu: PaneContextMenuState;
  paneId: string;
  onClose: () => void;
  termRef: React.RefObject<Terminal | null>;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const splitPane = useWorkspaceStore((s) => s.splitPane);

  useEffect(() => {
    function handleClick(e: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") onClose();
    }
    document.addEventListener("mousedown", handleClick);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("mousedown", handleClick);
      document.removeEventListener("keydown", handleKey);
    };
  }, [onClose]);

  // Clamp position to viewport
  useEffect(() => {
    if (!menuRef.current) return;
    const rect = menuRef.current.getBoundingClientRect();
    const el = menuRef.current;
    if (rect.right > window.innerWidth) {
      el.style.left = `${window.innerWidth - rect.width - 4}px`;
    }
    if (rect.bottom > window.innerHeight) {
      el.style.top = `${window.innerHeight - rect.height - 4}px`;
    }
  }, []);

  function handlePaste() {
    navigator.clipboard
      .readText()
      .then((text) => {
        if (text && termRef.current) {
          // Paste via the terminal's input handler which routes to pty_write
          termRef.current.paste(text);
        }
      })
      .catch(console.error);
    onClose();
  }

  function handleCopy() {
    const sel = termRef.current?.getSelection();
    if (sel) {
      navigator.clipboard.writeText(sel).catch(console.error);
    }
    onClose();
  }

  const hasSelection = !!termRef.current?.getSelection();

  return (
    <div
      ref={menuRef}
      className="context-menu"
      style={{ left: menu.x, top: menu.y }}
    >
      <button
        className={`context-menu-item ${!hasSelection ? "context-menu-item-disabled" : ""}`}
        onClick={handleCopy}
        disabled={!hasSelection}
      >
        <span>Copy</span>
        <span className="context-menu-shortcut">Ctrl+Shift+C</span>
      </button>
      <button className="context-menu-item" onClick={handlePaste}>
        <span>Paste</span>
      </button>
      <div className="context-menu-separator" />
      <button
        className="context-menu-item"
        onClick={() => {
          splitPane(paneId, "horizontal");
          onClose();
        }}
      >
        <span>Split Right</span>
        <span className="context-menu-shortcut">Ctrl+D</span>
      </button>
      <button
        className="context-menu-item"
        onClick={() => {
          splitPane(paneId, "vertical");
          onClose();
        }}
      >
        <span>Split Down</span>
        <span className="context-menu-shortcut">Ctrl+Shift+D</span>
      </button>
    </div>
  );
}

function PaneToolbar({
  paneId,
  isFocused,
}: {
  paneId: string;
  isFocused: boolean;
}) {
  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const closePane = useWorkspaceStore((s) => s.closePane);
  const surfaceTitle = useWorkspaceStore((s) => {
    for (const ws of Object.values(s.workspaces)) {
      const surface = ws.surfaces[paneId];
      if (surface) return surface.title || "Terminal";
    }
    return "Terminal";
  });

  return (
    <div
      className={`pane-toolbar ${isFocused ? "pane-toolbar-focused" : ""}`}
      onMouseDown={(e) => e.preventDefault()}
    >
      <span className="pane-toolbar-title">{surfaceTitle}</span>
      <div className="pane-toolbar-actions">
        <button
          className="pane-toolbar-btn"
          onClick={() => splitPane(paneId, "horizontal")}
          title="Split Right (Ctrl+D)"
        >
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect
              x="0.5"
              y="0.5"
              width="11"
              height="11"
              rx="1"
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
            />
            <line
              x1="6"
              y1="1"
              x2="6"
              y2="11"
              stroke="currentColor"
              strokeWidth="1"
            />
          </svg>
        </button>
        <button
          className="pane-toolbar-btn"
          onClick={() => splitPane(paneId, "vertical")}
          title="Split Down (Ctrl+Shift+D)"
        >
          <svg width="12" height="12" viewBox="0 0 12 12">
            <rect
              x="0.5"
              y="0.5"
              width="11"
              height="11"
              rx="1"
              fill="none"
              stroke="currentColor"
              strokeWidth="1"
            />
            <line
              x1="1"
              y1="6"
              x2="11"
              y2="6"
              stroke="currentColor"
              strokeWidth="1"
            />
          </svg>
        </button>
        <button
          className="pane-toolbar-btn pane-toolbar-btn-close"
          onClick={() => closePane(paneId)}
          title="Close Pane (Ctrl+W)"
        >
          <svg width="10" height="10" viewBox="0 0 10 10">
            <line
              x1="1"
              y1="1"
              x2="9"
              y2="9"
              stroke="currentColor"
              strokeWidth="1.5"
            />
            <line
              x1="9"
              y1="1"
              x2="1"
              y2="9"
              stroke="currentColor"
              strokeWidth="1.5"
            />
          </svg>
        </button>
      </div>
    </div>
  );
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
  const [flashBorder, setFlashBorder] = useState(false);
  const [paneContextMenu, setPaneContextMenu] =
    useState<PaneContextMenuState | null>(null);
  const setFocusedPane = useWorkspaceStore((s) => s.setFocusedPane);
  const registerSurface = useWorkspaceStore((s) => s.registerSurface);
  const unregisterSurface = useWorkspaceStore((s) => s.unregisterSurface);
  const hasUnreadNotification = useWorkspaceStore((s) => {
    for (const ws of Object.values(s.workspaces)) {
      const surface = ws.surfaces[paneId];
      if (surface) return surface.hasUnreadNotification;
    }
    return false;
  });
  const xtermTheme = useConfigStore((s) => s.xtermTheme);
  const configTheme = useConfigStore((s) => s.theme);

  const lastFindTermRef = useRef("");

  const handleFind = useCallback(
    (term: string, opts: { caseSensitive: boolean }) => {
      lastFindTermRef.current = term;
      searchAddonRef.current?.findNext(term, {
        caseSensitive: opts.caseSensitive,
      });
    },
    [],
  );

  const handleFindNext = useCallback(() => {
    if (lastFindTermRef.current) {
      searchAddonRef.current?.findNext(lastFindTermRef.current);
    }
  }, []);

  const handleFindPrevious = useCallback(() => {
    if (lastFindTermRef.current) {
      searchAddonRef.current?.findPrevious(lastFindTermRef.current);
    }
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

  // Flash border on focus
  useEffect(() => {
    if (isFocused) {
      setFlashBorder(true);
      const timer = setTimeout(() => setFlashBorder(false), 500);
      return () => clearTimeout(timer);
    }
  }, [isFocused]);

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
    registerTerminal(paneId, term);

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

    function fireNotification(wsId: string, title: string, body: string) {
      const state = useWorkspaceStore.getState();
      const config = useConfigStore.getState().config;
      const notificationCommand =
        config?.general.notification_command.trim() ?? "";

      state.addNotification(wsId, title, body);
      state.setSurfaceUnread(paneId, true);
      if (config?.notifications.desktop ?? true) {
        sendDesktopNotification(title, body).catch(console.error);
      }
      if (notificationCommand) {
        sendCustomNotification(notificationCommand, title, body).catch(
          console.error,
        );
      }
    }

    function handleScanEvent(event: ScanEventData) {
      if (disposed) return;

      // Handle OSC 9/99/777 notification events
      if (event.event_type === "notification") {
        const state = useWorkspaceStore.getState();
        const wsId = Object.entries(state.workspaces).find(([, ws]) =>
          Object.prototype.hasOwnProperty.call(ws.surfaces, paneId),
        )?.[0];
        if (!wsId) return;

        // Debounce
        const now = Date.now();
        if (now - lastNotifyTime < 5000) return;
        lastNotifyTime = now;

        fireNotification(wsId, event.title, event.body);
        return;
      }

      if (event.event_type !== "prompt_detected") return;

      // Check if workspace containing this pane is unfocused
      const state = useWorkspaceStore.getState();
      const wsId = Object.entries(state.workspaces).find(([, ws]) =>
        Object.prototype.hasOwnProperty.call(ws.surfaces, paneId),
      )?.[0];
      if (!wsId || wsId === state.activeWorkspaceId) return;

      // Suppress spurious prompt_detected events caused by terminal resize
      // during workspace switch (shell redraws prompt → OSC 133 → false positive)
      const now = Date.now();
      if (now - getLastWorkspaceSwitchTime() < 2000) return;

      // Debounce
      if (now - lastNotifyTime < 5000) return;
      lastNotifyTime = now;

      const ws = state.workspaces[wsId];
      const title = "Prompt waiting";
      const body = `${ws?.name ?? "Workspace"} needs attention`;
      fireNotification(wsId, title, body);
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
    // Skip when container is hidden (display:none → 0 dimensions) to avoid
    // spurious PTY resizes that cause the shell to redraw its prompt.
    let resizeRaf: number | null = null;
    const resizeObserver = new ResizeObserver(() => {
      if (resizeRaf !== null) return;
      resizeRaf = requestAnimationFrame(() => {
        resizeRaf = null;
        if (!container.clientWidth || !container.clientHeight) return;
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
      unregisterTerminal(paneId);
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

  const paneClasses = [
    "terminal-pane",
    isFocused ? "terminal-pane-focused" : "",
    !isFocused && hasUnreadNotification ? "pane-notification-ring" : "",
    flashBorder ? "pane-focus-flash" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={paneClasses}
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
      <PaneToolbar paneId={paneId} isFocused={isFocused} />
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
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setFocusedPane(paneId);
          setPaneContextMenu({ x: e.clientX, y: e.clientY });
        }}
        style={{
          width: "100%",
          height: "calc(100% - 28px)",
        }}
      />
      {paneContextMenu && (
        <PaneContextMenu
          menu={paneContextMenu}
          paneId={paneId}
          onClose={() => setPaneContextMenu(null)}
          termRef={termRef}
        />
      )}
    </div>
  );
}
