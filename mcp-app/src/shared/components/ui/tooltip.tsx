"use client";

import * as React from "react";
import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";

import { cn } from "@/lib/utils";
import { Slot } from "@/shared/components/ui/slot";

const TooltipProvider = ({
  delay,
  delayDuration,
  ...props
}: React.ComponentPropsWithoutRef<typeof BaseTooltip.Provider> & {
  delayDuration?: number; // fallback compatibility
}) => {
  return (
    <BaseTooltip.Provider delay={delay ?? delayDuration ?? 0} {...props} />
  );
};

const Tooltip = BaseTooltip.Root;

const TooltipTrigger = React.forwardRef<
  HTMLButtonElement,
  React.ComponentPropsWithoutRef<typeof BaseTooltip.Trigger> & {
    asChild?: boolean;
  }
>(({ asChild, ...props }, ref) => {
  return (
    <BaseTooltip.Trigger
      ref={ref}
      data-slot="tooltip-trigger"
      render={asChild ? <Slot.Root /> : undefined}
      {...props}
    />
  );
});
TooltipTrigger.displayName = "TooltipTrigger";

const TooltipContent = React.forwardRef<
  HTMLDivElement,
  React.ComponentPropsWithoutRef<typeof BaseTooltip.Popup> & {
    sideOffset?: number;
    align?: "start" | "center" | "end";
    side?: "top" | "right" | "bottom" | "left";
  }
>(
  (
    {
      className,
      sideOffset = 0,
      align = "center",
      side = "top",
      children,
      ...props
    },
    ref
  ) => {
    return (
      <BaseTooltip.Portal>
        <BaseTooltip.Positioner
          sideOffset={sideOffset}
          align={align}
          side={side}
        >
          <BaseTooltip.Popup
            ref={ref}
            data-slot="tooltip-content"
            className={cn(
              "z-50 inline-flex w-fit max-w-xs origin-(--transform-origin) items-center gap-1.5 rounded-xl bg-foreground px-3 py-1.5 text-xs text-background has-data-[slot=kbd]:pr-1.5 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-lg data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-95 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
              className
            )}
            render={(renderProps, state) => (
              <div
                {...renderProps}
                data-state={state.open ? "delayed-open" : "closed"}
                data-open={state.open ? "" : undefined}
                data-closed={!state.open ? "" : undefined}
              />
            )}
            {...props}
          >
            {children}
            <BaseTooltip.Arrow className="z-50 size-2.5 translate-y-[calc(-50%_-_2px)] rotate-45 rounded-[2px] bg-foreground fill-foreground data-[side=left]:translate-x-[-1.5px] data-[side=right]:translate-x-[1.5px]" />
          </BaseTooltip.Popup>
        </BaseTooltip.Positioner>
      </BaseTooltip.Portal>
    );
  }
);
TooltipContent.displayName = "TooltipContent";

export { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger };
