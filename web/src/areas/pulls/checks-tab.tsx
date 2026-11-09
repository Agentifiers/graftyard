import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  CancelCircleIcon,
  InformationCircleIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "cn";
import { runLook, StateIconOf } from "./bits";
import { checkRollup } from "./merge-rules";
import { useRerun } from "./mutations";
import {
  annotationsQuery,
  checkRunsQuery,
  checkSuitesQuery,
  combinedStatusQuery,
  mergeRequirementsQuery,
  pullQuery,
  type Pull,
} from "./queries";

type Run = Schemas["check-run"];

const ANNOTATION = {
  failure: { icon: CancelCircleIcon, tone: "text-destructive" },
  warning: { icon: Alert02Icon, tone: "text-warning" },
  notice: { icon: InformationCircleIcon, tone: "text-info" },
} as const;

// Check runs (with output and annotations, re-run per run or per suite) and the legacy
// commit statuses, for the head commit. Runs poll while any is unfinished.
export function ChecksTab({
  pull,
  runId,
  onRun,
}: {
  pull: Pull;
  runId?: number;
  onRun: (id: number) => void;
}) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const ref = { ...pull, ref: pr.head.sha };
  const { data } = useSuspenseQuery(checkRunsQuery(ref));
  const { data: suites } = useSuspenseQuery(checkSuitesQuery(ref));
  const { data: req } = useSuspenseQuery(mergeRequirementsQuery(pull));
  const rerun = useRerun(pull);
  const runs = data.check_runs;
  const selected =
    runs.find((r) => r.id === runId) ?? runs.find((r) => r.conclusion === "failure") ?? runs[0];
  const roll = checkRollup(runs);
  const required = new Set(req.required_checks);

  return (
    <div className="flex flex-col gap-4" data-testid="checks-tab">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span data-testid="checks-summary">
          {roll.passed} of {roll.total} checks passed
          {roll.failed > 0 && `, ${roll.failed} failed`}
          {roll.pending > 0 && `, ${roll.pending} running`}
        </span>
        <span className="text-muted-foreground font-mono text-xs">
          on {pr.head.sha.slice(0, 7)}
        </span>
        {suites.check_suites.map((s) => (
          <Button
            key={s.id}
            size="sm"
            variant="outline"
            className="ml-auto"
            disabled={rerun.isPending || s.status !== "completed"}
            onClick={() => rerun.mutate({ suiteId: s.id })}
          >
            <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
            Re-run all checks
          </Button>
        ))}
      </div>
      {runs.length === 0 ? (
        <p className="text-muted-foreground rounded-sm border px-3 py-6 text-center text-sm">
          No checks have reported on this commit.
        </p>
      ) : (
        <div className="grid items-start gap-3 md:grid-cols-[16rem_minmax(0,1fr)]">
          <ul className="divide-y rounded-sm border" role="listbox" aria-label="Check runs">
            {runs.map((run) => {
              const look = runLook(run);
              return (
                <li key={run.id}>
                  <button
                    type="button"
                    role="option"
                    aria-selected={run.id === selected?.id}
                    className="hover:bg-muted/50 aria-selected:bg-muted flex w-full items-start gap-2 px-3 py-2 text-left text-sm"
                    onClick={() => onRun(run.id)}
                    data-run={run.name}
                    data-status={run.status}
                    data-conclusion={run.conclusion ?? ""}
                  >
                    <StateIconOf {...look} className="mt-0.5" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">{run.name}</span>
                      <span className="text-muted-foreground block text-xs capitalize">
                        {look.label}
                        {required.has(run.name) && " · required"}
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
          {selected && (
            <RunDetail pull={pull} run={selected} required={required.has(selected.name)} />
          )}
        </div>
      )}
      <Region name="commit statuses" rows={2}>
        <Statuses pull={pull} sha={pr.head.sha} />
      </Region>
    </div>
  );
}

function RunDetail({ pull, run, required }: { pull: Pull; run: Run; required: boolean }) {
  const rerun = useRerun(pull);
  const look = runLook(run);
  return (
    <section
      className="bg-card overflow-hidden rounded-sm border"
      data-testid="run-detail"
      data-run={run.name}
    >
      <header className="bg-muted/50 flex flex-wrap items-center gap-2 border-b px-3 py-2">
        <StateIconOf {...look} />
        <h3 className="font-semibold">{run.name}</h3>
        {required && (
          <Badge variant="outline" size="sm">
            Required
          </Badge>
        )}
        <span className="text-muted-foreground text-xs">
          {run.app?.name}
          {run.completed_at ? (
            <>
              {" · completed "}
              <Ago at={run.completed_at} />
            </>
          ) : run.started_at ? (
            <>
              {" · started "}
              <Ago at={run.started_at} />
            </>
          ) : null}
        </span>
        <Button
          size="sm"
          variant="outline"
          className="ml-auto"
          disabled={rerun.isPending || run.status !== "completed"}
          onClick={() => rerun.mutate({ runId: run.id })}
          data-testid="rerun"
        >
          <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
          Re-run
        </Button>
      </header>
      <div className="flex flex-col gap-3 p-3">
        {run.output.title && <p className="font-medium">{run.output.title}</p>}
        {run.output.summary && <MarkdownBody text={run.output.summary} />}
        {run.output.text && (
          <pre className="bg-muted rounded-sm p-2 font-mono text-xs leading-5 break-words whitespace-pre-wrap">
            {run.output.text.split("\n").map((line, i) => (
              <span
                key={i}
                className={cn(
                  "block",
                  line.startsWith("+") && "bg-success/15",
                  line.startsWith("-") && "bg-destructive/15",
                )}
              >
                {line || " "}
              </span>
            ))}
          </pre>
        )}
        {run.output.annotations_count > 0 && (
          <Region name="annotations" rows={2}>
            <Annotations pull={pull} runId={run.id} />
          </Region>
        )}
      </div>
    </section>
  );
}

function Annotations({ pull, runId }: { pull: Pull; runId: number }) {
  const { data } = useSuspenseQuery(annotationsQuery({ ...pull, runId }));
  if (data.length === 0) return null;
  return (
    <div className="flex flex-col gap-1.5" data-testid="annotations">
      <h4 className="text-muted-foreground text-xs font-semibold">Annotations</h4>
      {data.map((a, i) => {
        const level = (a.annotation_level ?? "notice") as keyof typeof ANNOTATION;
        const look = ANNOTATION[level] ?? ANNOTATION.notice;
        return (
          <div key={i} className="rounded-sm border" data-level={level}>
            <div className="flex items-start gap-2 px-3 py-2 text-sm">
              <HugeiconsIcon
                icon={look.icon}
                strokeWidth={2}
                className={cn("mt-0.5 size-4 shrink-0", look.tone)}
              />
              <div className="min-w-0 flex-1">
                <p className="font-medium">{a.title ?? level}</p>
                <p className="text-muted-foreground font-mono text-xs">
                  {a.path}#L{a.start_line}
                  {a.end_line !== a.start_line && `-L${a.end_line}`}
                </p>
                <MarkdownBody text={a.message ?? ""} className="prose-p:my-0 mt-1" />
              </div>
            </div>
            {a.raw_details && (
              <pre className="bg-muted border-t px-3 py-1.5 font-mono text-xs break-words whitespace-pre-wrap">
                {a.raw_details}
              </pre>
            )}
          </div>
        );
      })}
    </div>
  );
}

function Statuses({ pull, sha }: { pull: Pull; sha: string }) {
  const { data } = useSuspenseQuery(combinedStatusQuery({ ...pull, ref: sha }));
  return (
    <section className="flex flex-col gap-1.5" data-testid="statuses" data-state={data.state}>
      <h3 className="text-sm font-semibold">
        Commit statuses{" "}
        <Badge
          variant={
            data.state === "success"
              ? "success-light"
              : data.state === "failure"
                ? "destructive-light"
                : "warning-light"
          }
          size="sm"
        >
          {data.state}
        </Badge>
      </h3>
      {data.statuses.length === 0 ? (
        <p className="text-muted-foreground text-sm">No commit statuses reported.</p>
      ) : (
        <ul className="divide-y rounded-sm border">
          {data.statuses.map((s) => {
            const look = runLook({
              status: s.state === "pending" ? "in_progress" : "completed",
              conclusion:
                s.state === "success" ? "success" : s.state === "pending" ? null : "failure",
            });
            return (
              <li key={s.id} className="flex items-center gap-2 px-3 py-2 text-sm">
                <StateIconOf
                  {...look}
                  className={s.state === "pending" ? "animate-none" : undefined}
                />
                <span className="font-medium">{s.context}</span>
                <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
                  {s.description}
                </span>
                {s.required && (
                  <Badge variant="outline" size="sm">
                    Required
                  </Badge>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}
