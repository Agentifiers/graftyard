import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { PeopleGrid, PeopleTabs } from "@/areas/activity/people";
import { watchersQuery } from "@/areas/activity/queries";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";

// GET /repos/{owner}/{repo}/subscribers: everyone watching, the viewer included.
export const Route = createFileRoute("/$owner/$repo/watchers")({
  loader: ({ context, params }) => void context.queryClient.prefetchQuery(watchersQuery(params)),
  component: WatchersPage,
});

function WatchersPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-3">
      <PeopleTabs {...params} active="watchers" />
      <Region name="watchers" rows={6}>
        <Watchers />
      </Region>
    </div>
  );
}

function Watchers() {
  const { data } = useSuspenseQuery(watchersQuery(Route.useParams()));
  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="min-h-10 flex-row items-center py-1.5">
        <h2 className="text-sm font-semibold" data-testid="watcher-total">
          {data.length} watching
        </h2>
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        <PeopleGrid
          people={data}
          empty={{
            title: "Nobody is watching",
            body: "Watch this repository to hear about every change.",
          }}
        />
      </FramePanel>
    </Frame>
  );
}
