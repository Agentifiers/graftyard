import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { commentsCollection } from "@/api/collections";
import { issueQuery } from "@/areas/core/queries";
import { Slot } from "@/areas/slots";
import { Conversation } from "@/components/conversation";
import { Ago, issueState, LabelBadge, StateBadge } from "@/components/github";
import { Region } from "@/components/region";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/issues/$number")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(issueQuery(params));
    void commentsCollection(params)
      .preload()
      .catch(() => {});
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
  return (
    <div className="flex flex-col gap-3">
      <div>
        <h1 className="text-xl font-semibold">
          {issue.title} <span className="text-muted-foreground font-normal">#{issue.number}</span>
        </h1>
        <div className="text-muted-foreground mt-1.5 flex flex-wrap items-center gap-2 text-sm">
          <StateBadge state={issueState(issue)} />
          <span>
            <span className="text-foreground font-medium">{issue.user?.login}</span> opened{" "}
            <Ago at={issue.created_at} /> · {issue.comments}{" "}
            {issue.comments === 1 ? "comment" : "comments"}
          </span>
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
    </div>
  );
}

function Thread() {
  const params = Route.useParams();
  const { data: issue } = useSuspenseQuery(issueQuery(params));
  return (
    <Conversation
      {...params}
      opening={{
        author: issue.user,
        at: issue.created_at,
        body: issue.body || "_No description provided._",
      }}
    />
  );
}
