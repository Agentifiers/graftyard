import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { commentsCollection } from "@/areas/issues/collections";
import { ChecksTab } from "@/areas/pulls/checks-tab";
import { CommitList } from "@/areas/pulls/commit-list";
import { ConversationTab } from "@/areas/pulls/conversation-tab";
import { FilesTab } from "@/areas/pulls/files-tab";
import { PullHeader } from "@/areas/pulls/header";
import { MergeBox } from "@/areas/pulls/merge-box";
import { checkRollup } from "@/areas/pulls/merge-rules";
import {
  checkRunsQuery,
  mergeRequirementsQuery,
  pullCommitsQuery,
  pullEventsQuery,
  pullFilesQuery,
  pullQuery,
  reviewCommentsQuery,
  reviewsQuery,
  threadsQuery,
  viewedQuery,
  type Pull,
} from "@/areas/pulls/queries";
import { ReviewBox } from "@/areas/pulls/review-box";
import { useReviewModel } from "@/areas/pulls/review-data";
import { Slot } from "@/areas/slots";
import { CHECK_LOOK, StateIconOf } from "@/areas/pulls/bits";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "cn";

const TABS = ["conversation", "commits", "checks", "files"] as const;
type Tab = (typeof TABS)[number];

type Search = { tab?: Exclude<Tab, "conversation">; view?: "split"; run?: number };

export const Route = createFileRoute("/$owner/$repo/pull/$number")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  validateSearch: (s: Record<string, unknown>): Search => ({
    tab: s.tab === "commits" || s.tab === "checks" || s.tab === "files" ? s.tab : undefined,
    view: s.view === "split" ? "split" : undefined,
    run:
      typeof s.run === "number"
        ? s.run
        : typeof s.run === "string" && /^\d+$/.test(s.run)
          ? Number(s.run)
          : undefined,
  }),
  loaderDeps: ({ search }) => ({ tab: search.tab }),
  loader: async ({ context, params, deps }) => {
    const qc = context.queryClient;
    void qc.prefetchQuery(reviewsQuery(params));
    void qc.prefetchQuery(reviewCommentsQuery(params));
    void qc.prefetchQuery(threadsQuery(params));
    void qc.prefetchQuery(mergeRequirementsQuery(params));
    void qc.prefetchQuery(pullCommitsQuery(params));
    if (!deps.tab) {
      void qc.prefetchQuery(pullEventsQuery(params));
      void commentsCollection(params)
        .preload()
        .catch(() => {});
    }
    if (deps.tab === "files") {
      void qc.prefetchQuery(pullFilesQuery(params));
      void qc.prefetchQuery(viewedQuery(params));
    }
    // Checks are keyed by the head sha, which only the PR knows.
    const pr = await qc.ensureQueryData(pullQuery(params)).catch(() => undefined);
    if (pr) void qc.prefetchQuery(checkRunsQuery({ ...params, ref: pr.head.sha }));
  },
  component: PullPage,
});

function PullPage() {
  const params = Route.useParams();
  const { tab = "conversation" } = Route.useSearch();
  return (
    <div className="flex flex-col gap-3">
      <Region name="pull request" fallback={<Skeleton className="h-20" />}>
        <PullHeader
          pull={params}
          actions={
            <Region name="review" fallback={null}>
              <ReviewAction />
            </Region>
          }
        />
      </Region>
      <TabBar />
      {tab === "files" ? (
        <Region name="files" rows={10}>
          <Files />
        </Region>
      ) : (
        <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_18rem]">
          <div className="flex min-w-0 flex-col gap-4">
            {tab === "conversation" && (
              <Region
                name="conversation"
                rows={6}
                onRetry={() => commentsCollection(params).reload()}
              >
                <Thread />
              </Region>
            )}
            {tab === "commits" && (
              <Region name="commits" rows={5}>
                <Commits />
              </Region>
            )}
            {tab === "checks" && (
              <Region name="checks" rows={5}>
                <Checks />
              </Region>
            )}
          </div>
          <aside className="flex flex-col gap-4 lg:sticky lg:top-4">
            <Region name="pull request" fallback={null}>
              <PullSidebar />
            </Region>
          </aside>
        </div>
      )}
    </div>
  );
}

