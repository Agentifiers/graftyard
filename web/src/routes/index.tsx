import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { RepositoryIcon, StarIcon } from "@hugeicons/core-free-icons";
import { userReposQuery } from "@/areas/core/queries";
import { Ago } from "@/components/github";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel, FrameTitle } from "@/components/reui/frame";

export const Route = createFileRoute("/")({
  loader: ({ context }) => void context.queryClient.prefetchQuery(userReposQuery()),
  component: Home,
});

function Home() {
  return (
    <main className="mx-auto max-w-6xl p-4 sm:p-6">
      <Frame spacing="sm" dense>
        <FrameHeader>
          <FrameTitle>Your repositories</FrameTitle>
        </FrameHeader>
        <FramePanel className="rounded-t-none p-0">
          <Region name="repositories" rows={3}>
            <RepoList />
          </Region>
        </FramePanel>
      </Frame>
    </main>
  );
}

function RepoList() {
  const { data: repos } = useSuspenseQuery(userReposQuery());
  return (
    <ul className="divide-y">
      {repos.map((r) => (
        <li key={r.id} className="flex items-start gap-3 px-4 py-3">
          <HugeiconsIcon
            icon={RepositoryIcon}
            strokeWidth={2}
            className="text-muted-foreground mt-0.5 size-4"
          />
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <Link
                to="/$owner/$repo"
                params={{ owner: r.owner.login, repo: r.name }}
                className="text-primary text-sm font-semibold hover:underline"
              >
                {r.full_name}
              </Link>
              <Badge variant="outline" size="sm" className="capitalize">
                {r.visibility}
              </Badge>
            </div>
            {r.description && (
              <p className="text-muted-foreground mt-0.5 text-sm">{r.description}</p>
            )}
            <div className="text-muted-foreground mt-1.5 flex flex-wrap items-center gap-4 text-xs">
              {r.language && <span>{r.language}</span>}
              <span className="flex items-center gap-1">
                <HugeiconsIcon icon={StarIcon} strokeWidth={2} className="size-3.5" />
                {r.stargazers_count}
              </span>
              {r.pushed_at && (
                <span>
                  Updated <Ago at={r.pushed_at} />
                </span>
              )}
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}
