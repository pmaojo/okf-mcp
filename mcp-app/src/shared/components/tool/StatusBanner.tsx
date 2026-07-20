import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/shared/components/ui/alert";

export function ErrorBanner({ title, detail }: { title: string; detail?: string }) {
  return (
    <Alert variant="destructive">
      <AlertTitle>{title}</AlertTitle>
      {detail && <AlertDescription>{detail}</AlertDescription>}
    </Alert>
  );
}

export function EmptyBanner({ children }: { children: React.ReactNode }) {
  return (
    <Alert>
      <AlertTitle>No result yet</AlertTitle>
      <AlertDescription>{children}</AlertDescription>
    </Alert>
  );
}
