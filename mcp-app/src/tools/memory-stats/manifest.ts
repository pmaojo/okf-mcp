import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryStatsView } from "./view";

export const memoryStatsManifest: ToolManifest = {
  slug: "memory_stats",
  title: "Stats",
  description: "Graph-wide statistics: hubs, orphans, and counts by type/tag — no arguments needed.",
  version: "1.0.0",
  component: MemoryStatsView,
  config: { requiredPermissions: ["callServerTool"] },
};
