// @vitest-environment jsdom
import { act, useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { useOverlayFocus } from "./overlay-focus";

function OverlayHarness({ onClose }: { onClose: () => void }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const firstButtonRef = useRef<HTMLButtonElement>(null);

  useOverlayFocus({
    containerRef,
    initialFocusRef: firstButtonRef,
    onClose,
  });

  return (
    <div ref={containerRef} tabIndex={-1}>
      <button ref={firstButtonRef} type="button">
        First
      </button>
      <button type="button">Second</button>
    </div>
  );
}

async function flushOverlayEffects(): Promise<void> {
  await act(async () => {
    await new Promise<void>((resolve) => {
      window.requestAnimationFrame(() => resolve());
    });
  });
}

describe("useOverlayFocus", () => {
  let mountNode: HTMLDivElement;
  let root: Root;
  let outsideButton: HTMLButtonElement;

  beforeEach(() => {
    mountNode = document.createElement("div");
    document.body.appendChild(mountNode);
    root = createRoot(mountNode);

    outsideButton = document.createElement("button");
    outsideButton.type = "button";
    outsideButton.textContent = "Outside";
    document.body.appendChild(outsideButton);
    document.body.style.overflow = "";
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    mountNode.remove();
    outsideButton.remove();
    document.body.style.overflow = "";
  });

  it("moves focus to the requested initial element and locks body scroll", async () => {
    outsideButton.focus();

    await act(async () => {
      root.render(<OverlayHarness onClose={vi.fn()} />);
    });
    await flushOverlayEffects();

    expect(document.activeElement?.textContent).toBe("First");
    expect(document.body.style.overflow).toBe("hidden");
  });

  it("wraps focus on Tab and Shift+Tab", async () => {
    await act(async () => {
      root.render(<OverlayHarness onClose={vi.fn()} />);
    });
    await flushOverlayEffects();

    const buttons = mountNode.querySelectorAll("button");
    const first = buttons[0]!;
    const second = buttons[1]!;

    second.focus();
    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab" }));
    });
    expect(document.activeElement).toBe(first);

    first.focus();
    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Tab", shiftKey: true }),
      );
    });
    expect(document.activeElement).toBe(second);
  });

  it("closes once on Escape and restores focus on unmount", async () => {
    const onClose = vi.fn();
    outsideButton.focus();

    await act(async () => {
      root.render(<OverlayHarness onClose={onClose} />);
    });
    await flushOverlayEffects();

    act(() => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);

    act(() => {
      root.unmount();
    });
    expect(document.activeElement).toBe(outsideButton);
  });
});
