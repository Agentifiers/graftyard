import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useLiveSuspenseQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Calendar03Icon,
  CheckmarkCircle02Icon,
  CircleIcon,
  Comment01Icon,
} from "@hugeicons/core-free-icons";
import { issuesCollection } from "@/areas/issues/collections";
import { formatDue } from "@/areas/issues/pickers";
import { milestoneQuery } from "@/areas/issues/queries";
import { Ago, LabelBadge, StateIcon, UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/milestones/$number")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  validateSearch: (s: Record<string, unknown>): { state?: "closed" } => ({
    state: s.state === "closed" ? "closed" : undefined,
  }),
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(milestoneQuery(params));
    void issuesCollection(params)
      .preload()
      .catch(() => {});
  },
  component: MilestonePage,
});

function MilestonePage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Region name="milestone" fallback={<Skeleton className="h-24" />}>
        <Header />
      </Region>
      <Frame spacing="sm" dense>
        <Region name="milestone issues" rows={6} onRetry={() => issuesCollection(params).reload()}>
          <Issues />
        </Region>
      </Frame>
    </div>
  );
}

function Header() {
  const params = Route.useParams();
  const { data: m } = useSuspenseQuery(milestoneQuery(params));
  const total = m.open_issues + m.closed_issues;
  const pct = total ? Math.round((m.closed_issues / total) * 100) : 0;
  const overdue = m.state === "open" && m.due_on && new Date(m.due_on) < new Date();
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="text-xl font-semibold" data-testid="milestone-title">
            {m.title}
          </h1>
          <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-3 text-sm">
            {m.state === "closed" ? (
              <Badge variant="info-light">Closed</Badge>
            ) : (
              <Badge variant="success-light">Open</Badge>
            )}
            <span
              className={
                overdue ? "text-destructive flex items-center gap-1" : "flex items-center gap-1"
              }
            >
              <HugeiconsIcon icon={Calendar03Icon} strokeWidth={2} className="size-4" />
              {m.due_on
                ? `${overdue ? "Past due by" : "Due by"} ${formatDue(m.due_on)}`
                : "No due date"}
            </span>
            <span>
              Created by {m.creator?.login} <Ago at={m.created_at} />
            </span>
          </p>
        </div>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            nativeButton={false}
            render={
              <Link
                to="/$owner/$repo/milestones"
                params={{ owner: params.owner, repo: params.repo }}
              />
            }
          >
            All milestones
          </Button>
          <Button
            variant="outline"
            size="sm"
            nativeButton={false}
            render={<Link to="/$owner/$repo/milestones/$number/edit" params={params} />}
          >
            Edit milestone
          </Button>
          <Button
            size="sm"
            nativeButton={false}
            render={
              <Link
                to="/$owner/$repo/issues/new"
                params={{ owner: params.owner, repo: params.repo }}
              />
            }
          >
            New issue
          </Button>
        </div>
      </div>
      {m.description && <MarkdownBody text={m.description} />}
      <div className="flex max-w-md flex-col gap-1">
        <Progress value={pct} aria-label="Milestone progress" />
        <p className="text-muted-foreground text-xs" data-testid="milestone-progress">
          <span className="text-foreground font-semibold">{pct}%</span> complete · {m.open_issues}{" "}
          open · {m.closed_issues} closed
        </p>
      </div>
    </div>
  );
}

function Issues() {
  const params = Route.useParams();
  const { state = "open" } = Route.useSearch();
  const { data } = useLiveSuspenseQuery({
    query: (q) =>
      q
        .from({ issue: issuesCollection(params) })
        .fn.where(({ issue }) => issue.milestone?.number === params.number)
        .orderBy(({ issue }) => issue.updated_at, "desc"),
    queryKey: ["milestone-issues", params.owner, params.repo, params.number],
  });
  const open = data.filter((i) => i.state === "open");
  const closed = data.filter((i) => i.state === "closed");
  const shown = state === "open" ? open : closed;
  return (
    <>
      <FrameHeader className="flex-row items-center gap-4 py-2 text-sm">
        <Link
          from={Route.fullPath}
          search={{ state: undefined }}
          data-active={state === "open" || undefined}
          className="text-muted-foreground data-active:text-foreground flex items-center gap-1.5 font-medium data-active:font-semibold"
        >
          <HugeiconsIcon icon={CircleIcon} strokeWidth={2} className="size-4" />
          {open.length} Open
        </Link>
        <Link
          from={Route.fullPath}
          search={{ state: "closed" }}
          data-active={state === "closed" || undefined}
          className="text-muted-foreground data-active:text-foreground flex items-center gap-1.5 font-medium data-active:font-semibold"
        >
          <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} className="size-4" />
          {closed.length} Closed
        </Link>
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        {shown.length === 0 ? (
          <p
            className="text-muted-foreground px-4 py-10 text-center text-sm"
            data-testid="milestone-issues-empty"
          >
            {state === "open"
              ? "No open issues in this milestone."
              : "No closed issues in this milestone."}
          </p>
        ) : (
          <ul className="divide-y" data-testid="milestone-issues">
            {shown.map((i) => (
              <li key={i.number} className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5">
                <span className="mt-0.5">
                  <StateIcon
                    state={
                      i.pull_request
                        ? i.state === "closed"
                          ? "pr-closed"
                          : "pr-open"
                        : i.state === "closed"
                          ? "closed"
                          : "open"
                    }
                  />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <Link
                      to={
                        i.pull_request
                          ? "/$owner/$repo/pull/$number"
                          : "/$owner/$repo/issues/$number"
                      }
                      params={{ owner: params.owner, repo: params.repo, number: i.number }}
                      className="text-sm font-semibold hover:underline"
                    >
                      {i.title}
                    </Link>
                    {i.labels.map((l) => (
                      <LabelBadge key={typeof l === "string" ? l : l.name} label={l} />
                    ))}
                  </div>
                  <p className="text-muted-foreground mt-0.5 text-xs">
                    #{i.number} opened <Ago at={i.created_at} /> by {i.user?.login}
                  </p>
                </div>
                <span className="flex shrink-0 items-center gap-3 self-center">
                  {(i.assignees ?? []).map((u) => (
                    <UserAvatar key={u.login} user={u} />
                  ))}
                  {i.comments > 0 && (
                    <span className="text-muted-foreground flex items-center gap-1 text-xs">
                      <HugeiconsIcon icon={Comment01Icon} strokeWidth={2} className="size-3.5" />
                      {i.comments}
                    </span>
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </FramePanel>
    </>
  );
}
