import { useState, type ReactNode } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Comment01Icon,
  Delete02Icon,
  Edit02Icon,
  EyeIcon,
  GitBranchIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
  GitPullRequestDraftIcon,
  GitPullRequestIcon,
  RefreshIcon,
  UserAdd01Icon,
  UserRemove01Icon,
  Clock01Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Conversation, type TimelineEvent } from "@/components/conversation";
import { Ago } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "cn";
import { BranchName } from "./bits";
import { useDismissReview, WriteError } from "./mutations";
import { pullEventsQuery, pullQuery, type Pull } from "./queries";
import type { ReviewModel } from "./review-data";
import { ThreadView } from "./threads";
import type { PullEvent } from "./ui-api";

type Review = Schemas["pull-request-review"];

const REVIEW_LOOK: Record<string, { icon: typeof EyeIcon; tone: string; verb: string }> = {
  APPROVED: { icon: CheckmarkCircle02Icon, tone: "text-success", verb: "approved these changes" },
  CHANGES_REQUESTED: {
    icon: CancelCircleIcon,
    tone: "text-destructive",
    verb: "requested changes",
  },
  COMMENTED: { icon: EyeIcon, tone: "text-muted-foreground", verb: "reviewed" },
  DISMISSED: { icon: EyeIcon, tone: "text-muted-foreground", verb: "reviewed" },
};

// The Conversation tab: the issue comments the issues area owns, plus this area's
// reviews, review threads, and PR events, on one rail through Conversation's `events`.
export function ConversationTab({
  pull,
  model,
  mergeBox,
}: {
  pull: Pull;
  model: ReviewModel;
  mergeBox: ReactNode;
}) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const { data: events } = useSuspenseQuery(pullEventsQuery(pull));
  const open = pr.state === "open";

  const threadsByReview = new Map<number, typeof model.threads>();
  for (const t of model.threads) {
    if (t.pending) continue;
    const id = t.root.pull_request_review_id ?? 0;
    threadsByReview.set(id, [...(threadsByReview.get(id) ?? []), t]);
  }

  const rail: TimelineEvent[] = [
    ...model.reviews
      .filter(
        (r): r is Review & { submitted_at: string } =>
          r.state !== "PENDING" && Boolean(r.submitted_at),
      )
      // A bare reply shows inside its thread, not as its own review entry.
      .filter((r) => r.body || r.state !== "COMMENTED" || threadsByReview.has(r.id))
      .map((r) => ({
        key: `review-${r.id}`,
        at: r.submitted_at,
        author: r.user,
        node: (
          <ReviewEntry
            pull={pull}
            review={r}
            mine={r.user?.login === model.viewer.login}
            open={open}
          >
            {(threadsByReview.get(r.id) ?? []).map((t) => (
              <ThreadView
                key={t.root.id}
                pull={pull}
                thread={t}
                viewer={model.viewer.login}
                showContext
                open={open}
              />
            ))}
          </ReviewEntry>
        ),
      })),
    // A thread whose review is not listed still shows, on its own, at its first comment: the
    // edge gives no single-comment review to a commenter whose verdict approves or requests
    // changes, nor to a comment made before it gave them.
    ...[...threadsByReview]
      .filter(([id]) => !model.reviews.some((r) => r.id === id))
      .flatMap(([, ts]) => ts)
      .map((t) => ({
        key: `thread-${t.root.id}`,
        at: t.root.created_at,
        author: t.root.user,
        node: (
          <ThreadView pull={pull} thread={t} viewer={model.viewer.login} showContext open={open} />
        ),
      })),
    ...events.map((e) => ({
      key: `event-${e.id}`,
      at: e.created_at,
      author: e.actor,
      node: <EventLine event={e} />,
    })),
  ];

  return (
    <div className="flex flex-col gap-4">
      <Conversation
        {...pull}
        opening={{
          author: pr.user,
          at: pr.created_at,
          body: pr.body || "_No description provided._",
        }}
        events={rail}
        composer={!pr.locked}
      />
      {mergeBox}
    </div>
  );
}

function ReviewEntry({
  pull,
  review,
  mine,
  open,
  children,
}: {
  pull: Pull;
  review: Review & { submitted_at: string };
  mine: boolean;
  open: boolean;
  children: ReactNode;
}) {
  const look = REVIEW_LOOK[review.state] ?? REVIEW_LOOK.COMMENTED;
  const dismissable = open && (review.state === "APPROVED" || review.state === "CHANGES_REQUESTED");
  return (
    <div className="flex flex-col gap-2" data-review={review.id} data-review-state={review.state}>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <HugeiconsIcon icon={look.icon} strokeWidth={2} className={cn("size-4", look.tone)} />
        <span className="font-semibold">{review.user?.login}</span>
        <span className="text-muted-foreground">
          {look.verb} <Ago at={review.submitted_at} />
        </span>
        {review.state === "DISMISSED" && (
          <Badge variant="outline" size="sm">
            Dismissed
          </Badge>
        )}
        {dismissable && !mine && <DismissButton pull={pull} reviewId={review.id} />}
      </div>
      {review.body && (
        <div
          className={cn("bg-card rounded-sm border", review.state === "DISMISSED" && "opacity-70")}
        >
          <MarkdownBody text={review.body} className="px-3 py-1" />
        </div>
      )}
      {children}
    </div>
  );
}

