import { useEffect, useMemo, useRef, useState } from "react";
import { gsap } from "gsap";
import type { ToolComponentProps } from "@/core/framework/tool-contract";
import { ToolHeader } from "@/shared/components/tool/ToolHeader";
import {
  ToolLayout,
  ToolSplit,
  RunPanel,
  ResultPanel,
} from "@/shared/components/tool/ToolLayout";
import { Field } from "@/shared/components/tool/Field";
import { ErrorBanner, EmptyBanner } from "@/shared/components/tool/StatusBanner";
import {
  ConceptGraph,
  type GraphNodeSpec,
  type GraphEdgeSpec,
} from "@/shared/components/graph/ConceptGraph";
import { Input } from "@/shared/components/ui/input";
import { Button } from "@/shared/components/ui/button";
import { Badge } from "@/shared/components/ui/badge";
import { Switch } from "@/shared/components/ui/switch";
import { useServerTool } from "@/shared/hooks/useServerTool";
import { parseToolPayload } from "@/lib/tool-result";
import type { MemoryReasonResult } from "@/lib/mcp-types";

type PropertyKind = "transitive" | "symmetric" | "sub_property_of" | "inverse_of";

interface ClassRow {
  subclass: string;
  superclass: string;
}

interface PropertyRow {
  kind: PropertyKind;
  property: string;
  sub: string;
  sup: string;
  inverse: string;
}

const emptyClassRow = (): ClassRow => ({ subclass: "", superclass: "" });
const emptyPropertyRow = (): PropertyRow => ({
  kind: "transitive",
  property: "",
  sub: "",
  sup: "",
  inverse: "",
});

/** Builds the graph ConceptGraph renders from the flat triple list — concept
 * objects reuse their neighborhood node; literal objects (tags, rdf:type)
 * become one-off leaf nodes attached to whichever subject asserted them. */
function buildGraph(data: MemoryReasonResult): {
  nodes: GraphNodeSpec[];
  assertedEdges: GraphEdgeSpec[];
  derivedEdges: GraphEdgeSpec[];
} {
  const depthByConcept = new Map<string, number>();
  const existsByConcept = new Map<string, boolean>();
  let maxKnownDepth = 0;
  for (const n of data.neighborhood) {
    depthByConcept.set(n.concept_id, n.depth);
    existsByConcept.set(n.concept_id, n.exists);
    maxKnownDepth = Math.max(maxKnownDepth, n.depth);
  }

  const nodesById = new Map<string, GraphNodeSpec>();
  const ensureConceptNode = (id: string) => {
    if (nodesById.has(id)) return;
    nodesById.set(id, {
      id,
      label: id,
      depth: depthByConcept.get(id) ?? maxKnownDepth + 1,
      root: id === data.root,
      broken: existsByConcept.get(id) === false,
    });
  };

  const assertedEdges: GraphEdgeSpec[] = [];
  const derivedEdges: GraphEdgeSpec[] = [];

  data.triples.forEach((t, i) => {
    ensureConceptNode(t.subject);
    let targetId: string;
    if (t.object.kind === "concept") {
      ensureConceptNode(t.object.value);
      targetId = t.object.value;
    } else {
      targetId = `lit:${i}:${t.object.value}`;
      const subjectDepth = depthByConcept.get(t.subject) ?? maxKnownDepth;
      nodesById.set(targetId, {
        id: targetId,
        label: t.object.value,
        depth: subjectDepth + 1,
      });
    }
    const edge: GraphEdgeSpec = {
      source: t.subject,
      target: targetId,
      label: t.predicate,
      derived: t.derived,
    };
    (t.derived ? derivedEdges : assertedEdges).push(edge);
  });

  return { nodes: [...nodesById.values()], assertedEdges, derivedEdges };
}

/** Renders the graph and drives its own two-wave GSAP entrance: asserted
 * facts appear first (what the documents actually say), derived facts
 * follow a beat later with an accent pulse (what the reasoner added). With
 * no derived triples at all, everything appears in one wave — there's
 * nothing to stage. */
