import { useRef, useCallback } from "react";
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

  return (
    <Group
      orientation={node.type}
      id={node.id}
      onLayoutChange={(layout) =>
        onPaneLayout(
          node.id,
          node.children.map(
            (child, index) => layout[child.id] ?? node.sizes[index]!,
          ),
        )
      }
    >
      {node.children.map((child, i) => (
        <PanelWithHandle key={child.id} index={i} total={node.children.length}>
          <Panel id={child.id} defaultSize={`${node.sizes[i]}`} minSize="5">
            <RenderNode
              node={child}
              focusedPaneId={focusedPaneId}
              cwd={cwd}
              workspaceId={workspaceId}
              onPaneLayout={onPaneLayout}
            />
          </Panel>
        </PanelWithHandle>
      ))}
    </Group>
  );
}

/** Wraps a Panel with a resize handle before it (except the first child). */
function PanelWithHandle({
  children,
  index,
  total,
}: {
  children: React.ReactNode;
  index: number;
  total: number;
}) {
  if (total <= 1 || index === 0) {
    return <>{children}</>;
  }

  return (
    <>
      <Separator className="resize-handle" />
      {children}
    </>
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

  // Debounce layout changes to avoid write storms during drag
  const rafRef = useRef<number | null>(null);
  const pendingRef = useRef<{ id: string; sizes: number[] } | null>(null);
  const debouncedPaneLayout = useCallback(
    (splitId: string, sizes: number[]) => {
      pendingRef.current = { id: splitId, sizes };
      if (rafRef.current === null) {
        rafRef.current = requestAnimationFrame(() => {
          rafRef.current = null;
          if (pendingRef.current) {
            updatePaneSizes(pendingRef.current.id, pendingRef.current.sizes);
            pendingRef.current = null;
          }
        });
      }
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
      onPaneLayout={debouncedPaneLayout}
    />
  );
}
