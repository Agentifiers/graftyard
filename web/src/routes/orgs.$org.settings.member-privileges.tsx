import { createFileRoute } from "@tanstack/react-router";
import { orgQuery } from "@/areas/core/queries";
import { MemberPrivilegesForm } from "@/areas/orgs/settings";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

// Base permission and the members_can_* flags. Each control saves on change.
export const Route = createFileRoute("/orgs/$org/settings/member-privileges")({
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(orgQuery(params.org)),
  component: () => {
    const { org } = Route.useParams();
    return (
      <div className="flex flex-col gap-4">
        <PageHeader
          title="Member privileges"
          description="What members can do without an owner. Each change saves at once."
        />
        <Region name="member privileges" rows={8}>
          <MemberPrivilegesForm org={org} />
        </Region>
      </div>
    );
  },
});
