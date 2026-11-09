import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import { CheckmarkCircle02Icon, Delete02Icon, Edit02Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago, UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { useEditComment, useReply, useResolveThread, WriteError } from "./mutations";
import { parsePatch } from "./patch";
import type { Pull } from "./queries";
import type { ThreadState } from "./ui-api";

export type ReviewComment = Schemas["pull-request-review-comment"];

export type Thread = {
  root: ReviewComment;
  replies: ReviewComment[];
  resolved: boolean;
  resolvedBy: string | null;
  // The viewer's unsubmitted comments, shown only to them.
  pending: boolean;
};

// Groups review comments into threads: a root and the replies that point at it.
export function threadsOf(
  comments: ReviewComment[],
  states: ThreadState[],
  pendingIds: Set<number>,
): Thread[] {
  const byRoot = new Map<number, Thread>();
  const state = new Map(states.map((s) => [s.root_id, s]));
  for (const c of [...comments].sort((a, b) => a.created_at.localeCompare(b.created_at))) {
    const rootId = c.in_reply_to_id ?? c.id;
    const t = byRoot.get(rootId);
    if (t) t.replies.push(c);
    else if (!c.in_reply_to_id)
      byRoot.set(c.id, {
        root: c,
        replies: [],
        resolved: state.get(c.id)?.resolved ?? false,
        resolvedBy: state.get(c.id)?.resolved_by?.login ?? null,
        pending: pendingIds.has(c.pull_request_review_id ?? -1),
      });
  }
  return [...byRoot.values()];
}

export const threadAnchor = (t: Thread) => ({
  path: t.root.path,
  side: t.root.side,
  line: t.root.line ?? 0,
});

const lineLabel = (c: ReviewComment) =>
  c.start_line && c.start_line !== c.line ? `lines ${c.start_line} to ${c.line}` : `line ${c.line}`;

// The few diff lines a thread is about, for the conversation rail.
function HunkTail({ hunk }: { hunk: string }) {
  const lines = parsePatch(hunk)
    .flatMap((l) => (l.kind === "hunk" ? [] : [l]))
    .slice(-4);
  return (
    <div className="bg-muted/30 overflow-x-auto border-b font-mono text-xs leading-5">
      {lines.map((l, i) => (
        <div
          key={i}
          className={cn(
            "px-3 whitespace-pre",
            l.kind === "add" && "bg-success/12",
            l.kind === "del" && "bg-destructive/12",
          )}
        >
          {l.kind === "add" ? "+" : l.kind === "del" ? "-" : " "}
          {l.text}
        </div>
      ))}
    </div>
  );
}

export function ThreadView({
  pull,
  thread,
  viewer,
  showContext,
  open,
}: {
  pull: Pull;
  thread: Thread;
  viewer: string;
  // On the conversation rail a thread shows its file and hunk; inline it does not.
  showContext?: boolean;
  open: boolean;
}) {
  const resolve = useResolveThread(pull);
  const [expanded, setExpanded] = useState(!thread.resolved);
  const shown = expanded || !thread.resolved;
  return (
    <div
      className="bg-card overflow-hidden rounded-sm border"
      data-thread={thread.root.id}
      data-resolved={thread.resolved || undefined}
    >
      <div className="bg-muted/50 flex flex-wrap items-center gap-2 border-b px-3 py-1.5 text-xs">
        {showContext ? (
          <code className="truncate font-mono font-medium">{thread.root.path}</code>
        ) : (
          <span className="text-muted-foreground">Comment on {lineLabel(thread.root)}</span>
        )}
        {showContext && <span className="text-muted-foreground">{lineLabel(thread.root)}</span>}
        {thread.pending && (
          <Badge variant="warning-light" size="sm">
            Pending
          </Badge>
        )}
        {thread.resolved && (
          <span className="text-muted-foreground flex items-center gap-1">
            <HugeiconsIcon
              icon={CheckmarkCircle02Icon}
              strokeWidth={2}
              className="text-success size-3.5"
            />
            Resolved{thread.resolvedBy ? ` by ${thread.resolvedBy}` : ""}
          </span>
        )}
        <span className="ml-auto flex gap-1">
          {thread.resolved && (
            <Button variant="ghost" size="xs" onClick={() => setExpanded((e) => !e)}>
              {shown ? "Hide" : "Show"} conversation
            </Button>
          )}
          {!thread.pending && open && (
            <Button
              variant="outline"
              size="xs"
              disabled={resolve.isPending}
              onClick={() => {
                resolve.mutate({ rootId: thread.root.id, resolved: !thread.resolved });
                setExpanded(thread.resolved);
              }}
            >
              {thread.resolved ? "Unresolve conversation" : "Resolve conversation"}
            </Button>
          )}
        </span>
      </div>
      {shown && (
        <>
          {showContext && thread.root.diff_hunk && <HunkTail hunk={thread.root.diff_hunk} />}
          <ul className="divide-y">
            {[thread.root, ...thread.replies].map((c) => (
              <CommentItem key={c.id} pull={pull} comment={c} mine={c.user?.login === viewer} />
            ))}
          </ul>
          {!thread.pending && open && <ReplyBox pull={pull} to={thread.root.id} />}
        </>
      )}
    </div>
  );
}

function CommentItem({
  pull,
  comment,
  mine,
}: {
  pull: Pull;
  comment: ReviewComment;
  mine: boolean;
}) {
  const edit = useEditComment(pull);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(comment.body);
  return (
    <li className="flex gap-2 px-3 py-2" data-comment={comment.id}>
      <UserAvatar user={comment.user} />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-xs">
          <span className="font-semibold">{comment.user?.login}</span>
          <span className="text-muted-foreground">
            <Ago at={comment.created_at} />
            {comment.updated_at !== comment.created_at && " · edited"}
          </span>
          {mine && !editing && (
            <span className="ml-auto flex gap-0.5">
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Edit comment"
                onClick={() => setEditing(true)}
              >
                <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-xs"
                aria-label="Delete comment"
                disabled={edit.isPending}
                onClick={() => edit.mutate({ id: comment.id, body: null })}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </span>
          )}
        </div>
        {editing ? (
          <form
            className="mt-1 flex flex-col gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              if (!draft.trim()) return;
              edit.mutate({ id: comment.id, body: draft }, { onSuccess: () => setEditing(false) });
            }}
          >
            <Textarea
              aria-label="Edit comment"
              className="min-h-16 rounded-sm"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
            />
            <div className="flex justify-end gap-1.5">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => (setEditing(false), setDraft(comment.body))}
              >
                Cancel
              </Button>
              <Button type="submit" size="sm" disabled={edit.isPending || !draft.trim()}>
                Update comment
              </Button>
            </div>
          </form>
        ) : (
          <MarkdownBody text={comment.body} className="[&_p]:my-1" />
        )}
      </div>
    </li>
  );
}

