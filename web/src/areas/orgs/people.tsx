import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Alert02Icon, MoreHorizontalIcon, Shield01Icon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { Ago, UserAvatar } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import {
  Autocomplete,
  AutocompleteContent,
  AutocompleteEmpty,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from "@/components/reui/autocomplete";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { ext, send, type InvitationRole, type OrganizationInvitation } from "./api";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  FieldMessage,
  FormFailure,
  optimistic,
  refetch,
  RoleBadge,
} from "./kit";
import {
  invitationsQuery,
  membersQuery,
  outsideCollaboratorsQuery,
  ownersQuery,
  publicMembersQuery,
  teamsQuery,
  twoFactorDisabledQuery,
  usersDirectoryQuery,
} from "./queries";

type User = Schemas["simple-user"];

// ---- Members ----

export type MemberFilter = { role?: "admin" | "member"; tfa?: "disabled" };

// GET /orgs/{org}/members, with owners from ?role=admin and the 2FA column from
// ?filter=2fa_disabled, so each row shows what GHES knows.
export function MemberList({ org, q, filter }: { org: string; q: string; filter: MemberFilter }) {
  const { data: members } = useSuspenseQuery(membersQuery(org));
  const { data: owners } = useSuspenseQuery(ownersQuery(org));
  const { data: noTfa } = useSuspenseQuery(twoFactorDisabledQuery(org));
  const { data: publics } = useSuspenseQuery(publicMembersQuery(org));
  const { data: viewer } = useSuspenseQuery(userQuery());
  const ownerSet = new Set(owners.map((o) => o.login));
  const noTfaSet = new Set(noTfa.map((o) => o.login));
  const publicSet = new Set(publics.map((o) => o.login));
  const text = q.trim().toLowerCase();
  const rows = members.filter(
    (m) =>
      (!text || `${m.login} ${m.name ?? ""}`.toLowerCase().includes(text)) &&
      (!filter.role || (filter.role === "admin") === ownerSet.has(m.login)) &&
      (!filter.tfa || noTfaSet.has(m.login)),
  );
  if (!rows.length) {
    return (
      <EmptyRow>
        {members.length ? "No member matches." : "This organization has no members."}
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="members">
      {rows.map((m) => (
        <MemberRow
          key={m.id}
          org={org}
          user={m}
          owner={ownerSet.has(m.login)}
          tfa={!noTfaSet.has(m.login)}
          isPublic={publicSet.has(m.login)}
          self={m.login === viewer.login}
          lastOwner={ownerSet.size === 1 && ownerSet.has(m.login)}
        />
      ))}
    </ul>
  );
}

function MemberRow({
  org,
  user,
  owner,
  tfa,
  isPublic,
  self,
  lastOwner,
}: {
  org: string;
  user: User;
  owner: boolean;
  tfa: boolean;
  isPublic: boolean;
  self: boolean;
  lastOwner: boolean;
}) {
  const [confirm, setConfirm] = useState<"remove" | "convert">();
  const ownersKey = ownersQuery(org).queryKey;
  const membersKey = membersQuery(org).queryKey;

  const role = useMutation({
    mutationFn: (next: "admin" | "member") =>
      send(
        api.PUT("/orgs/{org}/memberships/{username}", {
          params: { path: { org, username: user.login } },
          body: { role: next },
        }),
      ),
    onMutate: (next) =>
      optimistic<User[]>(ownersKey, (old) =>
        next === "admin" ? [...old, user] : old.filter((o) => o.login !== user.login),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not change ${user.login}'s role`)(e);
    },
    onSuccess: (_d, next) =>
      toast.success(`${user.login} is now ${next === "admin" ? "an owner" : "a member"}`),
    onSettled: () => refetch(ownersKey),
  });

  const remove = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/members/{username}", {
          params: { path: { org, username: user.login } },
        }),
      ),
    onMutate: () => {
      setConfirm(undefined);
      return optimistic<User[]>(membersKey, (old) => old.filter((o) => o.login !== user.login));
    },
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not remove ${user.login}`)(e);
    },
    onSuccess: () => toast.success(`Removed ${user.login} from ${org}`),
    onSettled: () =>
      refetch(["orgs", org, "members"], ["orgs", org, "teams"], ["orgs", org, "public_members"]),
  });

  const convert = useMutation({
    mutationFn: () =>
      send(
        api.PUT("/orgs/{org}/outside_collaborators/{username}", {
          params: { path: { org, username: user.login } },
          body: { async: false },
        }),
      ),
    onMutate: () => {
      setConfirm(undefined);
      return optimistic<User[]>(membersKey, (old) => old.filter((o) => o.login !== user.login));
    },
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not convert ${user.login}`)(e);
    },
    onSuccess: () => toast.success(`${user.login} is now an outside collaborator`),
    onSettled: () =>
      refetch(["orgs", org, "members"], outsideCollaboratorsQuery(org).queryKey, [
        "orgs",
        org,
        "teams",
      ]),
  });

  const publicize = useMutation({
    mutationFn: (show: boolean) =>
      send(
        show
          ? api.PUT("/orgs/{org}/public_members/{username}", {
              params: { path: { org, username: user.login } },
            })
          : api.DELETE("/orgs/{org}/public_members/{username}", {
              params: { path: { org, username: user.login } },
            }),
      ),
    onMutate: (show) =>
      optimistic<User[]>(publicMembersQuery(org).queryKey, (old) =>
        show ? [...old, user] : old.filter((o) => o.login !== user.login),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Membership visibility not changed")(e);
    },
    onSettled: () => refetch(publicMembersQuery(org).queryKey, ["users", user.login, "orgs"]),
  });

  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-2.5" data-member={user.login}>
      <UserAvatar user={user} size="default" />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner"
          params={{ owner: user.login }}
          className="text-primary text-sm font-semibold hover:underline"
        >
          {user.name ?? user.login}
        </Link>
        <p className="text-muted-foreground text-xs">
          {user.login}
          {self && " · you"}
        </p>
      </div>
      <Badge
        variant={tfa ? "success-light" : "warning-light"}
        size="sm"
        data-2fa={tfa ? "on" : "off"}
        title={tfa ? "Two-factor authentication is on" : "Two-factor authentication is off"}
      >
        <HugeiconsIcon icon={tfa ? Shield01Icon : Alert02Icon} strokeWidth={2} />
        {tfa ? "2FA" : "No 2FA"}
      </Badge>
      <Badge variant="outline" size="sm">
        {isPublic ? "Public" : "Private"}
      </Badge>
      <NativeSelect
        size="sm"
        aria-label={`Role for ${user.login}`}
        value={owner ? "admin" : "member"}
        disabled={role.isPending || (lastOwner && owner)}
        title={lastOwner ? "An organization needs at least one owner." : undefined}
        onChange={(e) => role.mutate(e.target.value as "admin" | "member")}
        data-role-select={user.login}
      >
        <NativeSelectOption value="member">Member</NativeSelectOption>
        <NativeSelectOption value="admin">Owner</NativeSelectOption>
      </NativeSelect>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button variant="ghost" size="icon-sm" aria-label={`Manage ${user.login}`}>
              <HugeiconsIcon icon={MoreHorizontalIcon} strokeWidth={2} />
            </Button>
          }
        />
        <DropdownMenuContent align="end">
          {self && (
            <>
              <DropdownMenuItem onClick={() => publicize.mutate(!isPublic)}>
                {isPublic ? "Make membership private" : "Make membership public"}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
            </>
          )}
          <DropdownMenuItem disabled={owner} onClick={() => setConfirm("convert")}>
            Convert to outside collaborator
          </DropdownMenuItem>
          <DropdownMenuItem
            variant="destructive"
            disabled={lastOwner}
            onClick={() => setConfirm("remove")}
            data-remove={user.login}
          >
            Remove from organization
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Confirm
        open={confirm === "remove"}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title={`Remove ${user.login} from ${org}?`}
        description={`${user.login} loses access to every private and internal repository of ${org}, and leaves all of its teams.`}
        action="Remove member"
        pending={remove.isPending}
        onConfirm={() => remove.mutate()}
      />
      <Confirm
        open={confirm === "convert"}
        onOpenChange={(o) => !o && setConfirm(undefined)}
        title={`Convert ${user.login} to an outside collaborator?`}
        description={`${user.login} leaves the organization and its teams, but keeps access to the repositories they reach today.`}
        action="Convert"
        pending={convert.isPending}
        onConfirm={() => convert.mutate()}
      />
    </li>
  );
}

