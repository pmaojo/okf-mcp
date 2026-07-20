import type { ToolManifest } from "@/core/framework/tool-contract";
import { SpecProposeView } from "./view";

export const specProposeManifest: ToolManifest = {
  slug: "spec_propose",
  title: "Propose spec",
  description: "Create a spec-driven proposal (requirements + design) before implementing.",
  version: "1.0.0",
  component: SpecProposeView,
  config: { requiredPermissions: ["callServerTool"] },
};
