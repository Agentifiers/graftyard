import { useMemo, useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, Search01Icon } from "@hugeicons/core-free-icons";
import { orgReposFilteredQuery } from "@/areas/orgs/queries";
import { matchRepo, RepoRows } from "@/areas/orgs/repos";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

// GET /orgs/{org}/repos with GHES's type and sort, plus a name search and a language
// filter on the client.

const TYPES = ["all", "public", "private", "internal", "sources", "forks", "member"] as const;
const SORTS = ["pushed", "updated", "created", "full_name"] as const;
type Search = {
  q?: string;
  kind?: (typeof TYPES)[number];
  language?: string;
  sort?: (typeof SORTS)[number];
};
const pick = <T extends string>(list: readonly T[], v: unknown) =>
  list.includes(v as T) ? (v as T) : undefined;

export const Route = createFileRoute("/orgs/$org/repositories")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    kind: pick(TYPES, s.kind),
    language: typeof s.language === "string" && s.language ? s.language : undefined,
    sort: pick(SORTS, s.sort),
  }),
  loaderDeps: ({ search }) => ({ kind: search.kind ?? "all", sort: search.sort ?? "pushed" }),
  loader: ({ context, params, deps }) =>
    void context.queryClient.prefetchQuery(
      orgReposFilteredQuery(params.org, {
        type: deps.kind,
        sort: deps.sort,
        direction: deps.sort === "full_name" ? "asc" : "desc",
      }),
    ),
  component: RepositoriesPage,
});

function RepositoriesPage() {
  const { org } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const [text, setText] = useState(search.q ?? "");
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
          value={search.kind ?? "all"}
          onChange={(e) => set({ kind: e.target.value as Search["kind"] })}
        >
          <NativeSelectOption value="all">All</NativeSelectOption>
          <NativeSelectOption value="public">Public</NativeSelectOption>
          <NativeSelectOption value="internal">Internal</NativeSelectOption>
          <NativeSelectOption value="private">Private</NativeSelectOption>
          <NativeSelectOption value="sources">Sources</NativeSelectOption>
          <NativeSelectOption value="forks">Forks</NativeSelectOption>
          <NativeSelectOption value="member">Yours through teams</NativeSelectOption>
        </NativeSelect>
        <Region name="languages" rows={1} fallback={null}>
          <LanguageSelect value={search.language} onChange={(language) => set({ language })} />
        </Region>
        <NativeSelect
          size="sm"
          aria-label="Sort"
          value={search.sort ?? "pushed"}
          onChange={(e) => set({ sort: e.target.value as Search["sort"] })}
        >
          <NativeSelectOption value="pushed">Last pushed</NativeSelectOption>
          <NativeSelectOption value="updated">Last updated</NativeSelectOption>
          <NativeSelectOption value="created">Newest</NativeSelectOption>
          <NativeSelectOption value="full_name">Name</NativeSelectOption>
        </NativeSelect>
        <Button size="sm" className="ml-auto" render={<Link to="/new" />}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          New repository
        </Button>
      </div>
      <Panel flush>
        <Region name="repositories" rows={5}>
          <List org={org} text={text} />
        </Region>
      </Panel>
    </div>
  );
}

function useRepos(org: string) {
  const search = Route.useSearch();
  const sort = search.sort ?? "pushed";
  return useSuspenseQuery(
    orgReposFilteredQuery(org, {
      type: search.kind ?? "all",
      sort,
      direction: sort === "full_name" ? "asc" : "desc",
    }),
  ).data;
}

function LanguageSelect({
  value,
  onChange,
}: {
  value: string | undefined;
  onChange: (v: string | undefined) => void;
}) {
  const repos = useRepos(Route.useParams().org);
  const languages = useMemo(
    () => [...new Set(repos.flatMap((r) => (r.language ? [r.language] : [])))].sort(),
    [repos],
  );
  return (
    <NativeSelect
      size="sm"
      aria-label="Language"
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || undefined)}
    >
      <NativeSelectOption value="">All languages</NativeSelectOption>
      {languages.map((l) => (
        <NativeSelectOption key={l} value={l}>
          {l}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}

function List({ org, text }: { org: string; text: string }) {
  const search = Route.useSearch();
  const repos = useRepos(org);
  const [q] = useDebouncedValue(text, { wait: 200 });
  const shown = repos.filter((r) => matchRepo(r, q, search.language ?? ""));
  return (
    <>
      <p className="text-muted-foreground border-b px-4 py-2 text-xs" data-testid="repo-count">
        {shown.length} of {repos.length} repositories
      </p>
      <RepoRows
        repos={shown}
        empty={
          repos.length
            ? "No repositories match these filters."
            : "This organization has no repositories of this type."
        }
      />
    </>
  );
}
