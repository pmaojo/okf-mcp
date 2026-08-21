/**
 * @file Central Tool Registry
 *
 * @description
 * This registry is the seam between the generic app shell and the tool-specific
 * UI modules.
 *
 * Architectural note:
 * Dynamic imports are intentionally avoided here. Static imports keep the
 * single-file build predictable and make starter customization easier for
 * teammates who are still learning the structure.
 */

import type { ToolManifest } from "@/core/framework/tool-contract";
import { memorySearchManifest } from "./memory-search/manifest";
import { memoryResolveManifest } from "./memory-resolve/manifest";
import { memoryReasonManifest } from "./memory-reason/manifest";
import { memoryCommitManifest } from "./memory-commit/manifest";
import { memoryHistoryManifest } from "./memory-history/manifest";
import { memoryDeleteManifest } from "./memory-delete/manifest";
import { memoryListManifest } from "./memory-list/manifest";
import { memoryBacklinksManifest } from "./memory-backlinks/manifest";
import { memoryEmbedManifest } from "./memory-embed/manifest";
import { memoryPatchManifest } from "./memory-patch/manifest";
import { memoryBulkCommitManifest } from "./memory-bulk-commit/manifest";
import { memoryBulkPatchManifest } from "./memory-bulk-patch/manifest";
import { memoryValidateManifest } from "./memory-validate/manifest";
import { memoryStatusManifest } from "./memory-status/manifest";
import { memoryStatsManifest } from "./memory-stats/manifest";
import { specProposeManifest } from "./spec-propose/manifest";
import { specTasksManifest } from "./spec-tasks/manifest";
import { specStatusManifest } from "./spec-status/manifest";
import { skillIngestManifest } from "./skill-ingest/manifest";

/**
 * Array of all actively registered tool manifests — the 15 `memory_*`
 * tools plus `spec_propose`/`spec_tasks`/`spec_status`/`skill_ingest`
 * from `crates/memory-tools`. Slugs must match the server-side tool
 * names exactly (see `ToolSpec::name` in
 * `crates/memory-tools/src/lib.rs`).
 */
const manifests: ToolManifest[] = [
  memorySearchManifest,
  memoryResolveManifest,
  memoryReasonManifest,
  memoryCommitManifest,
  memoryHistoryManifest,
  memoryDeleteManifest,
  memoryListManifest,
  memoryBacklinksManifest,
  memoryEmbedManifest,
  memoryPatchManifest,
  memoryBulkCommitManifest,
  memoryBulkPatchManifest,
  memoryValidateManifest,
  memoryStatusManifest,
  memoryStatsManifest,
  specProposeManifest,
  specTasksManifest,
  specStatusManifest,
  skillIngestManifest,
];

/**
 * Record mapping a tool slug to the React component that renders its UI.
 */
export const TOOL_COMPONENTS: Record<string, ToolManifest["component"]> =
  manifests.reduce(
    (acc, manifest) => {
      acc[manifest.slug] = manifest.component;
      return acc;
    },
    {} as Record<string, ToolManifest["component"]>
  );
