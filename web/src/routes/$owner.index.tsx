import { useMemo, useState } from "react";
import { createFileRoute, Link, redirect, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  BookOpen01Icon,
  RepositoryIcon,
  Search01Icon,
  StarIcon,
  UserGroupIcon,
  UserMultipleIcon,
} from "@hugeicons/core-free-icons";
import { userQuery } from "@/areas/core/queries";
import { ContributionGraph } from "@/areas/orgs/contributions";
import {
  PeopleList,
  PinnedRepos,
  ProfileCard,
  ProfileOrgs,
  ProfileReadme,
} from "@/areas/orgs/profile";
import {
  profileExtrasQuery,
  profileUserQuery,
  starredByQuery,
  userReposListQuery,
} from "@/areas/orgs/queries";
import { matchRepo, RepoRows } from "@/areas/orgs/repos";
import { Region } from "@/components/region";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";

// A user's profile. An organization's login redirects to its own pages.

const TABS = ["overview", "repositories", "stars", "followers", "following"] as const;
type Tab = (typeof TABS)[number];
const TYPES = ["all", "owner", "member"] as const;
const SORTS = ["updated", "full_name", "pushed", "created"] as const;

type Search = {
  tab?: Tab;
  q?: string;
  // "type" would clash with the search page's own `type` param in loose navigations.
  owned?: (typeof TYPES)[number];
  language?: string;
  sort?: (typeof SORTS)[number];
};

const pick = <T extends string>(list: readonly T[], v: unknown) =>
  list.includes(v as T) ? (v as T) : undefined;

export const Route = createFileRoute("/$owner/")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    tab: pick(TABS, s.tab),
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    owned: pick(TYPES, s.owned),
    language: typeof s.language === "string" && s.language ? s.language : undefined,
    sort: pick(SORTS, s.sort),
  }),
  beforeLoad: async ({ context, params }) => {
    // Only the redirect waits on this read; a failure falls through to the page, whose
    // regions show their own errors.
    const user = await context.queryClient
      .ensureQueryData(profileUserQuery(params.owner))
      .catch(() => undefined);
    if (user?.type === "Organization") {
      throw redirect({ to: "/orgs/$org", params: { org: user.login }, replace: true });
    }
  },
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(profileExtrasQuery(params.owner));
    void context.queryClient.prefetchQuery(userQuery());
  },
  component: ProfilePage,
});

function ProfilePage() {
  const { owner } = Route.useParams();
  const { tab = "overview" } = Route.useSearch();
  return (
    <main className="mx-auto grid max-w-6xl items-start gap-6 p-4 sm:p-6 md:grid-cols-[16rem_minmax(0,1fr)]">
      <aside className="flex flex-col gap-4">
        <Region name="profile" rows={6}>
          <ProfileCard login={owner} />
        </Region>
        <Region name="organizations" rows={1} fallback={<Skeleton className="h-8 w-32" />}>
          <ProfileOrgs login={owner} />
        </Region>
      </aside>
      <div className="flex min-w-0 flex-col gap-4">
        <ProfileTabs owner={owner} tab={tab} />
        {tab === "overview" && <Overview owner={owner} />}
        {tab === "repositories" && (
          <Region name="repositories" rows={6}>
            <Repositories owner={owner} />
          </Region>
        )}
        {tab === "stars" && (
          <Region name="stars" rows={6}>
            <Stars owner={owner} />
          </Region>
        )}
        {(tab === "followers" || tab === "following") && (
          <Region name={tab} rows={6}>
            <PeopleList login={owner} kind={tab} />
          </Region>
        )}
      </div>
    </main>
  );
}

const TAB_INFO: Record<Tab, { label: string; icon: typeof BookOpen01Icon }> = {
  overview: { label: "Overview", icon: BookOpen01Icon },
  repositories: { label: "Repositories", icon: RepositoryIcon },
  stars: { label: "Stars", icon: StarIcon },
  followers: { label: "Followers", icon: UserGroupIcon },
  following: { label: "Following", icon: UserMultipleIcon },
};

