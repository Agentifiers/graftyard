import { createFileRoute, Link } from "@tanstack/react-router";
import { Deliveries, HookEditor } from "@/areas/orgs/hooks";
import { deliveriesQuery, hookQuery } from "@/areas/orgs/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

// A webhook's settings and, for an existing hook, its recent deliveries.
export const Route = createFileRoute("/orgs/$org/settings/hooks/$hookId")({
  loader: ({ context: { queryClient }, params }) => {
    const id = Number(params.hookId);
    if (!Number.isInteger(id)) return;
    void queryClient.prefetchQuery(hookQuery(params.org, id));
    void queryClient.prefetchQuery(deliveriesQuery(params.org, id));
  },
  component: HookPage,
});

function HookPage() {
  const { org, hookId } = Route.useParams();
  const id = hookId === "new" ? "new" : Number(hookId);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={id === "new" ? "Add webhook" : "Manage webhook"}
        description={
          <Link to="/orgs/$org/settings/hooks" params={{ org }} className="hover:underline">
            Back to webhooks
          </Link>
        }
      />
      <Panel title="Settings" flush>
        <Region name="webhook" rows={6}>
          <HookEditor org={org} id={id} />
        </Region>
      </Panel>
      {id !== "new" && (
        <Panel title="Recent deliveries" flush>
          <Region name="deliveries" rows={4}>
            <Deliveries org={org} hookId={id} />
          </Region>
        </Panel>
      )}
    </div>
  );
}
