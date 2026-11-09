import { createFileRoute, Link } from "@tanstack/react-router";
import { HookList } from "@/areas/orgs/hooks";
import { hooksQuery } from "@/areas/orgs/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/orgs/$org/settings/hooks/")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(hooksQuery(params.org)),
  component: () => {
    const { org } = Route.useParams();
    return (
      <div className="flex flex-col gap-4" data-testid="org-hooks">
        <PageHeader
          title="Webhooks"
          description="GHES sends a POST with event details to each URL when something happens in any repository of the organization."
        />
        <Panel
          title="Webhooks"
          flush
          actions={
            <Button
              size="sm"
              render={
                <Link to="/orgs/$org/settings/hooks/$hookId" params={{ org, hookId: "new" }} />
              }
              data-testid="add-hook"
            >
              Add webhook
            </Button>
          }
        >
          <Region name="webhooks" rows={3}>
            <HookList org={org} />
          </Region>
        </Panel>
      </div>
    );
  },
});
