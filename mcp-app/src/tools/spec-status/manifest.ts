import type { ToolManifest } from "@/core/framework/tool-contract";
import { SpecStatusView } from "./view";

export const specStatusManifest: ToolManifest = {
  slug: "spec_status",
  title: "Spec status",
  description: "Progress of a spec in one call — resume work, or let another agent check in.",
  version: "1.0.0",
  component: SpecStatusView,
  config: { requiredPermissions: ["callServerTool"] },
};
