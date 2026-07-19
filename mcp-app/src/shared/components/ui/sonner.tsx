import { useSyncExternalStore } from "react";
import { Toaster as Sonner, type ToasterProps } from "sonner";
import CircleCheckIcon from "lucide-react/dist/esm/icons/circle-check";
import InfoIcon from "lucide-react/dist/esm/icons/info";
import TriangleAlertIcon from "lucide-react/dist/esm/icons/triangle-alert";
import OctagonXIcon from "lucide-react/dist/esm/icons/octagon-x";
import Loader2Icon from "lucide-react/dist/esm/icons/loader-2";

// The host drives the .dark class on <html> (see McpProvider); there is
// no next-themes provider in this app, so we read that class directly.
function subscribeToDarkClass(onChange: () => void) {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["class"],
  });
  return () => observer.disconnect();
}

function useIsDark() {
  return useSyncExternalStore(
    subscribeToDarkClass,
    () => document.documentElement.classList.contains("dark"),
    () => false
  );
}

const Toaster = ({ ...props }: ToasterProps) => {
  const isDark = useIsDark();
  const theme: ToasterProps["theme"] = isDark ? "dark" : "light";

  return (
    <Sonner
      theme={theme as ToasterProps["theme"]}
      className="toaster group"
      icons={{
        success: <CircleCheckIcon className="size-4" />,
        info: <InfoIcon className="size-4" />,
        warning: <TriangleAlertIcon className="size-4" />,
        error: <OctagonXIcon className="size-4" />,
        loading: <Loader2Icon className="size-4 animate-spin" />,
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      toastOptions={{
        classNames: {
          toast:
            "group toast group-[.toaster]:bg-background group-[.toaster]:text-foreground group-[.toaster]:border-border shadow-lg",
          description: "group-[.toast]:text-muted-foreground",
          actionButton:
            "group-[.toast]:bg-primary group-[.toast]:text-primary-foreground",
          cancelButton:
            "group-[.toast]:bg-muted group-[.toast]:text-muted-foreground",
        },
      }}
      {...props}
    />
  );
};

export { Toaster };
