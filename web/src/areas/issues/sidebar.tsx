import { useState, type ReactNode } from "react";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { eq, useLiveQuery } from "@tanstack/react-db";
import { Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  LockIcon,
  Notification02Icon,
  NotificationOff02Icon,
  PinIcon,
  PinOffIcon,
  SquareUnlock02Icon,
} from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { send, toastFailure } from "./api";
import { pinKey, pinnedCollection } from "./collections";
import { useIssueChange, type LockReason } from "./issue-writes";
import { AssigneePicker, formatDue, LabelPicker, MilestonePicker } from "./pickers";
import { commentsQuery, issueQuery, subscriptionQuery } from "./queries";
import { LabelBadge, UserAvatar } from "@/components/github";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Progress } from "@/components/ui/progress";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Separator } from "@/components/ui/separator";

type Thread = { owner: string; repo: string; number: number };

function Section({
  title,
  action,
  children,
}: {
  title: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-1.5 text-sm" data-sidebar-section={title}>
      <div className="text-muted-foreground flex items-center justify-between text-xs font-semibold">
        {title}
        {action}
      </div>
      {children}
    </section>
  );
}

// The issue page's right column: one region per section, each editing through the same
// optimistic issue write. The issue read is shared with the header.
export function IssueSidebar(t: Thread) {
  return (
    <div className="flex flex-col gap-4" data-testid="issue-sidebar">
      <Region name="issue sidebar" rows={6}>
        <Metadata {...t} />
      </Region>
      <Separator />
      <Region name="participants" rows={1}>
        <Participants {...t} />
      </Region>
      <Separator />
      <Region name="notifications" rows={2}>
        <Subscription {...t} />
      </Region>
      <Separator />
      <Region name="issue actions" rows={2}>
        <Actions {...t} />
      </Region>
    </div>
  );
}

function Metadata(t: Thread) {
  const { data: issue } = useSuspenseQuery(issueQuery(t));
  const change = useIssueChange(t);
  const repo = { owner: t.owner, repo: t.repo };
  const labelNames = issue.labels.map((l) => (typeof l === "string" ? l : (l.name ?? "")));
  const assignees = issue.assignees ?? [];
  const m = issue.milestone;
  const total = m ? m.open_issues + m.closed_issues : 0;
  return (
    <div className="flex flex-col gap-4">
      <Section
        title="Assignees"
        action={
          <AssigneePicker
            {...repo}
            selected={assignees.map((u) => u.login)}
            onCommit={(users) => change.mutate({ kind: "assignees", users })}
          />
        }
      >
        {assignees.length === 0 ? (
          <AssignYourself {...t} />
        ) : (
          <ul className="flex flex-col gap-1" data-testid="assignees">
            {assignees.map((u) => (
              <li key={u.login} className="flex items-center gap-2">
                <UserAvatar user={u} />
                <span className="font-medium">{u.login}</span>
              </li>
            ))}
          </ul>
        )}
      </Section>
      <Section
        title="Labels"
        action={
          <LabelPicker
            {...repo}
            selected={labelNames}
            onCommit={(labels) => change.mutate({ kind: "labels", labels })}
          />
        }
      >
        {issue.labels.length === 0 ? (
          <span className="text-muted-foreground text-xs">None yet</span>
        ) : (
          <div className="flex flex-wrap gap-1" data-testid="labels">
            {issue.labels.map((l) => (
              <LabelBadge key={typeof l === "string" ? l : l.name} label={l} />
            ))}
          </div>
        )}
      </Section>
      <Section
        title="Milestone"
        action={
          <MilestonePicker
            {...repo}
            selected={m?.number ?? null}
            onCommit={(milestone) => change.mutate({ kind: "milestone", milestone })}
          />
        }
      >
        {m ? (
          <div className="flex flex-col gap-1" data-testid="milestone">
            <Link
              to="/$owner/$repo/milestones/$number"
              params={{ ...repo, number: m.number }}
              className="font-medium hover:underline"
            >
              {m.title}
            </Link>
            <Progress
              value={total ? (m.closed_issues / total) * 100 : 0}
              aria-label="Milestone progress"
            />
            <span className="text-muted-foreground text-xs">
              {m.due_on ? `Due ${formatDue(m.due_on)}` : "No due date"}
            </span>
          </div>
        ) : (
          <span className="text-muted-foreground text-xs">No milestone</span>
        )}
      </Section>
    </div>
  );
}

function AssignYourself(t: Thread) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const change = useIssueChange(t);
  return (
    <span className="text-muted-foreground text-xs">
      No one —{" "}
      <button
        type="button"
        className="text-primary hover:underline"
        onClick={() => change.mutate({ kind: "assignees", users: [viewer] })}
      >
        assign yourself
      </button>
    </span>
  );
}

function Participants(t: Thread) {
  const { data: issue } = useSuspenseQuery(issueQuery(t));
  const { data: comments } = useSuspenseQuery(commentsQuery(t));
  const people = new Map<string, { login: string; avatar_url: string }>();
  for (const u of [issue.user, ...comments.map((c) => c.user), ...(issue.assignees ?? [])]) {
    if (u) people.set(u.login, u);
  }
  return (
    <Section title={`${people.size} ${people.size === 1 ? "participant" : "participants"}`}>
      <div className="flex flex-wrap gap-1" data-testid="participants">
        {[...people.values()].map((u) => (
          <span key={u.login} title={u.login}>
            <UserAvatar user={u} />
          </span>
        ))}
      </div>
    </Section>
  );
}

// GHES has no per-issue subscription route; the issue's notification thread stands in,
// keyed by the issue id.
function Subscription(t: Thread) {
  const queryClient = useQueryClient();
  const { data: issue } = useSuspenseQuery(issueQuery(t));
  const q = subscriptionQuery(issue.id);
  const { data: sub } = useSuspenseQuery(q);
  const toggle = useMutation({
    mutationFn: async (subscribe: boolean) => {
      const path = { thread_id: issue.id };
      if (subscribe) {
        await send(
          api.PUT("/notifications/threads/{thread_id}/subscription", {
            params: { path },
            body: { ignored: false },
          }),
        );
      } else {
        await send(
          api.DELETE("/notifications/threads/{thread_id}/subscription", { params: { path } }),
        );
      }
    },
    onMutate: async (subscribe) => {
      await queryClient.cancelQueries({ queryKey: q.queryKey });
      const before = queryClient.getQueryData(q.queryKey);
      if (before) queryClient.setQueryData(q.queryKey, { ...before, subscribed: subscribe });
      return { before };
    },
    onError: (e, _v, ctx) => {
      queryClient.setQueryData(q.queryKey, ctx?.before);
      toastFailure("Changing your subscription", e);
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: q.queryKey }),
  });
  return (
    <Section title="Notifications">
      <Button
        variant="outline"
        size="sm"
        className="w-full"
        data-subscribed={sub.subscribed}
        onClick={() => toggle.mutate(!sub.subscribed)}
      >
        <HugeiconsIcon
          icon={sub.subscribed ? NotificationOff02Icon : Notification02Icon}
          strokeWidth={2}
          data-icon="inline-start"
        />
        {sub.subscribed ? "Unsubscribe" : "Subscribe"}
      </Button>
      <p className="text-muted-foreground text-xs">
        {sub.subscribed
          ? "You're receiving notifications because you're subscribed to this thread."
          : "You're not receiving notifications from this thread."}
      </p>
    </Section>
  );
}

const LOCK_REASONS: { value: LockReason | ""; label: string }[] = [
  { value: "", label: "Choose a reason (optional)" },
  { value: "off-topic", label: "Off-topic" },
  { value: "too heated", label: "Too heated" },
  { value: "resolved", label: "Resolved" },
  { value: "spam", label: "Spam" },
];

function Actions(t: Thread) {
  const { data: issue } = useSuspenseQuery(issueQuery(t));
  const change = useIssueChange(t);
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState<LockReason | "">("");
  const pins = useLiveQuery((q) =>
    q.from({ p: pinnedCollection() }).where(({ p }) => eq(p.key, pinKey(t))),
  );
  const pinned = (pins.data ?? []).length > 0;
  const togglePin = () => {
    const c = pinnedCollection();
    if (pinned) c.delete(pinKey(t));
    else
      c.insert({
        key: pinKey(t),
        owner: t.owner,
        repo: t.repo,
        number: t.number,
        pinnedAt: new Date().toISOString(),
      });
  };
  return (
    <div className="flex flex-col gap-1">
      {issue.locked ? (
        <Button
          variant="ghost"
          size="sm"
          className="justify-start"
          onClick={() => change.mutate({ kind: "unlock" })}
        >
          <HugeiconsIcon icon={SquareUnlock02Icon} strokeWidth={2} data-icon="inline-start" />
          Unlock conversation
        </Button>
      ) : (
        <Button variant="ghost" size="sm" className="justify-start" onClick={() => setOpen(true)}>
          <HugeiconsIcon icon={LockIcon} strokeWidth={2} data-icon="inline-start" />
          Lock conversation
        </Button>
      )}
      <Button
        variant="ghost"
        size="sm"
        className="justify-start"
        onClick={togglePin}
        data-pinned={pinned}
      >
        <HugeiconsIcon
          icon={pinned ? PinOffIcon : PinIcon}
          strokeWidth={2}
          data-icon="inline-start"
        />
        {pinned ? "Unpin issue" : "Pin issue"}
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="rounded-sm sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Lock conversation on this issue</DialogTitle>
            <DialogDescription>
              Other users can't add new comments to this issue. You and other collaborators with
              access to this repository can still leave comments that others can see.
            </DialogDescription>
          </DialogHeader>
          <label className="flex flex-col gap-1 text-sm">
            <span className="font-medium">Reason for locking</span>
            <NativeSelect
              aria-label="Reason for locking"
              value={reason}
              onChange={(e) => setReason(e.target.value as LockReason | "")}
            >
              {LOCK_REASONS.map((r) => (
                <NativeSelectOption key={r.value} value={r.value}>
                  {r.label}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </label>
          <DialogFooter>
            <Button variant="outline" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              onClick={() => {
                setOpen(false);
                change.mutate({ kind: "lock", reason: reason || null });
              }}
            >
              Lock conversation
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
