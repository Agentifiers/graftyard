import { RecordIcon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Issues",
    to: "/$owner/$repo/issues",
    icon: RecordIcon,
    order: 10,
    match: ["/$owner/$repo/issues/$number", "/$owner/$repo/issues/new"],
  },
];
