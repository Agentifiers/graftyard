import { createFileRoute, Link, Outlet } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitBranchIcon, RepositoryIcon, StarIcon } from "@hugeicons/core-free-icons";
import { repoQuery } from "@/areas/core/queries";
import { Slot } from "@/areas/slots";
import { Region } from "@/components/region";
import { TabNav } from "@/components/tab-nav";
import { Badge } from "@/components/reui/badge";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo")({
  loader: ({ context, params }) => void context.queryClient.prefetchQuery(repoQuery(params)),
  component: RepoLayout,
});

function RepoLayout() {
  const params = Route.useParams();
  return (
    <>
      <div className="bg-card border-b">
        <div className="mx-auto max-w-6xl px-4 pt-4">
          <Region name="repository" fallback={<Skeleton className="mb-3 h-12 w-2/3" />}>
            <RepoHeader />
          </Region>
          <RepoTabs owner={params.owner} repo={params.repo} />
        </div>
      </div>
      <main className="mx-auto max-w-6xl p-4 sm:p-6">
        <Outlet />
      </main>
    </>
  );
}

function RepoHeader() {
  const { data: repo } = useSuspenseQuery(repoQuery(Route.useParams()));
  return (
    <div className="flex flex-wrap items-start gap-x-6 gap-y-2 pb-3">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <HugeiconsIcon
            icon={RepositoryIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <h1 className="text-lg">
            <span className="text-muted-foreground">{repo.owner.login}</span>
            <span className="text-muted-foreground px-1">/</span>
            <Link
              to="/$owner/$repo"
              params={{ owner: repo.owner.login, repo: repo.name }}
              className="font-semibold hover:underline"
            >
              {repo.name}
            </Link>
          </h1>
          <Badge variant="outline" className="capitalize">
            {repo.visibility ?? (repo.private ? "private" : "public")}
          </Badge>
        </div>
        {repo.description && (
          <p className="text-muted-foreground mt-1 text-sm">{repo.description}</p>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Slot name="repo-actions" props={{ owner: repo.owner.login, repo: repo.name }} bare />
      </div>
      <div className="text-muted-foreground flex w-full items-center gap-4 text-xs">
        <span className="flex items-center gap-1" title="Default branch">
          <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className="size-3.5" />
          <code className="font-mono">{repo.default_branch}</code>
        </span>
        <span className="flex items-center gap-1" title="Stars">
          <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3.5" />
          {repo.stargazers_count}
        </span>
      </div>
    </div>
  );
}

function RepoTabs({ owner, repo }: { owner: string; repo: string }) {
  return <TabNav slot="repo" params={{ owner, repo }} label="Repository" />;
}
