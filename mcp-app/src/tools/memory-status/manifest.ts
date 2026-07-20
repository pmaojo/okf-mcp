import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryStatusView } from "./view";

export const memoryStatusManifest: ToolManifest = {
  slug: "memory_status",
  title: "Status",
  description: "Quick operational summary of system health — no arguments needed.",
  version: "1.0.0",
  component: MemoryStatusView,
  config: { requiredPermissions: ["callServerTool"] },
};
