import { Analytics01Icon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

// The code browser's own pages sit under core's Code tab, whose match list names them.
export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Insights",
    to: "/$owner/$repo/pulse",
    icon: Analytics01Icon,
    order: 80,
  },
];