// ---- Invite ----

type InviteValues = {
  who: string;
  role: InvitationRole;
  teams: number[];
};

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

// POST /orgs/{org}/invitations by user id or by email, with a role and teams.
export function InviteForm({ org, onDone }: { org: string; onDone: () => void }) {
  const { data: people } = useSuspenseQuery(usersDirectoryQuery());
  const { data: members } = useSuspenseQuery(membersQuery(org));
  const { data: invites } = useSuspenseQuery(invitationsQuery(org));
  const { data: teams } = useSuspenseQuery(teamsQuery(org));
  const [failure, setFailure] = useState<string>();
  const taken = new Set([
    ...members.map((m) => m.login),
    ...invites.flatMap((i) => (i.login ? [i.login] : [])),
  ]);
  const candidates = people.filter((p) => !taken.has(p.login));
  const key = invitationsQuery(org).queryKey;
  const invite = useMutation({
    mutationFn: (v: InviteValues) => {
      const user = people.find((p) => p.login === v.who.trim());
      return send(
        ext.POST("/orgs/{org}/invitations", {
          params: { path: { org } },
          body: user
            ? { invitee_id: user.id, role: v.role, team_ids: v.teams }
            : { email: v.who.trim(), role: v.role, team_ids: v.teams },
        }),
      );
    },
    onSettled: () => refetch(key),
  });
  const form = useForm({
    defaultValues: { who: "", role: "direct_member", teams: [] } as InviteValues,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const inv = await invite.mutateAsync(value);
        toast.success(`Invited ${inv.login ?? inv.email}`);
        onDone();
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, { invitee_id: "who", email: "who", team_ids: "teams" }),
        );
        failed("Invitation not sent")(e);
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        e.stopPropagation();
        void form.handleSubmit();
      }}
    >
      <DialogHeader>
        <DialogTitle>Invite a member to {org}</DialogTitle>
        <DialogDescription>
          Search by username or name, or type an email address for someone without an account.
        </DialogDescription>
      </DialogHeader>
      <form.Field
        name="who"
        validators={{
          onSubmit: ({ value }) => {
            const v = value.trim();
            if (!v) return "Choose a person or type an email address.";
            if (people.some((p) => p.login === v)) {
              return taken.has(v) ? `${v} is already a member or invited.` : undefined;
            }
            return EMAIL_RE.test(v)
              ? undefined
              : "No user has that name. Type a full email address.";
          },
        }}
      >
        {(field) => (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="invite-who">Username, name, or email</Label>
            <Autocomplete
              items={candidates.map((c) => c.login)}
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v)}
              openOnInputClick
            >
              <AutocompleteInput
                id="invite-who"
                placeholder="lin, Kenji, or jo@contractor.test"
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
                autoComplete="off"
              />
              <AutocompleteContent>
                <AutocompleteEmpty>
                  Nobody matches. Press Invite to send to an email.
                </AutocompleteEmpty>
                <AutocompleteList>
                  {(login: string) => {
                    const p = candidates.find((c) => c.login === login);
                    return (
                      <AutocompleteItem key={login} value={login} data-candidate={login}>
                        {p && <UserAvatar user={p} />}
                        <span className="font-medium">{login}</span>
                        <span className="text-muted-foreground truncate">{p?.name}</span>
                      </AutocompleteItem>
                    );
                  }}
                </AutocompleteList>
              </AutocompleteContent>
            </Autocomplete>
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Field name="role">
        {(field) => (
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-1.5 text-sm font-medium">Role in the organization</legend>
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as InvitationRole)}
            >
              {(
                [
                  [
                    "direct_member",
                    "Member",
                    "Sees every member and non-secret team, and can create repositories if allowed.",
                  ],
                  [
                    "admin",
                    "Owner",
                    "Full administrative rights over the organization and every repository.",
                  ],
                  [
                    "billing_manager",
                    "Billing manager",
                    "Manages billing settings only. No repository access.",
                  ],
                ] as const
              ).map(([value, label, help]) => (
                <label key={value} className="flex items-start gap-2 text-sm">
                  <RadioGroupItem value={value} className="mt-0.5" />
                  <span>
                    <span className="font-medium">{label}</span>
                    <span className="text-muted-foreground block text-xs">{help}</span>
                  </span>
                </label>
              ))}
            </RadioGroup>
          </fieldset>
        )}
      </form.Field>
      <form.Field name="teams">
        {(field) => (
          <fieldset className="flex flex-col gap-1.5">
            <legend className="mb-1.5 text-sm font-medium">Teams</legend>
            <div className="flex max-h-40 flex-col gap-1 overflow-y-auto rounded-sm border p-2">
              {teams.map((t) => (
                <label key={t.id} className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={field.state.value.includes(t.id)}
                    onCheckedChange={(on) =>
                      field.handleChange(
                        on
                          ? [...field.state.value, t.id]
                          : field.state.value.filter((x) => x !== t.id),
                      )
                    }
                    data-invite-team={t.slug}
                  />
                  {t.name}
                  {t.parent && (
                    <span className="text-muted-foreground text-xs">in {t.parent.name}</span>
                  )}
                </label>
              ))}
            </div>
            <FieldMessage field={field} />
          </fieldset>
        )}
      </form.Field>
      <FormFailure>{failure}</FormFailure>
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-testid="send-invite">
              {submitting ? "Sending…" : "Send invitation"}
            </Button>
          )}
        </form.Subscribe>
      </DialogFooter>
    </form>
  );
}