// Tab counts come from their own reads; a failing count shows no number, not an error.
function TabBar() {
  const params = Route.useParams();
  const { tab = "conversation" } = Route.useSearch();
  const pr = useQuery(pullQuery(params)).data;
  const reviews = useQuery(reviewCommentsQuery(params)).data;
  const runs = useQuery({
    ...checkRunsQuery({ ...params, ref: pr?.head.sha ?? "" }),
    enabled: Boolean(pr),
  }).data;
  const roll = runs ? checkRollup(runs.check_runs) : undefined;
  const look = roll ? CHECK_LOOK[roll.state] : undefined;
  const items: { tab: Tab; label: string; count?: number; extra?: React.ReactNode }[] = [
    {
      tab: "conversation",
      label: "Conversation",
      count: pr ? pr.comments + (reviews?.length ?? 0) : undefined,
    },
    { tab: "commits", label: "Commits", count: pr?.commits },
    {
      tab: "checks",
      label: "Checks",
      count: roll?.total,
      extra: look && roll && roll.total > 0 ? <StateIconOf {...look} className="size-3.5" /> : null,
    },
    { tab: "files", label: "Files changed", count: pr?.changed_files },
  ];
  return (
    <nav className="flex gap-1 border-b" aria-label="Pull request" role="tablist">
      {items.map((it) => (
        <Link
          key={it.tab}
          from={Route.fullPath}
          search={(prev) => ({
            ...prev,
            tab: it.tab === "conversation" ? undefined : it.tab,
            run: undefined,
          })}
          role="tab"
          aria-selected={tab === it.tab}
          className={cn(
            "text-muted-foreground hover:text-foreground -mb-px flex items-center gap-1.5 border-b-2 border-transparent px-3 py-2 text-sm font-medium",
            "aria-selected:border-primary aria-selected:text-foreground",
          )}
          data-tab={it.tab}
        >
          {it.label}
          {it.extra}
          {it.count !== undefined && (
            <Badge variant="secondary" size="sm">
              {it.count}
            </Badge>
          )}
        </Link>
      ))}
    </nav>
  );
}

function ReviewAction() {
  const pull = Route.useParams();
  const model = useReviewModel(pull);
  return <ReviewBox pull={pull} model={model} />;
}

function Thread() {
  const pull: Pull = Route.useParams();
  const model = useReviewModel(pull);
  return (
    <ConversationTab
      pull={pull}
      model={model}
      mergeBox={
        <Region name="merge box" rows={3}>
          <MergeBox pull={pull} />
        </Region>
      }
    />
  );
}

function Commits() {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(pullCommitsQuery(params));
  return <CommitList owner={params.owner} repo={params.repo} commits={data} />;
}

function Checks() {
  const params = Route.useParams();
  const { run } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <ChecksTab
      pull={params}
      runId={run}
      onRun={(id) => navigate({ search: (prev) => ({ ...prev, run: id }), replace: true })}
    />
  );
}

function Files() {
  const pull = Route.useParams();
  const { view } = Route.useSearch();
  const navigate = Route.useNavigate();
  const model = useReviewModel(pull);
  return (
    <FilesTab
      pull={pull}
      model={model}
      view={view ?? "unified"}
      onView={(v) =>
        navigate({
          search: (prev) => ({ ...prev, view: v === "split" ? "split" : undefined }),
          replace: true,
        })
      }
    />
  );
}

// Other areas' widgets (reviewers, agent session) under the pull's own sidebar slot.
function PullSidebar() {
  const params = Route.useParams();
  const { data: pr } = useSuspenseQuery(pullQuery(params));
  return (
    <div className="flex flex-col gap-4">
      <Slot name="pull-sidebar" props={{ ...params, headSha: pr.head.sha }} />
    </div>
  );
}
