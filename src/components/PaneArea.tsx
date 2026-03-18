import { Group, Panel, Separator } from "react-resizable-panels";
import { useWorkspaceStore } from "../stores/workspace";
import type { PaneNode } from "../stores/workspace";
import TerminalPane from "./TerminalPane";

function RenderNode({ node }: { node: PaneNode }) {
  const focusedPaneId = useWorkspaceStore((s) => s.focusedPaneId);

  if (node.type === "leaf") {
    return (
      <TerminalPane paneId={node.id} isFocused={node.id === focusedPaneId} />
    );
  }

  return (
    <Group orientation={node.type} id={node.id}>
      {node.children.map((child, i) => (
        <PanelWithHandle key={child.id} index={i} total={node.children.length}>
          <Panel id={child.id} defaultSize={node.sizes[i]} minSize={5}>
            <RenderNode node={child} />
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

export default function PaneArea() {
  const root = useWorkspaceStore((s) => s.root);
  return <RenderNode node={root} />;
}
