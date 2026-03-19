import { useEffect } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import PaneArea from "./components/PaneArea";
import Sidebar from "./components/Sidebar";
import { useWorkspaceStore } from "./stores/workspace";
import type { Direction } from "./stores/workspace";
import "./App.css";

export default function App() {
  const splitPane = useWorkspaceStore((s) => s.splitPane);
  const closePane = useWorkspaceStore((s) => s.closePane);
  const moveFocus = useWorkspaceStore((s) => s.moveFocus);
  const createWorkspace = useWorkspaceStore((s) => s.createWorkspace);
  const closeWorkspace = useWorkspaceStore((s) => s.closeWorkspace);
  const switchWorkspace = useWorkspaceStore((s) => s.switchWorkspace);
  const workspaceOrder = useWorkspaceStore((s) => s.workspaceOrder);
  const activeWorkspaceId = useWorkspaceStore((s) => s.activeWorkspaceId);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // NOTE: These shortcuts override terminal keys (Ctrl+D = EOF, Ctrl+W = delete word,
      // Ctrl+N = next history). This matches SPEC.md. Users can still exit shells via `exit`.

      // Ctrl+Shift+W: close workspace
      if (e.ctrlKey && e.shiftKey && e.key === "W") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        if (state.workspaceOrder.length <= 1) return;
        const ws = state.workspaces[state.activeWorkspaceId];
        if (!ws) return;
        const paneCount = Object.keys(ws.surfaces).length;
        if (paneCount > 1) {
          if (
            !window.confirm(
              `Close workspace "${ws.name}" with ${paneCount} panes?`,
            )
          ) {
            return;
          }
        }
        closeWorkspace(state.activeWorkspaceId);
        return;
      }

      // Ctrl+Shift+D: split down (vertical)
      if (e.ctrlKey && e.shiftKey && e.key === "D") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) splitPane(ws.focusedPaneId, "vertical");
        return;
      }

      // Ctrl+N: new workspace
      if (e.ctrlKey && !e.shiftKey && e.key === "n") {
        e.preventDefault();
        createWorkspace();
        return;
      }

      // Ctrl+D: split right (horizontal)
      if (e.ctrlKey && !e.shiftKey && e.key === "d") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) splitPane(ws.focusedPaneId, "horizontal");
        return;
      }

      // Ctrl+W: close pane
      if (e.ctrlKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        const state = useWorkspaceStore.getState();
        const ws = state.workspaces[state.activeWorkspaceId];
        if (ws) closePane(ws.focusedPaneId);
        return;
      }

      // Ctrl+1..9: jump to workspace by position
      if (e.ctrlKey && !e.shiftKey && !e.altKey) {
        const digit = parseInt(e.key, 10);
        if (digit >= 1 && digit <= 9) {
          e.preventDefault();
          const state = useWorkspaceStore.getState();
          const targetId = state.workspaceOrder[digit - 1];
          if (targetId) {
            switchWorkspace(targetId);
          }
          return;
        }
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
  }, [
    splitPane,
    closePane,
    moveFocus,
    createWorkspace,
    closeWorkspace,
    switchWorkspace,
  ]);

  return (
    <div className="app">
      <Group orientation="horizontal">
        <Panel id="sidebar" defaultSize={15} minSize={8} maxSize={30}>
          <Sidebar />
        </Panel>
        <Separator className="resize-handle sidebar-separator" />
        <Panel id="main" defaultSize={85}>
          <div className="workspace-container">
            {workspaceOrder.map((id) => (
              <div
                key={id}
                className="workspace-pane-area"
                style={{
                  display: id === activeWorkspaceId ? "flex" : "none",
                }}
              >
                <PaneArea workspaceId={id} />
              </div>
            ))}
          </div>
        </Panel>
      </Group>
    </div>
  );
}
