import { createFileRoute, Link } from "@tanstack/react-router";
import { Deliveries, HookEditor } from "@/areas/repo-admin/hooks";
import { deliveriesQuery, hookQuery } from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

// One webhook and its recent deliveries; `new` adds a webhook.
export const Route = createFileRoute("/$owner/$repo/settings/hooks/$hookId")({
  loader: ({ context: { queryClient }, params }) => {
    const id = Number(params.hookId);
    if (!id) return;
    void queryClient.prefetchQuery(hookQuery({ ...params, id }));
    void queryClient.prefetchQuery(deliveriesQuery({ ...params, id }));
  },
  component: HookPage,
});

function HookPage() {
  const { owner, repo, hookId } = Route.useParams();
  const id = hookId === "new" ? "new" : Number(hookId);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title={
          <>
            <Link
              to="/$owner/$repo/settings/hooks"
              params={{ owner, repo }}
              className="text-muted-foreground hover:underline"
            >
              Webhooks
            </Link>
            <span className="text-muted-foreground px-1">/</span>
            {id === "new" ? "Add webhook" : "Manage webhook"}
          </>
        }
      />
      <Panel title="Settings" flush>
        <Region name="webhook" rows={6}>
          <HookEditor key={hookId} params={{ owner, repo }} id={id} />
        </Region>
      </Panel>
      {id !== "new" && (
        <Panel title="Recent deliveries" flush>
          <Region name="deliveries" rows={4}>
            <Deliveries params={{ owner, repo }} hookId={id} />
          </Region>
        </Panel>
      )}
    </div>
  );
}
