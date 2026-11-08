import { useState, type ReactNode } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, Edit02Icon, MoreHorizontalIcon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { errorText, toastFailure, ValidationError } from "@/areas/issues/api";
import { commentsCollection, draftComment } from "@/areas/issues/collections";
import { MarkdownEditor } from "@/areas/issues/markdown-editor";
import { Reactions } from "@/areas/issues/reactions";
import { userQuery } from "@/areas/core/queries";
import { Ago, UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import {
  Timeline,
  TimelineContent,
  TimelineHeader,
  TimelineIndicator,
  TimelineItem,
  TimelineSeparator,
} from "@/components/reui/timeline";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";

type Thread = { owner: string; repo: string; number: number };
type Author = { login: string; avatar_url: string } | null;
type Post = { author: Author; at: string; body: string };

// One post's frame: the author line, an optional menu at its right, the body, and a footer.
export function CommentCard({
  author,
  at,
  body,
  pending,
  edited,
  menu,
  footer,
  children,
}: Post & {
  pending?: boolean;
  edited?: boolean;
  menu?: ReactNode;
  footer?: ReactNode;
  // Replaces the rendered body, for an inline editor.
  children?: ReactNode;
}) {
  return (
    <div className="bg-card overflow-hidden rounded-sm border" data-pending={pending || undefined}>
      <div className="bg-muted/50 flex items-center gap-2 border-b px-3 py-1.5 text-xs">
        <span className="font-semibold">{author?.login}</span>
        <span className="text-muted-foreground">
          commented <Ago at={at} />
        </span>
        {edited && <span className="text-muted-foreground">· edited</span>}
        <span className="ml-auto flex items-center gap-1">
          {pending && (
            <Badge variant="warning-light" size="sm">
              Sending
            </Badge>
          )}
          {menu}
        </span>
      </div>
      {children ?? <MarkdownBody text={body} className="px-3 py-1" />}
      {footer && <div className="px-3 pb-2">{footer}</div>}
    </div>
  );
}

// One stop on the rail: the author's avatar, and the line down to the next stop.
// ReUI hides the line on the last item, so the composer, always last, ends it.
function RailItem({
  author,
  step,
  children,
}: {
  author: Author;
  step: number;
  children: React.ReactNode;
}) {
  return (
    <TimelineItem step={step} className="group-data-[orientation=vertical]/timeline:ms-10">
      <TimelineHeader>
        <TimelineSeparator className="bg-border group-data-[orientation=vertical]/timeline:-left-7 group-data-[orientation=vertical]/timeline:translate-y-7 group-data-[orientation=vertical]/timeline:h-[calc(100%-2rem)]" />
        <TimelineIndicator className="flex size-6 items-center justify-center overflow-hidden rounded-sm border-none bg-transparent group-data-[orientation=vertical]/timeline:-left-7">
          <UserAvatar user={author} />
        </TimelineIndicator>
      </TimelineHeader>
      <TimelineContent className="text-foreground">{children}</TimelineContent>
    </TimelineItem>
  );
}

// Something other than a comment that belongs on the rail, such as a review, a label
// change, a merge, or a force-push. The area that owns it renders `node`.
export type TimelineEvent = {
  key: string;
  at: string;
  author: Author;
  node: React.ReactNode;
};

// The opening post, every comment and extra event in time order, and the composer, on
// one rail. value={0} keeps ReUI's "completed" styling off, so the rail stays the border
// colour. `composer={false}` hides the comment box, for a locked thread. `opening.node`
// replaces the opening card, for a page that edits its own description.
// `composerActions` adds buttons beside Comment; each gets the draft and a way to clear it.
export function Conversation({
  opening,
  events = [],
  composer = true,
  composerActions,
  ...thread
}: Thread & {
  opening: Post & { node?: ReactNode };
  events?: TimelineEvent[];
  composer?: boolean;
  composerActions?: (draft: { body: string; clear: () => void }) => ReactNode;
}) {
  const comments = commentsCollection(thread);
  const { data } = useLiveSuspenseQuery({
    query: (q) => q.from({ c: comments }).orderBy(({ c }) => c.created_at, "asc"),
  });
  const items = [
    ...data.map((c) => ({
      key: String(c.id),
      at: c.created_at,
      author: c.user,
      node: <Comment comment={c} pending={c.$hasPendingWrites} thread={thread} />,
    })),
    ...events,
  ].sort((a, b) => a.at.localeCompare(b.at));
  const all = [
    {
      key: "opening",
      author: opening.author,
      node: opening.node ?? <CommentCard {...opening} />,
    },
    ...items,
  ];
  return (
    <Timeline value={0} data-testid="comments">
      {all.map(({ key, author, node }, i) => (
        <RailItem key={key} author={author} step={i + 1}>
          {node}
        </RailItem>
      ))}
      {composer && <CommentBox {...thread} step={all.length + 1} actions={composerActions} />}
    </Timeline>
  );
}

type IssueComment = Schemas["issue-comment"];

// A comment with its menu (edit, delete) and reactions. Both writes go through the
// comments collection, so the change shows at once and rolls back if the server refuses.
function Comment({
  comment,
  pending,
  thread,
}: {
  comment: IssueComment;
  pending: boolean;
  thread: Thread;
}) {
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const saved = comment.id > 0;
  const remove = async () => {
    setConfirm(false);
    const tx = commentsCollection(thread).delete(comment.id);
    try {
      await tx.when("settled");
    } catch (e) {
      toastFailure("Deleting the comment", e);
    }
  };
  return (
    <div data-comment-id={comment.id}>
      <CommentCard
        author={comment.user}
        at={comment.created_at}
        body={comment.body ?? ""}
        pending={pending}
        edited={comment.updated_at !== comment.created_at}
        menu={
          saved && (
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button variant="ghost" size="icon-xs" aria-label="Comment actions">
                    <HugeiconsIcon icon={MoreHorizontalIcon} strokeWidth={2} />
                  </Button>
                }
              />
              <DropdownMenuContent align="end" className="w-40">
                <DropdownMenuItem onClick={() => setEditing(true)}>
                  <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
                  Edit
                </DropdownMenuItem>
                <DropdownMenuItem variant="destructive" onClick={() => setConfirm(true)}>
                  <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                  Delete
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          )
        }
        footer={
          saved && !editing ? (
            <Reactions kind="comment" owner={thread.owner} repo={thread.repo} id={comment.id} />
          ) : undefined
        }
      >
        {editing ? (
          <EditComment comment={comment} thread={thread} done={() => setEditing(false)} />
        ) : undefined}
      </CommentCard>
      <AlertDialog open={confirm} onOpenChange={setConfirm}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this comment?</AlertDialogTitle>
            <AlertDialogDescription>This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction variant="destructive" onClick={() => void remove()}>
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

// Inline editor for a comment. The edit shows at once; on failure the old text comes
// back on the card and the editor reopens with the new text.
function EditComment({
  comment,
  thread,
  done,
}: {
  comment: IssueComment;
  thread: Thread;
  done: () => void;
}) {
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { body: comment.body ?? "" },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const tx = commentsCollection(thread).update(comment.id, (d) => {
        d.body = value.body;
        d.updated_at = new Date().toISOString();
      });
      try {
        await tx.when("settled");
        done();
      } catch (e) {
        if (e instanceof ValidationError && e.fields.body) {
          formApi.setFieldMeta("body", (m) => ({ ...m, errorMap: { onServer: e.fields.body } }));
        }
        setFailure(errorText(e));
        toastFailure("Saving the comment", e);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-2 p-3"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="body"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "A comment can't be empty."),
        }}
      >
        {(field) => (
          <>
            <MarkdownEditor
              label="Edit comment"
              context={`${thread.owner}/${thread.repo}`}
              value={field.state.value}
              onBlur={field.handleBlur}
              onChange={field.handleChange}
              invalid={field.state.meta.errors.length > 0}
              onSubmitShortcut={() => void form.handleSubmit()}
            />
            {field.state.meta.errors.length > 0 && (
              <p className="text-destructive text-xs" data-field-error="body">
                {field.state.meta.errors.join(", ")}
              </p>
            )}
          </>
        )}
      </form.Field>
      {failure && <p className="text-destructive text-xs">Not saved: {failure}.</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" size="sm" onClick={done}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" size="sm" disabled={submitting}>
              Update comment
            </Button>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}

// Inserts into the comments collection. The collection shows the comment at once and
// POSTs it; if the POST fails the comment disappears and the form keeps the text.
function CommentBox({
  step,
  actions,
  ...thread
}: Thread & {
  step: number;
  actions?: (draft: { body: string; clear: () => void }) => ReactNode;
}) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { body: "" },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const tx = commentsCollection(thread).insert(draftComment(viewer, value.body.trim()));
      formApi.reset();
      try {
        await tx.when("settled");
      } catch (e) {
        formApi.setFieldValue("body", value.body);
        if (e instanceof ValidationError && e.fields.body) {
          formApi.setFieldMeta("body", (m) => ({ ...m, errorMap: { onServer: e.fields.body } }));
        }
        setFailure(errorText(e));
      }
    },
  });

  return (
    <RailItem author={viewer} step={step}>
      <form
        className="bg-card flex flex-col gap-2 rounded-sm border p-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="body"
          validators={{
            onSubmit: ({ value }) => (value.trim() ? undefined : "Write something first."),
          }}
        >
          {(field) => (
            <>
              <MarkdownEditor
                label="Comment"
                placeholder="Leave a comment"
                context={`${thread.owner}/${thread.repo}`}
                className="min-h-24"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={field.handleChange}
                invalid={field.state.meta.errors.length > 0}
                onSubmitShortcut={() => void form.handleSubmit()}
              />
              {field.state.meta.errors.length > 0 && (
                <p className="text-destructive text-xs" data-field-error="body">
                  {field.state.meta.errors.join(", ")}
                </p>
              )}
            </>
          )}
        </form.Field>
        {failure && (
          <Alert variant="destructive">
            <AlertTitle>Comment not posted</AlertTitle>
            <AlertDescription>{failure}. Your text is back in the box.</AlertDescription>
          </Alert>
        )}
        <div className="flex justify-end gap-2">
          <form.Subscribe selector={(s) => s.values.body}>
            {(body) => actions?.({ body, clear: () => form.reset() })}
          </form.Subscribe>
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(submitting) => (
              <Button type="submit" size="sm" disabled={submitting}>
                Comment
              </Button>
            )}
          </form.Subscribe>
        </div>
      </form>
    </RailItem>
  );
}
