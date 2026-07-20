import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryListView } from "./view";

export const memoryListManifest: ToolManifest = {
  slug: "memory_list",
  title: "List",
  description: "List concept metadata under a path prefix, without reading content.",
  version: "1.0.0",
  component: MemoryListView,
  config: { requiredPermissions: ["callServerTool"] },
};
