import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryHistoryView } from "./view";

export const memoryHistoryManifest: ToolManifest = {
  slug: "memory_history",
  title: "History",
  description: "Revision timeline for a concept, newest first, paginated by seq.",
  version: "1.0.0",
  component: MemoryHistoryView,
  config: { requiredPermissions: ["callServerTool"] },
};
