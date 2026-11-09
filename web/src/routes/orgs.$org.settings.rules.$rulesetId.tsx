import { createFileRoute, Link } from "@tanstack/react-router";
import { orgReposQuery, rulesetQuery, teamsQuery } from "@/areas/orgs/queries";
import { RulesetEditor, RulesetHistory } from "@/areas/orgs/rulesets";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

type Search = { target?: "branch" | "tag" };

// One org ruleset; `new` builds one, with ?target=branch or tag.
export const Route = createFileRoute("/orgs/$org/settings/rules/$rulesetId")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    target: s.target === "tag" ? "tag" : s.target === "branch" ? "branch" : undefined,
  }),
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(teamsQuery(params.org));
    void queryClient.prefetchQuery(orgReposQuery(params.org));
    const id = Number(params.rulesetId);
    if (id) void queryClient.prefetchQuery(rulesetQuery(params.org, id));
  },
  component: RulesetPage,
});

function RulesetPage() {
  const { org, rulesetId } = Route.useParams();
  const { target = "branch" } = Route.useSearch();
  const id = rulesetId === "new" ? "new" : Number(rulesetId);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={
          <>
            <Link
              to="/orgs/$org/settings/rules"
              params={{ org }}
              className="text-muted-foreground hover:underline"
            >
              Rulesets
            </Link>{" "}
            / {id === "new" ? `New ${target} ruleset` : "Edit ruleset"}
          </>
        }
      />
      <Panel flush>
        <Region name="ruleset" rows={8}>
          <RulesetEditor org={org} id={id} target={target} />
        </Region>
      </Panel>
      {id !== "new" && (
        <Panel title="History" flush>
          <Region name="ruleset history" rows={2}>
            <RulesetHistory org={org} id={id} />
          </Region>
        </Panel>
      )}
    </div>
  );
}
