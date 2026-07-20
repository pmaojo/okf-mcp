import * as React from "react";
import { cn } from "@/lib/utils";

/* -------------------------------------------------------------------------- */
/* Sidebar                                                                    */
/* -------------------------------------------------------------------------- */

const SidebarBody = ({
  className,
  ...props
}: React.ComponentProps<"aside">) => (
  <aside
    data-slot="sidebar-body"
    className={cn(
      "flex h-full w-64 shrink-0 flex-col overflow-y-auto bg-card border-r",
      className
    )}
    {...props}
  />
);

const SidebarSection = ({
  className,
  ...props
}: React.ComponentProps<"div">) => (
  <div
    data-slot="sidebar-section"
    className={cn("flex flex-col gap-0.5 px-2 py-2", className)}
    {...props}
  />
);

const SidebarLabel = ({ className, ...props }: React.ComponentProps<"p">) => (
  <p
    data-slot="sidebar-label"
    className={cn(
      "px-3 py-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground",
      className
    )}
    {...props}
  />
);

interface SidebarItemProps extends React.ComponentProps<"button"> {
  active?: boolean;
  icon?: React.ReactNode;
}

const SidebarItem = ({
  className,
  active,
  icon,
  children,
  ...props
}: SidebarItemProps) => (
  <button
    data-slot="sidebar-item"
    data-active={active || undefined}
    className={cn(
      "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
      "hover:bg-muted hover:text-foreground",
      "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
      active
        ? "bg-primary text-primary-foreground hover:bg-primary/90 hover:text-primary-foreground"
        : "text-muted-foreground",
      className
    )}
    {...props}
  >
    {icon && (
      <span
        data-slot="sidebar-item-icon"
        className="size-4 shrink-0 [&>svg]:size-4"
      >
        {icon}
      </span>
    )}
    <span data-slot="sidebar-item-label" className="flex-1 truncate text-left">
      {children}
    </span>
  </button>
);

const SidebarFooter = ({
  className,
  ...props
}: React.ComponentProps<"div">) => (
  <div
    data-slot="sidebar-footer"
    className={cn("mt-auto border-t px-2 py-3", className)}
    {...props}
  />
);

export {
  SidebarBody,
  SidebarSection,
  SidebarLabel,
  SidebarItem,
  SidebarFooter,
};
