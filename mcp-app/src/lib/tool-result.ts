import type { CallToolResult } from "@modelcontextprotocol/sdk/types.js";

/**
 * Extracts and parses the JSON payload a `memory_*` tool call returns.
 * The server may put it in `structuredContent` or as text inside
 * `content[0]` — handle both since the transport in front of this app
 * is not guaranteed to normalize that.
 */
export function parseToolPayload<T>(
  result: CallToolResult | null | undefined
): T | null {
  if (!result) return null;

  const structured = (result as { structuredContent?: unknown })
    .structuredContent;
  if (structured && typeof structured === "object") {
    return structured as T;
  }

  const textBlock = result.content?.find(
    (block): block is { type: "text"; text: string } => block.type === "text"
  );
  if (!textBlock) return null;

  try {
    return JSON.parse(textBlock.text) as T;
  } catch {
    return null;
  }
}

export function formatErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/** Short concept_id-shaped ids stay readable; long hashes get clipped. */
export function shortHash(hash: string, length = 10): string {
  return hash.length > length ? `${hash.slice(0, length)}…` : hash;
}
