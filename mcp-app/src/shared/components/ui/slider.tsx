"use client";

import * as React from "react";
import { Slider as BaseSlider } from "@base-ui/react/slider";

import { cn } from "@/lib/utils";

const Slider = React.forwardRef<
  HTMLDivElement,
  Omit<
    React.ComponentPropsWithoutRef<typeof BaseSlider.Root>,
    "value" | "defaultValue" | "onValueChange"
  > & {
    value?: number | number[];
    defaultValue?: number | number[];
    onValueChange?: (value: number[]) => void;
  }
>(
  (
    {
      className,
      defaultValue,
      value,
      min = 0,
      max = 100,
      onValueChange,
      ...props
    },
    ref
  ) => {
    const _values = React.useMemo(
      () =>
        Array.isArray(value)
          ? value
          : Array.isArray(defaultValue)
            ? defaultValue
            : typeof value === "number"
              ? [value]
              : typeof defaultValue === "number"
                ? [defaultValue]
                : [min],
      [value, defaultValue, min]
    );

    return (
      <BaseSlider.Root
        ref={ref}
        data-slot="slider"
        defaultValue={toSliderValue(defaultValue)}
        value={toSliderValue(value)}
        min={min}
        max={max}
        className={cn(
          "relative flex w-full touch-none items-center select-none data-disabled:opacity-50 data-vertical:h-full data-vertical:min-h-40 data-vertical:w-auto data-vertical:flex-col",
          className
        )}
        {...props}
        onValueChange={(val) => {
          if (onValueChange) {
            const arr = Array.isArray(val) ? val : [val];
            onValueChange(arr);
          }
        }}
      >
        <BaseSlider.Control className="relative flex w-full touch-none items-center select-none data-vertical:h-full data-vertical:flex-col">
          <BaseSlider.Track
            data-slot="slider-track"
            className="relative grow overflow-hidden rounded-full bg-input/90 data-horizontal:h-2 data-horizontal:w-full data-vertical:h-full data-vertical:w-2"
          >
            <BaseSlider.Indicator
              data-slot="slider-range"
              className="absolute bg-primary select-none data-horizontal:h-full data-vertical:w-full"
            />
          </BaseSlider.Track>
          {Array.from({ length: _values.length }, (_, index) => (
            <BaseSlider.Thumb
              data-slot="slider-thumb"
              key={index}
              index={index}
              className="block h-4 w-6 shrink-0 rounded-full bg-white shadow-md ring-1 ring-black/10 transition-[color,box-shadow,background-color] select-none not-dark:bg-clip-padding hover:ring-4 hover:ring-ring/30 focus-visible:ring-4 focus-visible:ring-ring/30 focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-50 data-vertical:h-6 data-vertical:w-4"
            />
          ))}
        </BaseSlider.Control>
      </BaseSlider.Root>
    );
  }
);

function toSliderValue(value: number | number[] | undefined) {
  return typeof value === "number" ? [value] : value;
}

Slider.displayName = "Slider";

export { Slider };
