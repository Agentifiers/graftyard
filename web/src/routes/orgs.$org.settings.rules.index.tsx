import { createFileRoute, Link } from "@tanstack/react-router";
import { rulesetsQuery } from "@/areas/orgs/queries";
import { RulesetList } from "@/areas/orgs/rulesets";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/orgs/$org/settings/rules/")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(rulesetsQuery(params.org)),
  component: () => {
    const { org } = Route.useParams();
    return (
      <div className="flex flex-col gap-4">
        <PageHeader
          title="Rulesets"
          description="Rules for branches and tags across the organization's repositories. Repository rulesets stack on top."
        />
        <Panel
          title="Rulesets"
          flush
          actions={
            <>
              <Button
                size="sm"
                variant="outline"
                render={
                  <Link
                    to="/orgs/$org/settings/rules/$rulesetId"
                    params={{ org, rulesetId: "new" }}
                    search={{ target: "tag" }}
                  />
                }
              >
                New tag ruleset
              </Button>
              <Button
                size="sm"
                render={
                  <Link
                    to="/orgs/$org/settings/rules/$rulesetId"
                    params={{ org, rulesetId: "new" }}
                    search={{ target: "branch" }}
                  />
                }
                data-testid="new-ruleset"
              >
                New branch ruleset
              </Button>
            </>
          }
        >
          <Region name="rulesets" rows={3}>
            <RulesetList org={org} />
          </Region>
        </Panel>
      </div>
    );
  },
});
