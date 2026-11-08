import { CodeIcon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Code",
    to: "/$owner/$repo",
    icon: CodeIcon,
    order: 0,
    match: [
      "/$owner/$repo/tree/$",
      "/$owner/$repo/blob/$",
      "/$owner/$repo/blame/$",
      "/$owner/$repo/commits/$",
      "/$owner/$repo/commit/$sha",
      "/$owner/$repo/branches",
      "/$owner/$repo/tags",
    ],
  },
];
