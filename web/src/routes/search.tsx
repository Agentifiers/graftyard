import { Fragment, type ReactNode } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft01Icon,
  ArrowRight01Icon,
  CodeIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  LabelIcon,
  Location01Icon,
  RecordIcon,
  RepositoryIcon,
  Search01Icon,
  StarIcon,
  UserIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { repoQuery } from "@/areas/core/queries";
import {
  SEARCH_PAGE_SIZE,
  searchQuery,
  type SearchItems,
  type SearchResult,
  type ValidationError,
} from "@/areas/activity/queries";
import {
  SEARCH_TYPES,
  SEARCH_TYPE_KEYS,
  formatQualifier,
  parseQuery,
  withoutQualifier,
  type SearchType,
} from "@/areas/activity/search-query";
import { Ago, issueState, LabelBadge, pullState, StateIcon, UserAvatar } from "@/components/github";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";
import type { IconSvgElement } from "@hugeicons/react";

// GHES search: ?q= with qualifiers, ?type= for the result kind, sort, order, and page.
// Each type is its own query, so the sidebar counts and the results fail independently.

type Search = { q?: string; type?: SearchType; sort?: string; order?: "asc" | "desc"; p?: number };

export const Route = createFileRoute("/search")({
  validateSearch: (s: Record<string, unknown>): Search => {
    const type = SEARCH_TYPE_KEYS.includes(s.type as SearchType)
      ? (s.type as SearchType)
      : undefined;
    const sorts = SEARCH_TYPES[type ?? "repositories"].sorts as Record<string, string>;
    return {
      q: typeof s.q === "string" && s.q.trim() ? s.q : undefined,
      type: type === "repositories" ? undefined : type,
      sort: typeof s.sort === "string" && s.sort in sorts ? s.sort : undefined,
      order: s.order === "asc" ? "asc" : undefined,
      p: Number(s.p) > 1 ? Math.floor(Number(s.p)) : undefined,
    };
  },
  component: SearchPage,
});

const ICONS: Record<SearchType, IconSvgElement> = {
  repositories: RepositoryIcon,
  code: CodeIcon,
  commits: GitCommitIcon,
  issues: RecordIcon,
  pullrequests: GitPullRequestIcon,
  users: UserIcon,
  labels: LabelIcon,
};

// Labels are searched one repository at a time: the query's repo: qualifier, else edge.
function useLabelRepo(q: string) {
  const repo =
    parseQuery(q).qualifiers.find((x) => x.key === "repo" && !x.negated)?.value ?? "graftyard/edge";
  const [owner, name] = repo.split("/");
  return { owner: owner ?? "graftyard", repo: name ?? "edge" };
}

function SearchPage() {
  const { q = "", type = "repositories" } = Route.useSearch();
  return (
    <main className="mx-auto max-w-6xl p-4 sm:p-6">
      <SearchForm key={q} q={q} />
      {!q ? (
        <Tips />
      ) : (
        <div className="mt-4 grid items-start gap-4 md:grid-cols-[13rem_minmax(0,1fr)]">
          <TypeNav q={q} active={type} />
          <Region name={`${SEARCH_TYPES[type].label.toLowerCase()} results`} rows={6}>
            <Results key={type} type={type} />
          </Region>
        </div>
      )}
    </main>
  );
}

