import { createFileRoute, Link } from "@tanstack/react-router";
import { RuleInsights } from "@/areas/repo-admin/rulesets";
import { ruleSuitesQuery, usersQuery, type SuiteFilters } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { useQuery } from "@tanstack/react-query";

const PERIODS = ["hour", "day", "week", "month"] as const;
const RESULTS = ["all", "pass", "fail", "bypass"] as const;

export const Route = createFileRoute("/$owner/$repo/settings/rules/insights")({
  validateSearch: (s: Record<string, unknown>): Partial<SuiteFilters> => ({
    time_period: PERIODS.find((p) => p === s.time_period),
    rule_suite_result: RESULTS.find((r) => r === s.rule_suite_result),
    actor_name: typeof s.actor_name === "string" && s.actor_name ? s.actor_name : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context: { queryClient }, params, deps }) =>
    void queryClient.prefetchQuery(
      ruleSuitesQuery({
        ...params,
        time_period: deps.time_period ?? "week",
        rule_suite_result: deps.rule_suite_result ?? "all",
        actor_name: deps.actor_name,
      }),
    ),
  component: InsightsPage,
});

function InsightsPage() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  // The actor filter is optional chrome, so it never suspends the page.
  const people = useQuery(usersQuery()).data?.map((u) => u.login) ?? [];
  const filters: SuiteFilters = {
    time_period: search.time_period ?? "week",
    rule_suite_result: search.rule_suite_result ?? "all",
    actor_name: search.actor_name,
  };
  const set = (patch: Partial<SuiteFilters>) =>
    navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={
          <>
            <Link
              to="/$owner/$repo/settings/rules"
              params={params}
              className="text-muted-foreground hover:underline"
            >
              Rulesets
            </Link>
            <span className="text-muted-foreground px-1">/</span>
            Rule insights
          </>
        }
        description="Every push the rulesets evaluated. Select a row to see each rule's verdict."
      />
      <Panel
        title="Rule suites"
        flush
        actions={
          <>
            <NativeSelect
              size="sm"
              aria-label="Time period"
              value={filters.time_period}
              onChange={(e) => set({ time_period: e.target.value as SuiteFilters["time_period"] })}
            >
              {PERIODS.map((p) => (
                <NativeSelectOption key={p} value={p}>
                  Last {p}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <NativeSelect
              size="sm"
              aria-label="Result"
              value={filters.rule_suite_result}
              onChange={(e) =>
                set({ rule_suite_result: e.target.value as SuiteFilters["rule_suite_result"] })
              }
            >
              {RESULTS.map((r) => (
                <NativeSelectOption key={r} value={r}>
                  {r === "all" ? "All results" : r[0].toUpperCase() + r.slice(1)}
                </NativeSelectOption>
              ))}
            </NativeSelect>
            <NativeSelect
              size="sm"
              aria-label="Actor"
              value={filters.actor_name ?? ""}
              onChange={(e) => set({ actor_name: e.target.value || undefined })}
            >
              <NativeSelectOption value="">All actors</NativeSelectOption>
              {people.map((p) => (
                <NativeSelectOption key={p} value={p}>
                  {p}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </>
        }
      >
        <Region name="rule insights" rows={6}>
          <RuleInsights params={params} filters={filters} />
        </Region>
      </Panel>
    </div>
  );
}
