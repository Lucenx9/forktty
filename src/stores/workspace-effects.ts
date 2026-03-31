/**
 * Centralized side-effects driven by workspace store changes.
 *
 * Instead of scattering Zustand subscriptions across React components,
 * all state → side-effect reactions live here. Call `startWorkspaceEffects()`
 * once after session hydration; call the returned cleanup function on unmount.
 */
import { useWorkspaceStore } from "./workspace";
import {
  saveSession,
  updateTrayTooltip,
  hasTauriRuntime,
  logError,
} from "../lib/pty-bridge";
import { buildSessionPayload } from "../lib/session-persistence";

const SESSION_SAVE_DEBOUNCE_MS = 2000;

function computeTotalUnread(): number {
  const { workspaces } = useWorkspaceStore.getState();
  return Object.values(workspaces).reduce((sum, ws) => sum + ws.unreadCount, 0);
}

/**
 * Start all workspace-driven side-effects.
 * Returns a cleanup function that tears down every subscription.
 */
export function startWorkspaceEffects(): () => void {
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let lastUnread = computeTotalUnread();

  // Sync document title immediately
  document.title = lastUnread > 0 ? `ForkTTY (${lastUnread})` : "ForkTTY";

  const unsub = useWorkspaceStore.subscribe(() => {
    // --- Session persistence (debounced) ---
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(() => {
      saveSession(buildSessionPayload()).catch(logError);
    }, SESSION_SAVE_DEBOUNCE_MS);

    // --- Document title + tray tooltip ---
    const unread = computeTotalUnread();
    if (unread !== lastUnread) {
      lastUnread = unread;
      document.title = unread > 0 ? `ForkTTY (${unread})` : "ForkTTY";
      if (hasTauriRuntime()) {
        updateTrayTooltip(unread).catch(logError);
      }
    }
  });

  // Flush pending session save when the window is about to close.
  // This is fire-and-forget — the async IPC may not complete, but it
  // catches the common case where a debounced save is still pending.
  function handleBeforeUnload() {
    if (saveTimer) {
      clearTimeout(saveTimer);
      saveTimer = null;
      saveSession(buildSessionPayload()).catch(logError);
    }
  }
  window.addEventListener("beforeunload", handleBeforeUnload);

  return () => {
    unsub();
    window.removeEventListener("beforeunload", handleBeforeUnload);
    if (saveTimer) clearTimeout(saveTimer);
  };
}