function ReplyBox({ pull, to }: { pull: Pull; to: number }) {
  const reply = useReply(pull);
  const [open, setOpen] = useState(false);
  const form = useForm({
    defaultValues: { body: "" },
    onSubmit: async ({ value, formApi }) => {
      try {
        await reply.mutateAsync({ to, body: value.body.trim() });
        formApi.reset();
        setOpen(false);
      } catch (e) {
        if (e instanceof WriteError && e.fields.body)
          formApi.setFieldMeta("body", (m) => ({
            ...m,
            errorMap: { ...m.errorMap, onServer: e.fields.body },
          }));
      }
    },
  });
  if (!open)
    return (
      <div className="border-t px-3 py-2">
        <button
          type="button"
          className="text-muted-foreground bg-background hover:border-ring w-full rounded-sm border px-2 py-1 text-left text-sm"
          onClick={() => setOpen(true)}
        >
          Reply...
        </button>
      </div>
    );
  return (
    <form
      className="flex flex-col gap-1.5 border-t px-3 py-2"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="body"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Write a reply first."),
        }}
      >
        {(field) => (
          <>
            <Textarea
              autoFocus
              aria-label="Reply"
              className="min-h-16 rounded-sm"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            {field.state.meta.errors.length > 0 && (
              <p className="text-destructive text-xs" role="alert">
                {field.state.meta.errors.join(", ")}
              </p>
            )}
          </>
        )}
      </form.Field>
      <div className="flex justify-end gap-1.5">
        <Button type="button" variant="ghost" size="sm" onClick={() => setOpen(false)}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(busy) => (
            <Button type="submit" size="sm" disabled={busy}>
              Reply
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}
