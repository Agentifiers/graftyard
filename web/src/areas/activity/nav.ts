import { DashboardSpeed01Icon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "user-menu",
    label: "API status",
    to: "/rate-limit",
    icon: DashboardSpeed01Icon,
    order: 30,
    group: "You",
  },
];