// The query box. TanStack Form keeps the draft; submitting writes it to the URL.
function SearchForm({ q }: { q: string }) {
  const navigate = Route.useNavigate();
  const form = useForm({
    defaultValues: { q },
    onSubmit: ({ value }) =>
      navigate({ search: (prev) => ({ ...prev, q: value.q.trim() || undefined, p: undefined }) }),
  });
  const parsed = parseQuery(q);
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      className="flex flex-col gap-2"
      role="search"
    >
      <div className="flex gap-2">
        <form.Field name="q">
          {(field) => (
            <label className="relative flex-1">
              <HugeiconsIcon
                icon={Search01Icon}
                strokeWidth={2}
                className="text-muted-foreground absolute top-1/2 left-2.5 size-4 -translate-y-1/2"
              />
              <Input
                type="search"
                aria-label="Search query"
                placeholder="Search, or use qualifiers: repo:graftyard/edge is:open label:bug author:tomas language:rust"
                className="h-9 rounded-sm pl-8 font-mono text-sm"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
                data-testid="search-input"
              />
            </label>
          )}
        </form.Field>
        <Button type="submit" className="h-9 rounded-sm">
          Search
        </Button>
      </div>
      {parsed.qualifiers.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5" data-testid="qualifier-chips">
          <span className="text-muted-foreground text-xs">Filters:</span>
          {parsed.qualifiers.map((x) => (
            <Badge
              key={formatQualifier(x)}
              variant={x.negated ? "destructive-light" : "primary-light"}
              size="lg"
              className="gap-1 font-mono"
            >
              {formatQualifier(x)}
              <Link
                to="/search"
                search={(prev) => ({
                  ...prev,
                  q: withoutQualifier(q, x) || undefined,
                  p: undefined,
                })}
                aria-label={`Remove ${formatQualifier(x)}`}
                className="hover:text-foreground"
              >
                ×
              </Link>
            </Badge>
          ))}
        </div>
      )}
    </form>
  );
}

function Tips() {
  const examples = [
    ["is:open label:bug repo:graftyard/edge", "issues"],
    ["retry author:mira", "pullrequests"],
    ["language:rust stars:>50", "repositories"],
    ["should_retry language:rust", "code"],
    ["rate limit", "commits"],
    ["location:lisbon", "users"],
    ["good repo:graftyard/edge", "labels"],
  ] as const;
  return (
    <Frame spacing="sm" dense className="mt-4">
      <FramePanel>
        <h2 className="text-sm font-semibold">Search across repositories, code, and people</h2>
        <p className="text-muted-foreground mt-1 text-sm">
          Qualifiers narrow a search. Prefix one with <code className="font-mono">-</code> to
          exclude it.
        </p>
        <ul className="mt-3 flex flex-col gap-1.5 text-sm">
          {examples.map(([q, type]) => (
            <li key={q}>
              <Link
                to="/search"
                search={{ q, type: type === "repositories" ? undefined : type }}
                className="text-primary font-mono hover:underline"
              >
                {q}
              </Link>
              <span className="text-muted-foreground">
                {" "}
                in {SEARCH_TYPES[type].label.toLowerCase()}
              </span>
            </li>
          ))}
        </ul>
      </FramePanel>
    </Frame>
  );
}

// ---- The type sidebar, each with its own count ----

function TypeNav({ q, active }: { q: string; active: SearchType }) {
  return (
    <nav
      aria-label="Result types"
      className="flex flex-col gap-0.5 md:sticky md:top-4"
      data-testid="search-types"
    >
      <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">Filter by</h2>
      {SEARCH_TYPE_KEYS.map((t) => (
        <Link
          key={t}
          to="/search"
          search={{ q, type: t === "repositories" ? undefined : t }}
          data-active={t === active || undefined}
          data-type={t}
          className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 text-sm font-medium"
        >
          <HugeiconsIcon icon={ICONS[t]} strokeWidth={2} className="size-4" />
          <span className="flex-1">{SEARCH_TYPES[t].label}</span>
          <Count type={t} q={q} />
        </Link>
      ))}
    </nav>
  );
}

function Count({ type, q }: { type: SearchType; q: string }) {
  const labelRepo = useLabelRepo(q);
  const repo = useQuery({ ...repoQuery(labelRepo), enabled: type === "labels" });
  const { data, isError, isPending } = useQuery({
    ...searchQuery(type, { q, repositoryId: repo.data?.id }),
    enabled: type !== "labels" || Boolean(repo.data),
  });
  if (isError)
    return (
      <span className="text-destructive text-xs" title="Could not count">
        !
      </span>
    );
  if (isPending) return <Skeleton className="h-3.5 w-5" />;
  return (
    <Badge
      variant="secondary"
      size="sm"
      className="tabular-nums"
      data-count={data.kind === "ok" ? data.total : "invalid"}
    >
      {data.kind === "ok" ? compact(data.total) : "–"}
    </Badge>
  );
}

