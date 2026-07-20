import * as React from "react";

export interface SlotProps extends React.HTMLAttributes<HTMLElement> {
  children?: React.ReactNode;
}

type SlottableElement = React.ReactElement<
  React.HTMLAttributes<HTMLElement> & React.RefAttributes<HTMLElement>
>;
type SlotPropValue =
  | string
  | number
  | boolean
  | React.CSSProperties
  | React.EventHandler<React.SyntheticEvent>
  | undefined;
type SlotPropRecord = Record<string, SlotPropValue>;

const SlotComponent = React.forwardRef<HTMLElement, SlotProps>(
  (props, forwardedRef) => {
    const { children, ...slotProps } = props;

    if (React.isValidElement(children)) {
      const child = children as SlottableElement;

      return React.cloneElement(child, {
        ...mergeProps(slotProps, child.props),
        ref: composeRefs(forwardedRef, child.props.ref),
      });
    }

    return React.Children.count(children) > 1
      ? React.Children.only(null)
      : null;
  }
);

SlotComponent.displayName = "Slot";

function mergeProps(
  slotProps: React.HTMLAttributes<HTMLElement>,
  childProps: React.HTMLAttributes<HTMLElement>
) {
  const overrideProps = { ...childProps };
  const slotPropRecord = slotProps as SlotPropRecord;
  const childPropRecord = childProps as SlotPropRecord;
  const overridePropRecord = overrideProps as SlotPropRecord;

  for (const propName in childProps) {
    const slotValue = slotPropRecord[propName];
    const childValue = childPropRecord[propName];

    const isHandler = /^on[A-Z]/.test(propName);
    if (isHandler) {
      if (isEventHandler(slotValue) && isEventHandler(childValue)) {
        overridePropRecord[propName] = (event: React.SyntheticEvent) => {
          childValue(event);
          slotValue(event);
        };
      } else if (slotValue !== undefined) {
        overridePropRecord[propName] = slotValue;
      }
    } else if (propName === "style") {
      overrideProps.style = {
        ...(isStyle(slotValue) ? slotValue : undefined),
        ...(isStyle(childValue) ? childValue : undefined),
      };
    } else if (propName === "className") {
      overrideProps.className = [slotValue, childValue]
        .filter(Boolean)
        .join(" ");
    }
  }

  return { ...slotProps, ...overrideProps };
}

function isEventHandler(
  value: SlotPropValue
): value is React.EventHandler<React.SyntheticEvent> {
  return typeof value === "function";
}

function isStyle(value: SlotPropValue): value is React.CSSProperties {
  return typeof value === "object" && value !== null;
}

function composeRefs<T>(...refs: Array<React.Ref<T> | undefined>) {
  return (node: T | null) => {
    refs.forEach((ref) => {
      if (typeof ref === "function") {
        ref(node);
      } else if (ref != null) {
        ref.current = node;
      }
    });
  };
}

export const Slot = SlotComponent as React.ForwardRefExoticComponent<
  SlotProps & React.RefAttributes<HTMLElement>
> & {
  Root: typeof SlotComponent;
};

Slot.Root = SlotComponent;
