import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { MilestoneForm } from "@/areas/issues/milestone-form";
import { milestonesCollection } from "@/areas/issues/collections";
import { milestoneQuery } from "@/areas/issues/queries";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/milestones/$number_/edit")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(milestoneQuery(params));
    void milestonesCollection(params)
      .preload()
      .catch(() => {});
  },
  component: EditMilestonePage,
});

function EditMilestonePage() {
  return (
    <div>
      <PageHeader title="Edit milestone" />
      <Region name="milestone" rows={4}>
        <Body />
      </Region>
    </div>
  );
}

function Body() {
  const { owner, repo, number } = Route.useParams();
  const { data: milestone } = useSuspenseQuery(milestoneQuery({ owner, repo, number }));
  return <MilestoneForm owner={owner} repo={repo} milestone={milestone} />;
}
