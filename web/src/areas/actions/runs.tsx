import type { ReactNode } from "react";
import { Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { useDebouncedCallback } from "@tanstack/react-pacer";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowLeft01Icon,
  ArrowRight01Icon,
  Calendar03Icon,
  Cancel01Icon,
  GitBranchIcon,
  Search01Icon,
  Timer01Icon,
} from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Frame, FrameHeader, FramePanel } from "@/components/reui/frame";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import {
  branchesQuery,
  runsQuery,
  RUNS_PER_PAGE,
  workflowsQuery,
  type RunFilters,
} from "./queries";
import {
  Duration,
  EVENT_LABELS,
  STATUS_FILTERS,
  StatusIcon,
  isStatusFilter,
  lookLabel,
} from "./status";

type Repo = { owner: string; repo: string };

export type RunSearch = Omit<RunFilters, "workflow">;

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);

// The typed search params of both runs lists.
export function validateRunSearch(search: Record<string, unknown>): RunSearch {
  const page = Number(search.page);
  return {
    branch: str(search.branch),
    event: str(search.event),
    status: isStatusFilter(search.status) ? search.status : undefined,
    actor: str(search.actor),
    page: Number.isInteger(page) && page > 1 ? page : undefined,
  };
}

// The Actions page frame: the workflow list on the left, the page on the right.
export function ActionsShell({
  active,
  children,
  ...repo
}: Repo & { active?: number; children: ReactNode }) {
  return (
    <div className="grid items-start gap-6 md:grid-cols-[14rem_minmax(0,1fr)]">
      <nav aria-label="Workflows" className="flex flex-col gap-1 text-sm md:sticky md:top-4">
        <h2 className="text-muted-foreground px-2 pb-1 text-xs font-medium">Actions</h2>
        <Region name="workflows" rows={4}>
          <WorkflowList active={active} {...repo} />
        </Region>
      </nav>
      <div className="min-w-0">{children}</div>
    </div>
  );
}

const navItem =
  "text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 font-medium";

function WorkflowList({ active, ...repo }: Repo & { active?: number }) {
  const { data } = useSuspenseQuery(workflowsQuery(repo));
  return (
    <ul className="flex flex-col gap-0.5" data-testid="workflow-list">
      <li>
        <Link
          to="/$owner/$repo/actions"
          params={repo}
          data-active={active === undefined || undefined}
          className={navItem}
        >
          All workflows
        </Link>
      </li>
      {data.workflows.map((w) => (
        <li key={w.id}>
          <Link
            to="/$owner/$repo/actions/workflows/$workflowId"
            params={{ ...repo, workflowId: w.id }}
            data-active={active === w.id || undefined}
            className={navItem}
          >
            <span className="truncate">{w.name}</span>
            {w.state !== "active" && (
              <Badge variant="outline" size="sm" className="ml-auto">
                Disabled
              </Badge>
            )}
          </Link>
        </li>
      ))}
    </ul>
  );
}

const EVENTS = ["push", "pull_request", "workflow_dispatch", "schedule"];

// The filter bar and the runs, one region each. `setSearch` merges into the route's
// search params and resets the page.
export function RunsPanel({
  workflow,
  search,
  setSearch,
  ...repo
}: Repo & {
  workflow?: Schemas["workflow"];
  search: RunSearch;
  setSearch: (patch: Partial<RunSearch>) => void;
}) {
  const filtered = Boolean(search.branch || search.event || search.status || search.actor);
  const setActor = useDebouncedCallback(
    (actor: string) => setSearch({ actor: actor.trim() || undefined }),
    {
      wait: 300,
    },
  );
  return (
    <Frame spacing="sm" dense>
      <FrameHeader className="flex-row flex-wrap items-center gap-2 py-2">
        <Region name="branches" fallback={<NativeSelect size="sm" disabled aria-label="Branch" />}>
          <BranchFilter
            value={search.branch}
            onChange={(branch) => setSearch({ branch })}
            {...repo}
          />
        </Region>
        <NativeSelect
          size="sm"
          aria-label="Event"
          value={search.event ?? ""}
          onChange={(e) => setSearch({ event: e.target.value || undefined })}
        >
          <NativeSelectOption value="">Any event</NativeSelectOption>
          {EVENTS.map((e) => (
            <NativeSelectOption key={e} value={e}>
              {e}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        <NativeSelect
          size="sm"
          aria-label="Status"
          value={search.status ?? ""}
          onChange={(e) =>
            setSearch({ status: isStatusFilter(e.target.value) ? e.target.value : undefined })
          }
        >
          <NativeSelectOption value="">Any status</NativeSelectOption>
          {STATUS_FILTERS.map((s) => (
            <NativeSelectOption key={s} value={s}>
              {s === "completed" ? "Completed" : lookLabel(s)}
            </NativeSelectOption>
          ))}
        </NativeSelect>
        <label className="relative w-40">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground absolute top-1/2 left-2 size-3.5 -translate-y-1/2"
          />
          <Input
            key={search.actor ?? ""}
            type="search"
            aria-label="Actor"
            placeholder="Actor"
            className="h-7 rounded-sm pl-7 text-xs"
            defaultValue={search.actor}
            onChange={(e) => setActor(e.target.value)}
          />
        </label>
        {filtered && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              setSearch({
                branch: undefined,
                event: undefined,
                status: undefined,
                actor: undefined,
              })
            }
          >
            <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} data-icon="inline-start" />
            Clear filters
          </Button>
        )}
      </FrameHeader>
      <FramePanel className="rounded-t-none p-0">
        <Region name="workflow runs" rows={8}>
          <RunList workflow={workflow?.id} search={search} setSearch={setSearch} {...repo} />
        </Region>
      </FramePanel>
    </Frame>
  );
}

