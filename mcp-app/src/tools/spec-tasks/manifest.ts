import type { ToolManifest } from "@/core/framework/tool-contract";
import { SpecTasksView } from "./view";

export const specTasksManifest: ToolManifest = {
  slug: "spec_tasks",
  title: "Spec tasks",
  description: "Break a proposed spec into linked, trackable tasks.",
  version: "1.0.0",
  component: SpecTasksView,
  config: { requiredPermissions: ["callServerTool"] },
};
