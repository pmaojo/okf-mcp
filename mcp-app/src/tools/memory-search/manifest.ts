import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemorySearchView } from "./view";

export const memorySearchManifest: ToolManifest = {
  slug: "memory_search",
  title: "Search",
  description:
    "Hybrid search over concept_id, title, tags and body — compact candidates, not full documents.",
  version: "1.0.0",
  component: MemorySearchView,
  config: { requiredPermissions: ["callServerTool"] },
};
