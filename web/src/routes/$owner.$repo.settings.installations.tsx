import { createFileRoute, Link } from "@tanstack/react-router";
import { Installations } from "@/areas/repo-admin/integrations";
import { installationsQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/installations")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(installationsQuery(params)),
  component: InstallationsPage,
});

function InstallationsPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-installations">
      <PageHeader
        title="GitHub Apps"
        description={
          <>
            Apps installed on this repository. Install, configure, or remove them from the{" "}
            <Link
              to="/orgs/$org/settings"
              params={{ org: params.owner }}
              className="text-primary hover:underline"
            >
              organization's settings
            </Link>
            .
          </>
        }
      />
      <Panel title="Installed GitHub Apps" flush>
        <Region name="installations" rows={3}>
          <Installations params={params} />
        </Region>
      </Panel>
    </div>
  );
}