// ---- Pending invitations ----

export function InvitationList({ org }: { org: string }) {
  const { data } = useSuspenseQuery(invitationsQuery(org));
  if (!data.length) return <EmptyRow>No pending invitations.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="invitations">
      {data.map((i) => (
        <InvitationRow key={i.id} org={org} invitation={i} />
      ))}
    </ul>
  );
}

function InvitationRow({
  org,
  invitation: i,
}: {
  org: string;
  invitation: OrganizationInvitation;
}) {
  const key = invitationsQuery(org).queryKey;
  const cancel = useMutation({
    mutationFn: () =>
      send(
        ext.DELETE("/orgs/{org}/invitations/{invitation_id}", {
          params: { path: { org, invitation_id: i.id } },
        }),
      ),
    onMutate: () =>
      optimistic<OrganizationInvitation[]>(key, (old) => old.filter((x) => x.id !== i.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Invitation not cancelled")(e);
    },
    onSuccess: () => toast.success(`Cancelled the invitation for ${i.login ?? i.email}`),
    onSettled: () => refetch(key),
  });
  const who = i.login ?? i.email ?? "Unknown";
  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-2.5" data-invitation={who}>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-semibold">
          {i.login ? (
            <Link to="/$owner" params={{ owner: i.login }} className="text-primary hover:underline">
              {i.login}
            </Link>
          ) : (
            i.email
          )}
        </p>
        <p className="text-muted-foreground text-xs">
          Invited by {i.inviter.login} <Ago at={i.created_at} />
          {i.team_count > 0 && ` · ${i.team_count} ${i.team_count === 1 ? "team" : "teams"}`}
        </p>
      </div>
      <RoleBadge role={i.role === "direct_member" ? "member" : i.role} />
      <Button
        variant="outline"
        size="sm"
        onClick={() => cancel.mutate()}
        disabled={cancel.isPending}
        data-cancel-invitation={who}
      >
        Cancel invitation
      </Button>
    </li>
  );
}

