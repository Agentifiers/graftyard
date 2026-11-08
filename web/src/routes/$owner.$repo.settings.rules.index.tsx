import { createFileRoute, Link } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { CheckListIcon } from "@hugeicons/core-free-icons";
import { RulesetList } from "@/areas/repo-admin/rulesets";
import { rulesetsQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/$owner/$repo/settings/rules/")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(rulesetsQuery(params)),
  component: RulesPage,
});

function RulesPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-rules">
      <PageHeader
        title="Rulesets"
        description="Rules that apply to branches or tags. Anyone with read access can see them."
        actions={
          <Button
            variant="outline"
            size="sm"
            render={<Link to="/$owner/$repo/settings/rules/insights" params={params} />}
          >
            <HugeiconsIcon icon={CheckListIcon} strokeWidth={2} data-icon="inline-start" />
            Rule insights
          </Button>
        }
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
                  to="/$owner/$repo/settings/rules/$rulesetId"
                  params={{ ...params, rulesetId: "new" }}
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
                  to="/$owner/$repo/settings/rules/$rulesetId"
                  params={{ ...params, rulesetId: "new" }}
                  search={{ target: "branch" }}
                />
              }
            >
              New branch ruleset
            </Button>
          </>
        }
      >
        <Region name="rulesets" rows={3}>
          <RulesetList params={params} />
        </Region>
      </Panel>
    </div>
  );
}
