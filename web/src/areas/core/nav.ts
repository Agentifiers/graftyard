import { CodeIcon, GitPullRequestIcon, RecordIcon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  { slot: "repo", label: "Code", to: "/$owner/$repo", icon: CodeIcon, order: 0 },
  {
    slot: "repo",
    label: "Issues",
    to: "/$owner/$repo/issues",
    icon: RecordIcon,
    order: 10,
    match: ["/$owner/$repo/issues/$number"],
  },
  {
    slot: "repo",
    label: "Pull requests",
    to: "/$owner/$repo/pulls",
    icon: GitPullRequestIcon,
    order: 20,
    match: ["/$owner/$repo/pull/$number"],
  },
];
