import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryReasonView } from "./view";

export const memoryReasonManifest: ToolManifest = {
  slug: "memory_reason",
  title: "Reason",
  description:
    "Bounded OWL-RL/RDFS-lite reasoning over the concept neighborhood — subclass, transitive, symmetric and inverse-property closure, animated as it materializes.",
  version: "1.0.0",
  component: MemoryReasonView,
  config: { requiredPermissions: ["callServerTool"] },
};
