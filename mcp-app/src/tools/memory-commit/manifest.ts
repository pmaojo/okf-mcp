import type { ToolManifest } from "@/core/framework/tool-contract";
import { MemoryCommitView } from "./view";

export const memoryCommitManifest: ToolManifest = {
  slug: "memory_commit",
  title: "Commit",
  description: "Write a document with compare-and-swap conflict detection, or dry-run it first.",
  version: "1.0.0",
  component: MemoryCommitView,
  config: { requiredPermissions: ["callServerTool"] },
};
