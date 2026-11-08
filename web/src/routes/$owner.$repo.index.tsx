import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { repoQuery } from "@/areas/core/queries";
import { Slot } from "@/areas/slots";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel, FrameTitle } from "@/components/reui/frame";

export const Route = createFileRoute("/$owner/$repo/")({
  component: RepoOverview,
});

function RepoOverview() {
  const params = Route.useParams();
  return (
    <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
      <div className="flex min-w-0 flex-col gap-4">
        <Slot name="repo-overview" props={params} />
      </div>
      <div className="flex flex-col gap-4">
        <About />
        <Slot name="repo-sidebar" props={params} />
      </div>
    </div>
  );
}

function About() {
  return (
    <Frame spacing="sm" dense>
      <FrameHeader>
        <FrameTitle>About</FrameTitle>
      </FrameHeader>
      <FramePanel className="rounded-t-none">
        <Region name="repository">
          <AboutBody />
        </Region>
      </FramePanel>
    </Frame>
  );
}

function AboutBody() {
  const params = Route.useParams();
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  return (
    <dl className="grid grid-cols-[8rem_1fr] gap-y-2 text-sm">
      <dt className="text-muted-foreground">Language</dt>
      <dd>{repo.language ?? "None"}</dd>
      <dt className="text-muted-foreground">License</dt>
      <dd>{repo.license?.name ?? "None"}</dd>
      <dt className="text-muted-foreground">Open issues</dt>
      <dd>
        <Link to="/$owner/$repo/issues" params={params} className="text-primary hover:underline">
          {repo.open_issues_count}
        </Link>
        <span className="text-muted-foreground">
          {" "}
          (pull requests included, as GHES counts them)
        </span>
      </dd>
      <dt className="text-muted-foreground">Clone</dt>
      <dd>
        <code className="bg-muted rounded px-1.5 py-0.5 font-mono text-xs">{repo.clone_url}</code>
      </dd>
      {repo.topics && repo.topics.length > 0 && (
        <>
          <dt className="text-muted-foreground">Topics</dt>
          <dd className="flex flex-wrap gap-1">
            {repo.topics.map((t) => (
              <Badge key={t} variant="primary-light">
                {t}
              </Badge>
            ))}
          </dd>
        </>
      )}
    </dl>
  );
}
