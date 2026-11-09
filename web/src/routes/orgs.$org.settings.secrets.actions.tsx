import { createFileRoute } from "@tanstack/react-router";
import { secretsQuery } from "@/areas/actions/scope";
import { ConfigList } from "@/areas/actions/secrets";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/orgs/$org/settings/secrets/actions")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(secretsQuery({ kind: "org", org: params.org })),
  component: OrgSecretsPage,
});

function OrgSecretsPage() {
  const { org } = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Actions secrets"
        description="Encrypted values workflows in this organization's repositories can read. Choose which repositories get each one."
      />
      <Region name="organization secrets" rows={3}>
        <ConfigList scope={{ kind: "org", org }} kind="secret" />
      </Region>
    </div>
  );
}
