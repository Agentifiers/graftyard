import { createFileRoute } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft02Icon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Loading03Icon,
  MinusSignCircleIcon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { commentsCollection } from "@/api/collections";
import { checkRunsQuery, pullFilesQuery, pullQuery } from "@/areas/core/queries";
import { Slot } from "@/areas/slots";
import { Conversation } from "@/components/conversation";
import { FileDiff } from "@/components/diff";
import { Ago, pullState, StateBadge } from "@/components/github";
import { Region } from "@/components/region";
import { cn } from "cn";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/pull/$number")({
  params: {
    parse: ({ number }) => ({ number: Number(number) }),
    stringify: ({ number }) => ({ number: String(number) }),
  },
  loader: async ({ context, params }) => {
    const { queryClient } = context;
    void queryClient.prefetchQuery(pullFilesQuery(params));
    void commentsCollection(params)
      .preload()
      .catch(() => {});
    // Checks are keyed by the head sha, which only the PR knows.
    const pr = await queryClient.ensureQueryData(pullQuery(params)).catch(() => undefined);
    if (pr) void queryClient.prefetchQuery(checkRunsQuery({ ...params, ref: pr.head.sha }));
  },
  component: PullPage,
});

function PullPage() {
  const params = Route.useParams();
  return (
    <div className="flex flex-col gap-4">
      <Region name="pull request" fallback={<Skeleton className="h-16" />}>
        <PullHeader />
      </Region>
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="flex min-w-0 flex-col gap-4">
          <Section title="Conversation">
            <Region
              name="conversation"
              rows={4}
              onRetry={() => commentsCollection(params).reload()}
            >
              <Thread />
            </Region>
          </Section>
          <Section title="Files changed">
            <Region name="files" rows={6}>
              <Files />
            </Region>
          </Section>
        </div>
        <div className="flex flex-col gap-4 lg:sticky lg:top-4">
          <Section title="Checks">
            <Region name="checks" rows={4}>
              <Checks />
            </Region>
          </Section>
          <Region name="pull request" fallback={null}>
            <PullSidebar />
          </Region>
        </div>
      </div>
    </div>
  );
}

function Section({
  title,
  className,
  children,
}: {
  title: string;
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <section className={cn("flex flex-col gap-3", className)} data-section={title}>
      <h2 className="text-sm font-semibold">{title}</h2>
      {children}
    </section>
  );
}

// Other areas' widgets (reviewers, merge box, agent session). They need the head sha.
function PullSidebar() {
  const params = Route.useParams();
  const { data: pr } = useSuspenseQuery(pullQuery(params));
  return <Slot name="pull-sidebar" props={{ ...params, headSha: pr.head.sha }} />;
}

function PullHeader() {
  const { data: pr } = useSuspenseQuery(pullQuery(Route.useParams()));
  return (
    <div>
      <h1 className="text-xl font-semibold">
        {pr.title} <span className="text-muted-foreground font-normal">#{pr.number}</span>
      </h1>
      <div className="text-muted-foreground mt-1.5 flex flex-wrap items-center gap-2 text-sm">
        <StateBadge state={pullState(pr)} />
        <span>
          <span className="text-foreground font-medium">{pr.user.login}</span>{" "}
          {pr.merged_at ? "merged" : "wants to merge"} {pr.commits} commits into
        </span>
        <code className="bg-muted text-foreground rounded border px-1.5 py-0.5 font-mono text-xs">
          {pr.base.ref}
        </code>
        <HugeiconsIcon
          icon={ArrowLeft02Icon}
          strokeWidth={2}
          className="size-3.5"
          aria-label="from"
        />
        <code className="bg-muted text-foreground rounded border px-1.5 py-0.5 font-mono text-xs">
          {pr.head.ref}
        </code>
        <span className="font-mono text-xs">
          <span className="text-success">+{pr.additions}</span>{" "}
          <span className="text-destructive">-{pr.deletions}</span>
        </span>
      </div>
    </div>
  );
}

function Thread() {
  const params = Route.useParams();
  const { data: pr } = useSuspenseQuery(pullQuery(params));
  return (
    <Conversation
      {...params}
      opening={{
        author: pr.user,
        at: pr.created_at,
        body: pr.body || "_No description provided._",
      }}
    />
  );
}

function Files() {
  const { data: files } = useSuspenseQuery(pullFilesQuery(Route.useParams()));
  return (
    <div className="flex flex-col gap-3" data-testid="files">
      {files.map((f) => (
        <FileDiff key={f.filename} file={f} />
      ))}
    </div>
  );
}

type CheckRun = Schemas["check-run"];

function checkLook(run: CheckRun) {
  if (run.status === "in_progress")
    return { icon: Loading03Icon, tone: "text-warning animate-spin", label: "In progress" };
  if (run.status !== "completed")
    return { icon: Clock01Icon, tone: "text-muted-foreground", label: "Queued" };
  switch (run.conclusion) {
    case "success":
      return { icon: CheckmarkCircle02Icon, tone: "text-success", label: "Success" };
    case "failure":
    case "timed_out":
    case "action_required":
      return {
        icon: CancelCircleIcon,
        tone: "text-destructive",
        label: run.conclusion === "failure" ? "Failure" : run.conclusion,
      };
    default:
      return {
        icon: MinusSignCircleIcon,
        tone: "text-muted-foreground",
        label: run.conclusion ?? "Neutral",
      };
  }
}

function Checks() {
  const params = Route.useParams();
  const { data: pr } = useSuspenseQuery(pullQuery(params));
  const { data } = useSuspenseQuery(checkRunsQuery({ ...params, ref: pr.head.sha }));
  const runs = data.check_runs;
  const passed = runs.filter((r) => r.conclusion === "success").length;
  return (
    <div className="flex flex-col gap-2" data-testid="checks">
      <p className="text-muted-foreground text-xs">
        {passed} of {data.total_count} checks passed
      </p>
      <ul className="divide-y rounded-lg border">
        {runs.map((run) => {
          const look = checkLook(run);
          return (
            <li key={run.id} className="flex items-start gap-2 px-3 py-2 text-sm">
              <HugeiconsIcon
                icon={look.icon}
                strokeWidth={2}
                className={`mt-0.5 size-4 shrink-0 ${look.tone}`}
                aria-label={look.label}
              />
              <div className="min-w-0 flex-1">
                <div className="truncate font-medium">{run.name}</div>
                <div className="text-muted-foreground text-xs break-words">
                  {/* A no-break space ties each dot to what follows, so a line never ends on one. */}
                  <span className="capitalize">{look.label}</span>
                  {run.output.summary && (
                    <>
                      {" "}
                      ·{"\u00a0"}
                      {run.output.summary}
                    </>
                  )}
                  {run.completed_at && (
                    <>
                      {" "}
                      <span className="whitespace-nowrap">
                        ·{"\u00a0"}
                        <Ago at={run.completed_at} />
                      </span>
                    </>
                  )}
                </div>
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}
