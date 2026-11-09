import { createFileRoute, Link } from "@tanstack/react-router";
import { Collaborators, Invitations, InviteForm, Teams } from "@/areas/repo-admin/access";
import { ArchivedNotice } from "@/areas/repo-admin/kit";
import {
  collaboratorsQuery,
  invitationsQuery,
  repoTeamsQuery,
  usersQuery,
} from "@/areas/repo-admin/queries";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/settings/access")({
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(collaboratorsQuery(params));
    void queryClient.prefetchQuery(invitationsQuery(params));
    void queryClient.prefetchQuery(repoTeamsQuery(params));
    void queryClient.prefetchQuery(usersQuery());
  },
  component: AccessPage,
});

function AccessPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-6" data-testid="settings-access">
      <PageHeader
        title="Collaborators and teams"
        description="Give people and teams access to this repository, and choose what each can do."
      />
      <Region name="repository settings" fallback={null}>
        <ArchivedNotice params={params} />
      </Region>
      <Panel title="Add people" flush>
        <Region name="invite" rows={1}>
          <InviteForm params={params} />
        </Region>
      </Panel>
      <Panel title="Direct access" flush>
        <Region name="collaborators" rows={3}>
          <Collaborators params={params} />
        </Region>
      </Panel>
      <Panel title="Pending invitations" flush>
        <Region name="invitations" rows={2}>
          <Invitations params={params} />
        </Region>
      </Panel>
      <Panel
        title="Teams with access"
        flush
        actions={
          <Link
            to="/orgs/$org/settings"
            params={{ org: params.owner }}
            className="text-primary text-xs hover:underline"
          >
            Manage teams in the organization
          </Link>
        }
      >
        <Region name="teams" rows={2}>
          <Teams params={params} />
        </Region>
      </Panel>
    </div>
  );
}
