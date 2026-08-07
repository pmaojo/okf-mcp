import { describe, it, expect } from "vitest";
import { TOOL_COMPONENTS } from "@/tools/registry";

const EXPECTED_SLUGS = [
  "memory_search",
  "memory_resolve",
  "memory_reason",
  "memory_commit",
  "memory_history",
  "memory_delete",
  "memory_list",
  "memory_backlinks",
  "memory_embed",
  "memory_patch",
  "memory_bulk_commit",
  "memory_validate",
  "memory_status",
  "memory_stats",
  "spec_propose",
  "spec_tasks",
  "spec_status",
  "skill_ingest",
];

describe("registry", () => {
  it("should register all 18 memory_*/spec_*/skill_ingest tool components", () => {
    expect(Object.keys(TOOL_COMPONENTS).sort()).toEqual(EXPECTED_SLUGS.sort());
    for (const slug of EXPECTED_SLUGS) {
      expect(TOOL_COMPONENTS[slug]).toBeDefined();
    }
  });
});
