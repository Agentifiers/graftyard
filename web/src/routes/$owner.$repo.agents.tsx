import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useDebouncedCallback } from "@tanstack/react-pacer/debouncer";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon } from "@hugeicons/core-free-icons";
import { TASK_STATES, type TaskState } from "@/areas/agents/api";
import { ActivityFeed } from "@/areas/agents/activity";
import { AcquireLeaseButton, LeasesPanel } from "@/areas/agents/leases";
import { NewTaskButton } from "@/areas/agents/new-task";
import { agentAppsQuery, leasesQuery, tasksQuery, type TaskFilters } from "@/areas/agents/queries";
import { TASK_STATE } from "@/areas/agents/task-state";
import { TaskGrid } from "@/areas/agents/tasks";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

const REQUESTERS = ["mira", "tomas", "lin", "devi"];

type Search = { state?: TaskState; app?: number; requester?: string; issue?: number };

const positiveInt = (v: unknown) => {
  const n = Number(v);
  return Number.isInteger(n) && n > 0 ? n : undefined;
};

export const Route = createFileRoute("/$owner/$repo/agents")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    state: TASK_STATES.includes(s.state as TaskState) ? (s.state as TaskState) : undefined,
    app: positiveInt(s.app),
    requester: typeof s.requester === "string" && s.requester ? s.requester : undefined,
    issue: positiveInt(s.issue),
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    void context.queryClient.prefetchQuery(tasksQuery(params, filtersOf(deps)));
    void context.queryClient.prefetchQuery(leasesQuery(params));
    void context.queryClient.prefetchQuery(agentAppsQuery());
  },
  component: AgentsPage,
});

const filtersOf = (s: Search): TaskFilters => ({
  state: s.state,
  app_id: s.app,
  requester: s.requester,
  issue_number: s.issue,
});

function AgentsPage() {
  const params = Route.useParams();
  const search = Route.useSearch();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Agents"
        description="Agent tasks, who holds which paths, and what agents are doing now."
        actions={<NewTaskButton {...params} />}
      />
      <Panel title="Tasks" actions={<Filters />} flush>
        <Region name="agent tasks" rows={6}>
          <TaskGrid {...params} filters={filtersOf(search)} />
        </Region>
      </Panel>
      <div className="grid items-start gap-4 lg:grid-cols-2">
        <Panel title="Leases" actions={<AcquireLeaseButton {...params} />} flush>
          <Region name="leases" rows={4}>
            <LeasesPanel {...params} />
          </Region>
        </Panel>
        <Panel title="Activity" flush>
          <Region name="activity">
            <ActivityFeed {...params} />
          </Region>
        </Panel>
      </div>
    </div>
  );
}

function Filters() {
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const apps = useQuery(agentAppsQuery());
  const set = (patch: Partial<Search>) =>
    navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });
  const [issue, setIssue] = useState(search.issue ? String(search.issue) : "");
  const setIssueFilter = useDebouncedCallback((v: string) => set({ issue: positiveInt(v) }), {
    wait: 300,
  });
  const any = Object.values(search).some((v) => v !== undefined);
  return (
    <div className="flex flex-wrap items-center gap-2" data-task-filters>
      <NativeSelect
        size="sm"
        aria-label="State"
        value={search.state ?? ""}
        onChange={(e) => set({ state: (e.target.value || undefined) as TaskState | undefined })}
      >
        <NativeSelectOption value="">Any state</NativeSelectOption>
        {TASK_STATES.map((s) => (
          <NativeSelectOption key={s} value={s}>
            {TASK_STATE[s].label}
          </NativeSelectOption>
        ))}
      </NativeSelect>
      <NativeSelect
        size="sm"
        aria-label="App"
        value={search.app ?? ""}
        onChange={(e) => set({ app: positiveInt(e.target.value) })}
      >
        <NativeSelectOption value="">Any app</NativeSelectOption>
        {(apps.data ?? [])
          .filter((a) => a.kind !== "mcp")
          .map((a) => (
            <NativeSelectOption key={a.app_id} value={a.app_id}>
              {a.slug}
            </NativeSelectOption>
          ))}
      </NativeSelect>
      <NativeSelect
        size="sm"
        aria-label="Requester"
        value={search.requester ?? ""}
        onChange={(e) => set({ requester: e.target.value || undefined })}
      >
        <NativeSelectOption value="">Anyone</NativeSelectOption>
        {REQUESTERS.map((login) => (
          <NativeSelectOption key={login} value={login}>
            {login}
          </NativeSelectOption>
        ))}
      </NativeSelect>
      <Input
        aria-label="Issue number"
        inputMode="numeric"
        placeholder="Issue #"
        className="h-7 w-24"
        value={issue}
        onChange={(e) => {
          setIssue(e.target.value);
          setIssueFilter(e.target.value);
        }}
      />
      {any && (
        <Button
          size="xs"
          variant="ghost"
          onClick={() => {
            setIssue("");
            set({ state: undefined, app: undefined, requester: undefined, issue: undefined });
          }}
        >
          <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} data-icon="inline-start" />
          Clear
        </Button>
      )}
    </div>
  );
}
