import { useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { commentsCollection, draftComment } from "@/api/collections";
import { userQuery } from "@/api/queries";
import { Ago, UserAvatar } from "@/components/github";
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

// GHES returns `body` as GitHub Flavored Markdown. react-markdown escapes raw HTML.
function Body({ text }: { text: string }) {
  return (
    <div className="prose prose-sm dark:prose-invert prose-p:my-2 prose-pre:bg-muted prose-pre:text-foreground prose-code:before:content-none prose-code:after:content-none prose-code:font-normal [&_:not(pre)>code]:bg-muted max-w-none px-3 py-1 leading-relaxed [&_:not(pre)>code]:rounded [&_:not(pre)>code]:px-1 [&_:not(pre)>code]:py-0.5">
      <Markdown remarkPlugins={[remarkGfm]}>{text}</Markdown>
    </div>
  );
}

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
      <Body text={body} />
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

// The opening post, every comment, and the composer, on one rail. value={0} keeps
// ReUI's "completed" styling off, so the rail stays the border colour.
export function Conversation({ opening, ...thread }: Thread & { opening: Post }) {
  const comments = commentsCollection(thread);
  const { data } = useLiveSuspenseQuery({
    query: (q) => q.from({ c: comments }).orderBy(({ c }) => c.created_at, "asc"),
  });
  const posts = [
    { key: "opening", ...opening, pending: false },
    ...data.map((c) => ({
      key: String(c.id),
      author: c.user,
      at: c.created_at,
      body: c.body ?? "",
      pending: c.$hasPendingWrites,
    })),
  ];
  return (
    <Timeline value={0} data-testid="comments">
      {posts.map(({ key, ...post }, i) => (
        <RailItem key={key} author={post.author} step={i + 1}>
          <CommentCard {...post} />
        </RailItem>
      ))}
      <CommentBox {...thread} step={posts.length + 1} />
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
