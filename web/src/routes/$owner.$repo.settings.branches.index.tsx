import { createFileRoute, Link } from "@tanstack/react-router";
import { ProtectionRules } from "@/areas/repo-admin/protection";
import { protectionRulesQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/$owner/$repo/settings/branches/")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(protectionRulesQuery(params)),
  component: BranchesSettings,
});

function BranchesSettings() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-branches">
      <PageHeader
        title="Branches"
        description="Protect important branches: require reviews and checks, and block force pushes and deletion."
      />
      <Panel
        title="Branch protection rules"
        flush
        actions={
          <Button
            size="sm"
            render={
              <Link
                to="/$owner/$repo/settings/branches/$pattern"
                params={{ ...params, pattern: "new" }}
              />
            }
          >
            Add rule
          </Button>
        }
      >
        <Region name="branch protection rules" rows={3}>
          <ProtectionRules params={params} />
        </Region>
      </Panel>
    </div>
  );
}
