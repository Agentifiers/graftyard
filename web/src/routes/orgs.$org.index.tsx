import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQueries, useSuspenseQuery } from "@tanstack/react-query";
import { userQuery } from "@/areas/core/queries";
import { languagesQuery } from "@/areas/code/queries";
import { PinnedRepos } from "@/areas/orgs/profile";
import {
  membersQuery,
  orgReposQuery,
  profileExtrasQuery,
  publicMembersQuery,
} from "@/areas/orgs/queries";
import { Language } from "@/areas/orgs/repos";
import { UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";

// An organization's overview: its README, pinned and popular repositories, the people,
// and the languages its repositories are written in.
export const Route = createFileRoute("/orgs/$org/")({
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(profileExtrasQuery(params.org));
    void queryClient.prefetchQuery(orgReposQuery(params.org));
    void queryClient.prefetchQuery(publicMembersQuery(params.org));
  },
  component: Overview,
});

function Overview() {
  const { org } = Route.useParams();
  return (
    <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_18rem]">
      <div className="flex min-w-0 flex-col gap-4">
        <Region name="organization README" rows={5}>
          <Readme org={org} />
        </Region>
        <Region name="pinned repositories" rows={4}>
          <Pinned org={org} />
        </Region>
        <Region name="repositories" rows={4}>
          <Recent org={org} />
        </Region>
      </div>
      <aside className="flex flex-col gap-4">
        <Panel title="People">
          <Region name="people" rows={2}>
            <People org={org} />
          </Region>
        </Panel>
        <Panel title="Top languages">
          <Region name="top languages" rows={3}>
            <TopLanguages org={org} />
          </Region>
        </Panel>
      </aside>
    </div>
  );
}

function Readme({ org }: { org: string }) {
  const { data } = useSuspenseQuery(profileExtrasQuery(org));
  if (!data.readme) return null;
  return (
    <div className="rounded-sm border p-4" data-testid="org-readme">
      <p className="text-muted-foreground mb-2 font-mono text-xs">{org} / .github / README.md</p>
      <MarkdownBody text={data.readme} />
    </div>
  );
}

function Pinned({ org }: { org: string }) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: members } = useSuspenseQuery(membersQuery(org));
  // Owners pin for the org; the mock's PUT checks the role, so a member's save is refused.
  return <PinnedRepos login={org} canEdit={members.some((m) => m.login === viewer.login)} />;
}

function Recent({ org }: { org: string }) {
  const { data } = useSuspenseQuery(orgReposQuery(org));
  const recent = [...data]
    .sort((a, b) => (b.pushed_at ?? "").localeCompare(a.pushed_at ?? ""))
    .slice(0, 5);
  return (
    <Panel
      title="Repositories"
      flush
      actions={
        <Button
          variant="ghost"
          size="xs"
          render={<Link to="/orgs/$org/repositories" params={{ org }} />}
        >
          View all {data.length}
        </Button>
      }
    >
      <ul className="divide-y" data-testid="recent-repos">
        {recent.map((r) => (
          <li key={r.id} className="flex items-center gap-2 px-4 py-2 text-sm">
            <Link
              to="/$owner/$repo"
              params={{ owner: r.owner.login, repo: r.name }}
              className="text-primary font-semibold hover:underline"
            >
              {r.name}
            </Link>
            <Badge variant="outline" size="sm" className="capitalize">
              {r.visibility}
            </Badge>
            <span className="text-muted-foreground ml-auto text-xs">
              <Language name={r.language} />
            </span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function People({ org }: { org: string }) {
  const { data } = useSuspenseQuery(publicMembersQuery(org));
  return (
    <div className="flex flex-col gap-2" data-testid="org-people">
      <div className="flex flex-wrap gap-1">
        {data.map((u) => (
          <Link key={u.id} to="/$owner" params={{ owner: u.login }} title={u.login}>
            <UserAvatar user={u} size="default" />
          </Link>
        ))}
      </div>
      {data.length === 0 && (
        <p className="text-muted-foreground text-xs">No one shows their membership publicly.</p>
      )}
      <Link
        to="/orgs/$org/people"
        params={{ org }}
        className="text-primary text-xs hover:underline"
      >
        View all people
      </Link>
    </div>
  );
}

function TopLanguages({ org }: { org: string }) {
  const { data: repos } = useSuspenseQuery(orgReposQuery(org));
  const results = useSuspenseQueries({
    queries: repos.map((r) => languagesQuery({ owner: r.owner.login, repo: r.name })),
  });
  const bytes = new Map<string, number>();
  for (const { data } of results) {
    for (const [lang, n] of Object.entries(data)) bytes.set(lang, (bytes.get(lang) ?? 0) + n);
  }
  // A repository with no language stats yet still counts by its primary language.
  if (bytes.size === 0) {
    for (const r of repos) if (r.language) bytes.set(r.language, (bytes.get(r.language) ?? 0) + 1);
  }
  const total = [...bytes.values()].reduce((a, b) => a + b, 0);
  const top = [...bytes].sort((a, b) => b[1] - a[1]).slice(0, 5);
  if (!top.length) return <p className="text-muted-foreground text-xs">No code yet.</p>;
  return (
    <ul className="flex flex-col gap-1.5 text-xs" data-testid="top-languages">
      {top.map(([lang, n]) => (
        <li key={lang} className="flex items-center gap-2">
          <Language name={lang} />
          <span className="text-muted-foreground ml-auto tabular-nums">
            {((n / total) * 100).toFixed(1)}%
          </span>
        </li>
      ))}
    </ul>
  );
}
