import { createFileRoute, Link } from "@tanstack/react-router";
import { allReleasesQuery, branchesQuery, tagsQuery } from "@/areas/releases/queries";
import { ReleaseForm } from "@/areas/releases/release-form";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/releases/new")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(tagsQuery(params));
    void context.queryClient.prefetchQuery(branchesQuery(params));
    void context.queryClient.prefetchQuery(allReleasesQuery(params));
  },
  component: NewRelease,
});

function NewRelease() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-2">
      <PageHeader
        title="Draft a new release"
        description={
          <>
            Package this repository's software with notes and binaries.{" "}
            <Link
              to="/$owner/$repo/releases"
              params={params}
              className="text-primary hover:underline"
            >
              Back to releases
            </Link>
          </>
        }
      />
      <Region name="release form" rows={8}>
        <ReleaseForm {...params} />
      </Region>
    </div>
  );
}
