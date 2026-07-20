import * as React from "react";
import ChevronLeftIcon from "lucide-react/dist/esm/icons/chevron-left";
import ChevronRightIcon from "lucide-react/dist/esm/icons/chevron-right";
import MoreHorizontalIcon from "lucide-react/dist/esm/icons/more-horizontal";
import { cn } from "@/lib/utils";
import { Button } from "@/shared/components/ui/button";

/* -------------------------------------------------------------------------- */
/* Pagination                                                                 */
/* -------------------------------------------------------------------------- */

const Pagination = ({ className, ...props }: React.ComponentProps<"nav">) => (
  <nav
    role="navigation"
    aria-label="pagination"
    data-slot="pagination"
    className={cn("flex items-center justify-center", className)}
    {...props}
  />
);

const PaginationContent = ({
  className,
  ...props
}: React.ComponentProps<"ul">) => (
  <ul
    data-slot="pagination-content"
    className={cn("flex items-center gap-1", className)}
    {...props}
  />
);

const PaginationItem = ({
  className,
  ...props
}: React.ComponentProps<"li">) => (
  <li data-slot="pagination-item" className={cn("", className)} {...props} />
);

interface PaginationButtonProps extends React.ComponentProps<typeof Button> {
  isActive?: boolean;
}

const PaginationButton = ({
  className,
  isActive,
  size = "icon-sm",
  ...props
}: PaginationButtonProps) => (
  <Button
    aria-current={isActive ? "page" : undefined}
    variant={isActive ? "default" : "ghost"}
    size={size}
    className={className}
    {...props}
  />
);

const PaginationPrevious = ({
  className,
  ...props
}: React.ComponentProps<typeof PaginationButton>) => (
  <PaginationButton
    aria-label="Go to previous page"
    size="icon-sm"
    className={className}
    {...props}
  >
    <ChevronLeftIcon />
  </PaginationButton>
);

const PaginationNext = ({
  className,
  ...props
}: React.ComponentProps<typeof PaginationButton>) => (
  <PaginationButton
    aria-label="Go to next page"
    size="icon-sm"
    className={className}
    {...props}
  >
    <ChevronRightIcon />
  </PaginationButton>
);

const PaginationEllipsis = ({
  className,
  ...props
}: React.ComponentProps<"span">) => (
  <span
    data-slot="pagination-ellipsis"
    aria-hidden
    className={cn(
      "flex size-8 items-center justify-center text-muted-foreground",
      className
    )}
    {...props}
  >
    <MoreHorizontalIcon className="size-4" />
  </span>
);

export {
  Pagination,
  PaginationContent,
  PaginationItem,
  PaginationButton,
  PaginationPrevious,
  PaginationNext,
  PaginationEllipsis,
};
