import { GitPullRequestIcon } from "@hugeicons/core-free-icons";
import type { NavEntry } from "../nav";

export const nav: NavEntry[] = [
  {
    slot: "repo",
    label: "Pull requests",
    to: "/$owner/$repo/pulls",
    icon: GitPullRequestIcon,
    order: 20,
    match: ["/$owner/$repo/pull/$number", "/$owner/$repo/compare"],
  },
];
