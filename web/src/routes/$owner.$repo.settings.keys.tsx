import { createFileRoute } from "@tanstack/react-router";
import { DeployKeyForm, DeployKeys } from "@/areas/repo-admin/integrations";
import { deployKeysQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/keys")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(deployKeysQuery(params)),
  component: KeysPage,
});

function KeysPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-keys">
      <PageHeader
        title="Deploy keys"
        description="An SSH key that grants one machine access to this repository. A key works on one repository and one account only."
      />
      <Panel title="Deploy keys" flush>
        <Region name="deploy keys" rows={2}>
          <DeployKeys params={params} />
        </Region>
      </Panel>
      <Panel title="Add deploy key" flush>
        <DeployKeyForm params={params} />
      </Panel>
    </div>
  );
}
