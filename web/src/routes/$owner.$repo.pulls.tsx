import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { pullsQuery } from "@/api/queries";
import { Ago, LabelBadge, pullState, StateBadge, StateIcon } from "@/components/github";
import { Region } from "@/components/region";
import { Frame, FrameHeader, FramePanel, FrameTitle } from "@/components/reui/frame";

export const Route = createFileRoute("/$owner/$repo/pulls")({
  loader: ({ context, params }) => void context.queryClient.prefetchQuery(pullsQuery(params)),
  component: PullsPage,
});

function PullsPage() {
  return (
    <Frame spacing="sm" dense>
      <FrameHeader>
        <FrameTitle>Pull requests</FrameTitle>
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        <Region name="pull requests" rows={4}>
          <PullList />
        </Region>
      </FramePanel>
    </Frame>
  );
}

function PullList() {
  const params = Route.useParams();
  const { data: pulls } = useSuspenseQuery(pullsQuery(params));
  if (pulls.length === 0)
    return (
      <p className="text-muted-foreground px-4 py-10 text-center text-sm">No pull requests yet.</p>
    );
  return (
    <ul className="divide-y" data-testid="pull-list">
      {pulls.map((pr) => {
        const state = pullState(pr);
        return (
          <li key={pr.id} className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5">
            <span className="mt-0.5">
              <StateIcon state={state} />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                <Link
                  to="/$owner/$repo/pull/$number"
                  params={{ ...params, number: pr.number }}
                  className="text-sm font-semibold hover:underline"
                >
                  {pr.title}
                </Link>
                {pr.labels.map((l) => (
                  <LabelBadge key={l.id} label={l} />
                ))}
              </div>
              <p className="text-muted-foreground mt-0.5 text-xs">
                #{pr.number} by {pr.user?.login} · updated <Ago at={pr.updated_at} /> ·{" "}
                <code className="font-mono">{pr.head.ref}</code>
              </p>
            </div>
            <StateBadge state={state} size="default" />
          </li>
        );
      })}
    </ul>
  );
}
