import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { repoQuery } from "@/areas/core/queries";
import { PeopleGrid, PeopleTabs } from "@/areas/activity/people";
import { PEOPLE_PAGE_SIZE, stargazersQuery } from "@/areas/activity/queries";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";

// GET /repos/{owner}/{repo}/stargazers with the star+json media type, paged.
export const Route = createFileRoute("/$owner/$repo/stargazers")({
  validateSearch: (s: Record<string, unknown>): { page?: number } => ({
    page: Number(s.page) > 1 ? Math.floor(Number(s.page)) : undefined,
  }),
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ context, params, deps }) =>
    void context.queryClient.prefetchQuery(stargazersQuery({ ...params, page: deps.page })),
  component: StargazersPage,
});

function StargazersPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-3">
      <PeopleTabs {...params} active="stargazers" />
      <Frame spacing="sm" dense>
        <FrameHeader className="min-h-10 flex-row items-center py-1.5">
          <Region name="repository" rows={1} fallback={<span className="text-sm">Stargazers</span>}>
            <Heading />
          </Region>
        </FrameHeader>
        <FramePanel className="rounded-t-none p-0">
          <Region name="stargazers" rows={6}>
            <Stargazers />
          </Region>
        </FramePanel>
      </Frame>
    </div>
  );
}

function Heading() {
  const { data: repo } = useSuspenseQuery(repoQuery(Route.useParams()));
  return (
    <h2 className="text-sm font-semibold" data-testid="stargazer-total">
      {repo.stargazers_count.toLocaleString()} stargazers
    </h2>
  );
}

function Stargazers() {
  const params = Route.useParams();
  const { page = 1 } = Route.useSearch();
  const { data } = useSuspenseQuery(stargazersQuery({ ...params, page }));
  return (
    <>
      <PeopleGrid
        people={data}
        empty={{ title: "No stargazers yet", body: "Be the first to star this repository." }}
      />
      {(page > 1 || data.length === PEOPLE_PAGE_SIZE) && (
        <div className="flex justify-center gap-2 border-t p-2">
          <Button
            variant="outline"
            size="sm"
            className="rounded-sm"
            disabled={page <= 1}
            nativeButton={false}
            render={
              <Link from={Route.fullPath} search={{ page: page > 2 ? page - 1 : undefined }} />
            }
          >
            Previous
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="rounded-sm"
            disabled={data.length < PEOPLE_PAGE_SIZE}
            nativeButton={false}
            render={<Link from={Route.fullPath} search={{ page: page + 1 }} />}
            data-testid="stargazers-next"
          >
            Next
          </Button>
        </div>
      )}
    </>
  );
}