function ReasoningGraph({ data }: { data: MemoryReasonResult }) {
  const { nodes, assertedEdges, derivedEdges } = useMemo(() => buildGraph(data), [data]);
  const assertedNodeIds = useMemo(() => {
    const ids = new Set<string>([data.root]);
    for (const e of assertedEdges) {
      ids.add(e.source);
      ids.add(e.target);
    }
    return ids;
  }, [assertedEdges, data.root]);
  const assertedNodes = useMemo(
    () => nodes.filter((n) => assertedNodeIds.has(n.id)),
    [nodes, assertedNodeIds]
  );

  const [phase, setPhase] = useState<"asserted" | "all">(
    derivedEdges.length === 0 ? "all" : "asserted"
  );
  const containerRef = useRef<HTMLDivElement>(null);
  const animatedIds = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (phase !== "asserted") return;
    const timer = window.setTimeout(() => setPhase("all"), 1100);
    return () => window.clearTimeout(timer);
  }, [phase]);

  useEffect(() => {
    const raf = requestAnimationFrame(() => {
      const root = containerRef.current;
      if (!root) return;
      const elements = Array.from(
        root.querySelectorAll<HTMLElement>(".react-flow__node, .react-flow__edge")
      );
      const fresh = elements.filter((el) => {
        const id = el.dataset.id ?? "";
        if (!id || animatedIds.current.has(id)) return false;
        animatedIds.current.add(id);
        return true;
      });
      if (fresh.length === 0) return;

      gsap.fromTo(
        fresh,
        { opacity: 0, scale: 0.82, transformOrigin: "50% 50%" },
        { opacity: 1, scale: 1, duration: 0.45, stagger: 0.045, ease: "back.out(1.8)" }
      );

      const freshDerived = fresh.filter((el) => (el.dataset.id ?? "").startsWith("derived-"));
      if (freshDerived.length > 0) {
        gsap.fromTo(
          freshDerived,
          { filter: "drop-shadow(0 0 0px var(--accent))" },
          {
            filter: "drop-shadow(0 0 7px var(--accent))",
            duration: 0.5,
            delay: 0.3,
            repeat: 1,
            yoyo: true,
            ease: "sine.inOut",
          }
        );
      }
    });
    return () => cancelAnimationFrame(raf);
  }, [phase]);

  const visibleNodes = phase === "asserted" ? assertedNodes : nodes;
  const visibleEdges = phase === "asserted" ? assertedEdges : [...assertedEdges, ...derivedEdges];

  return (
    <div ref={containerRef}>
      <ConceptGraph nodes={visibleNodes} edges={visibleEdges} height={420} />
    </div>
  );
}

function ReasoningResult({ data }: { data: MemoryReasonResult }) {
  const truncatedKeys = Object.entries(data.truncated)
    .filter(([, v]) => v)
    .map(([k]) => k.replace(/_/g, " "));

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge variant="secondary">{data.asserted_count} asserted</Badge>
        <Badge variant={data.derived_count > 0 ? "default" : "secondary"}>
          {data.derived_count} derived
        </Badge>
        <Badge variant="outline">{data.persisted ? "persisted" : "not persisted"}</Badge>
        {truncatedKeys.length > 0 && (
          <Badge variant="destructive">truncated: {truncatedKeys.join(", ")}</Badge>
        )}
      </div>

      {/* Keyed by signature: a new concept_id/axioms/rerun should restage the
          animation from scratch, not carry over "already animated" state from
          a previous graph — a fresh key remounts ReasoningGraph cleanly. */}
      <ReasoningGraph
        key={`${data.root}|${data.asserted_count}|${data.derived_count}|${data.persisted}`}
        data={data}
      />

      <details className="text-xs">
        <summary className="cursor-pointer font-bold uppercase tracking-wide text-muted-foreground">
          Show as list ({data.triples.length})
        </summary>
        <ul className="mt-2 space-y-1 font-mono">
          {data.triples.map((t, i) => (
            <li key={i} className={t.derived ? "text-accent-foreground" : undefined}>
              {t.subject} —{t.predicate}→ {t.object.value}
              {t.derived ? " (derived)" : ""}
            </li>
          ))}
        </ul>
      </details>
    </div>
  );
}

