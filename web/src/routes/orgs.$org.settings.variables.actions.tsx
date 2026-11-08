import { createFileRoute } from "@tanstack/react-router";
import { variablesQuery } from "@/areas/actions/scope";
import { ConfigList } from "@/areas/actions/secrets";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/orgs/$org/settings/variables/actions")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(variablesQuery({ kind: "org", org: params.org })),
  component: OrgVariablesPage,
});

function OrgVariablesPage() {
  const { org } = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Actions variables"
        description="Plain text configuration shared with this organization's repositories."
      />
      <Region name="organization variables" rows={3}>
        <ConfigList scope={{ kind: "org", org }} kind="variable" />
      </Region>
    </div>
  );
}
