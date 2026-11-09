import {
  ComputerIcon,
  DatabaseIcon,
  Key01Icon,
  PlayCircleIcon,
  ServerStack01Icon,
  Settings02Icon,
} from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

const REPO_GROUP = "Code and automation";
const ORG_GROUP = "Code, planning, and automation";

export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Actions",
    to: "/$owner/$repo/actions",
    icon: PlayCircleIcon,
    order: 30,
    match: [
      "/$owner/$repo/actions/workflows/$workflowId",
      "/$owner/$repo/actions/runs/$runId",
      "/$owner/$repo/actions/runs/$runId/job/$jobId",
    ],
  },
  {
    slot: "repo-settings",
    group: REPO_GROUP,
    label: "Actions",
    to: "/$owner/$repo/settings/actions/general",
    icon: Settings02Icon,
    order: 20,
  },
  {
    slot: "repo-settings",
    group: REPO_GROUP,
    label: "Secrets and variables",
    to: "/$owner/$repo/settings/secrets/actions",
    icon: Key01Icon,
    order: 21,
  },
  {
    slot: "repo-settings",
    group: REPO_GROUP,
    label: "Runners",
    to: "/$owner/$repo/settings/actions/runners",
    icon: ComputerIcon,
    order: 22,
  },
  {
    slot: "repo-settings",
    group: REPO_GROUP,
    label: "Caches",
    to: "/$owner/$repo/settings/actions/caches",
    icon: DatabaseIcon,
    order: 23,
  },
  {
    slot: "org-settings",
    group: ORG_GROUP,
    label: "Actions secrets",
    to: "/orgs/$org/settings/secrets/actions",
    icon: Key01Icon,
    order: 20,
  },
  {
    slot: "org-settings",
    group: ORG_GROUP,
    label: "Actions variables",
    to: "/orgs/$org/settings/variables/actions",
    icon: Key01Icon,
    order: 21,
  },
  {
    slot: "org-settings",
    group: ORG_GROUP,
    label: "Runners",
    to: "/orgs/$org/settings/actions/runners",
    icon: ComputerIcon,
    order: 22,
  },
  {
    slot: "org-settings",
    group: ORG_GROUP,
    label: "Runner groups",
    to: "/orgs/$org/settings/actions/runner-groups",
    icon: ServerStack01Icon,
    order: 23,
  },
];