function ProfileTabs({ owner, tab }: { owner: string; tab: Tab }) {
  return (
    <nav className="-mb-px flex gap-1 overflow-x-auto border-b text-sm" aria-label="Profile">
      {TABS.map((t) => (
        <Link
          key={t}
          to="/$owner"
          params={{ owner }}
          search={t === "overview" ? {} : { tab: t }}
          data-active={t === tab || undefined}
          data-tab={t}
          className="text-muted-foreground hover:text-foreground data-active:border-primary data-active:text-foreground flex shrink-0 items-center gap-1.5 border-b-2 border-transparent px-3 py-2 font-medium"
        >
          <HugeiconsIcon icon={TAB_INFO[t].icon} strokeWidth={2} className="size-4" />
          {TAB_INFO[t].label}
        </Link>
      ))}
    </nav>
  );
}

function Overview({ owner }: { owner: string }) {
  return (
    <>
      <Region name="profile README" rows={4}>
        <ProfileReadme login={owner} />
      </Region>
      <Region name="pinned repositories" rows={4}>
        <PinnedForViewer owner={owner} />
      </Region>
      <Region name="contributions" rows={6}>
        <ContributionGraph login={owner} />
      </Region>
    </>
  );
}

function PinnedForViewer({ owner }: { owner: string }) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  return <PinnedRepos login={owner} canEdit={viewer.login === owner} />;
}

function Repositories({ owner }: { owner: string }) {
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const [text, setText] = useState(search.q ?? "");
  const [q] = useDebouncedValue(text, { wait: 250 });
  const type = search.owned ?? "owner";
  const sort = search.sort ?? "updated";
  const { data } = useSuspenseQuery(
    userReposListQuery(owner, {
      type,
      sort,
      direction: sort === "full_name" ? "asc" : "desc",
    }),
  );
  const languages = useMemo(
    () => [...new Set(data.flatMap((r) => (r.language ? [r.language] : [])))].sort(),
    [data],
  );
  const shown = data.filter((r) => matchRepo(r, q, search.language ?? ""));
  const set = (patch: Partial<Search>) =>
    void navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <InputGroup className="max-w-sm flex-1">
          <InputGroupAddon>
            <HugeiconsIcon icon={Search01Icon} strokeWidth={2} />
          </InputGroupAddon>
          <InputGroupInput
            placeholder="Find a repository…"
            aria-label="Find a repository"
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              set({ q: e.target.value || undefined });
            }}
          />
        </InputGroup>
        <NativeSelect
          size="sm"
          aria-label="Type"
          value={type}
          onChange={(e) => set({ owned: e.target.value as Search["owned"] })}
        >
          <NativeSelectOption value="owner">Owned</NativeSelectOption>
          <NativeSelectOption value="member">Through organizations</NativeSelectOption>
          <NativeSelectOption value="all">All</NativeSelectOption>
        </NativeSelect>
        <NativeSelect
          size="sm"
          aria-label="Language"
          value={search.language ?? ""}
          onChange={(e) => set({ language: e.target.value || undefined })}
        >
          <NativeSelectOption value="">All languages</NativeSelectOption>
          {languages.map((l) => (
            <NativeSelectOption key={l} value={l}>
              {l}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        <NativeSelect
          size="sm"
          aria-label="Sort"
          value={sort}
          onChange={(e) => set({ sort: e.target.value as Search["sort"] })}
        >
          <NativeSelectOption value="updated">Last updated</NativeSelectOption>
          <NativeSelectOption value="pushed">Last pushed</NativeSelectOption>
          <NativeSelectOption value="full_name">Name</NativeSelectOption>
          <NativeSelectOption value="created">Newest</NativeSelectOption>
        </NativeSelect>
      </div>
      <div className="rounded-sm border">
        <RepoRows
          repos={shown}
          empty={
            data.length
              ? "No repositories match these filters."
              : `${owner} doesn't have any repositories of this type yet.`
          }
        />
      </div>
    </div>
  );
}

function Stars({ owner }: { owner: string }) {
  const { data } = useSuspenseQuery(starredByQuery(owner));
  return (
    <div className="rounded-sm border">
      <RepoRows repos={data} empty={`${owner} hasn't starred any repositories yet.`} />
    </div>
  );
}
