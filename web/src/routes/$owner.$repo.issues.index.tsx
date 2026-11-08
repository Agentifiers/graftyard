import { createFileRoute, Link } from "@tanstack/react-router";
import { and, eq, ilike, isUndefined, useLiveSuspenseQuery } from "@tanstack/react-db";
import { HugeiconsIcon } from "@hugeicons/react";
import { Comment01Icon, Search01Icon } from "@hugeicons/core-free-icons";
import { issuesCollection } from "@/areas/issues/collections";
import { Ago, issueState, LabelBadge, StateIcon } from "@/components/github";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Input } from "@/components/ui/input";

const STATES = ["open", "closed", "all"] as const;
type StateFilter = (typeof STATES)[number];

export const Route = createFileRoute("/$owner/$repo/issues/")({
  validateSearch: (search: Record<string, unknown>): { state?: StateFilter; q?: string } => ({
    state: STATES.includes(search.state as StateFilter) ? (search.state as StateFilter) : undefined,
    q: typeof search.q === "string" && search.q ? search.q : undefined,
  }),
  // Start syncing early; the region shows any failure.
  loader: ({ params }) =>
    void issuesCollection(params)
      .preload()
      .catch(() => {}),
  component: IssuesPage,
});

function IssuesPage() {
  const params = Route.useParams();
  const { state = "open", q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="flex-row flex-wrap items-center gap-2 py-2">
        <div
          className="bg-background flex rounded-lg border p-0.5 text-sm"
          role="tablist"
          aria-label="Issue state"
        >
          {STATES.map((s) => (
            <Link
              key={s}
              from={Route.fullPath}
              search={(prev) => ({ ...prev, state: s === "open" ? undefined : s })}
              role="tab"
              aria-selected={state === s}
              className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-md px-2.5 py-1 font-medium capitalize"
            >
              {s}
            </Link>
          ))}
        </div>
        <label className="relative ml-auto w-full max-w-xs">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
          />
          <Input
            type="search"
            placeholder="Filter issues by title"
            className="pl-8"
            defaultValue={q}
            onChange={(e) =>
              navigate({
                search: (prev) => ({ ...prev, q: e.target.value || undefined }),
                replace: true,
              })
            }
          />
        </label>
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        <Region name="issues" rows={8} onRetry={() => issuesCollection(params).reload()}>
          <IssueList state={state} q={q} />
        </Region>
      </FramePanel>
    </Frame>
  );
}

function IssueList({ state, q }: { state: StateFilter; q: string }) {
  const params = Route.useParams();
  const issues = issuesCollection(params);
  const pattern = `%${q.replace(/[\\%_]/g, (c) => `\\${c}`)}%`;
  const { data } = useLiveSuspenseQuery({
    query: (qb) => {
      const base = qb
        .from({ issue: issues })
        .where(({ issue }) => and(isUndefined(issue.pull_request), ilike(issue.title, pattern)));
      const filtered = state === "all" ? base : base.where(({ issue }) => eq(issue.state, state));
      return filtered.orderBy(({ issue }) => issue.updated_at, "desc");
    },
  });

  if (data.length === 0) {
    return <p className="text-muted-foreground px-4 py-10 text-center text-sm">No issues match.</p>;
  }
  return (
    <ul className="divide-y" data-testid="issue-list">
      {data.map((issue) => (
        <li key={issue.number} className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5">
          <span className="mt-0.5">
            <StateIcon state={issueState(issue)} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
              <Link
                to="/$owner/$repo/issues/$number"
                params={{ ...params, number: issue.number }}
                className="text-sm font-semibold hover:underline"
              >
                {issue.title}
              </Link>
              {issue.labels.map((l) => (
                <LabelBadge key={typeof l === "string" ? l : l.name} label={l} />
              ))}
            </div>
            <p className="text-muted-foreground mt-0.5 text-xs">
              #{issue.number} {issue.state === "closed" ? "closed" : "opened"}{" "}
              <Ago
                at={
                  issue.state === "closed" && issue.closed_at ? issue.closed_at : issue.created_at
                }
              />{" "}
              by {issue.user?.login}
            </p>
          </div>
          {issue.comments > 0 && (
            <span
              className="text-muted-foreground flex w-8 shrink-0 items-center justify-end gap-1 self-center text-xs tabular-nums"
              title={`${issue.comments} comments`}
            >
              <HugeiconsIcon icon={Comment01Icon} strokeWidth={2} className="size-3.5" />
              {issue.comments}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
