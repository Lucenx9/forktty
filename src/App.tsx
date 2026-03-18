import { useEffect } from "react";
import PaneArea from "./components/PaneArea";
import { useWorkspaceStore } from "./stores/workspace";
import type { Direction } from "./stores/workspace";
import "./App.css";

export default function App() {
  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const closePane = useWorkspaceStore((s) => s.closePane);
  const moveFocus = useWorkspaceStore((s) => s.moveFocus);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // NOTE: These shortcuts override terminal keys (Ctrl+D = EOF, Ctrl+W = delete word).
      // This matches SPEC.md. Users can still exit shells via `exit` command.

      // Ctrl+Shift+D: split down (vertical)
      // Must check Shift first since Ctrl+D without Shift is split right
      if (e.ctrlKey && e.shiftKey && e.key === "D") {
        e.preventDefault();
        const paneId = useWorkspaceStore.getState().focusedPaneId;
        splitPane(paneId, "vertical");
        return;
      }

      // Ctrl+D: split right (horizontal)
      if (e.ctrlKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        const paneId = useWorkspaceStore.getState().focusedPaneId;
        splitPane(paneId, "horizontal");
        return;
      }

      // Ctrl+W: close pane
      if (e.ctrlKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        const paneId = useWorkspaceStore.getState().focusedPaneId;
        closePane(paneId);
        return;
      }

      // Alt+Arrow: navigate panes
      if (e.altKey && e.key.startsWith("Arrow")) {
        e.preventDefault();
        const dirMap: Record<string, Direction> = {
          ArrowLeft: "left",
          ArrowRight: "right",
          ArrowUp: "up",
          ArrowDown: "down",
        };
        const direction = dirMap[e.key];
        if (direction) {
          moveFocus(direction);
        }
        return;
      }
    }

    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [splitPane, closePane, moveFocus]);

  return (
    <div className="app">
      <PaneArea />
    </div>
  );
}
