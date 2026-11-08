import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft01Icon,
  ArrowRight01Icon,
  PlusSignIcon,
  Tag01Icon,
} from "@hugeicons/core-free-icons";
import { allReleasesQuery, latestReleaseQuery, releasesPageQuery } from "@/areas/releases/queries";
import { ReleaseCard } from "@/areas/releases/release-card";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";

export const Route = createFileRoute("/$owner/$repo/releases/")({
  validateSearch: (search: Record<string, unknown>): { page?: number } => {
    const page = Number(search.page);
    return { page: Number.isInteger(page) && page > 1 ? page : undefined };
  },
  loaderDeps: ({ search }) => ({ page: search.page ?? 1 }),
  loader: ({ context, params, deps }) => {
    void context.queryClient.prefetchQuery(releasesPageQuery({ ...params, page: deps.page }));
    void context.queryClient.prefetchQuery(latestReleaseQuery(params));
    void context.queryClient.prefetchQuery(allReleasesQuery(params));
  },
  component: ReleasesPage,
});

function ReleasesPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Releases"
        description={
          <Region name="release count" fallback={<>Releases of this repository</>}>
            <ReleaseCount />
          </Region>
        }
        actions={
          <>
            <Button
              variant="outline"
              size="sm"
              render={<Link to="/$owner/$repo/tags" params={params} />}
            >
              <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} data-icon="inline-start" />
              Tags
            </Button>
            <Button size="sm" render={<Link to="/$owner/$repo/releases/new" params={params} />}>
              <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
              Draft a new release
            </Button>
          </>
        }
      />
      <Region name="releases" rows={10}>
        <ReleaseList />
      </Region>
    </div>
  );
}

function ReleaseCount() {
  const { data } = useSuspenseQuery(allReleasesQuery(Route.useParams()));
  const drafts = data.filter((r) => r.draft).length;
  return (
    <span data-testid="release-count">
      {data.length} {data.length === 1 ? "release" : "releases"}
      {drafts > 0 && `, ${drafts} ${drafts === 1 ? "draft" : "drafts"}`}
    </span>
  );
}

function ReleaseList() {
  const params = Route.useParams();
  const { page = 1 } = Route.useSearch();
  const { data } = useSuspenseQuery(releasesPageQuery({ ...params, page }));
  const { data: latest } = useSuspenseQuery(latestReleaseQuery(params));

  if (data.releases.length === 0 && page === 1) {
    return (
      <Empty className="border" data-testid="releases-empty">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <HugeiconsIcon icon={Tag01Icon} strokeWidth={2} />
          </EmptyMedia>
          <EmptyTitle>There aren't any releases here</EmptyTitle>
          <EmptyDescription>
            Releases package software, with release notes and binary files, for people to use.
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent>
          <Button size="sm" render={<Link to="/$owner/$repo/releases/new" params={params} />}>
            Create a new release
          </Button>
        </EmptyContent>
      </Empty>
    );
  }

  return (
    <div className="flex flex-col gap-6" data-testid="release-list">
      {data.releases.map((r) => (
        <ReleaseCard key={r.id} {...params} release={r} latest={r.id === latest?.id} />
      ))}
      {(page > 1 || data.hasNext) && (
        <nav className="flex justify-center gap-2" aria-label="Pagination">
          <Button
            variant="outline"
            size="sm"
            disabled={page <= 1}
            render={
              <Link
                from={Route.fullPath}
                search={{ page: page - 1 > 1 ? page - 1 : undefined }}
                disabled={page <= 1}
              />
            }
          >
            <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
            Previous
          </Button>
          <span className="text-muted-foreground self-center text-sm tabular-nums">
            Page {page}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={!data.hasNext}
            render={
              <Link from={Route.fullPath} search={{ page: page + 1 }} disabled={!data.hasNext} />
            }
          >
            Next
            <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        </nav>
      )}
    </div>
  );
}
