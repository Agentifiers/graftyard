import { createFileRoute, Link } from "@tanstack/react-router";
import { RulesetEditor, RulesetHistory } from "@/areas/repo-admin/rulesets";
import {
  branchesQuery,
  checkNamesQuery,
  repoTeamsQuery,
  rulesetQuery,
} from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

type Search = { target?: "branch" | "tag" };

// One ruleset; `new` builds one, with ?target=branch or tag.
export const Route = createFileRoute("/$owner/$repo/settings/rules/$rulesetId")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    target: s.target === "tag" ? "tag" : s.target === "branch" ? "branch" : undefined,
  }),
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(branchesQuery(params));
    void queryClient.prefetchQuery(repoTeamsQuery(params));
    void queryClient.prefetchQuery(checkNamesQuery(params));
    const id = Number(params.rulesetId);
    if (id) void queryClient.prefetchQuery(rulesetQuery({ ...params, id }));
  },
  component: RulesetPage,
});

function RulesetPage() {
  const { owner, repo, rulesetId } = Route.useParams();
  const { target = "branch" } = Route.useSearch();
  const id = rulesetId === "new" ? "new" : Number(rulesetId);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={
          <>
            <Link
              to="/$owner/$repo/settings/rules"
              params={{ owner, repo }}
              className="text-muted-foreground hover:underline"
            >
              Rulesets
            </Link>
            <span className="text-muted-foreground px-1">/</span>
            {id === "new" ? `New ${target} ruleset` : "Edit ruleset"}
          </>
        }
      />
      <Panel title="Ruleset" flush>
        <Region name="ruleset" rows={8}>
          <RulesetEditor key={rulesetId} params={{ owner, repo }} id={id} target={target} />
        </Region>
      </Panel>
      {id !== "new" && (
        <Panel title="History" flush>
          <Region name="ruleset history" rows={2}>
            <RulesetHistory params={{ owner, repo }} id={id} />
          </Region>
        </Panel>
      )}
    </div>
  );
}
