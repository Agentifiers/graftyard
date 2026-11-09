import { createFileRoute } from "@tanstack/react-router";
import { PreReceiveHooks } from "@/areas/repo-admin/integrations";
import { preReceiveQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/pre-receive-hooks")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(preReceiveQuery(params)),
  component: PreReceivePage,
});

function PreReceivePage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-pre-receive">
      <PageHeader
        title="Pre-receive hooks"
        description="Scripts the site administrator installed that check every push. Where the administrator allows it, choose how each one applies here."
      />
      <Panel title="Hooks" flush>
        <Region name="pre-receive hooks" rows={3}>
          <PreReceiveHooks params={params} />
        </Region>
      </Panel>
    </div>
  );
}