function DismissButton({ pull, reviewId }: { pull: Pull; reviewId: number }) {
  const dismiss = useDismissReview(pull);
  const [open, setOpen] = useState(false);
  const form = useForm({
    defaultValues: { message: "" },
    onSubmit: async ({ value, formApi }) => {
      try {
        await dismiss.mutateAsync({ reviewId, message: value.message.trim() });
        setOpen(false);
      } catch (e) {
        if (e instanceof WriteError)
          formApi.setFieldMeta("message", (m) => ({
            ...m,
            errorMap: { ...m.errorMap, onServer: e.message },
          }));
      }
    },
  });
  if (!open)
    return (
      <Button variant="ghost" size="xs" className="ml-auto" onClick={() => setOpen(true)}>
        Dismiss review
      </Button>
    );
  return (
    <form
      className="ml-auto flex w-full flex-wrap items-start gap-1.5 sm:w-auto"
      data-testid="dismiss-form"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="message"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Say why you are dismissing it."),
        }}
      >
        {(f) => (
          <div className="flex flex-col">
            <Input
              autoFocus
              aria-label="Dismissal message"
              placeholder="Why dismiss this review?"
              className="h-7 w-64 rounded-sm text-xs"
              value={f.state.value}
              onChange={(e) => f.handleChange(e.target.value)}
            />
            {f.state.meta.errors.length > 0 && (
              <span className="text-destructive text-xs" role="alert">
                {f.state.meta.errors.join(", ")}
              </span>
            )}
          </div>
        )}
      </form.Field>
      <Button type="submit" size="xs" variant="destructive" disabled={dismiss.isPending}>
        Dismiss
      </Button>
      <Button type="button" size="xs" variant="ghost" onClick={() => setOpen(false)}>
        Cancel
      </Button>
    </form>
  );
}

const EVENT_ICON: Record<PullEvent["event"], typeof EyeIcon> = {
  merged: GitMergeIcon,
  closed: GitPullRequestClosedIcon,
  reopened: GitPullRequestIcon,
  ready_for_review: EyeIcon,
  convert_to_draft: GitPullRequestDraftIcon,
  renamed: Edit02Icon,
  base_ref_changed: GitBranchIcon,
  head_ref_deleted: Delete02Icon,
  head_ref_restored: GitBranchIcon,
  branch_updated: RefreshIcon,
  review_requested: UserAdd01Icon,
  review_request_removed: UserRemove01Icon,
  review_dismissed: Comment01Icon,
  auto_merge_enabled: Clock01Icon,
  auto_merge_disabled: Clock01Icon,
};

function eventText(e: PullEvent): ReactNode {
  switch (e.event) {
    case "merged":
      return (
        <>
          merged commit <code className="font-mono">{e.commit_id.slice(0, 7)}</code> into{" "}
          <BranchName>{e.base_ref}</BranchName>
          {e.merge_method && ` (${e.merge_method})`}
        </>
      );
    case "closed":
      return "closed this";
    case "reopened":
      return "reopened this";
    case "ready_for_review":
      return "marked this pull request as ready for review";
    case "convert_to_draft":
      return "marked this pull request as draft";
    case "renamed":
      return (
        <>
          changed the title <s>{e.rename.from}</s> <b>{e.rename.to}</b>
        </>
      );
    case "base_ref_changed":
      return (
        <>
          changed the base branch from <BranchName>{e.base.from}</BranchName> to{" "}
          <BranchName>{e.base.to}</BranchName>
        </>
      );
    case "head_ref_deleted":
      return (
        <>
          deleted the <BranchName>{e.ref}</BranchName> branch
        </>
      );
    case "head_ref_restored":
      return (
        <>
          restored the <BranchName>{e.ref}</BranchName> branch
        </>
      );
    case "branch_updated":
      return (
        <>
          merged <BranchName>{e.base_ref}</BranchName> into this branch (
          <code className="font-mono">{e.commit_id.slice(0, 7)}</code>)
        </>
      );
    case "review_requested":
    case "review_request_removed": {
      const who =
        e.requested_reviewer?.login ??
        (e.requested_team ? `graftyard/${e.requested_team.slug}` : "someone");
      return e.event === "review_requested"
        ? `requested a review from ${who}`
        : `removed the review request for ${who}`;
    }
    case "review_dismissed":
      return (
        <>
          dismissed {e.reviewer.login}'s review: <i>{e.dismissal_message}</i>
        </>
      );
    case "auto_merge_enabled":
      return `enabled auto-merge (${e.merge_method})`;
    case "auto_merge_disabled":
      return "disabled auto-merge";
  }
}

function EventLine({ event }: { event: PullEvent }) {
  return (
    <p
      className="text-muted-foreground flex flex-wrap items-center gap-1.5 py-1 text-sm"
      data-event={event.event}
    >
      <HugeiconsIcon
        icon={EVENT_ICON[event.event]}
        strokeWidth={2}
        className={cn(
          "size-4",
          event.event === "merged" && "text-info",
          event.event === "closed" && "text-destructive",
        )}
      />
      <span className="text-foreground font-semibold">{event.actor.login}</span>
      {eventText(event)}
      <Ago at={event.created_at} />
    </p>
  );
}
