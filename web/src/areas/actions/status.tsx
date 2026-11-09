import { useEffect, useState } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  CircleDashedIcon,
  Clock01Icon,
  HourglassIcon,
  Loading03Icon,
  MinusSignCircleIcon,
  StopCircleIcon,
} from "@hugeicons/core-free-icons";
import { cn } from "cn";

// Every status and conclusion a run, job, or step can show, with its icon, tone, and
// label. A completed item shows its conclusion; anything else shows its status.
const LOOKS = {
  success: {
    icon: CheckmarkCircle02Icon,
    tone: "text-success-foreground dark:text-success",
    label: "Success",
  },
  failure: { icon: CancelCircleIcon, tone: "text-destructive", label: "Failure" },
  timed_out: { icon: CancelCircleIcon, tone: "text-destructive", label: "Timed out" },
  startup_failure: { icon: CancelCircleIcon, tone: "text-destructive", label: "Startup failure" },
  action_required: { icon: CancelCircleIcon, tone: "text-destructive", label: "Action required" },
  cancelled: { icon: StopCircleIcon, tone: "text-muted-foreground", label: "Cancelled" },
  skipped: { icon: MinusSignCircleIcon, tone: "text-muted-foreground", label: "Skipped" },
  neutral: { icon: MinusSignCircleIcon, tone: "text-muted-foreground", label: "Neutral" },
  stale: { icon: MinusSignCircleIcon, tone: "text-muted-foreground", label: "Stale" },
  in_progress: {
    icon: Loading03Icon,
    tone: "text-warning-foreground dark:text-warning animate-spin",
    label: "In progress",
  },
  queued: { icon: Clock01Icon, tone: "text-muted-foreground", label: "Queued" },
  requested: { icon: Clock01Icon, tone: "text-muted-foreground", label: "Requested" },
  pending: { icon: CircleDashedIcon, tone: "text-muted-foreground", label: "Pending" },
  waiting: {
    icon: HourglassIcon,
    tone: "text-warning-foreground dark:text-warning",
    label: "Waiting",
  },
} as const;

export type Look = keyof typeof LOOKS;

export function lookOf(item: { status: string | null; conclusion: string | null }): Look {
  const key =
    item.status === "completed" ? (item.conclusion ?? "neutral") : (item.status ?? "queued");
  return key in LOOKS ? (key as Look) : "neutral";
}

export const lookLabel = (look: Look) => LOOKS[look].label;

export function StatusIcon({
  item,
  className,
}: {
  item: { status: string | null; conclusion: string | null };
  className?: string;
}) {
  const look = LOOKS[lookOf(item)];
  return (
    <HugeiconsIcon
      icon={look.icon}
      strokeWidth={2}
      className={cn("size-4 shrink-0", look.tone, className)}
      aria-label={look.label}
      data-status={lookOf(item)}
    />
  );
}

// "1m 12s", "48s", "2h 3m".
export function formatDuration(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

// Re-renders every second while `running`, so a live duration counts up.
export function useNow(running: boolean) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!running) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [running]);
  return now;
}

export function Duration({
  start,
  end,
  running,
}: {
  start: string | null | undefined;
  end: string | null | undefined;
  running: boolean;
}) {
  const now = useNow(running);
  if (!start) return <span className="text-muted-foreground">–</span>;
  const until = end ? Date.parse(end) : running ? now : null;
  if (until === null) return <span className="text-muted-foreground">–</span>;
  return <span className="tabular-nums">{formatDuration(until - Date.parse(start))}</span>;
}

export function formatBytes(n: number) {
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${v < 10 && u > 0 ? v.toFixed(1) : Math.round(v)} ${units[u]}`;
}

// Event names as GHES's UI writes them.
export const EVENT_LABELS: Record<string, string> = {
  push: "push",
  pull_request: "pull request",
  workflow_dispatch: "manually run",
  schedule: "scheduled",
};

// The status filter's options: every value `status` takes on the runs list.
export const STATUS_FILTERS = [
  "queued",
  "in_progress",
  "waiting",
  "completed",
  "success",
  "failure",
  "cancelled",
  "skipped",
] as const;
export type StatusFilter = (typeof STATUS_FILTERS)[number];
export const isStatusFilter = (v: unknown): v is StatusFilter =>
  STATUS_FILTERS.some((s) => s === v);
