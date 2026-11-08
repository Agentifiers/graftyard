import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import {
  allReleasesQuery,
  branchesQuery,
  latestReleaseQuery,
  tagsQuery,
} from "@/areas/releases/queries";
import { ReleaseForm } from "@/areas/releases/release-form";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

// Drafts have no page by tag in GHES, so the editor finds its release in the full list.
export const Route = createFileRoute("/$owner/$repo/releases/edit/$tag")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(tagsQuery(params));
    void context.queryClient.prefetchQuery(branchesQuery(params));
    void context.queryClient.prefetchQuery(allReleasesQuery(params));
    void context.queryClient.prefetchQuery(latestReleaseQuery(params));
  },
  component: EditRelease,
});

function EditRelease() {
  const { tag, ...repo } = Route.useParams();
  return (
    <div className="flex flex-col gap-2">
      <PageHeader
        title={
          <>
            Edit release <code className="font-mono">{tag}</code>
          </>
        }
        description={
          <Link to="/$owner/$repo/releases" params={repo} className="text-primary hover:underline">
            Back to releases
          </Link>
        }
      />
      <Region name="release form" rows={8}>
        <Editor />
      </Region>
    </div>
  );
}

function Editor() {
  const { tag, ...repo } = Route.useParams();
  const { data: releases } = useSuspenseQuery(allReleasesQuery(repo));
  const { data: latest } = useSuspenseQuery(latestReleaseQuery(repo));
  const release = releases.find((r) => r.tag_name === tag);
  if (!release) {
    return (
      <p className="text-muted-foreground border p-6 text-sm" data-testid="release-missing">
        No release uses the tag <code className="font-mono">{tag}</code>.
      </p>
    );
  }
  // Keyed by id, so a saved tag rename that lands back here starts a fresh form.
  return (
    <ReleaseForm key={release.id} {...repo} release={release} latest={release.id === latest?.id} />
  );
}
