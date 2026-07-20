import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryBulkCommitView } from "./view";

export const memoryBulkCommitManifest: ToolManifest = {
  slug: "memory_bulk_commit",
  title: "Bulk commit",
  description: "Commit several documents in one call, optionally atomic (all-or-nothing).",
  version: "1.0.0",
  component: MemoryBulkCommitView,
  config: { requiredPermissions: ["callServerTool"] },
};
