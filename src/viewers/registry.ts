import type { LucideIcon } from "lucide-react";
import type { FC } from "react";
import type { VizItem, VizKind } from "../types";

export interface ViewerProps {
  item: VizItem;
  /// The path to feed to `convertFileSrc`. Equals `item.abs_path` for local watches; for SSH
  /// watches, it's the local cache path returned by `fetch_remote_file`.
  displayPath: string;
}

export interface ViewerDefinition {
  kinds: VizKind[];
  icon: LucideIcon;
  label?: (kind: VizKind) => string;
  Component: FC<ViewerProps>;
}
