import type { ToolManifest } from "@/core/framework/tool-contract";
import { SkillIngestView } from "./view";

export const skillIngestManifest: ToolManifest = {
  slug: "skill_ingest",
  title: "Ingest skill",
  description: "Ingest skills from an external repo/folder/file, verbatim, without an LLM in the loop.",
  version: "1.0.0",
  component: SkillIngestView,
  config: { requiredPermissions: ["callServerTool"] },
};
