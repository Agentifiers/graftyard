import { HugeiconsIcon } from "@hugeicons/react";
import {
  CheckmarkCircle02Icon,
  CircleIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
  GitPullRequestDraftIcon,
  GitPullRequestIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";

type IconSvg = typeof CircleIcon;

// Every state an issue or pull request shows, with its badge variant and icon.
const STATES = {
  open: { label: "Open", variant: "success-light", icon: CircleIcon },
  closed: { label: "Closed", variant: "info-light", icon: CheckmarkCircle02Icon },
  "pr-open": { label: "Open", variant: "success-light", icon: GitPullRequestIcon },
  "pr-draft": { label: "Draft", variant: "outline", icon: GitPullRequestDraftIcon },
  "pr-merged": { label: "Merged", variant: "info-light", icon: GitMergeIcon },
  "pr-closed": { label: "Closed", variant: "destructive-light", icon: GitPullRequestClosedIcon },
} satisfies Record<string, { label: string; variant: BadgeProps["variant"]; icon: IconSvg }>;

export type DisplayState = keyof typeof STATES;

export function issueState(issue: Pick<Schemas["issue"], "state">): DisplayState {
  return issue.state === "closed" ? "closed" : "open";
}

export function pullState(pr: {
  state: string;
  draft?: boolean;
  merged_at: string | null;
}): DisplayState {
  if (pr.merged_at) return "pr-merged";
  if (pr.state === "closed") return "pr-closed";
  return pr.draft ? "pr-draft" : "pr-open";
}

export function StateBadge({
  state,
  size = "lg",
}: {
  state: DisplayState;
  size?: BadgeProps["size"];
}) {
  const s = STATES[state];
  return (
    <Badge
      variant={s.variant}
      size={size}
      className={state === "pr-closed" ? "dark:text-red-300" : undefined}
    >
      <HugeiconsIcon icon={s.icon} strokeWidth={2} />
      {s.label}
    </Badge>
  );
}

export function StateIcon({ state }: { state: DisplayState }) {
  const s = STATES[state];
  const tone = {
    "success-light": "text-success",
    "info-light": "text-info",
    outline: "text-muted-foreground",
    "destructive-light": "text-destructive",
  }[s.variant];
  return (
    <HugeiconsIcon
      icon={s.icon}
      strokeWidth={2}
      className={`size-4 shrink-0 ${tone}`}
      aria-label={s.label}
    />
  );
}

type Label = { name?: string; color?: string | null; description?: string | null };

// GHES sends label colours as hex without the hash.
export function LabelBadge({ label }: { label: Label | string }) {
  const l = typeof label === "string" ? { name: label, color: null } : label;
  const color = `#${l.color ?? "8b949e"}`;
  return (
    <Badge
      variant="outline"
      size="sm"
      title={typeof label === "string" ? undefined : (label.description ?? undefined)}
      style={{
        borderColor: `${color}66`,
        backgroundColor: `${color}22`,
        color: "var(--foreground)",
      }}
    >
      {l.name}
    </Badge>
  );
}

export function UserAvatar({
  user,
  size = "sm",
}: {
  user: { login: string; avatar_url: string } | null;
  size?: "sm" | "default";
}) {
  return (
    <Avatar size={size}>
      {user && <AvatarImage src={user.avatar_url} alt={user.login} />}
      <AvatarFallback>{user?.login.slice(0, 2).toUpperCase() ?? "?"}</AvatarFallback>
    </Avatar>
  );
}

const units: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 31536000],
  ["month", 2592000],
  ["day", 86400],
  ["hour", 3600],
  ["minute", 60],
];
const rtf = new Intl.RelativeTimeFormat("en", { numeric: "auto" });

export function Ago({ at }: { at: string }) {
  const seconds = (new Date(at).getTime() - Date.now()) / 1000;
  const [unit, size] = units.find(([, s]) => Math.abs(seconds) >= s) ?? ["second", 1];
  return (
    <time dateTime={at} title={new Date(at).toLocaleString()}>
      {rtf.format(Math.round(seconds / size), unit)}
    </time>
  );
}
