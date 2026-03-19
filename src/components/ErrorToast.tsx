import { useEffect, useState } from "react";

interface Toast {
  id: string;
  message: string;
  level: "error" | "warn" | "info";
}

const toasts: Toast[] = [];
let listeners: Array<() => void> = [];

function notify() {
  for (const fn of listeners) fn();
}

export function showToast(
  message: string,
  level: "error" | "warn" | "info" = "error",
) {
  const toast: Toast = { id: crypto.randomUUID(), message, level };
  toasts.push(toast);
  notify();
  setTimeout(() => {
    const idx = toasts.findIndex((t) => t.id === toast.id);
    if (idx !== -1) {
      toasts.splice(idx, 1);
      notify();
    }
  }, 5000);
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
    <div className="toast-container">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.level}`}>
          {t.message}
        </div>
      ))}
    </div>
  );
}
