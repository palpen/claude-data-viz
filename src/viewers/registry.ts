import type { LucideIcon } from "lucide-react";
import type { FC } from "react";
import type { VizItem, VizKind } from "../types";

export interface ViewerDefinition {
  kinds: VizKind[];
  icon: LucideIcon;
  label?: (kind: VizKind) => string;
  Component: FC<{ item: VizItem }>;
}
