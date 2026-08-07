/**
 * TypeScript mirrors of the JSON shapes returned by the 13 `memory_*`
 * tools in `crates/memory-tools/src/lib.rs`. Kept hand-written (no
 * codegen) because the Rust side has no schema export yet — if a field
 * name changes there, it must change here too.
 */

export interface SearchHit {
  concept_id: string;
  hash: string;
  type: string;
  title: string | null;
  tags: string[];
  uri: string;
}

export interface MemorySearchResult {
  results: SearchHit[];
  count: number;
}

/** Identical shape to memory_search's output. */
export type MemoryListResult = MemorySearchResult;

export interface NeighborhoodNode {
  concept_id: string;
  depth: number;
  exists: boolean;
  uri: string;
  parent: string | null;
}

export interface ResolvedDocument {
  concept_id: string;
  hash: string;
  version: number;
  type: string;
  title: string | null;
  tags: string[];
  markdown: string;
}

export interface MemoryResolveResult {
  document: ResolvedDocument;
  neighborhood: NeighborhoodNode[];
  truncated: {
    by_nodes: boolean;
    by_depth: boolean;
    by_bytes: boolean;
  };
}

export interface TripleObject {
  kind: "concept" | "literal";
  value: string;
}

export interface ReasonedTriple {
  subject: string;
  predicate: string;
  object: TripleObject;
  /** false = extracted straight from frontmatter/links; true = produced by materialize(). */
  derived: boolean;
}

export interface ReasonNeighborhoodNode {
  concept_id: string;
  depth: number;
  exists: boolean;
}

export interface MemoryReasonResult {
  root: string;
  asserted_count: number;
  derived_count: number;
  triples: ReasonedTriple[];
  neighborhood: ReasonNeighborhoodNode[];
  ontology_id: string | null;
  persisted: boolean;
  truncated: {
    traversal_by_nodes: boolean;
    traversal_by_depth: boolean;
    traversal_by_bytes: boolean;
    reasoning_by_iterations: boolean;
    reasoning_by_triples: boolean;
  };
}

export interface CommitLikeResult {
  concept_id: string;
  hash: string;
  version: number;
  created: boolean;
  no_change: boolean;
  revision_seq: number | null;
  dry_run?: boolean;
}

/** memory_commit and memory_patch share this exact response shape. */
export type MemoryCommitResult = CommitLikeResult;
export type MemoryPatchResult = CommitLikeResult;

export interface MemoryDeleteResult {
  concept_id: string;
  hash: string;
  version: number;
  revision_seq: number;
}

export interface Backlink {
  source: SearchHit;
  rel: string | null;
}

export interface MemoryBacklinksResult {
  backlinks: Backlink[];
  count: number;
}

export interface MemoryEmbedResult {
  embedded: string[];
  failed: { concept_id: string; error: string }[];
  remaining: number;
}

export type BulkCommitItem =
  | {
      status: "done";
      hash: string;
      version: number;
      created: boolean;
      no_change: boolean;
    }
  | {
      status: "failed";
      error:
        | { kind: "revision_conflict"; expected_hash: string | null; current_hash: string | null }
        | { kind: "error"; detail: string };
    }
  | { status: "skipped" };

export interface MemoryBulkCommitResult {
  applied: boolean;
  items: BulkCommitItem[];
}

export interface MemoryValidateResult {
  broken_links: { source: string; target: string }[];
  broken_links_total: number;
  deleted_referenced: { source: string; target: string }[];
  deleted_referenced_total: number;
  missing_embeddings: string[];
  missing_embeddings_total: number;
}

export interface MemoryStatusResult {
  documents: number;
  deleted_documents: number;
  missing_embeddings: number;
  broken_links: number;
  deleted_referenced: number;
  outbox_pending: number;
  outbox_failed: number;
}

export interface MemoryStatsResult {
  documents: number;
  deleted_documents: number;
  by_type: { type: string; count: number }[];
  by_tag: { tag: string; count: number }[];
  top_linked: { concept_id: string; incoming_links: number }[];
  orphans: string[];
}

export interface Revision {
  seq: number;
  base_hash: string | null;
  result_hash: string;
  actor: string;
  client_id: string;
  reason: string;
}

export interface MemoryHistoryResult {
  concept_id: string;
  revisions: Revision[];
}

export interface SpecProposeResult {
  concept_id: string;
  hash: string;
  version: number;
  created: boolean;
}

export interface SpecTasksResult {
  spec_id: string;
  created: number;
  task_ids: string[];
  skipped: { item: string; reason: string }[];
}

export interface SpecStatusResult {
  spec_id: string;
  spec_status: string;
  spec_title: string | null;
  tasks_total: number;
  by_status: {
    pending: number;
    in_progress: number;
    done: number;
    blocked: number;
    unknown: number;
  };
  progress: number;
  next_pending: string[];
  waiting_on_dependencies: number;
}

export type SkillIngestUnit = {
  concept_id: string;
  title: string;
  action: "commit" | "convert-verbatim";
  warnings: string[];
};

export type SkillIngestItem = {
  concept_id: string;
  mode: "verbatim";
  hash: string;
  version: number;
  created: boolean;
  warnings: string[];
};

export interface SkillIngestResult {
  source_url: string;
  format: string;
  license: string | null;
  dry_run?: boolean;
  units?: SkillIngestUnit[];
  ingested?: number;
  concept_ids?: string[];
  items?: SkillIngestItem[];
  skipped: { item: string; reason: string }[];
}
