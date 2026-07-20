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
  kicker?: string;
}) {
  return (
    <Card>
      <CardHeader>
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="default">{kicker ?? "okf-memory"}</Badge>
          <Badge variant="outline" className="font-mono normal-case">
            {slug}
          </Badge>
        </div>
        <CardTitle className="text-xl">{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
    </Card>
  );
}
