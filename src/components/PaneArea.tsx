import { useCallback } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";
import { useWorkspaceStore } from "../stores/workspace";
import type { PaneNode } from "../stores/workspace";
import TerminalPane from "./TerminalPane";

interface PaneAreaProps {
  workspaceId: string;
}

function RenderNode({
  node,
  focusedPaneId,
  cwd,
  workspaceId,
  onPaneLayout,
}: {
  node: PaneNode;
  focusedPaneId: string | undefined;
  cwd: string;
  workspaceId: string;
  onPaneLayout: (splitId: string, sizes: number[]) => void;
}) {
  if (node.type === "leaf") {
    return (
      <TerminalPane
        paneId={node.id}
        isFocused={node.id === focusedPaneId}
        cwd={cwd}
        workspaceId={workspaceId}
      />
    );
  }

  // Flatten panels and separators as direct siblings with stable keys.
  // This lets React reorder Panel DOM nodes on swap without remounting
  // (PanelWithHandle fragments caused remounts when index changed).
  const elements: React.ReactNode[] = [];
  for (let i = 0; i < node.children.length; i++) {
    if (i > 0) {
      elements.push(<Separator key={`sep-${i}`} className="resize-handle" />);
    }
    const child = node.children[i]!;
    elements.push(
      <Panel
        key={child.id}
        id={child.id}
        defaultSize={`${node.sizes[i]}`}
        minSize="5"
      >
        <RenderNode
          node={child}
          focusedPaneId={focusedPaneId}
          cwd={cwd}
          workspaceId={workspaceId}
          onPaneLayout={onPaneLayout}
        />
      </Panel>,
    );
  }

  return (
    <Group
      orientation={node.type}
      id={node.id}
      onLayoutChanged={(layout) =>
        onPaneLayout(
          node.id,
          node.children.map(
            (child, index) => layout[child.id] ?? node.sizes[index]!,
          ),
        )
      }
    >
      {elements}
    </Group>
  );
}

export default function PaneArea({ workspaceId }: PaneAreaProps) {
  const root = useWorkspaceStore((s) => s.workspaces[workspaceId]?.root);
  const isActive = useWorkspaceStore(
    (s) => s.activeWorkspaceId === workspaceId,
  );
  const focusedPaneId = useWorkspaceStore(
    (s) => s.workspaces[workspaceId]?.focusedPaneId,
  );
  const cwd = useWorkspaceStore(
    (s) => s.workspaces[workspaceId]?.workingDir ?? "",
  );
  const updatePaneSizes = useWorkspaceStore((s) => s.updatePaneSizes);

  const handlePaneLayout = useCallback(
    (splitId: string, sizes: number[]) => {
      updatePaneSizes(splitId, sizes);
    },
    [updatePaneSizes],
  );

  if (!root) return null;

  // Only pass focusedPaneId when this workspace is active,
  // so only the active workspace's focused terminal gets DOM focus
  const effectiveFocusId = isActive ? focusedPaneId : undefined;

  return (
    <RenderNode
      node={root}
      focusedPaneId={effectiveFocusId}
      cwd={cwd}
      workspaceId={workspaceId}
      onPaneLayout={handlePaneLayout}
    />
  );
}
