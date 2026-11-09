import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { eq, useLiveQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowDown01Icon,
  CheckmarkCircle02Icon,
  CircleIcon,
  Edit02Icon,
  LockIcon,
  PinIcon,
  UnavailableIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { ValidationError } from "@/areas/issues/api";
import {
  commentsCollection,
  draftComment,
  pinKey,
  pinnedCollection,
} from "@/areas/issues/collections";
import { useIssueChange } from "@/areas/issues/issue-writes";
import { MarkdownEditor } from "@/areas/issues/markdown-editor";
import {
  issueQuery,
  issueReactionsQuery,
  subscriptionQuery,
  timelineQuery,
} from "@/areas/issues/queries";
import { Reactions } from "@/areas/issues/reactions";
import { useTimelineEvents } from "@/areas/issues/timeline";
import { Slot } from "@/areas/slots";
import { CommentCard, Conversation } from "@/components/conversation";
import { Ago, LabelBadge, StateBadge } from "@/components/github";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { ButtonGroup } from "@/components/ui/button-group";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/issues/$number")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  loader: async ({ context, params }) => {
    const { queryClient } = context;
    void queryClient.prefetchQuery(timelineQuery(params));
    void queryClient.prefetchQuery(issueReactionsQuery({ ...params }));
    void commentsCollection(params)
      .preload()
      .catch(() => {});
    // The subscription is keyed by the issue id, which only the issue knows.
    const issue = await queryClient.ensureQueryData(issueQuery(params)).catch(() => undefined);
    if (issue) void queryClient.prefetchQuery(subscriptionQuery(issue.id));
  },
  component: IssuePage,
});

function IssuePage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Region name="issue" fallback={<Skeleton className="h-24" />}>
        <IssueHeader />
      </Region>
      <Separator />
      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_16rem]">
        <Region name="comments" rows={4} onRetry={() => commentsCollection(params).reload()}>
          <Thread />
        </Region>
        <aside className="flex flex-col gap-4">
          <Slot name="issue-sidebar" props={params} />
        </aside>
      </div>
    </div>
  );
}

function IssueHeader() {
  const params = Route.useParams();
  const { data: issue } = useSuspenseQuery(issueQuery(params));
  const [editing, setEditing] = useState(false);
  const pins = useLiveQuery((q) =>
    q.from({ p: pinnedCollection() }).where(({ p }) => eq(p.key, pinKey(params))),
  );
  const pinned = (pins.data ?? []).length > 0;
  const closedNotPlanned = issue.state === "closed" && issue.state_reason === "not_planned";
  return (
    <div className="flex flex-col gap-3">
      {editing ? (
        <TitleForm issue={issue} done={() => setEditing(false)} />
      ) : (
        <div className="flex flex-wrap items-start gap-3">
          <h1 className="min-w-0 flex-1 text-xl font-semibold" data-testid="issue-title">
            {issue.title} <span className="text-muted-foreground font-normal">#{issue.number}</span>
          </h1>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={() => setEditing(true)}>
              <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} data-icon="inline-start" />
              Edit
            </Button>
            <Button
              size="sm"
              render={<Link to="/$owner/$repo/issues/new" params={params} />}
              nativeButton={false}
            >
              New issue
            </Button>
          </div>
        </div>
      )}
      <div className="text-muted-foreground flex flex-wrap items-center gap-2 text-sm">
        {closedNotPlanned ? (
          <Badge variant="outline" size="lg" data-state="not_planned">
            <HugeiconsIcon icon={UnavailableIcon} strokeWidth={2} />
            Closed as not planned
          </Badge>
        ) : (
          <span data-state={issue.state}>
            <StateBadge state={issue.state === "closed" ? "closed" : "open"} />
          </span>
        )}
        <span>
          <span className="text-foreground font-medium">{issue.user?.login}</span> opened{" "}
          <Ago at={issue.created_at} /> · {issue.comments}{" "}
          {issue.comments === 1 ? "comment" : "comments"}
        </span>
        {issue.locked && (
          <Badge variant="warning-light" size="sm" data-testid="locked-badge">
            <HugeiconsIcon icon={LockIcon} strokeWidth={2} />
            Locked{issue.active_lock_reason ? ` as ${issue.active_lock_reason}` : ""}
          </Badge>
        )}
        {pinned && (
          <Badge variant="info-light" size="sm" data-testid="pinned-badge">
            <HugeiconsIcon icon={PinIcon} strokeWidth={2} />
            Pinned
          </Badge>
        )}
        {issue.labels.map((l) => (
          <LabelBadge key={typeof l === "string" ? l : l.name} label={l} />
        ))}
        {issue.pull_request && (
          <Link
            to="/$owner/$repo/pull/$number"
            params={params}
            className="text-primary hover:underline"
          >
            View pull request
          </Link>
        )}
      </div>
    </div>
  );
}

