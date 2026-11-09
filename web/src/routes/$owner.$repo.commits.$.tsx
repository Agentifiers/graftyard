import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedCallback } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft01Icon,
  ArrowRight01Icon,
  CodeIcon,
  GitCommitIcon,
  Search01Icon,
  SecurityCheckIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { commitsQuery, contributorsQuery, resolveQuery } from "@/areas/code/queries";
import { CopyButton, RefPicker, firstLine, joinPath, short, useSwitchRef } from "@/areas/code/refs";
import { Ago, UserAvatar } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Skeleton } from "@/components/ui/skeleton";

// Commit history at a ref, grouped by day. The splat is <ref>[/<path>]; ?author and ?page
// are typed search params, and ?path narrows further without changing the splat.

type Search = { author?: string; path?: string; page?: number };

export const Route = createFileRoute("/$owner/$repo/commits/$")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    author: typeof s.author === "string" && s.author ? s.author : undefined,
    path: typeof s.path === "string" && s.path ? s.path : undefined,
    page: Number(s.page) > 1 ? Math.floor(Number(s.page)) : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: async ({ context, params, deps }) => {
    const { owner, repo, _splat = "" } = params;
    const hit = await context.queryClient
      .ensureQueryData(resolveQuery({ owner, repo, splat: _splat }))
      .catch(() => undefined);
    if (hit) {
      void context.queryClient.prefetchQuery(
        commitsQuery({
          owner,
          repo,
          sha: hit.ref,
          path: deps.path ?? (hit.path || undefined),
          author: deps.author,
          page: deps.page ?? 1,
        }),
      );
    }
  },
  component: CommitsPage,
});

function CommitsPage() {
  const { owner, repo, _splat = "" } = Route.useParams();
  return (
    <Region name="commit history" fallback={<Skeleton className="h-64" />}>
      <CommitsBody owner={owner} repo={repo} splat={_splat} />
    </Region>
  );
}

