import { useState } from "react";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, UserGroupIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { repoQuery, userQuery } from "@/areas/core/queries";
import { Ago, UserAvatar } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { call, failed, optimistic, refetch } from "./client";
import { ROLE_INFO, ROLES, roleOf, type Role } from "./domain";
import { EmptyRow, Suggest, type RepoParams } from "./kit";
import { collaboratorsQuery, invitationsQuery, repoTeamsQuery, usersQuery } from "./queries";

type Collaborator = Schemas["collaborator"];
type Invitation = Schemas["repository-invitation"];

function RoleSelect({
  value,
  onChange,
  label,
  disabled,
}: {
  value: Role;
  onChange: (r: Role) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <NativeSelect
      size="sm"
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as Role)}
      title={ROLE_INFO[value].description}
    >
      {ROLES.map((r) => (
        <NativeSelectOption key={r} value={r}>
          {r[0].toUpperCase() + r.slice(1)}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}

// ---- Invite ----

export function InviteForm({ params }: { params: RepoParams }) {
  const { data: people } = useSuspenseQuery(usersQuery());
  const { data: collaborators } = useSuspenseQuery(collaboratorsQuery(params));
  const { data: invitations } = useSuspenseQuery(invitationsQuery(params));
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const [login, setLogin] = useState<string>();
  const [role, setRole] = useState<Role>("write");
  const [error, setError] = useState<string>();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const taken = new Set([
    viewer.login,
    repo.owner.login,
    ...collaborators.map((c) => c.login),
    ...invitations.flatMap((i) => (i.invitee ? [i.invitee.login] : [])),
  ]);
  const candidates = people.map((p) => p.login).filter((l) => !taken.has(l));
  const invite = useMutation({
    mutationFn: (v: { login: string; role: Role }) =>
      call(
        api.PUT("/repos/{owner}/{repo}/collaborators/{username}", {
          params: { path: { ...params, username: v.login } },
          body: { permission: ROLE_INFO[v.role].api },
        }),
      ),
    onError: (e) => {
      setError(e instanceof Error ? e.message : String(e));
      failed("Invitation not sent")(e);
    },
    onSuccess: (_d, v) => {
      setError(undefined);
      setLogin(undefined);
      toast.success(`Invited ${v.login} with ${v.role} access`);
    },
    onSettled: () => refetch(invitationsQuery(params).queryKey),
  });
  const person = people.find((p) => p.login === login);
  return (
    <form
      className="flex flex-col gap-2 px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!login) return setError("Choose someone to invite.");
        invite.mutate({ login, role });
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-56 flex-1">
          {person ? (
            <div
              className="flex h-8 items-center gap-2 rounded-sm border px-2 text-sm"
              data-testid="invitee"
            >
              <UserAvatar user={person} />
              <span className="font-medium">{person.login}</span>
              <span className="text-muted-foreground truncate">{person.name}</span>
              <button
                type="button"
                className="text-muted-foreground hover:text-foreground ml-auto text-xs"
                onClick={() => setLogin(undefined)}
              >
                Change
              </button>
            </div>
          ) : (
            <Suggest
              label="Search by username"
              placeholder="Search by username or name"
              items={candidates}
              disabled={repo.archived}
              allowFree
              onPick={(v) => {
                setError(undefined);
                setLogin(v);
              }}
              render={(l) => {
                const p = people.find((x) => x.login === l);
                return (
                  <span className="flex items-center gap-2">
                    {p && <UserAvatar user={p} />}
                    <span className="font-medium">{l}</span>
                    <span className="text-muted-foreground">{p?.name}</span>
                  </span>
                );
              }}
            />
          )}
        </div>
        <RoleSelect
          value={role}
          onChange={setRole}
          label="Role for the invitee"
          disabled={repo.archived}
        />
        <Button type="submit" size="sm" disabled={invite.isPending || repo.archived}>
          {invite.isPending ? "Inviting…" : "Invite"}
        </Button>
      </div>
      <p className="text-muted-foreground text-xs">{ROLE_INFO[role].description}</p>
      {error && (
        <p className="text-destructive text-xs" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

// ---- Collaborators ----

export function Collaborators({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(collaboratorsQuery(params));
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const key = collaboratorsQuery(params).queryKey;
  const change = useMutation({
    mutationFn: (v: { login: string; role: Role }) =>
      call(
        api.PUT("/repos/{owner}/{repo}/collaborators/{username}", {
          params: { path: { ...params, username: v.login } },
          body: { permission: ROLE_INFO[v.role].api },
        }),
      ),
    onMutate: (v) =>
      optimistic<Collaborator[]>(key, (old) =>
        old.map((c) => (c.login === v.login ? { ...c, role_name: v.role } : c)),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Role not changed")(e);
    },
    onSuccess: (_d, v) => toast.success(`${v.login} now has ${v.role} access`),
    onSettled: () => refetch(key),
  });
  const remove = useMutation({
    mutationFn: (login: string) =>
      call(
        api.DELETE("/repos/{owner}/{repo}/collaborators/{username}", {
          params: { path: { ...params, username: login } },
        }),
      ),
    onMutate: (login) =>
      optimistic<Collaborator[]>(key, (old) => old.filter((c) => c.login !== login)),
    onError: (e, _l, rollback) => {
      rollback?.();
      failed("Collaborator not removed")(e);
    },
    onSuccess: (_d, login) => toast.success(`Removed ${login}`),
    onSettled: () => refetch(key),
  });
  if (data.length === 0) return <EmptyRow>No one has direct access yet.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="collaborators">
      {data.map((c) => {
        const role = roleOf(c.role_name ?? "") ?? "read";
        const isOwner = c.login === repo.owner.login;
        return (
          <li key={c.login} className="flex items-center gap-3 px-4 py-2" data-login={c.login}>
            <UserAvatar user={c} />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">{c.login}</div>
              <div className="text-muted-foreground text-xs">{c.name ?? "Collaborator"}</div>
            </div>
            {isOwner ? (
              <Badge variant="outline">Owner</Badge>
            ) : (
              <>
                <RoleSelect
                  value={role}
                  label={`Role for ${c.login}`}
                  disabled={repo.archived}
                  onChange={(r) => change.mutate({ login: c.login, role: r })}
                />
                <Button
                  variant="ghost"
                  size="icon-sm"
                  aria-label={`Remove ${c.login}`}
                  disabled={repo.archived}
                  onClick={() => remove.mutate(c.login)}
                >
                  <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                </Button>
              </>
            )}
          </li>
        );
      })}
    </ul>
  );
}

// ---- Invitations ----

export function Invitations({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(invitationsQuery(params));
  const key = invitationsQuery(params).queryKey;
  const cancel = useMutation({
    mutationFn: (id: number) =>
      call(
        api.DELETE("/repos/{owner}/{repo}/invitations/{invitation_id}", {
          params: { path: { ...params, invitation_id: id } },
        }),
      ),
    onMutate: (id) => optimistic<Invitation[]>(key, (old) => old.filter((i) => i.id !== id)),
    onError: (e, _id, rollback) => {
      rollback?.();
      failed("Invitation not cancelled")(e);
    },
    onSuccess: () => toast.success("Invitation cancelled"),
    onSettled: () => refetch(key),
  });
  const update = useMutation({
    mutationFn: (v: { id: number; role: Role }) =>
      call(
        api.PATCH("/repos/{owner}/{repo}/invitations/{invitation_id}", {
          params: { path: { ...params, invitation_id: v.id } },
          body: { permissions: v.role },
        }),
      ),
    onMutate: (v) =>
      optimistic<Invitation[]>(key, (old) =>
        old.map((i) => (i.id === v.id ? { ...i, permissions: v.role } : i)),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Invitation not changed")(e);
    },
    onSettled: () => refetch(key),
  });
  if (data.length === 0) return <EmptyRow>No pending invitations.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="invitations">
      {data.map((i) => (
        <li
          key={i.id}
          className="flex items-center gap-3 px-4 py-2"
          data-invitee={i.invitee?.login}
        >
          <UserAvatar user={i.invitee} />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium">{i.invitee?.login}</div>
            <div className="text-muted-foreground text-xs">
              Invited by {i.inviter?.login} <Ago at={i.created_at} />
              {i.expired && " · expired"}
            </div>
          </div>
          <Badge variant="warning-light">Pending</Badge>
          <RoleSelect
            value={roleOf(i.permissions) ?? "read"}
            label={`Invited role for ${i.invitee?.login}`}
            onChange={(r) => update.mutate({ id: i.id, role: r })}
          />
          <Button variant="outline" size="sm" onClick={() => cancel.mutate(i.id)}>
            Cancel invite
          </Button>
        </li>
      ))}
    </ul>
  );
}

// ---- Teams ----

export function Teams({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(repoTeamsQuery(params));
  if (data.length === 0) return <EmptyRow>No teams have access.</EmptyRow>;
  return (
    <ul className="divide-y" data-testid="teams">
      {data.map((t) => {
        const role = roleOf(t.permission) ?? "read";
        return (
          <li key={t.id} className="flex items-center gap-3 px-4 py-2">
            <HugeiconsIcon
              icon={UserGroupIcon}
              strokeWidth={2}
              className="text-muted-foreground size-5"
            />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium">
                {params.owner}/{t.slug}
              </div>
              <div className="text-muted-foreground text-xs">{t.description}</div>
            </div>
            <Badge variant="outline" className="capitalize" title={ROLE_INFO[role].description}>
              {role}
            </Badge>
          </li>
        );
      })}
    </ul>
  );
}