function BranchFilter({
  value,
  onChange,
  ...repo
}: Repo & { value?: string; onChange: (b?: string) => void }) {
  const { data } = useSuspenseQuery(branchesQuery(repo));
  return (
    <NativeSelect
      size="sm"
      aria-label="Branch"
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || undefined)}
    >
      <NativeSelectOption value="">Any branch</NativeSelectOption>
      {data.map((b) => (
        <NativeSelectOption key={b.name} value={b.name}>
          {b.name}
        </NativeSelectOption>
      ))}
    </NativeSelect>
  );
}

function RunList({
  workflow,
  search,
  setSearch,
  ...repo
}: Repo & {
  workflow?: number;
  search: RunSearch;
  setSearch: (patch: Partial<RunSearch>) => void;
}) {
  const { data } = useSuspenseQuery(runsQuery({ ...repo, ...search, workflow }));
  const page = search.page ?? 1;
  const pages = Math.max(1, Math.ceil(data.total_count / RUNS_PER_PAGE));
  if (data.total_count === 0) {
    return (
      <div
        className="text-muted-foreground px-4 py-10 text-center text-sm"
        data-testid="runs-empty"
      >
        {search.branch || search.event || search.status || search.actor
          ? "No workflow runs match these filters."
          : "This workflow has no runs yet."}
      </div>
    );
  }
  return (
    <div>
      <div
        className="text-muted-foreground border-b px-4 py-2 text-xs font-medium"
        data-testid="runs-count"
      >
        {data.total_count} workflow run{data.total_count === 1 ? "" : "s"}
      </div>
      <ul className="divide-y" data-testid="run-list">
        {data.workflow_runs.map((run) => (
          <RunRow key={run.id} run={run} {...repo} />
        ))}
      </ul>
      {pages > 1 && (
        <div className="flex items-center justify-between border-t px-4 py-2 text-xs">
          <Button
            variant="outline"
            size="sm"
            disabled={page <= 1}
            onClick={() => setSearch({ page: page - 1 > 1 ? page - 1 : undefined })}
          >
            <HugeiconsIcon icon={ArrowLeft01Icon} strokeWidth={2} data-icon="inline-start" />
            Previous
          </Button>
          <span className="text-muted-foreground tabular-nums" data-testid="runs-page">
            Page {page} of {pages}
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={page >= pages}
            onClick={() => setSearch({ page: page + 1 })}
          >
            Next
            <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        </div>
      )}
    </div>
  );
}

function trigger(run: Schemas["workflow-run"]) {
  const pr = run.pull_requests?.[0];
  if (run.event === "pull_request" && pr)
    return `Pull request #${pr.number} synchronized by ${run.actor?.login}`;
  if (run.event === "push")
    return `Commit ${run.head_sha.slice(0, 7)} pushed by ${run.actor?.login}`;
  if (run.event === "schedule") return "Scheduled";
  if (run.event === "workflow_dispatch") return `Manually run by ${run.actor?.login}`;
  return `${run.event} by ${run.actor?.login}`;
}

function RunRow({ run, ...repo }: Repo & { run: Schemas["workflow-run"] }) {
  const running = run.status !== "completed";
  return (
    <li className="hover:bg-muted/40 flex items-start gap-3 px-4 py-2.5" data-run={run.id}>
      <StatusIcon item={run} className="mt-0.5" />
      <div className="min-w-0 flex-1">
        <Link
          to="/$owner/$repo/actions/runs/$runId"
          params={{ ...repo, runId: run.id }}
          className="text-sm font-semibold hover:underline"
        >
          {run.display_title}
        </Link>
        <p className="text-muted-foreground mt-0.5 text-xs">
          <span className="text-foreground font-medium">{run.name}</span> #{run.run_number}
          {(run.run_attempt ?? 1) > 1 && <> (attempt {run.run_attempt})</>}: {trigger(run)}
        </p>
      </div>
      <div className="hidden shrink-0 flex-col items-end gap-1 sm:flex">
        {run.head_branch && (
          <Badge variant="info-light" className="max-w-48 font-mono">
            <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} />
            <span className="truncate">{run.head_branch}</span>
          </Badge>
        )}
        <Badge variant="outline" size="sm" data-event={run.event}>
          {EVENT_LABELS[run.event] ?? run.event}
        </Badge>
      </div>
      <div className="text-muted-foreground flex w-28 shrink-0 flex-col items-end gap-0.5 text-xs">
        <span className="flex items-center gap-1">
          <HugeiconsIcon icon={Calendar03Icon} strokeWidth={2} className="size-3.5" />
          <Ago at={run.created_at} />
        </span>
        <span className="flex items-center gap-1" title="Duration">
          <HugeiconsIcon icon={Timer01Icon} strokeWidth={2} className="size-3.5" />
          <Duration
            start={run.run_started_at}
            end={running ? null : run.updated_at}
            running={running}
          />
        </span>
      </div>
    </li>
  );
}
