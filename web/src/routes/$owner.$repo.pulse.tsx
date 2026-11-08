import { createFileRoute, Link } from "@tanstack/react-router";
import {
  CodeFrequency,
  CommitActivity,
  Contributors,
  Pulse,
  PunchCard,
  Traffic,
} from "@/areas/code/insights";
import {
  codeFrequencyQuery,
  commitActivityQuery,
  contributorStatsQuery,
  participationQuery,
  punchCardQuery,
  viewsQuery,
} from "@/areas/code/queries";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

// Insights. The section is a typed search param, so every graph has its own URL.

const TABS = [
  ["pulse", "Pulse"],
  ["contributors", "Contributors"],
  ["commits", "Commits"],
  ["code-frequency", "Code frequency"],
  ["punch-card", "Punch card"],
  ["traffic", "Traffic"],
] as const;
type Tab = (typeof TABS)[number][0];

export const Route = createFileRoute("/$owner/$repo/pulse")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; per?: "week" } => ({
    tab: TABS.some(([t]) => t === s.tab) && s.tab !== "pulse" ? (s.tab as Tab) : undefined,
    per: s.per === "week" ? "week" : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    const q = context.queryClient;
    const prefetch = {
      pulse: () => q.prefetchQuery(participationQuery(params)),
      contributors: () => q.prefetchQuery(contributorStatsQuery(params)),
      commits: () => q.prefetchQuery(commitActivityQuery(params)),
      "code-frequency": () => q.prefetchQuery(codeFrequencyQuery(params)),
      "punch-card": () => q.prefetchQuery(punchCardQuery(params)),
      traffic: () => q.prefetchQuery(viewsQuery({ ...params, per: deps.per ?? "day" })),
    }[deps.tab ?? "pulse"];
    void prefetch();
  },
  component: InsightsPage,
});

function InsightsPage() {
  const params = Route.useParams();
  const { tab = "pulse", per } = Route.useSearch();
  const label = TABS.find(([t]) => t === tab)![1];
  return (
    <div className="grid items-start gap-6 md:grid-cols-[12rem_minmax(0,1fr)]">
      <nav aria-label="Insights" className="flex flex-col gap-0.5 text-sm md:sticky md:top-4">
        {TABS.map(([t, name]) => (
          <Link
            key={t}
            from={Route.fullPath}
            search={{ tab: t === "pulse" ? undefined : t }}
            data-active={tab === t || undefined}
            className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary rounded-sm border-l-2 border-transparent px-2 py-1.5 font-medium"
          >
            {name}
          </Link>
        ))}
      </nav>
      <div className="min-w-0">
        <PageHeader
          title={label}
          actions={
            tab === "traffic" && (
              <div
                className="flex rounded-sm border p-0.5 text-xs"
                role="tablist"
                aria-label="Traffic interval"
              >
                {(["day", "week"] as const).map((p) => (
                  <Link
                    key={p}
                    from={Route.fullPath}
                    search={(prev) => ({ ...prev, per: p === "week" ? "week" : undefined })}
                    role="tab"
                    aria-selected={(per ?? "day") === p}
                    className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2 py-0.5 font-medium capitalize"
                  >
                    {p === "day" ? "Daily" : "Weekly"}
                  </Link>
                ))}
              </div>
            )
          }
        />
        <Region name={label.toLowerCase()} rows={8}>
          {tab === "pulse" && <Pulse {...params} />}
          {tab === "contributors" && <Contributors {...params} />}
          {tab === "commits" && <CommitActivity {...params} />}
          {tab === "code-frequency" && <CodeFrequency {...params} />}
          {tab === "punch-card" && <PunchCard {...params} />}
          {tab === "traffic" && <Traffic {...params} per={per ?? "day"} />}
        </Region>
      </div>
    </div>
  );
}
