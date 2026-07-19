import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryValidateView } from "./view";

export const memoryValidateManifest: ToolManifest = {
  slug: "memory_validate",
  title: "Validate",
  description: "Report broken links, references to deleted concepts, and stale embeddings.",
  version: "1.0.0",
  component: MemoryValidateView,
  config: { requiredPermissions: ["callServerTool"] },
};
