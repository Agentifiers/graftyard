import { createFileRoute } from "@tanstack/react-router";
import { orgQuery } from "@/areas/core/queries";
import { renameQuery } from "@/areas/orgs/queries";
import { DangerZone, OrgProfileForm } from "@/areas/orgs/settings";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

// PATCH /orgs/{org} for the public profile and billing email, plus rename and delete.
export const Route = createFileRoute("/orgs/$org/settings/profile")({
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(orgQuery(params.org));
    void queryClient.prefetchQuery(renameQuery(params.org));
  },
  component: ProfileSettings,
});

function ProfileSettings() {
  const { org } = Route.useParams();
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Organization profile"
        description="What people see on the organization's pages."
      />
      <Region name="organization profile" rows={8}>
        <OrgProfileForm org={org} />
      </Region>
      <Region name="danger zone" rows={2}>
        <DangerZone org={org} />
      </Region>
    </div>
  );
}
