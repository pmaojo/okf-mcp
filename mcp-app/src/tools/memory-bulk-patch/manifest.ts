import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryBulkPatchView } from "./view";

export const memoryBulkPatchManifest: ToolManifest = {
  slug: "memory_bulk_patch",
  title: "Bulk patch",
  description: "Patch frontmatter on several documents in one call, optionally atomic (all-or-nothing).",
  version: "1.0.0",
  component: MemoryBulkPatchView,
  config: { requiredPermissions: ["callServerTool"] },
};
