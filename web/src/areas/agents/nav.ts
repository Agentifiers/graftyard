import { McpServerIcon, Robot01Icon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Agents",
    to: "/$owner/$repo/agents",
    icon: Robot01Icon,
    order: 40,
    match: ["/$owner/$repo/_graftyard/sessions/$uuid"],
  },
  {
    slot: "repo-settings",
    label: "Agents",
    to: "/$owner/$repo/settings/agents",
    icon: Robot01Icon,
    order: 25,
    group: "Code and automation",
  },
  {
    slot: "org-settings",
    label: "Agents",
    to: "/orgs/$org/settings/agents",
    icon: Robot01Icon,
    order: 26,
    group: "Code, planning, and automation",
  },
  {
    slot: "user-settings",
    label: "MCP clients",
    to: "/settings/mcp",
    icon: McpServerIcon,
    order: 60,
    group: "Integrations",
  },
];