const compact = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));

// ---- Results ----

function Results({ type }: { type: SearchType }) {
  const { q = "" } = Route.useSearch();
  return type === "labels" ? <LabelResults q={q} /> : <TypeResults type={type} />;
}

// Labels search one repository by id, so they read the repository first.
function LabelResults({ q }: { q: string }) {
  const { data: repo } = useSuspenseQuery(repoQuery(useLabelRepo(q)));
  return <TypeResults type="labels" repositoryId={repo.id} />;
}

function TypeResults({ type, repositoryId }: { type: SearchType; repositoryId?: number }) {
  const { q = "", sort, order = "desc", p = 1 } = Route.useSearch();
  const labelRepo = useLabelRepo(q);
  const { data } = useSuspenseQuery(searchQuery(type, { q, sort, order, page: p, repositoryId }));
  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="min-h-10 flex-row flex-wrap items-center gap-2 py-1.5">
        <h2 className="text-sm font-semibold" data-testid="result-total">
          {data.kind === "ok"
            ? `${data.total.toLocaleString()} ${data.total === 1 ? SEARCH_TYPES[type].one : SEARCH_TYPES[type].many}`
            : "Invalid query"}
          {type === "labels" && (
            <span className="text-muted-foreground font-normal">
              {" "}
              in {labelRepo.owner}/{labelRepo.repo}
            </span>
          )}
        </h2>
        <SortMenu type={type} />
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        {data.kind === "invalid" ? (
          <Invalid error={data.error} />
        ) : data.items.length === 0 ? (
          <NoResults q={q} type={type} />
        ) : (
          <>
            <ResultList type={type} result={data} />
            <Pager total={data.total} page={p} />
          </>
        )}
      </FramePanel>
    </Frame>
  );
}

function SortMenu({ type }: { type: SearchType }) {
  const { sort, order = "desc" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const sorts = SEARCH_TYPES[type].sorts as Record<string, string>;
  const value = sort ? `${sort}:${order}` : "";
  return (
    <label className="ml-auto flex items-center gap-1.5 text-xs">
      <span className="text-muted-foreground">Sort</span>
      <NativeSelect
        size="sm"
        value={value}
        aria-label="Sort results"
        data-testid="sort"
        onChange={(e) => {
          const [s, o] = e.target.value.split(":");
          void navigate({
            search: (prev) => ({
              ...prev,
              sort: s || undefined,
              order: o === "asc" ? "asc" : undefined,
              p: undefined,
            }),
          });
        }}
      >
        <NativeSelectOption value="">Best match</NativeSelectOption>
        {Object.entries(sorts).map(([k, label]) => (
          <Fragment key={k}>
            <NativeSelectOption value={`${k}:desc`}>{label}</NativeSelectOption>
            <NativeSelectOption value={`${k}:asc`}>{label} (reversed)</NativeSelectOption>
          </Fragment>
        ))}
      </NativeSelect>
    </label>
  );
}

function Invalid({ error }: { error: ValidationError }) {
  return (
    <div className="p-4">
      <Alert variant="destructive" data-testid="search-invalid">
        <AlertTitle>{error.message}</AlertTitle>
        <AlertDescription>
          <ul className="list-disc pl-4">
            {(error.errors ?? []).map((e, i) => (
              <li key={i}>
                <code className="font-mono">{e.field}</code> is {e.code.replace(/_/g, " ")}
                {e.field === "q" &&
                  e.code === "missing" &&
                  ": add a search term, not only qualifiers."}
              </li>
            ))}
          </ul>
        </AlertDescription>
      </Alert>
    </div>
  );
}

function NoResults({ q, type }: { q: string; type: SearchType }) {
  return (
    <div className="px-4 py-10 text-center text-sm" data-testid="no-results">
      <p className="font-medium">
        No {SEARCH_TYPES[type].label.toLowerCase()} match <code className="font-mono">{q}</code>.
      </p>
      <p className="text-muted-foreground mt-1">
        Try fewer qualifiers, or another type in the sidebar.
      </p>
    </div>
  );
}

function Pager({ total, page }: { total: number; page: number }) {
  // GHES returns at most the first 1,000 results of a search.
  const pages = Math.min(Math.ceil(total / SEARCH_PAGE_SIZE), 1000 / SEARCH_PAGE_SIZE);
  if (pages <= 1) return null;
  const to = (n: number) => ({
    to: "/search" as const,
    search: (prev: Search) => ({ ...prev, p: n > 1 ? n : undefined }),
  });
  const nums = Array.from({ length: pages }, (_, i) => i + 1);
  return (
    <nav
      aria-label="Pagination"
      className="flex items-center justify-center gap-1 border-t py-2"
      data-testid="pager"
    >
      <Button
        variant="ghost"
        size="sm"
        className="rounded-sm"
        disabled={page <= 1}
        nativeButton={false}
        render={<Link {...to(page - 1)} />}
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
        Previous
      </Button>
      {nums.map((n) => (
        <Button
          key={n}
          variant={n === page ? "outline" : "ghost"}
          size="icon-sm"
          className="rounded-sm tabular-nums"
          aria-current={n === page ? "page" : undefined}
          nativeButton={false}
          render={<Link {...to(n)} />}
        >
          {n}
        </Button>
      ))}
      <Button
        variant="ghost"
        size="sm"
        className="rounded-sm"
        disabled={page >= pages}
        nativeButton={false}
        render={<Link {...to(page + 1)} />}
        data-testid="pager-next"
      >
        Next
        <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} data-icon="inline-end" />
      </Button>
    </nav>
  );
}

