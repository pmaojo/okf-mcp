import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryPatchView } from "./view";

export const memoryPatchManifest: ToolManifest = {
  slug: "memory_patch",
  title: "Patch",
  description: "Selectively update frontmatter fields and tags without rewriting the body.",
  version: "1.0.0",
  component: MemoryPatchView,
  config: { requiredPermissions: ["callServerTool"] },
};
