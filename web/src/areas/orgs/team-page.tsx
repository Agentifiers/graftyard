import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { LockIcon, UserGroupIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { UserAvatar } from "@/components/github";
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
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { send } from "./api";
import {
  Confirm,
  EmptyRow,
  failed,
  FormFailure,
  optimistic,
  PERMISSIONS,
  permissionOf,
  refetch,
  RoleBadge,
  type Permission,
} from "./kit";
import {
  childTeamsQuery,
  membersQuery,
  orgReposQuery,
  teamQuery,
  teamMembersQuery,
  teamReposQuery,
  teamsQuery,
} from "./queries";
import { Visibility } from "./repos";

type TeamParams = { org: string; team: string };
type Member = Schemas["team-member"];
type Repo = Schemas["minimal-repository"];

export function TeamHeader({ org, team, actions }: TeamParams & { actions: React.ReactNode }) {
  const { data: t } = useSuspenseQuery(teamQuery({ org, team }));
  return (
    <div className="flex flex-wrap items-start gap-3" data-testid="team-header">
      <div className="bg-muted flex size-12 items-center justify-center rounded-sm">
        <HugeiconsIcon
          icon={t.privacy === "secret" ? LockIcon : UserGroupIcon}
          strokeWidth={2}
          className="size-6"
        />
      </div>
      <div className="min-w-0 flex-1">
        <p className="text-muted-foreground text-xs">
          <Link to="/orgs/$org/teams" params={{ org }} className="hover:underline">
            {org}
          </Link>
          {t.parent && (
            <>
              {" / "}
              <Link
                to="/orgs/$org/teams/$team"
                params={{ org, team: t.parent.slug }}
                className="hover:underline"
                data-parent={t.parent.slug}
              >
                {t.parent.name}
              </Link>
            </>
          )}
        </p>
        <h1 className="flex items-center gap-2 text-xl font-semibold">
          {t.name}
          <Badge variant="outline" size="sm">
            {t.privacy === "secret" ? "Secret" : "Visible"}
          </Badge>
        </h1>
        {t.description && <p className="text-muted-foreground text-sm">{t.description}</p>}
        <p className="text-muted-foreground mt-1 text-xs">
          @{org}/{t.slug} · {t.members_count} {t.members_count === 1 ? "member" : "members"} ·{" "}
          {t.repos_count} {t.repos_count === 1 ? "repository" : "repositories"}
          {t.notification_setting === "notifications_disabled" && " · mentions don't notify"}
        </p>
      </div>
      <div className="flex gap-2">{actions}</div>
    </div>
  );
}

// ---- Members ----

export function TeamMembers({ org, team }: TeamParams) {
  const { data } = useSuspenseQuery(teamMembersQuery({ org, team }));
  return (
    <>
      <AddMember org={org} team={team} current={data} />
      {data.length ? (
        <ul className="divide-y" data-testid="team-members">
          {data.map((m) => (
            <TeamMemberRow key={m.id} org={org} team={team} member={m} />
          ))}
        </ul>
      ) : (
        <EmptyRow>This team has no members yet.</EmptyRow>
      )}
    </>
  );
}

function AddMember({ org, team, current }: TeamParams & { current: Member[] }) {
  const { data: orgMembers } = useSuspenseQuery(membersQuery(org));
  const [login, setLogin] = useState("");
  const [role, setRole] = useState<"member" | "maintainer">("member");
  const [error, setError] = useState<string>();
  const direct = new Set(current.filter((m) => !m.inherited).map((m) => m.login));
  const candidates = orgMembers.filter((m) => !direct.has(m.login));
  const key = teamMembersQuery({ org, team }).queryKey;
  const add = useMutation({
    mutationFn: (v: { login: string; role: "member" | "maintainer" }) =>
      send(
        api.PUT("/orgs/{org}/teams/{team_slug}/memberships/{username}", {
          params: { path: { org, team_slug: team, username: v.login } },
          body: { role: v.role },
        }),
      ),
    onMutate: (v) => {
      const user = orgMembers.find((m) => m.login === v.login);
      return optimistic<Member[]>(key, (old) =>
        user
          ? [...old.filter((m) => m.login !== v.login), { ...user, role: v.role, inherited: false }]
          : old,
      );
    },
    onError: (e, _v, rollback) => {
      rollback?.();
      setError(e instanceof Error ? e.message : String(e));
      failed("Member not added")(e);
    },
    onSuccess: (_d, v) => {
      setLogin("");
      setError(undefined);
      toast.success(`Added ${v.login} as ${v.role === "maintainer" ? "a maintainer" : "a member"}`);
    },
    onSettled: () => refetch(key, teamQuery({ org, team }).queryKey),
  });
  return (
    <form
      className="flex flex-col gap-1.5 border-b px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        const v = login.trim();
        if (!v) return setError("Choose an organization member.");
        if (!candidates.some((c) => c.login === v)) {
          return setError(`Choose a member of ${org} who is not on this team yet.`);
        }
        add.mutate({ login: v, role });
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-56 flex-1">
          <Autocomplete
            items={candidates.map((c) => c.login)}
            value={login}
            onValueChange={(v) => {
              setLogin(v);
              setError(undefined);
            }}
            openOnInputClick
          >
            <AutocompleteInput
              aria-label="Add a member"
              placeholder="Add an organization member…"
              aria-invalid={Boolean(error) || undefined}
              autoComplete="off"
            />
            <AutocompleteContent>
              <AutocompleteEmpty>No organization member matches.</AutocompleteEmpty>
              <AutocompleteList>
                {(l: string) => {
                  const p = candidates.find((c) => c.login === l);
                  return (
                    <AutocompleteItem key={l} value={l} data-candidate={l}>
                      {p && <UserAvatar user={p} />}
                      <span className="font-medium">{l}</span>
                      <span className="text-muted-foreground truncate">{p?.name}</span>
                    </AutocompleteItem>
                  );
                }}
              </AutocompleteList>
            </AutocompleteContent>
          </Autocomplete>
        </div>
        <NativeSelect
          size="sm"
          aria-label="Team role"
          value={role}
          onChange={(e) => setRole(e.target.value as "member" | "maintainer")}
        >
          <NativeSelectOption value="member">Member</NativeSelectOption>
          <NativeSelectOption value="maintainer">Maintainer</NativeSelectOption>
        </NativeSelect>
        <Button type="submit" size="sm" disabled={add.isPending} data-testid="add-member">
          {add.isPending ? "Adding…" : "Add"}
        </Button>
      </div>
      <FormFailure>{error}</FormFailure>
    </form>
  );
}

function TeamMemberRow({ org, team, member }: TeamParams & { member: Member }) {
  const key = teamMembersQuery({ org, team }).queryKey;
  const role = useMutation({
    mutationFn: (next: "member" | "maintainer") =>
      send(
        api.PUT("/orgs/{org}/teams/{team_slug}/memberships/{username}", {
          params: { path: { org, team_slug: team, username: member.login } },
          body: { role: next },
        }),
      ),
    onMutate: (next) =>
      optimistic<Member[]>(key, (old) =>
        old.map((m) => (m.login === member.login ? { ...m, role: next } : m)),
      ),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not change ${member.login}'s role`)(e);
    },
    onSettled: () => refetch(key),
  });
  const remove = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/teams/{team_slug}/memberships/{username}", {
          params: { path: { org, team_slug: team, username: member.login } },
        }),
      ),
    onMutate: () => optimistic<Member[]>(key, (old) => old.filter((m) => m.login !== member.login)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not remove ${member.login}`)(e);
    },
    onSuccess: () => toast.success(`Removed ${member.login} from the team`),
    onSettled: () => refetch(key, teamQuery({ org, team }).queryKey),
  });
  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-2.5" data-team-member={member.login}>
      <UserAvatar user={member} size="default" />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner"
          params={{ owner: member.login }}
          className="text-primary text-sm font-semibold hover:underline"
        >
          {member.name ?? member.login}
        </Link>
        <p className="text-muted-foreground text-xs">{member.login}</p>
      </div>
      {member.inherited ? (
        <>
          <Badge variant="outline" size="sm" title="A member through a child team">
            Inherited
          </Badge>
          <RoleBadge role={member.role ?? "member"} />
        </>
      ) : (
        <>
          <NativeSelect
            size="sm"
            aria-label={`Team role for ${member.login}`}
            value={member.role ?? "member"}
            disabled={role.isPending}
            onChange={(e) => role.mutate(e.target.value as "member" | "maintainer")}
            data-team-role={member.login}
          >
            <NativeSelectOption value="member">Member</NativeSelectOption>
            <NativeSelectOption value="maintainer">Maintainer</NativeSelectOption>
          </NativeSelect>
          <Button
            variant="outline"
            size="sm"
            onClick={() => remove.mutate()}
            disabled={remove.isPending}
            data-remove-member={member.login}
          >
            Remove
          </Button>
        </>
      )}
    </li>
  );
}

// ---- Child teams ----

export function ChildTeams({ org, team }: TeamParams) {
  const { data } = useSuspenseQuery(childTeamsQuery({ org, team }));
  if (!data.length) {
    return (
      <EmptyRow>
        No child teams. Create a team and choose this one as its parent to nest it here.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="child-teams">
      {data.map((t) => (
        <li key={t.id} className="flex items-center gap-3 px-4 py-2.5" data-child={t.slug}>
          <HugeiconsIcon
            icon={UserGroupIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <Link
              to="/orgs/$org/teams/$team"
              params={{ org, team: t.slug }}
              className="text-primary text-sm font-semibold hover:underline"
            >
              {t.name}
            </Link>
            {t.description && <p className="text-muted-foreground text-xs">{t.description}</p>}
          </div>
        </li>
      ))}
    </ul>
  );
}

// ---- Repositories ----

export function TeamRepos({ org, team }: TeamParams) {
  const { data } = useSuspenseQuery(teamReposQuery({ org, team }));
  return (
    <>
      <AddRepo org={org} team={team} current={data} />
      {data.length ? (
        <ul className="divide-y" data-testid="team-repos">
          {data.map((r) => (
            <TeamRepoRow key={r.id} org={org} team={team} repo={r} />
          ))}
        </ul>
      ) : (
        <EmptyRow>This team has no repository access yet.</EmptyRow>
      )}
    </>
  );
}

function usePermission({ org, team }: TeamParams) {
  const key = teamReposQuery({ org, team }).queryKey;
  return useMutation({
    mutationFn: (v: { repo: Repo; permission: Permission }) =>
      send(
        api.PUT("/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}", {
          params: { path: { org, team_slug: team, owner: v.repo.owner.login, repo: v.repo.name } },
          body: { permission: v.permission },
        }),
      ),
    onMutate: (v) =>
      optimistic<Repo[]>(key, (old) => {
        const flags = flagsOf(v.permission);
        const row = { ...v.repo, permissions: flags };
        return old.some((r) => r.id === v.repo.id)
          ? old.map((r) => (r.id === v.repo.id ? row : r))
          : [...old, row];
      }),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Repository access not saved")(e);
    },
    onSettled: () => refetch(key, teamQuery({ org, team }).queryKey),
  });
}

function flagsOf(p: Permission) {
  const rank = PERMISSIONS.findIndex((x) => x.api === p);
  return { pull: true, triage: rank >= 1, push: rank >= 2, maintain: rank >= 3, admin: rank >= 4 };
}

function AddRepo({ org, team, current }: TeamParams & { current: Repo[] }) {
  const { data: repos } = useSuspenseQuery(orgReposQuery(org));
  const [name, setName] = useState("");
  const [permission, setPermission] = useState<Permission>("pull");
  const [error, setError] = useState<string>();
  const set = usePermission({ org, team });
  const have = new Set(current.map((r) => r.id));
  const candidates = repos.filter((r) => !have.has(r.id));
  return (
    <form
      className="flex flex-col gap-1.5 border-b px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        const repo = candidates.find((r) => r.name === name.trim() || r.full_name === name.trim());
        if (!repo) return setError(`Choose a repository of ${org}.`);
        set.mutate(
          { repo, permission },
          {
            onSuccess: () => {
              setName("");
              setError(undefined);
              toast.success(`Added ${repo.full_name}`);
            },
            onError: (err) => setError(err.message),
          },
        );
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <div className="min-w-56 flex-1">
          <Autocomplete
            items={candidates.map((r) => r.name)}
            value={name}
            onValueChange={(v) => {
              setName(v);
              setError(undefined);
            }}
            openOnInputClick
          >
            <AutocompleteInput
              aria-label="Add a repository"
              placeholder="Add a repository…"
              aria-invalid={Boolean(error) || undefined}
              autoComplete="off"
            />
            <AutocompleteContent>
              <AutocompleteEmpty>Every repository is already here.</AutocompleteEmpty>
              <AutocompleteList>
                {(n: string) => (
                  <AutocompleteItem key={n} value={n} data-candidate-repo={n}>
                    {org}/{n}
                  </AutocompleteItem>
                )}
              </AutocompleteList>
            </AutocompleteContent>
          </Autocomplete>
        </div>
        <PermissionSelect value={permission} onChange={setPermission} label="Permission" />
        <Button type="submit" size="sm" disabled={set.isPending} data-testid="add-repo">
          Add repository
        </Button>
      </div>
      <FormFailure>{error}</FormFailure>
    </form>
  );
}

function PermissionSelect({
  value,
  onChange,
  label,
  disabled,
}: {
  value: Permission;
  onChange: (p: Permission) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <NativeSelect
      size="sm"
      aria-label={label}
      value={value}
      disabled={disabled}
      onChange={(e) => onChange(e.target.value as Permission)}
    >
      {PERMISSIONS.map((p) => (
        <NativeSelectOption key={p.api} value={p.api}>
          {p.label}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}

function TeamRepoRow({ org, team, repo }: TeamParams & { repo: Repo }) {
  const key = teamReposQuery({ org, team }).queryKey;
  const set = usePermission({ org, team });
  const remove = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/teams/{team_slug}/repos/{owner}/{repo}", {
          params: { path: { org, team_slug: team, owner: repo.owner.login, repo: repo.name } },
        }),
      ),
    onMutate: () => optimistic<Repo[]>(key, (old) => old.filter((r) => r.id !== repo.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed(`Could not remove ${repo.full_name}`)(e);
    },
    onSuccess: () => toast.success(`Removed ${repo.full_name} from the team`),
    onSettled: () => refetch(key, teamQuery({ org, team }).queryKey),
  });
  const current = permissionOf(repo.permissions);
  return (
    <li className="flex flex-wrap items-center gap-3 px-4 py-2.5" data-team-repo={repo.full_name}>
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Link
          to="/$owner/$repo"
          params={{ owner: repo.owner.login, repo: repo.name }}
          className="text-primary text-sm font-semibold hover:underline"
        >
          {repo.full_name}
        </Link>
        <Visibility repo={repo} />
      </div>
      <PermissionSelect
        value={current}
        label={`Permission on ${repo.full_name}`}
        disabled={set.isPending}
        onChange={(permission) =>
          set.mutate(
            { repo, permission },
            {
              onSuccess: () =>
                toast.success(
                  `${repo.name}: ${PERMISSIONS.find((p) => p.api === permission)!.label}`,
                ),
            },
          )
        }
      />
      <Button
        variant="outline"
        size="sm"
        onClick={() => remove.mutate()}
        disabled={remove.isPending}
        data-remove-repo={repo.full_name}
      >
        Remove
      </Button>
    </li>
  );
}

// ---- Delete ----

export function DeleteTeam({ org, team }: TeamParams) {
  const { data: t } = useSuspenseQuery(teamQuery({ org, team }));
  const { data: children } = useSuspenseQuery(childTeamsQuery({ org, team }));
  const [open, setOpen] = useState(false);
  const navigate = useNavigate();
  const del = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/teams/{team_slug}", { params: { path: { org, team_slug: team } } }),
      ),
    onError: failed("Team not deleted"),
    onSuccess: async () => {
      setOpen(false);
      toast.success(`Deleted ${t.name}`);
      await refetch(teamsQuery(org).queryKey);
      void navigate({ to: "/orgs/$org/teams", params: { org }, replace: true });
    },
  });
  return (
    <>
      <Button
        variant="destructive"
        size="sm"
        onClick={() => setOpen(true)}
        data-testid="delete-team"
      >
        Delete team
      </Button>
      <Confirm
        open={open}
        onOpenChange={setOpen}
        title={`Delete ${t.name}?`}
        description={
          <>
            Members lose the access this team grants.
            {children.length > 0 &&
              ` Its ${children.length} child ${children.length === 1 ? "team moves" : "teams move"} up to ${t.parent?.name ?? "the top level"}.`}
          </>
        }
        phrase={t.slug}
        action="Delete this team"
        pending={del.isPending}
        onConfirm={() => del.mutate()}
      />
    </>
  );
}