// ---- Highlighting ----

type TextMatches = Schemas["search-result-text-matches"];

// The fragment GHES sent for a property, with its match indices wrapped in <mark>.
function Highlight({
  matches,
  property,
  fallback,
}: {
  matches?: TextMatches;
  property: string;
  fallback: ReactNode;
}) {
  const m = matches?.find((x) => x.property === property);
  if (!m?.fragment || !m.matches?.length) return <>{fallback}</>;
  const len = m.fragment.length;
  const ranges = m.matches
    .flatMap((x) => (x.indices?.length === 2 ? [x.indices as [number, number]] : []))
    .filter(([s]) => s < len)
    .sort((a, b) => a[0] - b[0]);
  const out: ReactNode[] = [];
  let at = 0;
  for (const [s, e] of ranges) {
    if (s < at) continue;
    out.push(m.fragment.slice(at, s));
    out.push(
      <mark key={s} className="bg-warning/30 text-foreground rounded-sm px-0.5" data-testid="match">
        {m.fragment.slice(s, e)}
      </mark>,
    );
    at = e;
  }
  out.push(m.fragment.slice(at));
  return <>{out}</>;
}

// ---- One renderer per type ----

function ResultList<T extends SearchType>({
  type,
  result,
}: {
  type: T;
  result: Extract<SearchResult<SearchItems[T]>, { kind: "ok" }>;
}) {
  const items = result.items as unknown[];
  return (
    <ul className="divide-y" data-testid="search-results" data-type={type}>
      {items.map((item, i) => (
        <li key={i} className="px-4 py-3">
          {type === "repositories" && <RepoResult r={item as SearchItems["repositories"]} />}
          {type === "code" && <CodeResult c={item as SearchItems["code"]} />}
          {type === "commits" && <CommitResult c={item as SearchItems["commits"]} />}
          {(type === "issues" || type === "pullrequests") && (
            <IssueResult i={item as SearchItems["issues"]} />
          )}
          {type === "users" && <UserResult u={item as SearchItems["users"]} />}
          {type === "labels" && <LabelResult l={item as SearchItems["labels"]} />}
        </li>
      ))}
    </ul>
  );
}

