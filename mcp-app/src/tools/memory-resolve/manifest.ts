import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryResolveView } from "./view";

export const memoryResolveManifest: ToolManifest = {
  slug: "memory_resolve",
  title: "Resolve",
  description:
    "Exact Markdown for one concept plus a bounded neighborhood of its [[links]], rendered as an interactive graph.",
  version: "1.0.0",
  component: MemoryResolveView,
  config: { requiredPermissions: ["callServerTool"] },
};
