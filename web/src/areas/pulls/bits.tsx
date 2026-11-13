import { useState, type ReactNode } from "react";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Comment01Icon,
  Loading03Icon,
  MinusSignCircleIcon,
  Settings02Icon,
  Tick02Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { cn } from "cn";
import type { CheckState, ReviewDecision } from "./merge-rules";

type IconSvg = typeof Tick02Icon;

// The look of a check state, keyed by state, so every place draws it the same way.
export const CHECK_LOOK: Record<CheckState, { icon: IconSvg; tone: string; label: string }> = {
  success: { icon: CheckmarkCircle02Icon, tone: "text-success", label: "Checks passed" },
  failure: { icon: CancelCircleIcon, tone: "text-destructive", label: "Checks failed" },
  pending: { icon: Loading03Icon, tone: "text-warning animate-spin", label: "Checks running" },
  none: { icon: MinusSignCircleIcon, tone: "text-muted-foreground", label: "No checks" },
};

type Run = Schemas["check-run"];

export function runLook(run: Pick<Run, "status" | "conclusion">) {
  if (run.status === "in_progress")
    return { icon: Loading03Icon, tone: "text-warning animate-spin", label: "In progress" };
  if (run.status !== "completed")
    return { icon: Clock01Icon, tone: "text-muted-foreground", label: "Queued" };
  switch (run.conclusion) {
    case "success":
      return { icon: CheckmarkCircle02Icon, tone: "text-success", label: "Success" };
    case "failure":
    case "timed_out":
    case "action_required":
      return {
        icon: CancelCircleIcon,
        tone: "text-destructive",
        label: run.conclusion === "failure" ? "Failure" : run.conclusion.replace("_", " "),
      };
    case "cancelled":
      return { icon: Alert02Icon, tone: "text-muted-foreground", label: "Cancelled" };
    default:
      return {
        icon: MinusSignCircleIcon,
        tone: "text-muted-foreground",
        label: run.conclusion ?? "Neutral",
      };
  }
}

export function StateIconOf({
  icon,
  tone,
  label,
  className,
}: {
  icon: IconSvg;
  tone: string;
  label: string;
  className?: string;
}) {
  return (
    <HugeiconsIcon
      icon={icon}
      strokeWidth={2}
      className={cn("size-4 shrink-0", tone, className)}
      aria-label={label}
    />
  );
}

export const REVIEW_LOOK: Record<
  ReviewDecision,
  { label: string; variant: "success-light" | "destructive-light" | "warning-light" }
> = {
  APPROVED: { label: "Approved", variant: "success-light" },
  CHANGES_REQUESTED: { label: "Changes requested", variant: "destructive-light" },
  REVIEW_REQUIRED: { label: "Review required", variant: "warning-light" },
};

export function ReviewBadge({ decision }: { decision: ReviewDecision }) {
  const look = REVIEW_LOOK[decision];
  return (
    <Badge
      variant={look.variant}
      size="sm"
      className={decision === "CHANGES_REQUESTED" ? "dark:text-red-300" : undefined}
    >
      {look.label}
    </Badge>
  );
}

export function CommentCount({ n }: { n: number }) {
  if (n === 0) return null;
  return (
    <span
      className="text-muted-foreground flex items-center gap-1 text-xs tabular-nums"
      title={`${n} comments`}
    >
      <HugeiconsIcon icon={Comment01Icon} strokeWidth={2} className="size-3.5" />
      {n}
    </span>
  );
}

export function BranchName({ children }: { children: ReactNode }) {
  return (
    <code className="bg-info/10 text-foreground rounded-sm border px-1.5 py-0.5 font-mono text-xs">
      {children}
    </code>
  );
}

// A sidebar heading with a gear that opens a filterable checklist, GitHub-style.
export function PickerSection<T>({
  title,
  label,
  items,
  keyOf,
  render,
  selected,
  onToggle,
  children,
  disabled,
  failed,
  onOpenChange,
}: {
  title: string;
  label: string;
  // Undefined while the candidates load; the caller loads them when the picker opens.
  items: T[] | undefined;
  keyOf: (t: T) => string;
  render: (t: T) => ReactNode;
  selected: (t: T) => boolean;
  onToggle: (t: T, on: boolean) => void;
  children: ReactNode;
  disabled?: boolean;
  // The candidates could not be read: the picker says so, the section still renders.
  failed?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const [q, setQ] = useState("");
  const shown = (items ?? []).filter((t) => keyOf(t).toLowerCase().includes(q.toLowerCase()));
  return (
    <section className="flex flex-col gap-1.5 border-b pb-3 last:border-b-0" data-sidebar={title}>
      <div className="flex items-center justify-between">
        <h3 className="text-muted-foreground text-xs font-semibold">{title}</h3>
        <Popover onOpenChange={(open) => onOpenChange?.(open)}>
          <PopoverTrigger
            disabled={disabled}
            render={
              <Button variant="ghost" size="icon-xs" aria-label={label}>
                <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} />
              </Button>
            }
          />
          <PopoverContent align="end" className="w-64 rounded-sm p-1.5">
            <Input
              autoFocus
              placeholder="Filter"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              className="h-7 rounded-sm text-xs"
            />
            <ul
              className="flex max-h-64 flex-col overflow-y-auto"
              role="listbox"
              aria-label={label}
            >
              {shown.map((t) => {
                const on = selected(t);
                return (
                  <li key={keyOf(t)} role="presentation">
                    <button
                      type="button"
                      role="option"
                      aria-selected={on}
                      className="hover:bg-muted flex w-full items-center gap-2 rounded-sm px-1.5 py-1 text-left text-sm"
                      onClick={() => onToggle(t, !on)}
                    >
                      <span className="flex size-4 items-center justify-center">
                        {on && (
                          <HugeiconsIcon icon={Tick02Icon} strokeWidth={2} className="size-3.5" />
                        )}
                      </span>
                      {render(t)}
                    </button>
                  </li>
                );
              })}
              {shown.length === 0 && (
                <li
                  role="presentation"
                  className={cn(
                    "px-1.5 py-2 text-xs",
                    failed ? "text-destructive" : "text-muted-foreground",
                  )}
                  data-picker-state={failed ? "failed" : items ? "empty" : "loading"}
                >
                  {failed ? "Could not load the list." : items ? "Nothing matches." : "Loading…"}
                </li>
              )}
            </ul>
          </PopoverContent>
        </Popover>
      </div>
      {children}
    </section>
  );
}
