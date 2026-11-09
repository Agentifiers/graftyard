import { useEffect, useState } from "react";
import { createFileRoute, Link, useLocation } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedValue } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  ArrowLeft01Icon,
  InformationCircleIcon,
  Refresh01Icon,
  Search01Icon,
} from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { byStep, useJobLog } from "@/areas/actions/log";
import { matchesIn, parseAnchor, StepSection, useScrollToAnchor } from "@/areas/actions/log-view";
import { jobQuery, jobsQuery, runQuery } from "@/areas/actions/queries";
import { Duration, lookLabel, lookOf, StatusIcon } from "@/areas/actions/status";
import { send, useWrite } from "@/areas/actions/write";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/actions/runs_/$runId/job/$jobId")({
  params: {
    parse: ({ runId, jobId }) => ({ runId: Number(runId), jobId: Number(jobId) }),
    stringify: ({ runId, jobId }) => ({ runId: String(runId), jobId: String(jobId) }),
  },
  validateSearch: (s: Record<string, unknown>): { q?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(runQuery(params));
    void context.queryClient.prefetchQuery(jobsQuery(params));
    void context.queryClient.prefetchQuery(jobQuery(params));
  },
  component: JobPage,
});

function JobPage() {
  const params = Route.useParams();
  return (
    <div className="grid items-start gap-6 md:grid-cols-[14rem_minmax(0,1fr)]">
      <nav aria-label="Jobs" className="flex flex-col gap-1 text-sm md:sticky md:top-4">
        <Link
          to="/$owner/$repo/actions/runs/$runId"
          params={params}
          className="text-muted-foreground hover:text-foreground mb-2 flex items-center gap-1 px-2"
        >
          <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} className="size-4" />
          Summary
        </Link>
        <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">Jobs</h2>
        <Region name="jobs" rows={4}>
          <JobList />
        </Region>
      </nav>
      <div className="min-w-0">
        <Region name="job" fallback={<Skeleton className="h-64" />}>
          <JobLog />
        </Region>
      </div>
    </div>
  );
}

function JobList() {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(jobsQuery(params));
  return (
    <ul className="flex flex-col gap-0.5" data-testid="job-list">
      {data.jobs.map((j) => (
        <li key={j.id}>
          <Link
            to="/$owner/$repo/actions/runs/$runId/job/$jobId"
            params={{ ...params, jobId: j.id }}
            data-active={j.id === params.jobId || undefined}
            className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 font-medium"
          >
            <StatusIcon item={j} />
            <span className="truncate">{j.name}</span>
          </Link>
        </li>
      ))}
    </ul>
  );
}