function RepoResult({ r }: { r: SearchItems["repositories"] }) {
  return (
    <div className="flex gap-3">
      <HugeiconsIcon
        icon={RepositoryIcon}
        strokeWidth={2}
        className="text-muted-foreground mt-0.5 size-4 shrink-0"
      />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner/$repo"
          params={{ owner: r.owner?.login ?? "", repo: r.name }}
          className="text-primary text-sm font-semibold hover:underline"
        >
          {r.owner?.login}/<Highlight matches={r.text_matches} property="name" fallback={r.name} />
        </Link>
        {r.description && (
          <p className="text-muted-foreground mt-0.5 text-sm">
            <Highlight matches={r.text_matches} property="description" fallback={r.description} />
          </p>
        )}
        <div className="mt-1.5 flex flex-wrap items-center gap-1">
          {r.topics?.map((t) => (
            <Link key={t} to="/search" search={{ q: `topic:${t}` }}>
              <Badge variant="primary-light" size="sm">
                {t}
              </Badge>
            </Link>
          ))}
        </div>
        <div className="text-muted-foreground mt-1.5 flex flex-wrap items-center gap-4 text-xs">
          {r.language && <span>{r.language}</span>}
          <span className="flex items-center gap-1">
            <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3.5" />
            {r.stargazers_count}
          </span>
          <span className="capitalize">{r.visibility}</span>
          <span>
            Updated <Ago at={r.updated_at} />
          </span>
        </div>
      </div>
    </div>
  );
}

function CodeResult({ c }: { c: SearchItems["code"] }) {
  const [owner, repo] = c.repository.full_name.split("/");
  const fragment = c.text_matches?.find((m) => m.property === "content");
  return (
    <div className="min-w-0">
      <p className="text-sm">
        <Link
          to="/$owner/$repo"
          params={{ owner, repo }}
          className="text-muted-foreground hover:underline"
        >
          {c.repository.full_name}
        </Link>
        <span className="text-muted-foreground px-1">·</span>
        <Link
          to="/$owner/$repo/blob/$"
          params={{ owner, repo, _splat: c.path }}
          className="text-primary font-mono font-semibold hover:underline"
        >
          {c.path}
        </Link>
      </p>
      {fragment && (
        <pre
          className="bg-muted mt-2 overflow-x-auto rounded-sm p-2 font-mono text-xs leading-relaxed"
          data-testid="code-fragment"
        >
          <Highlight matches={c.text_matches} property="content" fallback={fragment.fragment} />
        </pre>
      )}
      <p className="text-muted-foreground mt-1 text-xs">
        {c.language ?? "Text"}
        {c.line_numbers?.length ? ` · lines ${c.line_numbers.slice(0, 5).join(", ")}` : ""}
      </p>
    </div>
  );
}

function CommitResult({ c }: { c: SearchItems["commits"] }) {
  const [owner, repo] = c.repository.full_name.split("/");
  const [title, ...rest] = c.commit.message.split("\n");
  return (
    <div className="flex gap-3">
      <HugeiconsIcon
        icon={GitCommitIcon}
        strokeWidth={2}
        className="text-muted-foreground mt-0.5 size-4 shrink-0"
      />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner/$repo/commit/$sha"
          params={{ owner, repo, sha: c.sha }}
          className="text-sm font-semibold hover:underline"
        >
          <Highlight
            matches={c.text_matches?.map((m) => ({ ...m, fragment: m.fragment?.split("\n")[0] }))}
            property="message"
            fallback={title}
          />
        </Link>
        {rest.join(" ").trim() && (
          <p className="text-muted-foreground mt-0.5 line-clamp-1 text-xs">
            {rest.join(" ").trim()}
          </p>
        )}
        <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-1.5 text-xs">
          {c.author && <UserAvatar user={c.author} />}
          <span>{c.author?.login ?? c.commit.author.name}</span>
          committed <Ago at={c.commit.author.date} /> in
          <Link to="/$owner/$repo" params={{ owner, repo }} className="hover:underline">
            {c.repository.full_name}
          </Link>
          <code className="font-mono">{c.sha.slice(0, 7)}</code>
          {c.commit.verification?.verified && (
            <Badge variant="success-light" size="sm">
              Verified
            </Badge>
          )}
        </p>
      </div>
    </div>
  );
}

