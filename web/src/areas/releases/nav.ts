import { CloudServerIcon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "repo-settings",
    label: "Environments",
    to: "/$owner/$repo/settings/environments",
    icon: CloudServerIcon,
    order: 30,
    group: "Code and automation",
    match: ["/$owner/$repo/settings/environments/$name"],
  },
];
