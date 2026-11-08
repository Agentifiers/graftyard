import { HugeiconsIcon } from "@hugeicons/react";
import {
  BubbleChatQuestionIcon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Loading03Icon,
  MinusSignCircleIcon,
  Moon02Icon,
  TimerOffIcon,
} from "@hugeicons/core-free-icons";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { cn } from "cn";
import type { TaskState } from "./api";

// Every task state GitHub's agent task API names, with its label, badge, and icon.
export const TASK_STATE = {
  queued: { label: "Queued", variant: "outline", tone: "text-muted-foreground", icon: Clock01Icon },
  in_progress: {
    label: "In progress",
    variant: "warning-light",
    tone: "text-warning",
    icon: Loading03Icon,
  },
  waiting_for_user: {
    label: "Waiting for you",
    variant: "info-light",
    tone: "text-info",
    icon: BubbleChatQuestionIcon,
  },
  completed: {
    label: "Completed",
    variant: "success-light",
    tone: "text-success",
    icon: CheckmarkCircle02Icon,
  },
  failed: {
    label: "Failed",
    variant: "destructive-light",
    tone: "text-destructive",
    icon: CancelCircleIcon,
  },
  timed_out: {
    label: "Timed out",
    variant: "destructive-outline",
    tone: "text-destructive",
    icon: TimerOffIcon,
  },
  cancelled: {
    label: "Cancelled",
    variant: "outline",
    tone: "text-muted-foreground",
    icon: MinusSignCircleIcon,
  },
  idle: { label: "Idle", variant: "secondary", tone: "text-muted-foreground", icon: Moon02Icon },
} satisfies Record<
  TaskState,
  { label: string; variant: BadgeProps["variant"]; tone: string; icon: typeof Clock01Icon }
>;

export function TaskStateIcon({ state, className }: { state: TaskState; className?: string }) {
  const s = TASK_STATE[state];
  return (
    <HugeiconsIcon
      icon={s.icon}
      strokeWidth={2}
      aria-label={s.label}
      className={cn(
        "size-4 shrink-0",
        s.tone,
        state === "in_progress" && "animate-spin",
        className,
      )}
    />
  );
}

export function TaskStateBadge({
  state,
  size = "default",
}: {
  state: TaskState;
  size?: BadgeProps["size"];
}) {
  const s = TASK_STATE[state];
  return (
    <Badge
      variant={s.variant}
      size={size}
      data-task-state={state}
      className={cn(
        (state === "failed" || state === "timed_out") && "dark:text-red-300",
        state === "in_progress" && "dark:text-amber-300",
      )}
    >
      <HugeiconsIcon icon={s.icon} strokeWidth={2} />
      {s.label}
    </Badge>
  );
}
