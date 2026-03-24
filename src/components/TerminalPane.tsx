import { useEffect, useRef, useState, useCallback, memo } from "react";
import { createPortal } from "react-dom";
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
  logError,
} from "../lib/pty-bridge";
import {
  registerTerminal,
  unregisterTerminal,
  getSavedInstance,
  saveInstance,
  removeSavedInstance,
} from "../lib/terminal-registry";
import type { ScanEventData } from "../lib/pty-bridge";
import {
  useWorkspaceStore,
  updateSurfaceActivity,
  getLastWorkspaceSwitchTime,
} from "../stores/workspace";
import { useConfigStore } from "../stores/config";
import {
  resolveWorkspaceSpawnCwd,
  splitPaneWithInheritedCwd,
} from "../lib/workspace-launch";
import { Columns2, Rows2, Search, GripVertical, X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";

interface TerminalPaneProps {
  paneId: string;
  isFocused: boolean;
  cwd: string;
  workspaceId: string;
}

interface TerminalActionEventDetail {
  action: "copy" | "find";
  paneId: string;
}

// Custom pane drag state (HTML5 DnD crashes WebKitGTK on Wayland)
const paneDragState = { sourceId: null as string | null };
window.addEventListener("mouseup", () => {
  paneDragState.sourceId = null;
  document.body.classList.remove("pane-dragging");
});

const NOTIFICATION_DEDUPE_MS = 15000;
const recentNotificationMap = new Map<string, number>();

function pruneNotificationMap() {
  const now = Date.now();
  for (const [key, time] of recentNotificationMap) {
    if (now - time >= NOTIFICATION_DEDUPE_MS) {
      recentNotificationMap.delete(key);
    }
  }
}

function concatUint8Arrays(chunks: Uint8Array[]): Uint8Array {
  if (chunks.length === 1) {
    return chunks[0]!;
  }

  let totalLength = 0;
  for (const chunk of chunks) {
    totalLength += chunk.length;
  }

  const merged = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return merged;
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

  // Clamp position to viewport (wait for paint so getBoundingClientRect is valid)
  useEffect(() => {
    const rafId = requestAnimationFrame(() => {
      if (!menuRef.current) return;
      const rect = menuRef.current.getBoundingClientRect();
      const el = menuRef.current;
      if (rect.right > window.innerWidth) {
        el.style.left = `${window.innerWidth - rect.width - 4}px`;
      }
      if (rect.bottom > window.innerHeight) {
        el.style.top = `${window.innerHeight - rect.height - 4}px`;
      }
    });
    return () => cancelAnimationFrame(rafId);
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
      .catch(logError);
    onClose();
  }

  function handleCopy() {
    const sel = termRef.current?.getSelection();
    if (sel) {
      navigator.clipboard.writeText(sel).catch(logError);
    }
    onClose();
  }

  const hasSelection = !!termRef.current?.getSelection();

  return (
    <div ref={menuRef} className="context-menu" style={{ left: menu.x, top: menu.y }}>
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
          splitPaneWithInheritedCwd(paneId, "horizontal").catch(logError);
          onClose();
        }}
      >
        <span>Split Right</span>
        <span className="context-menu-shortcut">Ctrl+D</span>
      </button>
      <button
        className="context-menu-item"
        onClick={() => {
          splitPaneWithInheritedCwd(paneId, "vertical").catch(logError);
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
  hasUnreadNotification,
  isFindOpen,
  onToggleFind,
}: {
  paneId: string;
  isFocused: boolean;
  hasUnreadNotification: boolean;
  isFindOpen: boolean;
  onToggleFind: () => void;
}) {
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
      title="Drag to swap panes"
      onMouseDown={(e) => {
        // Buttons handle their own clicks; only start drag from the toolbar itself
        if ((e.target as HTMLElement).closest("button")) {
          e.preventDefault();
          return;
        }
        // Custom drag (HTML5 DnD crashes WebKitGTK on Wayland)
        e.preventDefault();
        paneDragState.sourceId = paneId;
        document.body.classList.add("pane-dragging");
      }}
    >
      <div className="pane-toolbar-leading">
        <span className="pane-toolbar-grip" aria-hidden="true">
          <GripVertical size={12} />
        </span>
        <span className="pane-toolbar-title">{surfaceTitle}</span>
        {hasUnreadNotification && (
          <span className="pane-toolbar-badge pane-toolbar-badge-alert">
            Needs input
          </span>
        )}
      </div>
      <div className="pane-toolbar-actions">
        <button
          className="pane-toolbar-btn"
          type="button"
          onClick={() =>
            splitPaneWithInheritedCwd(paneId, "horizontal").catch(logError)
          }
          title="Split Right (Ctrl+D)"
          aria-label="Split Right"
        >
          <Columns2 size={12} />
        </button>
        <button
          className="pane-toolbar-btn"
          type="button"
          onClick={() => splitPaneWithInheritedCwd(paneId, "vertical").catch(logError)}
          title="Split Down (Ctrl+Shift+D)"
          aria-label="Split Down"
        >
          <Rows2 size={12} />
        </button>
        <button
          className={`pane-toolbar-btn ${isFindOpen ? "pane-toolbar-btn-active" : ""}`}
          type="button"
          onClick={onToggleFind}
          title="Find (Ctrl+F)"
          aria-label="Find in Terminal"
          aria-pressed={isFindOpen}
        >
          <Search size={12} />
        </button>
        <button
          className="pane-toolbar-btn pane-toolbar-btn-close"
          type="button"
          onClick={() => closePane(paneId)}
          title="Close Pane (Ctrl+W)"
          aria-label="Close Pane"
        >
          <X size={12} />
        </button>
      </div>
    </div>
  );
}

const TerminalPane = memo(function TerminalPane({
  paneId,
  isFocused,
  cwd,
  workspaceId,
}: TerminalPaneProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const canvasAddonRef = useRef<CanvasAddon | null>(null);
  const searchAddonRef = useRef<SearchAddon | null>(null);
  const lastActivityCallRef = useRef(0);
  const [showFind, setShowFind] = useState(false);
  const [flashBorder, setFlashBorder] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [paneContextMenu, setPaneContextMenu] = useState<PaneContextMenuState | null>(
    null,
  );
  const setFocusedPane = useWorkspaceStore((s) => s.setFocusedPane);
  const swapPanes = useWorkspaceStore((s) => s.swapPanes);
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
  const fontSizeOffset = useConfigStore((s) => s.fontSizeOffset);

  const lastFindTermRef = useRef("");

  const handleFind = useCallback((term: string, opts: { caseSensitive: boolean }) => {
    lastFindTermRef.current = term;
    searchAddonRef.current?.findNext(term, {
      caseSensitive: opts.caseSensitive,
    });
  }, []);

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

  const handleCopySelection = useCallback(() => {
    const sel = termRef.current?.getSelection();
    if (sel) {
      navigator.clipboard.writeText(sel).catch(logError);
    }
  }, []);

  useEffect(() => {
    function handleTerminalAction(event: Event) {
      const detail = (event as CustomEvent<TerminalActionEventDetail>).detail;
      if (!detail || detail.paneId !== paneId) return;

      if (detail.action === "find") {
        setShowFind(true);
        return;
      }

      handleCopySelection();
    }

    window.addEventListener(
      "forktty-terminal-action",
      handleTerminalAction as EventListener,
    );
    return () =>
      window.removeEventListener(
        "forktty-terminal-action",
        handleTerminalAction as EventListener,
      );
  }, [handleCopySelection, paneId]);

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
      const fontFamily = configTheme.font_family ?? "monospace";
      const fontSize = (configTheme.font_size ?? 14) + fontSizeOffset;
      termRef.current.options.fontFamily = `'${fontFamily}', 'Symbols Nerd Font Mono', monospace`;
      termRef.current.options.fontSize = fontSize;
      // Only fit if container is visible (non-zero dimensions)
      const el = containerRef.current;
      if (el && el.clientWidth > 0 && el.clientHeight > 0) {
        fitAddonRef.current?.fit();
      }
    }
  }, [xtermTheme, configTheme, fontSizeOffset]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    // Check if a saved instance exists (re-adoption after swap/split)
    const saved = getSavedInstance(paneId);

    let term: Terminal;
    let wrapper: HTMLDivElement;
    let isNewInstance: boolean;
    const runtime = saved?.runtime ?? {
      ptyId: null,
      lastCols: null,
      lastRows: null,
    };
    let outputDrainRaf: number | null = null;
    let outputWriteInFlight = false;
    const outputQueue: Uint8Array[] = [];

    function drainOutputQueue() {
      if (disposed || outputWriteInFlight || outputQueue.length === 0) return;

      outputWriteInFlight = true;
      const payload = concatUint8Arrays(outputQueue.splice(0, outputQueue.length));
      term.write(payload, () => {
        outputWriteInFlight = false;
        if (disposed) return;
        if (outputQueue.length > 0) {
          outputDrainRaf = requestAnimationFrame(() => {
            outputDrainRaf = null;
            drainOutputQueue();
          });
        }
      });
    }

    function scheduleOutputDrain() {
      if (disposed || outputDrainRaf !== null) return;
      outputDrainRaf = requestAnimationFrame(() => {
        outputDrainRaf = null;
        drainOutputQueue();
      });
    }

    if (saved) {
      // Re-adopt: reuse existing terminal, wrapper, PTY — no spawn needed
      term = saved.terminal;
      wrapper = saved.wrapper;
      termRef.current = term;
      fitAddonRef.current = saved.fitAddon;
      canvasAddonRef.current = saved.canvasAddon;
      searchAddonRef.current = saved.searchAddon;
      container.appendChild(wrapper);
      registerTerminal(paneId, term);
      removeSavedInstance(paneId);
      isNewInstance = false;

      // Re-fit after reattaching to new container
      requestAnimationFrame(() => {
        if (container.clientWidth > 0 && container.clientHeight > 0) {
          saved.fitAddon.fit();
        }
      });
    } else {
      // Fresh terminal: create everything from scratch
      const cfgStore = useConfigStore.getState();
      const fontFamily = cfgStore.theme?.font_family ?? "monospace";
      const fontSize = (cfgStore.theme?.font_size ?? 14) + cfgStore.fontSizeOffset;

      term = new Terminal({
        cursorBlink: true,
        fontSize,
        fontFamily: `'${fontFamily}', 'Symbols Nerd Font Mono', monospace`,
        theme: cfgStore.xtermTheme ?? undefined,
      });

      termRef.current = term;

      const fitAddon = new FitAddon();
      fitAddonRef.current = fitAddon;
      term.loadAddon(fitAddon);

      // Use an intermediary wrapper so xterm DOM survives React unmount
      wrapper = document.createElement("div");
      wrapper.style.width = "100%";
      wrapper.style.height = "100%";
      container.appendChild(wrapper);
      term.open(wrapper);
      registerTerminal(paneId, term);

      // Let Ctrl+F and Ctrl+Shift+C bubble up to React (find bar, copy)
      term.attachCustomKeyEventHandler((e) => {
        if (e.ctrlKey && !e.shiftKey && e.key === "f") return false;
        if (e.ctrlKey && e.shiftKey && e.key === "C") return false;
        return true;
      });

      // Canvas renderer by default (WebGL has known bugs on WebKitGTK)
      try {
        const canvasAddon = new CanvasAddon();
        canvasAddonRef.current = canvasAddon;
        term.loadAddon(canvasAddon);
      } catch (err) {
        canvasAddonRef.current = null;
        logError(`Canvas renderer unavailable, falling back to DOM: ${err}`);
      }

      // Search addon for Ctrl+F
      const searchAddon = new SearchAddon();
      searchAddonRef.current = searchAddon;
      term.loadAddon(searchAddon);

      // Guard against zero-dimension container (not yet laid out)
      if (container.clientWidth > 0 && container.clientHeight > 0) {
        fitAddon.fit();
      }

      isNewInstance = true;
    }

    // Spawn PTY only for new instances (re-adopted instances keep their PTY)
    let disposed = false;
    let hasExited = false;
    let fontReadyCancelled = false;

    if ("fonts" in document) {
      document.fonts.ready
        .then(() => {
          if (disposed || fontReadyCancelled) return;
          if (container.clientWidth > 0 && container.clientHeight > 0) {
            fitAddonRef.current?.fit();
          }
        })
        .catch(logError);
    }

    if (isNewInstance) {
      // Debounce notifications: at most one per 5 seconds per pane
      let lastNotifyTime = 0;

      function fireNotification(wsId: string, title: string, body: string) {
        const state = useWorkspaceStore.getState();
        const config = useConfigStore.getState().config;
        const workspace = state.workspaces[wsId];
        const notificationCommand = config?.general.notification_command.trim() ?? "";
        const dedupeKey = `${wsId}:${title}:${body}`;
        const now = Date.now();
        const lastSeen = recentNotificationMap.get(dedupeKey) ?? 0;

        if (now - lastSeen < NOTIFICATION_DEDUPE_MS) {
          return;
        }
        recentNotificationMap.set(dedupeKey, now);
        pruneNotificationMap();

        state.addNotification(wsId, title, body);
        if (wsId !== state.activeWorkspaceId || workspace?.focusedPaneId !== paneId) {
          state.setSurfaceUnread(paneId, true);
        }
        if (config?.notifications.desktop ?? true) {
          sendDesktopNotification(title, body).catch(logError);
        }
        if (notificationCommand) {
          sendCustomNotification(notificationCommand, title, body).catch(logError);
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
        if (now - getLastWorkspaceSwitchTime() < 4000) return;

        // Debounce
        if (now - lastNotifyTime < 5000) return;
        const ws = state.workspaces[wsId];
        if (!ws) return;
        if (ws.unreadCount > 0 || ws.surfaces[paneId]?.hasUnreadNotification) {
          return;
        }

        lastNotifyTime = now;
        const title = "Prompt waiting";
        const body = `${ws.name} needs attention`;
        fireNotification(wsId, title, body);
      }

      resolveWorkspaceSpawnCwd(workspaceId, cwd)
        .then((spawnCwd) => {
          if (disposed) return null;

          return spawnPty({
            onOutput: (data) => {
              if (!disposed) {
                outputQueue.push(data as Uint8Array);
                scheduleOutputDrain();

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
                hasExited = true;
                runtime.ptyId = null;
                unregisterSurface(paneId);
                term.write("\r\n\x1b[90m[Process exited]\x1b[0m\r\n");
              }
            },
            cwd: spawnCwd || undefined,
            workspaceId,
            surfaceId: paneId,
            cols:
              container.clientWidth > 0
                ? Math.max(term.cols, 2)
                : (runtime.lastCols ?? 120),
            rows:
              container.clientHeight > 0
                ? Math.max(term.rows, 2)
                : (runtime.lastRows ?? 30),
            onScanEvent: handleScanEvent,
          });
        })
        .then((id) => {
          if (id === null) return;
          if (disposed) {
            // Pane was closed before spawn resolved; kill the orphaned PTY.
            killPty(id).catch(logError);
            return;
          }
          if (hasExited) {
            return;
          }
          runtime.ptyId = id;
          registerSurface(paneId, id);

          // Send initial resize based on actual terminal dimensions
          const { cols, rows } = term;
          resizePty(id, cols, rows).catch(logError);
        })
        .catch((err) => {
          logError(err);
          term.write(`\r\n\x1b[31mFailed to spawn PTY: ${err}\x1b[0m\r\n`);
        });
    }

    // Wire keyboard input to PTY
    const dataDisposable = term.onData((data) => {
      const id = runtime.ptyId;
      if (id !== null) {
        writePty(id, data).catch(logError);
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

    // Debounce PTY resize IPC to reduce SIGWINCH storms during panel drag
    let resizeTimeout: number | null = null;
    const resizeDisposable = term.onResize(({ cols, rows }) => {
      runtime.lastCols = cols;
      runtime.lastRows = rows;
      if (resizeTimeout !== null) clearTimeout(resizeTimeout);
      resizeTimeout = window.setTimeout(() => {
        resizeTimeout = null;
        const id = runtime.ptyId;
        if (id !== null) {
          resizePty(id, cols, rows).catch(logError);
        }
      }, 150);
    });

    return () => {
      fontReadyCancelled = true;
      if (resizeRaf !== null) {
        cancelAnimationFrame(resizeRaf);
      }
      if (resizeTimeout !== null) {
        clearTimeout(resizeTimeout);
      }
      resizeObserver.disconnect();
      dataDisposable.dispose();
      resizeDisposable.dispose();
      unregisterTerminal(paneId);

      // Check if surface still exists (swap/split) vs removed (close)
      const state = useWorkspaceStore.getState();
      const surfaceStillExists = Object.values(state.workspaces).some(
        (ws) => ws.surfaces[paneId],
      );

      if (surfaceStillExists) {
        // SWAP/SPLIT: preserve terminal instance for re-adoption
        wrapper.remove(); // detach from React container, keep in memory
        saveInstance(paneId, {
          terminal: term,
          wrapper,
          runtime,
          fitAddon: fitAddonRef.current!,
          canvasAddon: canvasAddonRef.current,
          searchAddon: searchAddonRef.current!,
        });
      } else {
        // CLOSE: destroy everything
        disposed = true;
        outputQueue.length = 0;
        if (outputDrainRaf !== null) {
          cancelAnimationFrame(outputDrainRaf);
        }
        canvasAddonRef.current?.dispose();
        canvasAddonRef.current = null;
        term.dispose();

        const id = runtime.ptyId;
        runtime.ptyId = null;
        if (id !== null) {
          killPty(id).catch(logError);
        }
        unregisterSurface(paneId);
      }
    };
    // paneId is stable for the lifetime of this component
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const paneClasses = [
    "terminal-pane",
    isFocused ? "terminal-pane-focused" : "",
    !isFocused ? "terminal-pane-inactive" : "",
    !isFocused && hasUnreadNotification ? "pane-notification-ring" : "",
    flashBorder ? "pane-focus-flash" : "",
    dragOver ? "pane-drag-over" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={paneClasses}
      onMouseEnter={() => {
        if (paneDragState.sourceId && paneDragState.sourceId !== paneId) {
          setDragOver(true);
        }
      }}
      onMouseLeave={() => setDragOver(false)}
      onMouseUp={() => {
        if (paneDragState.sourceId && paneDragState.sourceId !== paneId) {
          swapPanes(paneDragState.sourceId, paneId);
        }
        setDragOver(false);
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
          handleCopySelection();
          return;
        }
        // Prevent Tab from navigating focus away from the terminal
        // (WebKitGTK webview captures Tab for focus traversal by default)
        if (e.key === "Tab" && !e.ctrlKey && !e.altKey && !e.metaKey) {
          e.preventDefault();
        }
      }}
    >
      <PaneToolbar
        paneId={paneId}
        isFocused={isFocused}
        hasUnreadNotification={hasUnreadNotification}
        isFindOpen={showFind}
        onToggleFind={() => setShowFind((v) => !v)}
      />
      {showFind && (
        <FindBar
          onFind={handleFind}
          onFindNext={handleFindNext}
          onFindPrevious={handleFindPrevious}
          onClose={handleFindClose}
        />
      )}
      <div
        className="terminal-pane-shell"
        onClick={() => setFocusedPane(paneId)}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          setFocusedPane(paneId);
          setPaneContextMenu({ x: e.clientX, y: e.clientY });
        }}
        style={{
          width: "100%",
          flex: 1,
          minHeight: 0,
        }}
      >
        <div
          ref={containerRef}
          className="terminal-pane-surface"
          style={{
            width: "100%",
            height: "100%",
          }}
        />
      </div>
      {paneContextMenu &&
        createPortal(
          <PaneContextMenu
            menu={paneContextMenu}
            paneId={paneId}
            onClose={() => setPaneContextMenu(null)}
            termRef={termRef}
          />,
          document.body,
        )}
    </div>
  );
});

export default TerminalPane;