function CommitsBody({ owner, repo, splat }: { owner: string; repo: string; splat: string }) {
  const { data: at } = useSuspenseQuery(resolveQuery({ owner, repo, splat }));
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const switchRef = useSwitchRef("commits", { owner, repo }, at.path);
  const path = search.path ?? at.path;
  const [pathDraft, setPathDraft] = useState(path);
  const debouncedPath = useDebouncedCallback(
    (value: string) =>
      void navigate({
        search: (prev) => ({ ...prev, path: value || undefined, page: undefined }),
        replace: true,
      }),
    { wait: 350 },
  );
  const authors = useQuery(contributorsQuery({ owner, repo }));
  return (
    <div className="flex flex-col gap-3">
      <PageHeader
        title="Commits"
        description={
          <>
            History of <code className="font-mono">{path || repo}</code> on{" "}
            <code className="font-mono">{at.ref}</code>
          </>
        }
      />
      <div className="flex flex-wrap items-center gap-2">
        <RefPicker owner={owner} repo={repo} current={at.ref} onPick={switchRef} />
        <label className="relative w-full max-w-xs">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
          />
          <Input
            aria-label="Filter by path"
            placeholder="Filter by path, such as crates/wire"
            className="pl-8 font-mono text-xs"
            value={pathDraft}
            onChange={(e) => {
              setPathDraft(e.target.value);
              debouncedPath(e.target.value.trim().replace(/^\/+|\/+$/g, ""));
            }}
          />
        </label>
        <NativeSelect
          aria-label="Filter by author"
          value={search.author ?? ""}
          onChange={(e) =>
            void navigate({
              search: (prev) => ({ ...prev, author: e.target.value || undefined, page: undefined }),
            })
          }
        >
          <NativeSelectOption value="">All authors</NativeSelectOption>
          {authors.data?.map((a) => (
            <NativeSelectOption key={a.login} value={a.login ?? ""}>
              {a.login}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        {(search.author || search.path) && (
          <Button
            size="sm"
            variant="ghost"
            onClick={() => {
              setPathDraft(at.path);
              void navigate({ search: {} });
            }}
          >
            Clear filters
          </Button>
        )}
      </div>
      <Region name="commits" rows={10}>
        <CommitList
          owner={owner}
          repo={repo}
          refName={at.ref}
          path={path}
          author={search.author}
          page={search.page ?? 1}
        />
      </Region>
    </div>
  );
}

function CommitList({
  owner,
  repo,
  refName,
  path,
  author,
  page,
}: {
  owner: string;
  repo: string;
  refName: string;
  path: string;
  author?: string;
  page: number;
}) {
  const { data } = useSuspenseQuery(
    commitsQuery({ owner, repo, sha: refName, path: path || undefined, author, page }),
  );
  if (data.commits.length === 0) {
    return (
      <Empty className="border py-12">
        <EmptyHeader>
          <EmptyTitle>No commits match</EmptyTitle>
          <EmptyDescription>Try another path or author, or a different branch.</EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  const days = new Map<string, Schemas["commit"][]>();
  for (const c of data.commits) {
    const day = (c.commit.author?.date ?? "").slice(0, 10);
    days.set(day, [...(days.get(day) ?? []), c]);
  }
  return (
    <div className="flex flex-col gap-4" data-testid="commits">
      {[...days].map(([day, commits]) => (
        <section key={day}>
          <h2 className="text-muted-foreground mb-1.5 flex items-center gap-1.5 text-xs font-medium">
            <HugeiconsIcon icon={GitCommitIcon} strokeWidth={2} className="size-3.5" />
            Commits on{" "}
            {new Date(`${day}T00:00:00Z`).toLocaleDateString("en", {
              month: "short",
              day: "numeric",
              year: "numeric",
              timeZone: "UTC",
            })}
          </h2>
          <ul className="bg-card divide-y rounded-sm border">
            {commits.map((c) => (
              <CommitRow key={c.sha} owner={owner} repo={repo} commit={c} path={path} />
            ))}
          </ul>
        </section>
      ))}
      <nav className="flex items-center justify-center gap-2" aria-label="Pagination">
        <Button
          size="sm"
          variant="outline"
          disabled={page <= 1}
          render={
            page > 1 ? (
              <Link
                from={Route.fullPath}
                search={(prev) => ({ ...prev, page: page - 1 > 1 ? page - 1 : undefined })}
              />
            ) : undefined
          }
        >
          <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
          Newer
        </Button>
        <span className="text-muted-foreground text-xs" data-testid="page">
          Page {page}
        </span>
        <Button
          size="sm"
          variant="outline"
          disabled={!data.hasNext}
          render={
            data.hasNext ? (
              <Link from={Route.fullPath} search={(prev) => ({ ...prev, page: page + 1 })} />
            ) : undefined
          }
        >
          Older
          <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} data-icon="inline-end" />
        </Button>
      </nav>
    </div>
  );
}

function CommitRow({
  owner,
  repo,
  commit: c,
  path,
}: {
  owner: string;
  repo: string;
  commit: Schemas["commit"];
  path: string;
}) {
  const author =
    c.author && "login" in c.author
      ? { login: c.author.login, avatar_url: c.author.avatar_url }
      : null;
  const [title, ...rest] = c.commit.message.split("\n");
  const body = rest.join("\n").trim();
  const [open, setOpen] = useState(false);
  return (
    <li className="flex items-start gap-3 px-3 py-2" data-sha={c.sha}>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <Link
            to="/$owner/$repo/commit/$sha"
            params={{ owner, repo, sha: c.sha }}
            className="truncate text-sm font-semibold hover:underline"
          >
            {firstLine(title)}
          </Link>
          {body && (
            <Button
              size="xs"
              variant="ghost"
              onClick={() => setOpen(!open)}
              aria-expanded={open}
              aria-label="Show the description"
            >
              …
            </Button>
          )}
        </div>
        {open && (
          <pre className="text-muted-foreground mt-1 text-xs whitespace-pre-wrap">{body}</pre>
        )}
        <div className="text-muted-foreground mt-1 flex items-center gap-1.5 text-xs">
          <UserAvatar user={author} />
          <span className="text-foreground font-medium">
            {author?.login ?? c.commit.author?.name}
          </span>
          committed <Ago at={c.commit.author?.date ?? ""} />
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {c.commit.verification?.verified && (
          <Badge variant="success-light" size="sm" title="Signed with a verified key">
            <HugeiconsIcon icon={SecurityCheckIcon} strokeWidth={2} />
            Verified
          </Badge>
        )}
        <Link
          to="/$owner/$repo/commit/$sha"
          params={{ owner, repo, sha: c.sha }}
          className="hover:bg-muted rounded-sm border px-1.5 py-0.5 font-mono text-xs"
        >
          {short(c.sha)}
        </Link>
        <CopyButton value={c.sha} label="Copy the full SHA" />
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label="Browse the repository at this point"
          title="Browse the repository at this point"
          render={
            <Link
              to="/$owner/$repo/tree/$"
              params={{ owner, repo, _splat: joinPath(c.sha, path) }}
            />
          }
        >
          <HugeiconsIcon icon={CodeIcon} strokeWidth={2} />
        </Button>
      </div>
    </li>
  );
}
