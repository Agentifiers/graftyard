import { createFileRoute } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Alert02Icon, File01Icon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { RunWorkflow } from "@/areas/actions/dispatch";
import { runsQuery, workflowSourceQuery, workflowsQuery } from "@/areas/actions/queries";
import { ActionsShell, RunsPanel, validateRunSearch, type RunSearch } from "@/areas/actions/runs";
import { send, useWrite } from "@/areas/actions/write";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { CodeBlock } from "@/components/reui/code-block/code-block";
import { Button } from "@/components/ui/button";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Skeleton } from "@/components/ui/skeleton";

export const Route = createFileRoute("/$owner/$repo/actions/workflows/$workflowId")({
  params: {
    parse: ({ workflowId }) => ({ workflowId: Number(workflowId) }),
    stringify: ({ workflowId }) => ({ workflowId: String(workflowId) }),
  },
  validateSearch: validateRunSearch,
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    const { owner, repo, workflowId } = params;
    void context.queryClient.prefetchQuery(workflowsQuery({ owner, repo }));
    void context.queryClient.prefetchQuery(
      runsQuery({ owner, repo, workflow: workflowId, ...deps }),
    );
  },
  component: WorkflowPage,
});

function WorkflowPage() {
  const { owner, repo, workflowId } = Route.useParams();
  return (
    <ActionsShell owner={owner} repo={repo} active={workflowId}>
      <Region name="workflow" fallback={<Skeleton className="mb-4 h-12" />}>
        <WorkflowBody />
      </Region>
    </ActionsShell>
  );
}

function WorkflowBody() {
  const { owner, repo, workflowId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data } = useSuspenseQuery(workflowsQuery({ owner, repo }));
  const workflow = data.workflows.find((w) => w.id === workflowId);
  if (!workflow) {
    return (
      <Alert variant="destructive">
        <AlertTitle>No such workflow</AlertTitle>
        <AlertDescription>
          There is no workflow {workflowId} in {owner}/{repo}.
        </AlertDescription>
      </Alert>
    );
  }
  const setSearch = (patch: Partial<RunSearch>) =>
    navigate({ search: (prev) => ({ ...prev, page: undefined, ...patch }), replace: true });
  const active = workflow.state === "active";
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-start gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2 text-xl font-semibold">
            {workflow.name}
            {!active && (
              <Badge variant="outline" data-testid="workflow-state">
                Disabled
              </Badge>
            )}
          </h1>
          <p className="text-muted-foreground mt-1 flex items-center gap-1 font-mono text-xs">
            <HugeiconsIcon icon={File01Icon} strokeWidth={2} className="size-3.5" />
            {workflow.path}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ToggleWorkflow workflow={workflow} owner={owner} repo={repo} />
          <RunWorkflow workflow={workflow} owner={owner} repo={repo} />
        </div>
      </div>
      {!active && (
        <Alert variant="warning" data-testid="disabled-note">
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} />
          <AlertTitle>This workflow is disabled</AlertTitle>
          <AlertDescription>
            Its triggers do nothing until you enable it. Past runs stay listed.
          </AlertDescription>
        </Alert>
      )}
      <Region name="workflow file" rows={2}>
        <WorkflowSource workflowId={workflow.id} />
      </Region>
      <RunsPanel
        workflow={workflow}
        search={search}
        setSearch={setSearch}
        owner={owner}
        repo={repo}
      />
    </div>
  );
}

function ToggleWorkflow({
  workflow,
  owner,
  repo,
}: {
  workflow: Schemas["workflow"];
  owner: string;
  repo: string;
}) {
  const queryClient = useQueryClient();
  const key = workflowsQuery({ owner, repo }).queryKey;
  const enable = workflow.state !== "active";
  const toggle = useWrite({
    run: (on: boolean) =>
      send(
        on
          ? api.PUT("/repos/{owner}/{repo}/actions/workflows/{workflow_id}/enable", {
              params: { path: { owner, repo, workflow_id: workflow.id } },
            })
          : api.PUT("/repos/{owner}/{repo}/actions/workflows/{workflow_id}/disable", {
              params: { path: { owner, repo, workflow_id: workflow.id } },
            }),
      ),
    invalidates: () => [key],
    optimistic: (on) => {
      const before = queryClient.getQueryData(key);
      queryClient.setQueryData(
        key,
        (d) =>
          d && {
            ...d,
            workflows: d.workflows.map((w) =>
              w.id === workflow.id
                ? { ...w, state: on ? ("active" as const) : ("disabled_manually" as const) }
                : w,
            ),
          },
      );
      return () => queryClient.setQueryData(key, before);
    },
    success: (on) => `${workflow.name} ${on ? "enabled" : "disabled"}`,
    failure: (on) => `Could not ${on ? "enable" : "disable"} ${workflow.name}`,
  });
  return (
    <Button
      variant="outline"
      size="sm"
      onClick={() => toggle.mutate(enable)}
      disabled={toggle.isPending}
    >
      {enable ? "Enable workflow" : "Disable workflow"}
    </Button>
  );
}

function WorkflowSource({ workflowId }: { workflowId: number }) {
  const { owner, repo } = Route.useParams();
  const { data } = useSuspenseQuery(workflowSourceQuery({ owner, repo, workflowId }));
  return (
    <Collapsible>
      <Panel
        flush
        title={
          <span className="flex flex-wrap items-center gap-2">
            Triggers
            {data.events.map((e) => (
              <Badge key={e} variant="outline" size="sm" className="font-mono">
                {e}
              </Badge>
            ))}
          </span>
        }
        actions={
          <CollapsibleTrigger render={<Button variant="ghost" size="sm" />}>
            View workflow file
          </CollapsibleTrigger>
        }
      >
        <CollapsibleContent>
          <CodeBlock
            code={data.source}
            language="yaml"
            showLineNumbers
            variant="ghost"
            label={data.path}
          />
        </CollapsibleContent>
      </Panel>
    </Collapsible>
  );
}
