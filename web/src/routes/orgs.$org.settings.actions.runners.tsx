import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { runnerGroupsQuery } from "@/areas/actions/queries";
import { RunnerList } from "@/areas/actions/runners";
import { runnersQuery } from "@/areas/actions/scope";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/orgs/$org/settings/actions/runners")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(runnersQuery({ kind: "org", org: params.org }));
    void context.queryClient.prefetchQuery(runnerGroupsQuery(params.org));
  },
  component: OrgRunnersPage,
});

function OrgRunnersPage() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Runners"
        description="Self-hosted runners every repository in this organization can use, by runner group."
      />
      <Region name="organization runners" rows={4}>
        <OrgRunners />
      </Region>
    </div>
  );
}

function OrgRunners() {
  const { org } = Route.useParams();
  const { data } = useSuspenseQuery(runnerGroupsQuery(org));
  return <RunnerList scope={{ kind: "org", org }} groups={data.runner_groups} />;
}
