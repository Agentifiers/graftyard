import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { childTeamsQuery, teamMembersQuery, teamQuery } from "@/areas/orgs/queries";
import { TeamForm } from "@/areas/orgs/teams";
import { ChildTeams, DeleteTeam, TeamHeader, TeamMembers, TeamRepos } from "@/areas/orgs/team-page";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Skeleton } from "@/components/ui/skeleton";

// A team: members, child teams, and repositories as tabs, plus edit and delete.

const TABS = ["members", "teams", "repositories"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/orgs/$org/teams/$team")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; edit?: boolean } => ({
    tab: TABS.includes(s.tab as Tab) ? (s.tab as Tab) : undefined,
    edit: s.edit === true || s.edit === "true" ? true : undefined,
  }),
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(teamQuery(params));
    void queryClient.prefetchQuery(teamMembersQuery(params));
    void queryClient.prefetchQuery(childTeamsQuery(params));
  },
  component: TeamPage,
});

const LABEL: Record<Tab, string> = {
  members: "Members",
  teams: "Child teams",
  repositories: "Repositories",
};

function TeamPage() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const tab = search.tab ?? "members";
  const setEdit = (edit: boolean) =>
    void navigate({ search: (prev) => ({ ...prev, edit: edit || undefined }), replace: true });
  return (
    <div className="flex flex-col gap-4">
      <Region name="team" rows={2} fallback={<Skeleton className="h-14 w-2/3" />}>
        <TeamHeader
          {...params}
          actions={
            <>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setEdit(true)}
                data-testid="edit-team"
              >
                Edit team
              </Button>
              <DeleteTeam {...params} />
            </>
          }
        />
      </Region>
      <nav className="-mb-px flex gap-1 border-b text-sm" aria-label="Team">
        {TABS.map((t) => (
          <Link
            key={t}
            to="/orgs/$org/teams/$team"
            params={params}
            search={t === "members" ? {} : { tab: t }}
            data-active={t === tab || undefined}
            data-tab={t}
            className="text-muted-foreground hover:text-foreground data-active:border-primary data-active:text-foreground flex items-center gap-1.5 border-b-2 border-transparent px-3 py-2 font-medium"
          >
            {LABEL[t]}
            <Region name={`${t} count`} rows={1} fallback={null}>
              <TabCount tab={t} />
            </Region>
          </Link>
        ))}
      </nav>
      <Panel flush>
        {tab === "members" && (
          <Region name="team members" rows={4}>
            <TeamMembers {...params} />
          </Region>
        )}
        {tab === "teams" && (
          <Region name="child teams" rows={3}>
            <ChildTeams {...params} />
          </Region>
        )}
        {tab === "repositories" && (
          <Region name="team repositories" rows={4}>
            <TeamRepos {...params} />
          </Region>
        )}
      </Panel>
      <Dialog open={Boolean(search.edit)} onOpenChange={setEdit}>
        <DialogContent className="sm:max-w-lg">
          {search.edit && (
            <Region name="team form" rows={4}>
              <EditForm onDone={() => setEdit(false)} />
            </Region>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function TabCount({ tab }: { tab: Tab }) {
  const params = Route.useParams();
  const { data: team } = useSuspenseQuery(teamQuery(params));
  const { data: children } = useSuspenseQuery(childTeamsQuery(params));
  const n =
    tab === "members"
      ? team.members_count
      : tab === "repositories"
        ? team.repos_count
        : children.length;
  return (
    <Badge variant="secondary" size="sm" data-count={tab}>
      {n}
    </Badge>
  );
}

function EditForm({ onDone }: { onDone: () => void }) {
  const params = Route.useParams();
  const { data: team } = useSuspenseQuery(teamQuery(params));
  return <TeamForm org={params.org} team={team} onDone={onDone} />;
}
