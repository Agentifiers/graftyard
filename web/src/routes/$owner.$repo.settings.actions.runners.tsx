import { createFileRoute } from "@tanstack/react-router";
import { RunnerList } from "@/areas/actions/runners";
import { runnersQuery } from "@/areas/actions/scope";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/actions/runners")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(runnersQuery({ kind: "repo", ...params })),
  component: RunnersPage,
});

function RunnersPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Runners"
        description="Self-hosted runners for this repository. Organization runners shared with it also take its jobs."
      />
      <Region name="runners" rows={4}>
        <RunnerList scope={{ kind: "repo", ...params }} />
      </Region>
    </div>
  );
}
