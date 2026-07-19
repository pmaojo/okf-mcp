"use client";

import * as React from "react";
import { Popover as BasePopover } from "@base-ui/react/popover";

import { cn } from "@/lib/utils";
import { Slot } from "@/shared/components/ui/slot";

const Popover = BasePopover.Root;

const PopoverTrigger = React.forwardRef<
  HTMLButtonElement,
  React.ComponentPropsWithoutRef<typeof BasePopover.Trigger> & {
    asChild?: boolean;
  }
>(({ asChild, ...props }, ref) => {
  return (
    <BasePopover.Trigger
      ref={ref}
      data-slot="popover-trigger"
      render={asChild ? <Slot.Root /> : undefined}
      {...props}
    />
  );
});
PopoverTrigger.displayName = "PopoverTrigger";

const PopoverContent = React.forwardRef<
  HTMLDivElement,
  React.ComponentPropsWithoutRef<typeof BasePopover.Popup> & {
    align?: "start" | "center" | "end";
    side?: "top" | "right" | "bottom" | "left";
    sideOffset?: number;
  }
>(
  (
    { className, align = "center", side = "bottom", sideOffset = 4, ...props },
    ref
  ) => {
    return (
      <BasePopover.Portal>
        <BasePopover.Positioner
          align={align}
          side={side}
          sideOffset={sideOffset}
        >
          <BasePopover.Popup
            ref={ref}
            data-slot="popover-content"
            className={cn(
              "z-50 flex w-72 origin-(--transform-origin) flex-col gap-4 rounded-md border bg-popover p-4 text-sm text-popover-foreground shadow-lg ring-1 ring-foreground/5 outline-hidden duration-100 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 dark:ring-foreground/10 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
              className
            )}
            render={(renderProps, state) => (
              <div
                {...renderProps}
                data-open={state.open ? "" : undefined}
                data-closed={!state.open ? "" : undefined}
              />
            )}
            {...props}
          />
        </BasePopover.Positioner>
      </BasePopover.Portal>
    );
  }
);
PopoverContent.displayName = "PopoverContent";

const PopoverHeader = ({
  className,
  ...props
}: React.ComponentProps<"div">) => {
  return (
    <div
      data-slot="popover-header"
      className={cn("flex flex-col gap-1 text-sm", className)}
      {...props}
    />
  );
};

const PopoverTitle = ({ className, ...props }: React.ComponentProps<"h2">) => {
  return (
    <div
      data-slot="popover-title"
      className={cn("font-heading text-base font-medium", className)}
      {...props}
    />
  );
};

const PopoverDescription = ({
  className,
  ...props
}: React.ComponentProps<"p">) => {
  return (
    <p
      data-slot="popover-description"
      className={cn("text-muted-foreground", className)}
      {...props}
    />
  );
};

export {
  Popover,
  PopoverContent,
  PopoverDescription,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
};