function IssueResult({ i }: { i: SearchItems["issues"] }) {
  const [owner, repo] = i.repository_url.split("/repos/")[1].split("/");
  const isPull = Boolean(i.pull_request);
  const state = isPull
    ? pullState({ state: i.state, draft: i.draft, merged_at: i.pull_request?.merged_at ?? null })
    : issueState(i);
  const body = i.text_matches?.find((m) => m.property === "body");
  const title = <Highlight matches={i.text_matches} property="title" fallback={i.title} />;
  return (
    <div className="flex gap-3">
      <span className="mt-0.5">
        <StateIcon state={state} />
      </span>
      <div className="min-w-0 flex-1">
        <p className="text-muted-foreground text-xs">
          {owner}/{repo} #{i.number}
        </p>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {isPull ? (
            <Link
              to="/$owner/$repo/pull/$number"
              params={{ owner, repo, number: i.number }}
              className="text-sm font-semibold hover:underline"
            >
              {title}
            </Link>
          ) : (
            <Link
              to="/$owner/$repo/issues/$number"
              params={{ owner, repo, number: i.number }}
              className="text-sm font-semibold hover:underline"
            >
              {title}
            </Link>
          )}
          {i.labels.map((l) => (
            <Link
              key={l.name}
              to="/search"
              search={(prev) => ({
                ...prev,
                q: `${prev.q ?? ""} label:"${l.name}"`.trim(),
                p: undefined,
              })}
            >
              <LabelBadge
                label={{ name: l.name, color: l.color ?? null, description: l.description }}
              />
            </Link>
          ))}
        </div>
        {body && (
          <p className="text-muted-foreground mt-1 line-clamp-2 text-xs">
            <Highlight matches={i.text_matches} property="body" fallback={null} />
          </p>
        )}
        <p className="text-muted-foreground mt-1 text-xs">
          {i.user?.login} opened <Ago at={i.created_at} />
          {i.comments > 0 && ` · ${i.comments} comment${i.comments === 1 ? "" : "s"}`}
        </p>
      </div>
    </div>
  );
}

function UserResult({ u }: { u: SearchItems["users"] }) {
  return (
    <div className="flex gap-3">
      <UserAvatar user={u} size="default" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-2">
          <Link
            to="/$owner"
            params={{ owner: u.login }}
            className="text-primary text-sm font-semibold hover:underline"
          >
            <Highlight matches={u.text_matches} property="login" fallback={u.login} />
          </Link>
          {u.name && (
            <span className="text-muted-foreground text-sm">
              <Highlight matches={u.text_matches} property="name" fallback={u.name} />
            </span>
          )}
          {u.type === "Organization" && (
            <Badge variant="outline" size="sm">
              Organization
            </Badge>
          )}
        </div>
        {u.bio && (
          <p className="text-muted-foreground mt-0.5 text-sm">
            <Highlight matches={u.text_matches} property="bio" fallback={u.bio} />
          </p>
        )}
        <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-3 text-xs">
          {u.location && (
            <span className="flex items-center gap-1">
              <HugeiconsIcon icon={Location01Icon} strokeWidth={2} className="size-3" />
              {u.location}
            </span>
          )}
          <span>{u.followers} followers</span>
          <span>{u.public_repos} repositories</span>
        </p>
      </div>
    </div>
  );
}

function LabelResult({ l }: { l: SearchItems["labels"] }) {
  const { q = "" } = Route.useSearch();
  const { owner, repo } = useLabelRepo(q);
  return (
    <div className="flex flex-wrap items-center gap-3">
      <LabelBadge label={l} />
      <span className="text-muted-foreground flex-1 text-sm">
        <Highlight matches={l.text_matches} property="description" fallback={l.description} />
      </span>
      <Link
        to="/search"
        search={{ q: `repo:${owner}/${repo} label:"${l.name}"`, type: "issues" }}
        className="text-primary text-xs hover:underline"
      >
        Issues with this label
      </Link>
    </div>
  );
}
