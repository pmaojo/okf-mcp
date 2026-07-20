import { toast } from "sonner";
import type { App } from "@modelcontextprotocol/ext-apps";
import { Button } from "@/shared/components/ui/button";

/**
 * `app.updateModelContext` — NOT `app.sendMessage`. It silently hands
 * text/structured state to the host, which the host folds into the
 * model's context on ITS next turn (real user message or a future
 * `sendMessage`) without interrupting anything now. `sendMessage`
 * would inject a visible fake user turn immediately; that's the wrong
 * tool for "the human did something in the UI, make sure the agent
 * knows next time it looks" — this is the right one. Each call
 * overwrites the previous update; only the latest is kept.
 */
export function AddContextButton({
  app,
  text,
  label = "Add to agent context",
}: {
  app: App | null;
  text: string;
  label?: string;
}) {
  const handleUpdate = async () => {
    if (!app) {
      toast.error("Host bridge not ready.");
      return;
    }
    try {
      await app.updateModelContext({ content: [{ type: "text", text }] });
      toast.success("Context updated for the agent's next turn.");
    } catch {
      toast.error("Could not update the agent's context.");
    }
  };

  return (
    <Button type="button" variant="outline" size="sm" onClick={handleUpdate}>
      {label}
    </Button>
  );
}
