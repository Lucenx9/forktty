import { CircleAlert, Info, TriangleAlert, X } from "lucide-react";
import { useEffect, useState } from "react";

interface Toast {
  id: string;
  message: string;
  level: "error" | "warn" | "info";
}

const toasts: Toast[] = [];
const toastTimers = new Map<string, number>();
let listeners: Array<() => void> = [];
const TOAST_DURATION_MS = 5000;
const MAX_VISIBLE_TOASTS = 3;

function notify() {
  for (const fn of listeners) fn();
}

function titleForLevel(level: Toast["level"]): string {
  if (level === "error") return "Error";
  if (level === "warn") return "Warning";
  return "Info";
}

function removeToast(id: string): void {
  const timer = toastTimers.get(id);
  if (timer !== undefined) {
    window.clearTimeout(timer);
    toastTimers.delete(id);
  }

  const index = toasts.findIndex((toast) => toast.id === id);
  if (index !== -1) {
    toasts.splice(index, 1);
    notify();
  }
}

export function showToast(message: string, level: "error" | "warn" | "info" = "error") {
  const normalizedMessage = message.trim();
  if (!normalizedMessage) return;

  const duplicate = toasts.find(
    (toast) => toast.level === level && toast.message === normalizedMessage,
  );
  if (duplicate) {
    removeToast(duplicate.id);
  }

  const toast: Toast = {
    id: crypto.randomUUID(),
    message: normalizedMessage,
    level,
  };

  toasts.unshift(toast);
  if (toasts.length > MAX_VISIBLE_TOASTS) {
    for (const extraToast of toasts.slice(MAX_VISIBLE_TOASTS)) {
      const timer = toastTimers.get(extraToast.id);
      if (timer !== undefined) {
        window.clearTimeout(timer);
        toastTimers.delete(extraToast.id);
      }
    }
    toasts.splice(MAX_VISIBLE_TOASTS);
  }

  const timer = window.setTimeout(() => removeToast(toast.id), TOAST_DURATION_MS);
  toastTimers.set(toast.id, timer);
  notify();
}

export default function ErrorToast() {
  const [, setTick] = useState(0);

  useEffect(() => {
    const fn = () => setTick((t) => t + 1);
    listeners.push(fn);
    return () => {
      listeners = listeners.filter((l) => l !== fn);
    };
  }, []);

  if (toasts.length === 0) return null;

  return (
    <div className="toast-container" aria-label="Notifications">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`toast toast-${t.level}`}
          role={t.level === "error" ? "alert" : "status"}
          aria-live={t.level === "error" ? "assertive" : "polite"}
          aria-atomic="true"
        >
          <div className="toast-icon" aria-hidden="true">
            {t.level === "error" ? (
              <CircleAlert size={16} />
            ) : t.level === "warn" ? (
              <TriangleAlert size={16} />
            ) : (
              <Info size={16} />
            )}
          </div>
          <div className="toast-content">
            <div className="toast-title">{titleForLevel(t.level)}</div>
            <div className="toast-message">{t.message}</div>
          </div>
          <button
            type="button"
            className="toast-close"
            onClick={() => removeToast(t.id)}
            aria-label="Dismiss notification"
          >
            <X size={14} />
          </button>
          <div
            className="toast-progress"
            style={{
              animationDuration: `${TOAST_DURATION_MS}ms`,
            }}
          />
        </div>
      ))}
    </div>
  );
}
