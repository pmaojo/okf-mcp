import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryBacklinksView } from "./view";

export const memoryBacklinksManifest: ToolManifest = {
  slug: "memory_backlinks",
  title: "Backlinks",
  description: "Incoming [[links]] toward a concept, as a graph you can walk backwards.",
  version: "1.0.0",
  component: MemoryBacklinksView,
  config: { requiredPermissions: ["callServerTool"] },
};
