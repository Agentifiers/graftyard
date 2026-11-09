import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Search01Icon, UserAdd01Icon } from "@hugeicons/core-free-icons";
import { InvitationList, InviteForm, MemberList, OutsideList } from "@/areas/orgs/people";
import {
  invitationsQuery,
  membersQuery,
  outsideCollaboratorsQuery,
  ownersQuery,
  publicMembersQuery,
  twoFactorDisabledQuery,
} from "@/areas/orgs/queries";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

// Members with role and 2FA, pending invitations, and outside collaborators, as tabs.

const TABS = ["members", "invitations", "outside"] as const;
type Tab = (typeof TABS)[number];
type Search = {
  tab?: Tab;
  q?: string;
  role?: "admin" | "member";
  tfa?: "disabled";
  invite?: boolean;
};

export const Route = createFileRoute("/orgs/$org/people")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    tab: TABS.includes(s.tab as Tab) ? (s.tab as Tab) : undefined,
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    role: s.role === "admin" || s.role === "member" ? s.role : undefined,
    tfa: s.tfa === "disabled" ? "disabled" : undefined,
    invite: s.invite === true || s.invite === "true" ? true : undefined,
  }),
  loader: ({ context: { queryClient }, params: { org } }) => {
    void queryClient.prefetchQuery(membersQuery(org));
    void queryClient.prefetchQuery(ownersQuery(org));
    void queryClient.prefetchQuery(twoFactorDisabledQuery(org));
    void queryClient.prefetchQuery(publicMembersQuery(org));
    void queryClient.prefetchQuery(invitationsQuery(org));
  },
  component: PeoplePage,
});

const LABEL: Record<Tab, string> = {
  members: "Members",
  invitations: "Pending invitations",
  outside: "Outside collaborators",
};

function PeoplePage() {
  const { org } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const tab = search.tab ?? "members";
  const [q, setQ] = useState(search.q ?? "");
  const set = (patch: Partial<Search>) =>
    void navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <nav className="flex gap-1 text-sm" aria-label="People">
          {TABS.map((t) => (
            <Link
              key={t}
              to="/orgs/$org/people"
              params={{ org }}
              search={t === "members" ? {} : { tab: t }}
              data-active={t === tab || undefined}
              data-tab={t}
              className="text-muted-foreground hover:bg-muted data-active:bg-muted data-active:text-foreground flex items-center gap-1.5 rounded-sm px-3 py-1.5 font-medium"
            >
              {LABEL[t]}
              <Count org={org} tab={t} />
            </Link>
          ))}
        </nav>
        <Button
          size="sm"
          className="ml-auto"
          onClick={() => set({ invite: true })}
          data-testid="invite-member"
        >
          <HugeiconsIcon icon={UserAdd01Icon} strokeWidth={2} data-icon="inline-start" />
          Invite member
        </Button>
      </div>
      {tab === "members" && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <InputGroup className="max-w-sm flex-1">
              <InputGroupAddon>
                <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
              </InputGroupAddon>
              <InputGroupInput
                placeholder="Find a member…"
                aria-label="Find a member"
                value={q}
                onChange={(e) => {
                  setQ(e.target.value);
                  set({ q: e.target.value || undefined });
                }}
              />
            </InputGroup>
            <NativeSelect
              size="sm"
              aria-label="Role"
              value={search.role ?? ""}
              onChange={(e) => set({ role: (e.target.value || undefined) as Search["role"] })}
            >
              <NativeSelectOption value="">Every role</NativeSelectOption>
              <NativeSelectOption value="admin">Owners</NativeSelectOption>
              <NativeSelectOption value="member">Members</NativeSelectOption>
            </NativeSelect>
            <NativeSelect
              size="sm"
              aria-label="Two-factor authentication"
              value={search.tfa ?? ""}
              onChange={(e) => set({ tfa: (e.target.value || undefined) as Search["tfa"] })}
            >
              <NativeSelectOption value="">2FA: any</NativeSelectOption>
              <NativeSelectOption value="disabled">2FA: off</NativeSelectOption>
            </NativeSelect>
          </div>
          <Panel flush>
            <Region name="members" rows={6}>
              <MemberList org={org} q={q} filter={{ role: search.role, tfa: search.tfa }} />
            </Region>
          </Panel>
        </>
      )}
      {tab === "invitations" && (
        <Panel flush>
          <Region name="invitations" rows={3}>
            <InvitationList org={org} />
          </Region>
        </Panel>
      )}
      {tab === "outside" && (
        <Panel flush>
          <Region name="outside collaborators" rows={3}>
            <OutsideList org={org} />
          </Region>
        </Panel>
      )}
      <Dialog open={Boolean(search.invite)} onOpenChange={(o) => set({ invite: o || undefined })}>
        <DialogContent className="sm:max-w-lg">
          {search.invite && (
            <Region name="invite form" rows={4}>
              <InviteForm org={org} onDone={() => set({ invite: undefined })} />
            </Region>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

// The count beside each tab. A failed count shows nothing; its tab's region shows the error.
function Count({ org, tab }: { org: string; tab: Tab }) {
  const members = useQuery({ ...membersQuery(org), enabled: tab === "members" });
  const invites = useQuery({ ...invitationsQuery(org), enabled: tab === "invitations" });
  const outside = useQuery({ ...outsideCollaboratorsQuery(org), enabled: tab === "outside" });
  const data = { members, invitations: invites, outside }[tab].data;
  if (!data) return null;
  return (
    <Badge variant="secondary" size="sm" data-count={tab}>
      {data.length}
    </Badge>
  );
}
