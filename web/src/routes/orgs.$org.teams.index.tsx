import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, Search01Icon } from "@hugeicons/core-free-icons";
import { teamSettingsQuery, teamsQuery } from "@/areas/orgs/queries";
import { TeamForm, TeamTree } from "@/areas/orgs/teams";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";

// GET /orgs/{org}/teams drawn as a tree by each team's parent, and POST to create one.
export const Route = createFileRoute("/orgs/$org/teams/")({
  validateSearch: (s: Record<string, unknown>): { q?: string; new?: boolean } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    new: s.new === true || s.new === "true" ? true : undefined,
  }),
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(teamsQuery(params.org));
    void context.queryClient.prefetchQuery(teamSettingsQuery(params.org));
  },
  component: TeamsPage,
});

function TeamsPage() {
  const { org } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const [filter, setFilter] = useState(search.q ?? "");
  const setOpen = (open: boolean) =>
    void navigate({ search: (prev) => ({ ...prev, new: open || undefined }), replace: true });
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <InputGroup className="max-w-sm flex-1">
          <InputGroupAddon>
            <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="Find a team…"
            aria-label="Find a team"
            value={filter}
            onChange={(e) => {
              setFilter(e.target.value);
              void navigate({
                search: (prev) => ({ ...prev, q: e.target.value || undefined }),
                replace: true,
              });
            }}
          />
        </InputGroup>
        <Button size="sm" className="ml-auto" onClick={() => setOpen(true)} data-testid="new-team">
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          New team
        </Button>
      </div>
      <Panel title="Teams" flush>
        <Region name="teams" rows={5}>
          <TeamTree org={org} filter={filter} />
        </Region>
      </Panel>
      <Dialog open={Boolean(search.new)} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-lg">
          {search.new && (
            <Region name="team form" rows={4}>
              <TeamForm org={org} onDone={() => setOpen(false)} />
            </Region>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}
