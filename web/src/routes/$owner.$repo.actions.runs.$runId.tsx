import { createFileRoute, Link } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  ArrowLeft01Icon,
  CancelCircleIcon,
  Delete02Icon,
  Download01Icon,
  GitBranchIcon,
  GitCommitIcon,
  InformationCircleIcon,
  PackageIcon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, ApiError } from "@/api/client";
import { JobGraph } from "@/areas/actions/job-graph";
import {
  annotationsQuery,
  artifactsQuery,
  jobsQuery,
  pendingDeploymentsQuery,
  runGraphQuery,
  runQuery,
} from "@/areas/actions/queries";
import { RunActions } from "@/areas/actions/run-actions";
import { PendingDeployments } from "@/areas/actions/review";
import {
  Duration,
  EVENT_LABELS,
  formatBytes,
  lookLabel,
  lookOf,
  StatusIcon,
} from "@/areas/actions/status";
import { send, useWrite } from "@/areas/actions/write";
import { Ago, UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/actions/runs/$runId")({
  params: {
    parse: ({ runId }) => ({ runId: Number(runId) }),
    stringify: ({ runId }) => ({ runId: String(runId) }),
  },
  loader: ({ context, params }) => {
    const q = context.queryClient;
    void q.prefetchQuery(runQuery(params));
    void q.prefetchQuery(jobsQuery(params));
    void q.prefetchQuery(runGraphQuery(params));
    void q.prefetchQuery(artifactsQuery(params));
    void q.prefetchQuery(pendingDeploymentsQuery(params));
  },
  component: RunPage,
});

function RunPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Link
        to="/$owner/$repo/actions"
        params={params}
        className="text-muted-foreground hover:text-foreground flex w-fit items-center gap-1 text-sm"
      >
        <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} className="size-4" />
        All workflows
      </Link>
      <Region name="workflow run" fallback={<Skeleton className="h-24" />}>
        <RunHeader />
      </Region>
      <Region name="pending deployments" rows={1}>
        <PendingDeployments {...params} />
      </Region>
      <Panel title="Jobs" flush>
        <Region name="job graph" rows={3}>
          <Graph />
        </Region>
      </Panel>
      <Region name="annotations" rows={2}>
        <Annotations />
      </Region>
      <Region name="artifacts" rows={2}>
        <Artifacts />
      </Region>
    </div>
  );
}

function RunHeader() {
  const params = Route.useParams();
  const { data: run } = useSuspenseQuery(runQuery(params));
  const { data: jobs } = useSuspenseQuery(jobsQuery(params));
  const failed = jobs.jobs.some((j) => j.conclusion === "failure" || j.conclusion === "cancelled");
  const running = run.status !== "completed";
  const pr = run.pull_requests?.[0];
  return (
    <div className="flex flex-col gap-3" data-testid="run-summary">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2 text-xl font-semibold">
            <StatusIcon item={run} className="size-5" />
            {run.display_title}
            <span className="text-muted-foreground font-normal">#{run.run_number}</span>
          </h1>
          <p className="text-muted-foreground mt-1 text-sm">
            {run.name}
            {(run.run_attempt ?? 1) > 1 && <> · Attempt #{run.run_attempt}</>}
          </p>
        </div>
        <RunActions run={run} failed={failed} {...params} />
      </div>
      <dl className="bg-card grid grid-cols-2 gap-x-6 gap-y-3 rounded-sm border px-4 py-3 text-sm sm:grid-cols-4">
        <div className="col-span-2 sm:col-span-1">
          <dt className="text-muted-foreground text-xs">
            Triggered via {EVENT_LABELS[run.event] ?? run.event}
          </dt>
          <dd className="mt-1 flex flex-wrap items-center gap-1.5">
            {run.triggering_actor && <UserAvatar user={run.triggering_actor} />}
            <span className="font-medium">{run.triggering_actor?.login}</span>
            {pr ? (
              <Link
                to="/$owner/$repo/pull/$number"
                params={{ owner: params.owner, repo: params.repo, number: pr.number }}
                className="hover:underline"
              >
                #{pr.number}
              </Link>
            ) : (
              <code className="flex items-center gap-1 font-mono text-xs">
                <HugeiconsIcon icon={GitCommitIcon} strokeWidth={2} className="size-3.5" />
                {run.head_sha.slice(0, 7)}
              </code>
            )}
            <span className="text-muted-foreground">
              <Ago at={run.created_at} />
            </span>
          </dd>
        </div>
        <div>
          <dt className="text-muted-foreground text-xs">Status</dt>
          <dd className="mt-1 font-medium" data-testid="run-status">
            {lookLabel(lookOf(run))}
          </dd>
        </div>
        <div>
          <dt className="text-muted-foreground text-xs">Total duration</dt>
          <dd className="mt-1 font-medium" data-testid="run-duration">
            <Duration
              start={run.run_started_at}
              end={running ? null : run.updated_at}
              running={running}
            />
          </dd>
        </div>
        <div>
          <dt className="text-muted-foreground text-xs">Branch</dt>
          <dd className="mt-1">
            <Badge variant="info-light" className="font-mono">
              <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} />
              {run.head_branch}
            </Badge>
          </dd>
        </div>
      </dl>
    </div>
  );
}

function Graph() {
  const params = Route.useParams();
  const { data: graph } = useSuspenseQuery(runGraphQuery(params));
  const { data: jobs } = useSuspenseQuery(jobsQuery(params));
  // A re-run makes new job ids; the graph follows the jobs list's latest attempt.
  const byName = new Map(jobs.jobs.map((j) => [j.name, j.id]));
  const nodes = graph.jobs.map((g) => ({ ...g, job_id: byName.get(g.name) ?? g.job_id }));
  return <JobGraph graph={nodes} jobs={jobs.jobs} {...params} />;
}

