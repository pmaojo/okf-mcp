"use client";

import * as React from "react";
import { Accordion as BaseAccordion } from "@base-ui/react/accordion";
import ChevronDownIcon from "lucide-react/dist/esm/icons/chevron-down";

import { cn } from "@/lib/utils";

type AccordionValue = string | string[];
type BaseAccordionRootProps = React.ComponentPropsWithoutRef<
  typeof BaseAccordion.Root<string>
>;
type AccordionProps = Omit<
  BaseAccordionRootProps,
  "value" | "defaultValue" | "onValueChange" | "multiple"
> & {
  type?: "single" | "multiple";
  collapsible?: boolean;
  value?: AccordionValue;
  defaultValue?: AccordionValue;
  onValueChange?: (value: AccordionValue) => void;
};

function toBaseValue(value: AccordionValue | undefined): string[] | undefined {
  return typeof value === "string" ? [value] : value;
}

function fromBaseValue(value: string[], multiple: boolean): AccordionValue {
  return multiple ? value : (value[0] ?? "");
}

const Accordion = React.forwardRef<HTMLDivElement, AccordionProps>(
  ({ className, type, value, defaultValue, onValueChange, ...props }, ref) => {
    const multiple = type === "multiple";
    return (
      <BaseAccordion.Root
        ref={ref}
        data-slot="accordion"
        multiple={multiple}
        value={toBaseValue(value)}
        defaultValue={toBaseValue(defaultValue)}
        onValueChange={
          onValueChange
            ? (nextValue) => onValueChange(fromBaseValue(nextValue, multiple))
            : undefined
        }
        className={cn("w-full", className)}
        {...props}
      />
    );
  }
);
Accordion.displayName = "Accordion";

const AccordionItem = React.forwardRef<
  HTMLDivElement,
  React.ComponentPropsWithoutRef<typeof BaseAccordion.Item>
>(({ className, ...props }, ref) => {
  return (
    <BaseAccordion.Item
      ref={ref}
      data-slot="accordion-item"
      className={cn("border-b border-border", className)}
      {...props}
    />
  );
});
AccordionItem.displayName = "AccordionItem";

const AccordionTrigger = React.forwardRef<
  HTMLButtonElement,
  React.ComponentPropsWithoutRef<typeof BaseAccordion.Trigger>
>(({ className, children, ...props }, ref) => {
  return (
    <BaseAccordion.Header data-slot="accordion-header" className="flex w-full">
      <BaseAccordion.Trigger
        ref={ref}
        data-slot="accordion-trigger"
        className={cn(
          "group/accordion-trigger flex flex-1 items-center justify-between gap-4 py-4 text-left text-lg leading-7 font-medium text-foreground transition-colors hover:text-foreground/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:rounded-lg disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
          className
        )}
        render={(renderProps, state) => (
          <button
            {...renderProps}
            data-state={state.open ? "open" : "closed"}
            data-open={state.open ? "" : undefined}
            data-closed={!state.open ? "" : undefined}
          />
        )}
        {...props}
      >
        <span className="flex-1">{children}</span>
        <ChevronDownIcon className="size-5 transition-transform duration-200 group-data-[state=open]/accordion-trigger:rotate-180" />
      </BaseAccordion.Trigger>
    </BaseAccordion.Header>
  );
});
AccordionTrigger.displayName = "AccordionTrigger";

const AccordionContent = React.forwardRef<
  HTMLDivElement,
  React.ComponentPropsWithoutRef<typeof BaseAccordion.Panel>
>(({ className, children, ...props }, ref) => {
  return (
    <BaseAccordion.Panel
      ref={ref}
      data-slot="accordion-content"
      className={cn(
        "overflow-hidden text-base leading-6 text-foreground/90 data-[state=closed]:animate-out data-[state=open]:animate-in data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
        className
      )}
      render={(renderProps, state) => (
        <div
          {...renderProps}
          data-state={state.open ? "open" : "closed"}
          data-open={state.open ? "" : undefined}
          data-closed={!state.open ? "" : undefined}
        />
      )}
      {...props}
    >
      <div className="pb-4">{children}</div>
    </BaseAccordion.Panel>
  );
});
AccordionContent.displayName = "AccordionContent";

export { Accordion, AccordionContent, AccordionItem, AccordionTrigger };
