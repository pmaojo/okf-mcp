import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryDeleteView } from "./view";

export const memoryDeleteManifest: ToolManifest = {
  slug: "memory_delete",
  title: "Delete",
  description: "Logical delete of a concept with compare-and-swap safety.",
  version: "1.0.0",
  component: MemoryDeleteView,
  config: { requiredPermissions: ["callServerTool"] },
};
