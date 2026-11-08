import { createFileRoute } from "@tanstack/react-router";
import { AutolinkForm, Autolinks } from "@/areas/repo-admin/integrations";
import { autolinksQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/autolinks")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(autolinksQuery(params)),
  component: AutolinksPage,
});

function AutolinksPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-autolinks">
      <PageHeader
        title="Autolink references"
        description="Turn references such as TICKET-123 in issues, pull requests, and commits into links to an outside tracker."
      />
      <Panel title="Add an autolink reference" flush>
        <AutolinkForm params={params} />
      </Panel>
      <Panel title="Autolink references" flush>
        <Region name="autolinks" rows={2}>
          <Autolinks params={params} />
        </Region>
      </Panel>
    </div>
  );
}