const LEVEL = {
  failure: { icon: CancelCircleIcon, tone: "text-destructive" },
  warning: { icon: Alert02Icon, tone: "text-warning-foreground dark:text-warning" },
  notice: { icon: InformationCircleIcon, tone: "text-info-foreground dark:text-info" },
} as const;

function Annotations() {
  const params = Route.useParams();
  const { data: jobs } = useSuspenseQuery(jobsQuery(params));
  const checkRunIds = jobs.jobs
    .map((j) => Number(j.check_run_url.split("/").pop()))
    .filter((id) => Number.isFinite(id) && id > 0);
  const { data } = useSuspenseQuery(
    annotationsQuery({
      owner: params.owner,
      repo: params.repo,
      checkRunIds,
      stamp: jobs.jobs.map((j) => `${j.run_attempt}:${j.status}:${j.conclusion}`).join(","),
    }),
  );
  if (data.length === 0) return null;
  const nameOf = (checkRunId: number) =>
    jobs.jobs.find((j) => j.check_run_url.endsWith(`/${checkRunId}`));
  return (
    <Panel title={`Annotations (${data.length})`} flush>
      <ul className="divide-y" data-testid="annotations">
        {data.map((a, i) => {
          const look =
            LEVEL[(a.annotation_level as keyof typeof LEVEL) ?? "notice"] ?? LEVEL.notice;
          const job = nameOf(a.check_run_id);
          return (
            <li key={i} className="flex items-start gap-3 px-4 py-2.5 text-sm">
              <HugeiconsIcon
                icon={look.icon}
                strokeWidth={2}
                className={`mt-0.5 size-4 shrink-0 ${look.tone}`}
              />
              <div className="min-w-0 flex-1">
                <div className="font-medium">
                  {job ? (
                    <Link
                      to="/$owner/$repo/actions/runs/$runId/job/$jobId"
                      params={{ ...params, jobId: job.id }}
                      className="hover:underline"
                    >
                      {job.name}
                    </Link>
                  ) : null}
                  {a.title && (
                    <span className="text-muted-foreground font-normal">: {a.title}</span>
                  )}
                </div>
                <p className="mt-0.5 whitespace-pre-wrap">{a.message}</p>
                <a
                  href={a.blob_href}
                  className="text-muted-foreground font-mono text-xs hover:underline"
                >
                  {a.path}#L{a.start_line}
                  {a.end_line !== a.start_line && `-L${a.end_line}`}
                </a>
              </div>
            </li>
          );
        })}
      </ul>
    </Panel>
  );
}

// GHES answers the archive URL with a 302 to a signed blob; fetch follows it inside the
// app (the mock serves both), and the browser saves the zip.
async function download(owner: string, repo: string, id: number, name: string) {
  const { data, error, response } = await api.GET(
    "/repos/{owner}/{repo}/actions/artifacts/{artifact_id}/{archive_format}",
    { params: { path: { owner, repo, artifact_id: id, archive_format: "zip" } }, parseAs: "blob" },
  );
  if (!response.ok || !data) throw new ApiError(response.status, error);
  const url = URL.createObjectURL(data);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${name}.zip`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Artifacts() {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(artifactsQuery(params));
  const queryClient = useQueryClient();
  const key = artifactsQuery(params).queryKey;
  const remove = useWrite({
    run: (id: number) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/actions/artifacts/{artifact_id}", {
          params: { path: { owner: params.owner, repo: params.repo, artifact_id: id } },
        }),
      ),
    invalidates: () => [key],
    optimistic: (id) => {
      const before = queryClient.getQueryData(key);
      queryClient.setQueryData(
        key,
        (d) =>
          d && {
            ...d,
            total_count: d.total_count - 1,
            artifacts: d.artifacts.filter((a) => a.id !== id),
          },
      );
      return () => queryClient.setQueryData(key, before);
    },
    success: () => "Artifact deleted",
    failure: "Could not delete the artifact",
  });
  return (
    <Panel title={`Artifacts (${data.total_count})`} flush>
      {data.artifacts.length === 0 ? (
        <p className="text-muted-foreground px-4 py-4 text-sm" data-testid="artifacts-empty">
          This run produced no artifacts.
        </p>
      ) : (
        <ul className="divide-y" data-testid="artifacts">
          {data.artifacts.map((a) => (
            <li
              key={a.id}
              className="flex items-center gap-3 px-4 py-2 text-sm"
              data-artifact={a.name}
            >
              <HugeiconsIcon
                icon={PackageIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <span className="min-w-0 flex-1 truncate font-medium">{a.name}</span>
              <span className="text-muted-foreground text-xs tabular-nums">
                {formatBytes(a.size_in_bytes)}
              </span>
              {a.expires_at && (
                <span className="text-muted-foreground hidden text-xs sm:inline">
                  expires <Ago at={a.expires_at} />
                </span>
              )}
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Download ${a.name}`}
                disabled={a.expired}
                onClick={() =>
                  download(params.owner, params.repo, a.id, a.name).then(
                    () => toast.success(`Downloaded ${a.name}.zip`),
                    (e) =>
                      toast.error(`Could not download ${a.name}`, {
                        description: e instanceof Error ? e.message : String(e),
                      }),
                  )
                }
              >
                <HugeiconsIcon icon={Download01Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete ${a.name}`}
                onClick={() => remove.mutate(a.id)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
