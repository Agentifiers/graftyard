import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";
import {
  CheckListIcon,
  Comment01Icon,
  GitCommitIcon,
  GitPullRequestIcon,
  RecordIcon,
  SecurityCheckIcon,
  Tag01Icon,
} from "@hugeicons/core-free-icons";
import type { Thread } from "./queries";

// What a notification is about, keyed by GHES's subject.type, and where it opens.

const SUBJECTS: Record<string, { icon: IconSvgElement; label: string; tone: string }> = {
  Issue: { icon: RecordIcon, label: "Issue", tone: "text-success" },
  PullRequest: { icon: GitPullRequestIcon, label: "Pull request", tone: "text-success" },
  Release: { icon: Tag01Icon, label: "Release", tone: "text-info" },
  Commit: { icon: GitCommitIcon, label: "Commit", tone: "text-muted-foreground" },
  CheckSuite: { icon: CheckListIcon, label: "Check suite", tone: "text-destructive" },
  Discussion: { icon: Comment01Icon, label: "Discussion", tone: "text-info" },
  RepositoryVulnerabilityAlert: {
    icon: SecurityCheckIcon,
    label: "Security alert",
    tone: "text-warning-foreground",
  },
};

export function SubjectIcon({ type }: { type: string }) {
  const s = SUBJECTS[type] ?? SUBJECTS.Issue;
  return (
    <HugeiconsIcon
      icon={s.icon}
      strokeWidth={2}
      className={`size-4 shrink-0 ${s.tone}`}
      aria-label={s.label}
    />
  );
}

export const subjectLabel = (type: string) => SUBJECTS[type]?.label ?? type;

// Why GHES sent the notification, in the words its inbox uses.
export const REASONS: Record<string, string> = {
  assign: "Assigned",
  author: "Author",
  comment: "Comment",
  ci_activity: "CI activity",
  invitation: "Invitation",
  manual: "Subscribed",
  mention: "Mention",
  review_requested: "Review requested",
  security_alert: "Security alert",
  state_change: "State change",
  subscribed: "Watching",
  team_mention: "Team mention",
};

export const reasonLabel = (r: string) => REASONS[r] ?? r;

// The app route a thread opens, from the API URL GHES puts in subject.url.
export type SubjectLink =
  | { to: "/$owner/$repo/issues/$number"; params: { owner: string; repo: string; number: number } }
  | { to: "/$owner/$repo/pull/$number"; params: { owner: string; repo: string; number: number } }
  | { to: "/$owner/$repo/releases"; params: { owner: string; repo: string } }
  | { to: "/$owner/$repo/actions"; params: { owner: string; repo: string } }
  | { to: "/$owner/$repo"; params: { owner: string; repo: string } };

export function subjectLink(t: Thread): SubjectLink {
  const [owner, repo] = t.repository.full_name.split("/");
  const tail = t.subject.url.slice(t.repository.url.length + 1).split("/");
  const n = Number(tail[1]);
  if (tail[0] === "issues" && n)
    return { to: "/$owner/$repo/issues/$number", params: { owner, repo, number: n } };
  if (tail[0] === "pulls" && n)
    return { to: "/$owner/$repo/pull/$number", params: { owner, repo, number: n } };
  if (tail[0] === "releases") return { to: "/$owner/$repo/releases", params: { owner, repo } };
  if (tail[0] === "check-suites") return { to: "/$owner/$repo/actions", params: { owner, repo } };
  return { to: "/$owner/$repo", params: { owner, repo } };
}
