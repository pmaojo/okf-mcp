import { Badge } from "@/shared/components/ui/badge";
import {
  Card,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/shared/components/ui/card";

export function ToolHeader({
  slug,
  title,
  description,
  kicker,
}: {
  slug: string;
  title: string;
  description: string;
  kicker?: "danger";
}) {
  return (
    <Card>
      <CardHeader className="flex-row flex-wrap items-start justify-between gap-3 space-y-0">
        <div className="min-w-0">
          <CardTitle className="text-sm">{title}</CardTitle>
          <CardDescription className="mt-1">{description}</CardDescription>
        </div>
        <Badge
          variant={kicker === "danger" ? "destructive" : "outline"}
          className="shrink-0 font-mono normal-case"
        >
          {slug}
        </Badge>
      </CardHeader>
    </Card>
  );
}