// The title edits in place. The rename shows at once; a 422 (an empty title) shows on the field.
function TitleForm({ issue, done }: { issue: Schemas["issue"]; done: () => void }) {
  const params = Route.useParams();
  const change = useIssueChange(params);
  const form = useForm({
    defaultValues: { title: issue.title },
    onSubmit: async ({ value, formApi }) => {
      try {
        await change.mutateAsync({ kind: "title", title: value.title.trim() });
        done();
      } catch (e) {
        if (e instanceof ValidationError && e.fields.title) {
          formApi.setFieldMeta("title", (m) => ({ ...m, errorMap: { onServer: e.fields.title } }));
        }
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-1"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="title"
        validators={{
          onChange: ({ value }) =>
            value.length > 256 ? "Keep the title under 256 characters." : undefined,
        }}
      >
        {(field) => (
          <div className="flex flex-wrap items-center gap-2">
            <Input
              autoFocus
              aria-label="Title"
              className="h-9 min-w-0 flex-1 rounded-sm text-base"
              value={field.state.value}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
              onBlur={field.handleBlur}
              onChange={(e) => field.handleChange(e.target.value)}
              onKeyDown={(e) => e.key === "Escape" && done()}
            />
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" size="sm" disabled={submitting}>
                  Save
                </Button>
              )}
            </form.Subscribe>
            <Button type="button" variant="ghost" size="sm" onClick={done}>
              Cancel
            </Button>
            {field.state.meta.errors.length > 0 && (
              <p className="text-destructive w-full text-xs" data-field-error="title">
                Title {String(field.state.meta.errors.join(", ")).toLowerCase()}
              </p>
            )}
          </div>
        )}
      </form.Field>
    </form>
  );
}

function Thread() {
  const params = Route.useParams();
  const { data: issue } = useSuspenseQuery(issueQuery(params));
  const { data: viewer } = useSuspenseQuery(userQuery());
  const events = useTimelineEvents(params);
  const change = useIssueChange(params);
  const opening = {
    author: issue.user,
    at: issue.created_at,
    body: issue.body || "_No description provided._",
  };
  // Closing with text in the box posts the comment first, as GHES's "Close with comment" does.
  const closeWith = async (
    draft: { body: string; clear: () => void },
    state: "open" | "closed",
    reason: "completed" | "not_planned" | null,
  ) => {
    if (draft.body.trim()) {
      const tx = commentsCollection(params).insert(draftComment(viewer, draft.body.trim()));
      draft.clear();
      try {
        await tx.when("settled");
      } catch {
        return;
      }
    }
    change.mutate({ kind: "state", state, reason });
  };
  return (
    <div className="flex flex-col gap-4">
      <Conversation
        {...params}
        opening={{ ...opening, node: <Opening issue={issue} /> }}
        events={events}
        composer={!issue.locked}
        composerActions={(draft) =>
          issue.state === "open" ? (
            <ButtonGroup>
              <Button
                type="button"
                variant="outline"
                size="sm"
                onClick={() => void closeWith(draft, "closed", "completed")}
              >
                <HugeiconsIcon
                  icon={CheckmarkCircle02Icon}
                  strokeWidth={2}
                  className="text-info"
                  data-icon="inline-start"
                />
                {draft.body.trim() ? "Close with comment" : "Close issue"}
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger
                  render={
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      aria-label="More close options"
                    >
                      <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} />
                    </Button>
                  }
                />
                <DropdownMenuContent align="end" className="w-56">
                  <DropdownMenuItem onClick={() => void closeWith(draft, "closed", "completed")}>
                    <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} />
                    Close as completed
                  </DropdownMenuItem>
                  <DropdownMenuItem onClick={() => void closeWith(draft, "closed", "not_planned")}>
                    <HugeiconsIcon icon={UnavailableIcon} strokeWidth={2} />
                    Close as not planned
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </ButtonGroup>
          ) : (
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => void closeWith(draft, "open", null)}
            >
              <HugeiconsIcon
                icon={CircleIcon}
                strokeWidth={2}
                className="text-success"
                data-icon="inline-start"
              />
              {draft.body.trim() ? "Reopen with comment" : "Reopen issue"}
            </Button>
          )
        }
      />
      {issue.locked && (
        <Alert data-testid="locked-notice">
          <HugeiconsIcon icon={LockIcon} strokeWidth={2} />
          <AlertTitle>
            This conversation has been locked
            {issue.active_lock_reason ? ` as ${issue.active_lock_reason}` : ""}
          </AlertTitle>
          <AlertDescription>
            New comments are limited to collaborators. Unlock it from the sidebar to comment.
          </AlertDescription>
        </Alert>
      )}
    </div>
  );
}

// The opening post: the body with an inline editor and the issue's reactions.
function Opening({ issue }: { issue: Schemas["issue"] }) {
  const params = Route.useParams();
  const [editing, setEditing] = useState(false);
  return (
    <div data-testid="opening">
      <CommentCard
        author={issue.user}
        at={issue.created_at}
        body={issue.body || "_No description provided._"}
        menu={
          !editing && (
            <Button
              variant="ghost"
              size="icon-xs"
              aria-label="Edit description"
              onClick={() => setEditing(true)}
            >
              <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
            </Button>
          )
        }
        footer={!editing && <Reactions kind="issue" {...params} />}
      >
        {editing ? <BodyForm issue={issue} done={() => setEditing(false)} /> : undefined}
      </CommentCard>
    </div>
  );
}

function BodyForm({ issue, done }: { issue: Schemas["issue"]; done: () => void }) {
  const params = Route.useParams();
  const change = useIssueChange(params);
  const form = useForm({
    defaultValues: { body: issue.body ?? "" },
    onSubmit: async ({ value }) => {
      done();
      change.mutate({ kind: "body", body: value.body });
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
      <form.Field name="body">
        {(field) => (
          <MarkdownEditor
            label="Description"
            context={`${params.owner}/${params.repo}`}
            value={field.state.value}
            onBlur={field.handleBlur}
            onChange={field.handleChange}
            onSubmitShortcut={() => void form.handleSubmit()}
          />
        )}
      </form.Field>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="outline" size="sm" onClick={done}>
          Cancel
        </Button>
        <Button type="submit" size="sm">
          Update description
        </Button>
      </div>
    </form>
  );
}