export function MemoryReasonView({ app, toolResult }: ToolComponentProps) {
  const [conceptId, setConceptId] = useState("");
  const [depth, setDepth] = useState("");
  const [classRows, setClassRows] = useState<ClassRow[]>([]);
  const [propertyRows, setPropertyRows] = useState<PropertyRow[]>([]);
  const [maxIterations, setMaxIterations] = useState("");
  const [maxTriples, setMaxTriples] = useState("");
  const [persist, setPersist] = useState(true);

  const { activeResult, isError, isLoading, executeTool } = useServerTool(
    app,
    "memory_reason",
    toolResult
  );
  const parsed = parseToolPayload<MemoryReasonResult>(activeResult);

  const updateClassRow = (i: number, patch: Partial<ClassRow>) =>
    setClassRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));
  const updatePropertyRow = (i: number, patch: Partial<PropertyRow>) =>
    setPropertyRows((rs) => rs.map((r, idx) => (idx === i ? { ...r, ...patch } : r)));

  const run = () => {
    if (!conceptId.trim()) return;
    const args: Record<string, unknown> = { concept_id: conceptId.trim(), persist };

    const depthNum = Number(depth);
    if (Number.isFinite(depthNum) && depthNum > 0) args.depth = depthNum;

    const classes = classRows
      .filter((r) => r.subclass.trim() && r.superclass.trim())
      .map((r) => ({ subclass: r.subclass.trim(), superclass: r.superclass.trim() }));
    if (classes.length > 0) args.classes = classes;

    const properties = propertyRows
      .filter((r) => {
        if (r.kind === "sub_property_of") return r.sub.trim() && r.sup.trim();
        if (r.kind === "inverse_of") return r.property.trim() && r.inverse.trim();
        return r.property.trim();
      })
      .map((r) => {
        if (r.kind === "sub_property_of") return { kind: r.kind, sub: r.sub.trim(), sup: r.sup.trim() };
        if (r.kind === "inverse_of")
          return { kind: r.kind, property: r.property.trim(), inverse: r.inverse.trim() };
        return { kind: r.kind, property: r.property.trim() };
      });
    if (properties.length > 0) args.properties = properties;

    const maxIterNum = Number(maxIterations);
    if (Number.isFinite(maxIterNum) && maxIterNum > 0) args.max_iterations = maxIterNum;
    const maxTriplesNum = Number(maxTriples);
    if (Number.isFinite(maxTriplesNum) && maxTriplesNum > 0) args.max_triples = maxTriplesNum;

    void executeTool(args);
  };

  return (
    <ToolLayout>
      <ToolHeader
        slug="memory_reason"
        title="Reason over the concept graph"
        description="Bounded OWL-RL/RDFS-lite closure over the neighborhood: subclass, sub-property, transitive, symmetric and inverse-property inference — nothing without axioms you declare below."
      />
      <ToolSplit>
        <RunPanel defaultOpen={!toolResult}>
          <Field id="mr-id" label="concept_id">
            <Input
              id="mr-id"
              value={conceptId}
              onChange={(e) => setConceptId(e.target.value)}
              placeholder="people/alice"
              onKeyDown={(e) => e.key === "Enter" && run()}
            />
          </Field>
          <Field
            id="mr-depth"
            label="Depth"
            hint="neighborhood depth to gather facts from (default: server budget)"
          >
            <Input
              id="mr-depth"
              type="number"
              min={1}
              value={depth}
              onChange={(e) => setDepth(e.target.value)}
            />
          </Field>

          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-xs font-bold uppercase tracking-wide">
                Classes (subClassOf)
              </span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setClassRows((rs) => [...rs, emptyClassRow()])}
              >
                + class
              </Button>
            </div>
            {classRows.map((row, i) => (
              <div key={i} className="grid grid-cols-[1fr_1fr_auto] gap-2">
                <Input
                  placeholder="subclass (student)"
                  value={row.subclass}
                  onChange={(e) => updateClassRow(i, { subclass: e.target.value })}
                />
                <Input
                  placeholder="superclass (person)"
                  value={row.superclass}
                  onChange={(e) => updateClassRow(i, { superclass: e.target.value })}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setClassRows((rs) => rs.filter((_, idx) => idx !== i))}
                >
                  ✕
                </Button>
              </div>
            ))}
          </div>

          <div className="space-y-2">
            <div className="flex items-center justify-between">
              <span className="text-xs font-bold uppercase tracking-wide">Properties</span>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => setPropertyRows((rs) => [...rs, emptyPropertyRow()])}
              >
                + property
              </Button>
            </div>
            {propertyRows.map((row, i) => (
              <div key={i} className="space-y-1 border border-border p-2">
                <div className="flex items-center gap-2">
                  <select
                    className="h-9 flex-1 border-2 border-input bg-background px-2 text-sm"
                    value={row.kind}
                    onChange={(e) =>
                      updatePropertyRow(i, { kind: e.target.value as PropertyKind })
                    }
                  >
                    <option value="transitive">transitive</option>
                    <option value="symmetric">symmetric</option>
                    <option value="sub_property_of">sub_property_of</option>
                    <option value="inverse_of">inverse_of</option>
                  </select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => setPropertyRows((rs) => rs.filter((_, idx) => idx !== i))}
                  >
                    ✕
                  </Button>
                </div>
                {(row.kind === "transitive" || row.kind === "symmetric") && (
                  <Input
                    placeholder="property (depends_on)"
                    value={row.property}
                    onChange={(e) => updatePropertyRow(i, { property: e.target.value })}
                  />
                )}
                {row.kind === "sub_property_of" && (
                  <div className="grid grid-cols-2 gap-2">
                    <Input
                      placeholder="sub (depends_on)"
                      value={row.sub}
                      onChange={(e) => updatePropertyRow(i, { sub: e.target.value })}
                    />
                    <Input
                      placeholder="sup (related)"
                      value={row.sup}
                      onChange={(e) => updatePropertyRow(i, { sup: e.target.value })}
                    />
                  </div>
                )}
                {row.kind === "inverse_of" && (
                  <div className="grid grid-cols-2 gap-2">
                    <Input
                      placeholder="property (manages)"
                      value={row.property}
                      onChange={(e) => updatePropertyRow(i, { property: e.target.value })}
                    />
                    <Input
                      placeholder="inverse (managed_by)"
                      value={row.inverse}
                      onChange={(e) => updatePropertyRow(i, { inverse: e.target.value })}
                    />
                  </div>
                )}
              </div>
            ))}
          </div>

          <div className="grid grid-cols-2 gap-3">
            <Field id="mr-max-iter" label="Max iterations" hint="default 16">
              <Input
                id="mr-max-iter"
                type="number"
                min={1}
                value={maxIterations}
                onChange={(e) => setMaxIterations(e.target.value)}
              />
            </Field>
            <Field id="mr-max-triples" label="Max triples" hint="default 10000">
              <Input
                id="mr-max-triples"
                type="number"
                min={1}
                value={maxTriples}
                onChange={(e) => setMaxTriples(e.target.value)}
              />
            </Field>
          </div>

          <div className="flex items-center gap-3">
            <Switch id="mr-persist" checked={persist} onCheckedChange={setPersist} />
            <label htmlFor="mr-persist" className="text-sm">
              Persist derived triples
            </label>
          </div>

          <Button onClick={run} disabled={isLoading} className="w-full">
            {isLoading ? "Reasoning…" : "Run memory_reason"}
          </Button>
        </RunPanel>

        <ResultPanel>
          {isError && (
            <ErrorBanner
              title="memory_reason failed"
              detail="Check concept_id and the shape of classes/properties."
            />
          )}
          {!isError && parsed && <ReasoningResult data={parsed} />}
          {!isError && !parsed && !isLoading && (
            <EmptyBanner>Reason over a concept_id to see its closure here.</EmptyBanner>
          )}
        </ResultPanel>
      </ToolSplit>
    </ToolLayout>
  );
}
