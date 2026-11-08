import { createFileRoute, Link } from "@tanstack/react-router";
import { HookList } from "@/areas/repo-admin/hooks";
import { hooksQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/$owner/$repo/settings/hooks/")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(hooksQuery(params)),
  component: HooksPage,
});

function HooksPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4" data-testid="settings-hooks">
      <PageHeader
        title="Webhooks"
        description="GHES sends a POST with event details to each URL when a subscribed event happens."
      />
      <Panel
        title="Webhooks"
        flush
        actions={
          <Button
            size="sm"
            render={
              <Link
                to="/$owner/$repo/settings/hooks/$hookId"
                params={{ ...params, hookId: "new" }}
              />
            }
          >
            Add webhook
          </Button>
        }
      >
        <Region name="webhooks" rows={2}>
          <HookList params={params} />
        </Region>
      </Panel>
    </div>
  );
}
