/// <reference types="vite/client" />

declare module "lucide-react/dist/esm/icons/*" {
  import type { ComponentType, SVGProps } from "react";
  type LucideIcon = ComponentType<SVGProps<SVGSVGElement>>;
  const Icon: LucideIcon;
  export default Icon;
}
