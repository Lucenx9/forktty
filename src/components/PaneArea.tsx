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
}: {
  node: PaneNode;
  focusedPaneId: string | undefined;
  cwd: string;
}) {
  if (node.type === "leaf") {
    return (
      <TerminalPane
        paneId={node.id}
        isFocused={node.id === focusedPaneId}
        cwd={cwd}
      />
    );
  }

  return (
    <Group orientation={node.type} id={node.id}>
      {node.children.map((child, i) => (
        <PanelWithHandle key={child.id} index={i} total={node.children.length}>
          <Panel id={child.id} defaultSize={node.sizes[i]} minSize={5}>
            <RenderNode node={child} focusedPaneId={focusedPaneId} cwd={cwd} />
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

  if (!root) return null;

  // Only pass focusedPaneId when this workspace is active,
  // so only the active workspace's focused terminal gets DOM focus
  const effectiveFocusId = isActive ? focusedPaneId : undefined;

  return <RenderNode node={root} focusedPaneId={effectiveFocusId} cwd={cwd} />;
}