// ---- Outside collaborators ----

export function OutsideList({ org }: { org: string }) {
  const { data } = useSuspenseQuery(outsideCollaboratorsQuery(org));
  if (!data.length) {
    return (
      <EmptyRow>No outside collaborators. Repository admins add them per repository.</EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="outside">
      {data.map((u) => (
        <OutsideRow key={u.id} org={org} user={u} />
      ))}
    </ul>
  );
}

function OutsideRow({ org, user }: { org: string; user: User }) {
  const [confirm, setConfirm] = useState(false);
  const key = outsideCollaboratorsQuery(org).queryKey;
  const remove = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/outside_collaborators/{username}", {
          params: { path: { org, username: user.login } },
        }),
      ),
    onMutate: () => {
      setConfirm(false);
      return optimistic<User[]>(key, (old) => old.filter((o) => o.login !== user.login));
    },
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not remove ${user.login}`)(e);
    },
    onSuccess: () => toast.success(`Removed ${user.login} from every ${org} repository`),
    onSettled: () => refetch(key),
  });
  return (
    <li className="flex items-center gap-3 px-4 py-2.5" data-outside={user.login}>
      <UserAvatar user={user} size="default" />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner"
          params={{ owner: user.login }}
          className="text-primary text-sm font-semibold hover:underline"
        >
          {user.name ?? user.login}
        </Link>
        <p className="text-muted-foreground text-xs">{user.login}</p>
      </div>
      <Button variant="outline" size="sm" onClick={() => setConfirm(true)}>
        Remove
      </Button>
      <Confirm
        open={confirm}
        onOpenChange={setConfirm}
        title={`Remove ${user.login} from all repositories?`}
        description={`${user.login} loses access to every ${org} repository they collaborate on.`}
        action="Remove collaborator"
        pending={remove.isPending}
        onConfirm={() => remove.mutate()}
      />
    </li>
  );
}
