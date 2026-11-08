import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { commentsCollection, draftComment } from "@/areas/issues/collections";
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
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";

type Thread = { owner: string; repo: string; number: number };
type Author = { login: string; avatar_url: string } | null;
type Post = { author: Author; at: string; body: string };

export function CommentCard({ author, at, body, pending }: Post & { pending?: boolean }) {
  return (
    <div className="bg-card overflow-hidden rounded-lg border" data-pending={pending || undefined}>
      <div className="bg-muted/50 flex items-center gap-2 border-b px-3 py-1.5 text-xs">
        <span className="font-semibold">{author?.login}</span>
        <span className="text-muted-foreground">
          commented <Ago at={at} />
        </span>
        {pending && (
          <Badge variant="warning-light" size="sm" className="ml-auto">
            Sending
          </Badge>
        )}
      </div>
      <MarkdownBody text={body} className="px-3 py-1" />
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
// colour. `composer={false}` hides the comment box, for a locked thread.
export function Conversation({
  opening,
  events = [],
  composer = true,
  ...thread
}: Thread & { opening: Post; events?: TimelineEvent[]; composer?: boolean }) {
  const comments = commentsCollection(thread);
  const { data } = useLiveSuspenseQuery({
    query: (q) => q.from({ c: comments }).orderBy(({ c }) => c.created_at, "asc"),
  });
  const items = [
    ...data.map((c) => ({
      key: String(c.id),
      at: c.created_at,
      author: c.user,
      node: (
        <CommentCard
          author={c.user}
          at={c.created_at}
          body={c.body ?? ""}
          pending={c.$hasPendingWrites}
        />
      ),
    })),
    ...events,
  ].sort((a, b) => a.at.localeCompare(b.at));
  const all = [
    { key: "opening", author: opening.author, node: <CommentCard {...opening} /> },
    ...items,
  ];
  return (
    <Timeline value={0} data-testid="comments">
      {all.map(({ key, author, node }, i) => (
        <RailItem key={key} author={author} step={i + 1}>
          {node}
        </RailItem>
      ))}
      {composer && <CommentBox {...thread} step={all.length + 1} />}
    </Timeline>
  );
}

// Inserts into the comments collection. The collection shows the comment at once and
// POSTs it; if the POST fails the comment disappears and the form keeps the text.
function CommentBox({ step, ...thread }: Thread & { step: number }) {
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
        setFailure(e instanceof Error ? e.message : String(e));
      }
    },
  });

  return (
    <RailItem author={viewer} step={step}>
      <form
        className="bg-card flex flex-col gap-2 rounded-lg border p-3"
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
              <Textarea
                name={field.name}
                aria-label="Comment"
                placeholder="Leave a comment"
                className="min-h-24"
                value={field.state.value}
                onBlur={field.handleBlur}
                onChange={(e) => field.handleChange(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void form.handleSubmit();
                }}
              />
              {field.state.meta.errors.length > 0 && (
                <p className="text-destructive text-xs">{field.state.meta.errors.join(", ")}</p>
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
        <div className="flex justify-end">
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
