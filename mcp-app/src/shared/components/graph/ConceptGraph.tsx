import { useMemo } from "react";
import {
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  MarkerType,
  Position,
  ReactFlow,
  ReactFlowProvider,
  type Edge,
  type Node,
  type NodeProps,
} from "@xyflow/react";
import { cn } from "@/lib/utils";

export interface GraphNodeSpec {
  id: string;
  label: string;
  depth: number;
  broken?: boolean;
  root?: boolean;
}

export interface GraphEdgeSpec {
  source: string;
  target: string;
  label?: string;
  /**
   * true = produced by ontology-core's materialize(), not written in the
   * source document. Styled dashed/accent instead of solid/border, and
   * prefixed onto the React Flow edge id (`derived-...` vs `asserted-...`)
   * so a consumer (memory-reason's animated view) can target either group
   * with a plain CSS selector instead of threading extra props through.
   */
  derived?: boolean;
}

/**
 * Deterministic radial layout: the root sits at the center, every other
 * depth is a ring around it with nodes spread evenly by angle. No force
 * simulation and no layout dependency — good enough for the bounded
 * neighborhoods this server ever returns (budget-capped BFS).
 */
function layoutRadial(nodes: GraphNodeSpec[]): Map<string, { x: number; y: number }> {
  const byDepth = new Map<number, GraphNodeSpec[]>();
  for (const node of nodes) {
    const bucket = byDepth.get(node.depth) ?? [];
    bucket.push(node);
    byDepth.set(node.depth, bucket);
  }

  const positions = new Map<string, { x: number; y: number }>();
  const ringStep = 190;
  for (const [depth, ring] of [...byDepth.entries()].sort((a, b) => a[0] - b[0])) {
    if (depth === 0) {
      for (const node of ring) positions.set(node.id, { x: 0, y: 0 });
      continue;
    }
    const radius = ringStep * depth;
    ring.forEach((node, i) => {
      const angle = (2 * Math.PI * i) / ring.length - Math.PI / 2;
      positions.set(node.id, {
        x: radius * Math.cos(angle),
        y: radius * Math.sin(angle),
      });
    });
  }
  return positions;
}

function ConceptNodeView({ data, selected }: NodeProps) {
  const nodeData = data as unknown as GraphNodeSpec & { onClick?: (id: string) => void };
  return (
    <button
      type="button"
      onClick={() => nodeData.onClick?.(nodeData.id)}
      className={cn(
        "border-2 border-foreground px-3 py-2 font-mono text-xs font-bold whitespace-nowrap shadow-brutal-sm transition-transform hover:-translate-y-0.5",
        nodeData.root && "bg-primary text-primary-foreground",
        !nodeData.root && nodeData.broken && "bg-destructive text-white",
        !nodeData.root && !nodeData.broken && "bg-card text-card-foreground",
        selected && "ring-4 ring-accent"
      )}
    >
      <Handle type="target" position={Position.Top} className="!bg-foreground" />
      {nodeData.label}
      {nodeData.broken && !nodeData.root && (
        <span className="ml-1 uppercase">✕ broken</span>
      )}
      <Handle type="source" position={Position.Bottom} className="!bg-foreground" />
    </button>
  );
}

const nodeTypes = { concept: ConceptNodeView };

export function ConceptGraph({
  nodes,
  edges,
  onNodeClick,
  height = 440,
}: {
  nodes: GraphNodeSpec[];
  edges: GraphEdgeSpec[];
  onNodeClick?: (conceptId: string) => void;
  height?: number;
}) {
  const flowNodes: Node[] = useMemo(() => {
    const positions = layoutRadial(nodes);
    return nodes.map((node) => ({
      id: node.id,
      type: "concept",
      position: positions.get(node.id) ?? { x: 0, y: 0 },
      data: { ...node, onClick: onNodeClick },
    }));
  }, [nodes, onNodeClick]);

  const flowEdges: Edge[] = useMemo(
    () =>
      edges.map((edge, i) => {
        const color = edge.derived ? "var(--accent)" : "var(--border)";
        return {
          id: `${edge.derived ? "derived" : "asserted"}-${edge.source}->${edge.target}-${i}`,
          source: edge.source,
          target: edge.target,
          label: edge.label,
          style: {
            stroke: color,
            strokeWidth: 2,
            ...(edge.derived ? { strokeDasharray: "6 4" } : {}),
          },
          markerEnd: { type: MarkerType.ArrowClosed, color },
        };
      }),
    [edges]
  );

  if (nodes.length === 0) {
    return (
      <p className="border-2 border-dashed border-border p-6 text-center text-sm text-muted-foreground">
        No graph data yet.
      </p>
    );
  }

  return (
    <div
      className="border-2 border-foreground bg-background shadow-brutal"
      style={{ height }}
    >
      <ReactFlowProvider>
        <ReactFlow
          nodes={flowNodes}
          edges={flowEdges}
          nodeTypes={nodeTypes}
          fitView
          fitViewOptions={{ padding: 0.3 }}
          proOptions={{ hideAttribution: true }}
          minZoom={0.2}
        >
          <Background variant={BackgroundVariant.Cross} gap={28} size={1} color="var(--border)" />
          <Controls showInteractive={false} />
        </ReactFlow>
      </ReactFlowProvider>
    </div>
  );
}
