import { useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Building03Icon,
  Link01Icon,
  Location01Icon,
  Mail01Icon,
  PinIcon,
  UserGroupIcon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { send, ui, WriteError, type ProfileExtras } from "./api";
import { EmptyRow, failed, refetch } from "./kit";
import {
  followersQuery,
  followingQuery,
  isFollowingQuery,
  profileExtrasQuery,
  profileUserQuery,
  userOrgsListQuery,
  userReposListQuery,
} from "./queries";
import { RepoCards } from "./repos";

type PublicUser = Schemas["public-user"] | Schemas["private-user"];

// The left column of a user's profile: who they are, their follow counts, and their orgs.
export function ProfileCard({ login }: { login: string }) {
  const { data: user } = useSuspenseQuery(profileUserQuery(login));
  return (
    <div className="flex flex-col gap-3" data-testid="profile-card">
      <Avatar className="size-40 rounded-sm after:rounded-sm max-md:size-20">
        <AvatarImage src={user.avatar_url} alt={user.login} className="rounded-sm" />
        <AvatarFallback className="rounded-sm">
          {user.login.slice(0, 2).toUpperCase()}
        </AvatarFallback>
      </Avatar>
      <div>
        {user.name && <h1 className="text-xl font-semibold">{user.name}</h1>}
        <p className="text-muted-foreground text-lg" data-testid="profile-login">
          {user.login}
        </p>
      </div>
      {user.bio && <p className="text-sm">{user.bio}</p>}
      <FollowButton login={user.login} />
      <FollowCounts user={user} />
      <ul className="text-muted-foreground flex flex-col gap-1 text-sm">
        {user.company && (
          <Fact icon={Building03Icon}>
            {user.company.startsWith("@") ? (
              <Link
                to="/orgs/$org"
                params={{ org: user.company.slice(1) }}
                className="text-foreground hover:underline"
              >
                {user.company}
              </Link>
            ) : (
              user.company
            )}
          </Fact>
        )}
        {user.location && <Fact icon={Location01Icon}>{user.location}</Fact>}
        {user.email && (
          <Fact icon={Mail01Icon}>
            <a href={`mailto:${user.email}`} className="text-foreground hover:underline">
              {user.email}
            </a>
          </Fact>
        )}
        {user.blog && (
          <Fact icon={Link01Icon}>
            <a
              href={user.blog}
              className="text-foreground hover:underline"
              rel="nofollow noreferrer"
            >
              {user.blog.replace(/^https?:\/\//, "")}
            </a>
          </Fact>
        )}
        {user.twitter_username && <Fact icon={UserGroupIcon}>@{user.twitter_username}</Fact>}
      </ul>
    </div>
  );
}

function Fact({ icon, children }: { icon: typeof Link01Icon; children: React.ReactNode }) {
  return (
    <li className="flex items-center gap-2">
      <HugeiconsIcon icon={icon} strokeWidth={2} className="size-4 shrink-0" />
      <span className="min-w-0 truncate">{children}</span>
    </li>
  );
}

function FollowCounts({ user }: { user: PublicUser }) {
  return (
    <p
      className="text-muted-foreground flex items-center gap-1 text-sm"
      data-testid="follow-counts"
    >
      <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} className="size-4" />
      <Link
        to="/$owner"
        params={{ owner: user.login }}
        search={{ tab: "followers" }}
        className="hover:text-primary"
      >
        <strong className="text-foreground" data-followers={user.followers}>
          {user.followers}
        </strong>{" "}
        followers
      </Link>
      ·
      <Link
        to="/$owner"
        params={{ owner: user.login }}
        search={{ tab: "following" }}
        className="hover:text-primary"
      >
        <strong className="text-foreground">{user.following}</strong> following
      </Link>
    </p>
  );
}

// PUT or DELETE /user/following/{username}, shown at once on the button and the count.
export function FollowButton({ login }: { login: string }) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  if (viewer.login === login) {
    return (
      <Button variant="outline" size="sm" render={<Link to="/settings/profile" />}>
        Edit profile
      </Button>
    );
  }
  return <Follow login={login} />;
}

function Follow({ login }: { login: string }) {
  const client = useQueryClient();
  const { data: following } = useSuspenseQuery(isFollowingQuery(login));
  const statusKey = isFollowingQuery(login).queryKey;
  const userKey = profileUserQuery(login).queryKey;
  const toggle = useMutation({
    mutationFn: (follow: boolean) =>
      send(
        follow
          ? api.PUT("/user/following/{username}", { params: { path: { username: login } } })
          : api.DELETE("/user/following/{username}", { params: { path: { username: login } } }),
      ),
    onMutate: async (follow) => {
      await Promise.all([
        client.cancelQueries({ queryKey: statusKey }),
        client.cancelQueries({ queryKey: userKey }),
      ]);
      const before = { status: client.getQueryData(statusKey), user: client.getQueryData(userKey) };
      client.setQueryData(statusKey, follow);
      client.setQueryData<PublicUser>(
        userKey,
        (u) => u && { ...u, followers: u.followers + (follow ? 1 : -1) },
      );
      return before;
    },
    onError: (e, follow, before) => {
      client.setQueryData(statusKey, before?.status);
      client.setQueryData(userKey, before?.user);
      failed(follow ? `Could not follow ${login}` : `Could not unfollow ${login}`)(e);
    },
    onSuccess: (_d, follow) => toast.success(follow ? `Following ${login}` : `Unfollowed ${login}`),
    onSettled: () =>
      refetch(statusKey, userKey, followersQuery(login).queryKey, ["user"], ["users", "mira"]),
  });
  return (
    <Button
      variant={following ? "outline" : "default"}
      size="sm"
      onClick={() => toggle.mutate(!following)}
      disabled={toggle.isPending}
      aria-pressed={following}
      data-testid="follow-button"
    >
      {following ? "Unfollow" : "Follow"}
    </Button>
  );
}

export function ProfileOrgs({ login }: { login: string }) {
  const { data } = useSuspenseQuery(userOrgsListQuery(login));
  if (!data.length) return null;
  return (
    <div className="flex flex-col gap-2 border-t pt-3" data-testid="profile-orgs">
      <h2 className="text-sm font-semibold">Organizations</h2>
      <div className="flex flex-wrap gap-1">
        {data.map((o) => (
          <Link key={o.id} to="/orgs/$org" params={{ org: o.login }} title={o.login}>
            <Avatar size="default" className="rounded-sm after:rounded-sm">
              <AvatarImage src={o.avatar_url} alt={o.login} className="rounded-sm" />
              <AvatarFallback className="rounded-sm">{o.login.slice(0, 2)}</AvatarFallback>
            </Avatar>
          </Link>
        ))}
      </div>
    </div>
  );
}

export function ProfileReadme({ login }: { login: string }) {
  const { data } = useSuspenseQuery(profileExtrasQuery(login));
  if (!data.readme) return null;
  return (
    <div className="rounded-sm border p-4" data-testid="profile-readme">
      <p className="text-muted-foreground mb-2 font-mono text-xs">{login} / README.md</p>
      <MarkdownBody text={data.readme} />
    </div>
  );
}

// Pinned repositories, with a dialog that lets the owner choose up to six.
export function PinnedRepos({ login, canEdit }: { login: string; canEdit: boolean }) {
  const { data } = useSuspenseQuery(profileExtrasQuery(login));
  const [open, setOpen] = useState(false);
  return (
    <section className="flex flex-col gap-2" data-testid="pinned">
      <div className="flex items-center gap-2">
        <h2 className="text-sm font-semibold">
          {data.pinned.length ? "Pinned" : "Popular repositories"}
        </h2>
        {canEdit && (
          <Button
            variant="ghost"
            size="xs"
            className="ml-auto"
            onClick={() => setOpen(true)}
            data-testid="customize-pins"
          >
            <HugeiconsIcon icon={PinIcon} strokeWidth={2} data-icon="inline-start" />
            Customize your pins
          </Button>
        )}
      </div>
      {data.pinned.length ? <RepoCards repos={data.pinned} /> : <PopularRepos login={login} />}
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-md">
          {open && <PinEditor login={login} current={data} onDone={() => setOpen(false)} />}
        </DialogContent>
      </Dialog>
    </section>
  );
}

function PopularRepos({ login }: { login: string }) {
  const { data } = useSuspenseQuery(userReposListQuery(login, { sort: "pushed" }));
  const top = [...data].sort((a, b) => (b.stargazers_count ?? 0) - (a.stargazers_count ?? 0));
  if (!top.length) return <EmptyRow>{login} has no public repositories yet.</EmptyRow>;
  return <RepoCards repos={top.slice(0, 6)} />;
}

function PinEditor({
  login,
  current,
  onDone,
}: {
  login: string;
  current: ProfileExtras;
  onDone: () => void;
}) {
  const client = useQueryClient();
  const { data: repos } = useSuspenseQuery(userReposListQuery(login, { type: "all" }));
  const [ids, setIds] = useState(current.pinned.map((r) => r.id));
  const [error, setError] = useState<string>();
  const key = profileExtrasQuery(login).queryKey;
  const save = useMutation({
    mutationFn: (repository_ids: number[]) =>
      send(
        ui.PUT("/profiles/{login}/pins", { params: { path: { login } }, body: { repository_ids } }),
      ),
    onMutate: async (repository_ids) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<ProfileExtras>(key);
      const pinned = repository_ids.flatMap((id) => repos.find((r) => r.id === id) ?? []);
      client.setQueryData<ProfileExtras>(key, (old) => old && { ...old, pinned });
      return before;
    },
    onError: (e, _v, before) => {
      client.setQueryData(key, before);
      setError(e instanceof WriteError ? e.message : String(e));
      failed("Pins not saved")(e);
    },
    onSuccess: () => {
      toast.success("Pins saved");
      onDone();
    },
    onSettled: () => refetch(key),
  });
  const toggle = (id: number, on: boolean) =>
    setIds((prev) => (on ? [...prev, id] : prev.filter((x) => x !== id)));
  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        setError(undefined);
        save.mutate(ids);
      }}
    >
      <DialogHeader>
        <DialogTitle>Edit pinned items</DialogTitle>
        <DialogDescription>
          Select up to six public repositories you'd like to show. {6 - ids.length} remaining.
        </DialogDescription>
      </DialogHeader>
      <ul className="flex max-h-72 flex-col gap-1 overflow-y-auto rounded-sm border p-2">
        {repos.map((r) => (
          <li key={r.id}>
            <label className="hover:bg-muted flex items-center gap-2 rounded-sm px-1 py-1 text-sm">
              <Checkbox
                checked={ids.includes(r.id)}
                onCheckedChange={(on) => toggle(r.id, on)}
                data-pin={r.full_name}
              />
              <span className="font-medium">{r.full_name}</span>
              <span className="text-muted-foreground ml-auto text-xs capitalize">
                {r.visibility}
              </span>
            </label>
          </li>
        ))}
      </ul>
      {ids.length > 6 && (
        <p className="text-destructive text-xs" role="alert">
          Pin at most six repositories.
        </p>
      )}
      {error && (
        <p className="text-destructive text-xs" role="alert" data-form-error>
          {error}
        </p>
      )}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" disabled={save.isPending}>
          {save.isPending ? "Saving…" : "Save pins"}
        </Button>
      </DialogFooter>
    </form>
  );
}

// The Followers and Following tabs.
export function PeopleList({ login, kind }: { login: string; kind: "followers" | "following" }) {
  const { data } = useSuspenseQuery(
    kind === "followers" ? followersQuery(login) : followingQuery(login),
  );
  if (!data.length) {
    return (
      <EmptyRow>
        {kind === "followers"
          ? `${login} has no followers yet.`
          : `${login} isn't following anyone.`}
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y rounded-sm border" data-testid={`people-${kind}`}>
      {data.map((u) => (
        <li key={u.id} className="flex items-center gap-3 px-4 py-3" data-person={u.login}>
          <UserAvatar user={u} size="default" />
          <div className="min-w-0 flex-1">
            <Link
              to="/$owner"
              params={{ owner: u.login }}
              className="text-primary text-sm font-semibold hover:underline"
            >
              {u.name ?? u.login}
            </Link>
            <p className="text-muted-foreground text-xs">{u.login}</p>
          </div>
          <FollowButton login={u.login} />
        </li>
      ))}
    </ul>
  );
}
