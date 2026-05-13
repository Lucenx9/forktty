import { type RefObject, useEffect, useRef } from "react";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(", ");

function isVisible(element: HTMLElement): boolean {
  const style = window.getComputedStyle(element);
  return style.display !== "none" && style.visibility !== "hidden";
}

export function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter(
    (element) => !element.hasAttribute("aria-hidden") && isVisible(element),
  );
}

interface OverlayFocusOptions {
  containerRef: RefObject<HTMLElement | null>;
  initialFocusRef?: RefObject<HTMLElement | null>;
  onClose?: () => void;
  trapFocus?: boolean;
  closeOnEscape?: boolean;
  lockBodyScroll?: boolean;
  restoreFocus?: boolean;
}

export function useOverlayFocus({
  containerRef,
  initialFocusRef,
  onClose,
  trapFocus = true,
  closeOnEscape = true,
  lockBodyScroll = true,
  restoreFocus = true,
}: OverlayFocusOptions): void {
  const previousFocusRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const containerElement = container;

    previousFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;

    const originalOverflow = document.body.style.overflow;
    if (lockBodyScroll) {
      document.body.style.overflow = "hidden";
    }

    const rafId = requestAnimationFrame(() => {
      const focusTarget =
        initialFocusRef?.current ??
        getFocusableElements(containerElement)[0] ??
        containerElement;
      focusTarget.focus();
    });

    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape" && closeOnEscape && onClose) {
        event.preventDefault();
        onClose();
        return;
      }

      if (event.key !== "Tab" || !trapFocus) {
        return;
      }

      const focusable = getFocusableElements(containerElement);
      if (focusable.length === 0) {
        event.preventDefault();
        containerElement.focus();
        return;
      }

      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const activeElement = document.activeElement;

      if (!containerElement.contains(activeElement)) {
        event.preventDefault();
        first.focus();
        return;
      }

      if (event.shiftKey) {
        if (activeElement === first || activeElement === container) {
          event.preventDefault();
          last.focus();
        }
        return;
      }

      if (activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }

    document.addEventListener("keydown", handleKeyDown);
    return () => {
      cancelAnimationFrame(rafId);
      document.removeEventListener("keydown", handleKeyDown);

      if (lockBodyScroll) {
        document.body.style.overflow = originalOverflow;
      }

      if (restoreFocus && previousFocusRef.current && document.contains(previousFocusRef.current)) {
        previousFocusRef.current.focus();
      }
    };
  }, [
    closeOnEscape,
    containerRef,
    initialFocusRef,
    lockBodyScroll,
    onClose,
    restoreFocus,
    trapFocus,
  ]);
}
