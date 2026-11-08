import { createFileRoute } from "@tanstack/react-router";
import { MilestoneForm } from "@/areas/issues/milestone-form";
import { milestonesCollection } from "@/areas/issues/collections";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";

export const Route = createFileRoute("/$owner/$repo/milestones/new")({
  loader: ({ params }) =>
    void milestonesCollection(params)
      .preload()
      .catch(() => {}),
  component: NewMilestonePage,
});

function NewMilestonePage() {
  const params = Route.useParams();
  return (
    <div>
      <PageHeader title="New milestone" description="Track a set of issues toward a target date." />
      <Region name="milestone form" rows={4}>
        <MilestoneForm {...params} />
      </Region>
    </div>
  );
}