function JobLog() {
  const params = Route.useParams();
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { hash } = useLocation();
  const { data: job } = useSuspenseQuery(jobQuery(params));
  const { data: run } = useSuspenseQuery(runQuery(params));
  const log = useJobLog(params, job);
  const steps = byStep(log.lines);
  const anchor = parseAnchor(hash);
  const [query, setQuery] = useState(q);
  const [debounced] = useDebouncedValue(query, { wait: 200 });
  useEffect(() => {
    if (debounced !== q)
      void navigate({ search: { q: debounced || undefined }, replace: true, hash: true });
  }, [debounced, q, navigate]);

  // Open the step with the linked line, the running step, and failed steps.
  const [open, setOpen] = useState<Set<number>>(() => new Set());
  const [closed, setClosed] = useState<Set<number>>(() => new Set());
  const isOpen = (n: number) => {
    if (open.has(n)) return true;
    if (closed.has(n)) return false;
    const s = job.steps?.find((x) => x.number === n);
    return (
      anchor?.step === n ||
      s?.status === "in_progress" ||
      s?.conclusion === "failure" ||
      Boolean(debounced && matchesIn(steps.get(n), debounced) > 0)
    );
  };
  const toggle = (n: number) => {
    const now = isOpen(n);
    setOpen((o) => {
      const next = new Set(o);
      if (now) next.delete(n);
      else next.add(n);
      return next;
    });
    setClosed((c) => {
      const next = new Set(c);
      if (now) next.add(n);
      else next.delete(n);
      return next;
    });
  };
  useScrollToAnchor(anchor, log.source !== "connecting");

  const total = [...steps.values()].reduce((n, s) => n + matchesIn(s, debounced), 0);

  const rerun = useWrite({
    run: () =>
      send(
        api.POST("/repos/{owner}/{repo}/actions/jobs/{job_id}/rerun", {
          params: { path: { owner: params.owner, repo: params.repo, job_id: job.id } },
          body: {},
        }),
      ),
    invalidates: () => [["repos", params.owner, params.repo, "actions"]],
    success: () => `Re-running ${job.name}`,
    failure: `Could not re-run ${job.name}`,
  });
  // A re-run makes a new job: follow it once the jobs list has it.
  const { data: jobs } = useSuspenseQuery(jobsQuery(params));
  const latest = jobs.jobs.find((j) => j.name === job.name);
  useEffect(() => {
    if (rerun.isSuccess && latest && latest.id !== job.id)
      void navigate({ params: { ...params, jobId: latest.id }, replace: true });
  }, [rerun.isSuccess, latest, job.id, navigate, params]);

  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="flex-col gap-2 py-2">
        <div className="flex flex-wrap items-center gap-2">
          <StatusIcon item={job} className="size-5" />
          <h1 className="text-base font-semibold" data-testid="job-name">
            {job.name}
          </h1>
          <span className="text-muted-foreground text-xs" data-testid="job-status">
            {lookLabel(lookOf(job))}
            {job.status !== "queued" && job.status !== "waiting" && (
              <>
                {" "}
                in{" "}
                <Duration
                  start={job.started_at}
                  end={job.completed_at}
                  running={job.status === "in_progress"}
                />
              </>
            )}
          </span>
          <LogSource
            source={log.source}
            fallback={log.fallback}
            running={job.status === "in_progress"}
          />
          <div className="ml-auto flex items-center gap-2">
            <label className="relative w-56">
              <HugeiconsIcon
                icon={Search01Icon}
                strokeWidth={2}
                className="text-muted-foreground absolute top-1/2 left-2 size-3.5 -translate-y-1/2"
              />
              <Input
                type="search"
                aria-label="Search logs"
                placeholder="Search logs"
                className="h-7 rounded-sm pl-7 text-xs"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
              />
            </label>
            {debounced && (
              <span
                className="text-muted-foreground text-xs tabular-nums"
                data-testid="search-count"
              >
                {total} result{total === 1 ? "" : "s"}
              </span>
            )}
            <Button
              variant="outline"
              size="sm"
              disabled={run.status !== "completed" || rerun.isPending}
              onClick={() => rerun.mutate(undefined)}
              title={run.status !== "completed" ? "Wait for the run to finish" : undefined}
            >
              <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
              Re-run this job
            </Button>
          </div>
        </div>
        {job.runner_name && (
          <p className="text-muted-foreground text-xs">
            Ran on <span className="font-mono">{job.runner_name}</span> ({job.runner_group_name})
            with labels{" "}
            {job.labels.map((l) => (
              <Badge key={l} variant="outline" size="sm" className="mr-1 font-mono">
                {l}
              </Badge>
            ))}
          </p>
        )}
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0" data-testid="job-log">
        {job.status === "queued" || job.status === "waiting" ? (
          <p
            className="text-muted-foreground px-4 py-8 text-center text-sm"
            data-testid="job-queued"
          >
            {job.status === "waiting"
              ? "This job is waiting for a deployment review."
              : `Waiting for a runner with the labels ${job.labels.join(", ")} to pick up this job.`}
          </p>
        ) : log.source === "connecting" && log.error ? (
          <div className="p-4">
            <Alert variant="destructive" data-region-error="job log">
              <HugeiconsIcon icon={AlertCircleIcon} strokeWidth={2} />
              <AlertTitle>Could not load the log</AlertTitle>
              <AlertDescription>
                {log.error}
                <Button size="sm" variant="outline" className="mt-1" onClick={log.retry}>
                  Try again
                </Button>
              </AlertDescription>
            </Alert>
          </div>
        ) : log.source === "connecting" ? (
          <div className="p-4">
            <Skeleton className="h-40" />
          </div>
        ) : (
          (job.steps ?? []).map((s) => (
            <StepSection
              key={s.number}
              step={s}
              log={steps.get(s.number)}
              open={isOpen(s.number)}
              onToggle={() => toggle(s.number)}
              query={debounced}
              target={anchor?.step === s.number ? anchor.line : undefined}
              live={log.source === "live"}
            />
          ))
        )}
      </FramePanel>
      <span hidden data-log-lines={log.lines.length} data-log-source={log.source} />
    </Frame>
  );
}

function LogSource({
  source,
  fallback,
  running,
}: {
  source: string;
  fallback: boolean;
  running: boolean;
}) {
  if (source === "live")
    return (
      <Badge variant="success-light" size="sm" data-testid="log-live">
        <span className="bg-success size-1.5 animate-pulse rounded-sm" />
        Live
      </Badge>
    );
  if (fallback)
    return (
      <span
        className="text-muted-foreground flex items-center gap-1 text-xs"
        data-testid="log-fallback"
      >
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} className="size-3.5" />
        Live stream unavailable, showing saved logs{running ? " (refreshing)" : ""}
      </span>
    );
  return null;
}
