import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryEmbedView } from "./view";

export const memoryEmbedManifest: ToolManifest = {
  slug: "memory_embed",
  title: "Embed",
  description: "Force embedding generation and indexing for pending documents.",
  version: "1.0.0",
  component: MemoryEmbedView,
  config: { requiredPermissions: ["callServerTool"] },
};
