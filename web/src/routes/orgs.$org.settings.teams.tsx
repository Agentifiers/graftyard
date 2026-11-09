import { createFileRoute } from "@tanstack/react-router";
import { teamSettingsQuery } from "@/areas/orgs/queries";
import { TeamSettingsForm } from "@/areas/orgs/settings";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

// Who may create teams, and the defaults a new team starts with.
export const Route = createFileRoute("/orgs/$org/settings/teams")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(teamSettingsQuery(params.org)),
  component: () => {
    const { org } = Route.useParams();
    return (
      <div className="flex flex-col gap-4">
        <PageHeader title="Teams" description="Team creation and the defaults for new teams." />
        <Region name="team settings" rows={4}>
          <TeamSettingsForm org={org} />
        </Region>
      </div>
    );
  },
});
