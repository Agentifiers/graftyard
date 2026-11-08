import { useEffect, useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AiBrain01Icon,
  ArrowRight01Icon,
  CloudUploadIcon,
  CodeIcon,
  GitCommitIcon,
  GitPullRequestIcon,
  TestTube01Icon,
  Wrench01Icon,
} from "@hugeicons/core-free-icons";
import { GraftyardError, type Step, type StepKind } from "@/areas/agents/api";
import { agentAppsQuery, logQuery, taskQuery } from "@/areas/agents/queries";
import { TaskStateBadge } from "@/areas/agents/task-state";
import { CancelTaskButton } from "@/areas/agents/tasks";
import { FINAL_STATES } from "@/areas/agents/states";
import { TrustViewCard } from "@/areas/agents/trust-view";
import { Ago, UserAvatar } from "@/components/github";
import { MarkdownBody } from "@/components/markdown";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { CodeBlock, CodeBlockExpandButton } from "@/components/reui/code-block/code-block";
import {
  Timeline,
  TimelineContent,
  TimelineDate,
  TimelineHeader,
  TimelineIndicator,
  TimelineItem,
  TimelineSeparator,
  TimelineTitle,
} from "@/components/reui/timeline";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";

export const Route = createFileRoute("/$owner/$repo/_graftyard/sessions/$uuid")({
  loader: ({ context, params }) => {
    const r = { owner: params.owner, repo: params.repo, id: params.uuid };
    void context.queryClient.prefetchQuery(taskQuery(r));
    void context.queryClient.prefetchQuery(logQuery(r));
  },
  component: SessionPage,
});

const useTaskParams = () => {
  const { owner, repo, uuid } = Route.useParams();
  return { owner, repo, id: uuid };
};

function SessionPage() {
  return (
    <div className="flex flex-col gap-4">
      <Region name="task" fallback={<Skeleton className="h-16" />}>
        <TaskHeader />
      </Region>
      <Region name="session log" rows={8}>
        <SessionLogView />
      </Region>
    </div>
  );
}

function TaskHeader() {
  const r = useTaskParams();
  const { data: task } = useSuspenseQuery(taskQuery(r));
  const apps = useQuery(agentAppsQuery());
  const app = apps.data?.find((a) => a.app_id === task.app_id);
  return (
    <PageHeader
      title={
        <span className="flex flex-wrap items-center gap-2">
          Agent session{" "}
          <code className="text-muted-foreground font-mono text-base font-normal">{task.id}</code>
        </span>
      }
      description={
        <span className="flex flex-wrap items-center gap-2" data-task-header>
          <TaskStateBadge state={task.state} size="lg" />
          <span className="font-mono text-xs">{app?.slug ?? `App ${task.app_id}`}</span>
          {task.requester && (
            <span className="flex items-center gap-1">
              <UserAvatar user={task.requester} /> requested by {task.requester.login}
            </span>
          )}
          {task.issue_number !== null && (
            <Link
              to="/$owner/$repo/issues/$number"
              params={{ owner: r.owner, repo: r.repo, number: task.issue_number }}
              className="hover:underline"
            >
              for #{task.issue_number}
            </Link>
          )}
          <span>
            started <Ago at={task.created_at} />, updated <Ago at={task.updated_at} />
          </span>
          {task.prefix && (
            <code className="bg-muted rounded-sm px-1 font-mono text-xs">{task.prefix}</code>
          )}
          <span className="text-foreground w-full">{task.summary}</span>
          {(task.leases?.length ?? 0) > 0 && (
            <span className="w-full text-xs">
              Holds {task.leases!.length} lease{task.leases!.length === 1 ? "" : "s"}:{" "}
              {task.leases!.flatMap((l) => l.paths).join(", ")}
            </span>
          )}
        </span>
      }
      actions={<CancelTaskButton task={task} owner={r.owner} repo={r.repo} />}
    />
  );
}

function SessionLogView() {
  const r = useTaskParams();
  // The log is pushed when the run ends, so a 404 is retried while the task is live.
  const task = useQuery(taskQuery(r));
  const live = task.data !== undefined && !FINAL_STATES.has(task.data.state);
  const log = useQuery({
    ...logQuery(r),
    refetchInterval: (q) => (q.state.data || !live ? false : 4_000),
  });
  const finished = task.data?.state;
  useEffect(() => {
    if (finished && FINAL_STATES.has(finished) && log.isError) void log.refetch();
  }, [finished]);
  const [json, setJson] = useState(false);
  if (log.isPending) return <Skeleton className="h-64" />;
  if (log.isError) {
    if (log.error instanceof GraftyardError && log.error.status === 404) {
      return (
        <Empty className="border" data-log-missing>
          <EmptyHeader>
            <EmptyTitle>The log has not been pushed yet.</EmptyTitle>
            <EmptyDescription>
              graftyard-workspace pushes the session log when the run ends, after secret scanning
              and redaction. Check back once the task finishes.
            </EmptyDescription>
          </EmptyHeader>
        </Empty>
      );
    }
    throw log.error;
  }
  const data = log.data;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center justify-end gap-2 text-sm">
        <label className="flex items-center gap-2">
          <Switch
            checked={json}
            onCheckedChange={setJson}
            aria-label="Show raw JSON"
            data-json-toggle
          />
          Raw JSON (schema {data.schema})
        </label>
      </div>
      {json ? (
        <CodeBlock
          code={JSON.stringify(data, null, 2)}
          language="json"
          showLineNumbers
          maxLines={60}
          data-raw-json
        >
          <CodeBlockExpandButton />
        </CodeBlock>
      ) : (
        <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_20rem]">
          <div className="flex min-w-0 flex-col gap-4">
            <Panel title="Plan">
              <MarkdownBody text={data.plan} />
            </Panel>
            <Panel title={`Steps (${data.steps.length})`}>
              <Steps steps={data.steps} />
            </Panel>
          </div>
          <aside className="flex flex-col gap-4">
            <Panel title="Harness">
              <dl className="grid grid-cols-[5rem_1fr] gap-y-1 text-sm" data-harness>
                <dt className="text-muted-foreground">Name</dt>
                <dd className="font-mono">{data.harness.name}</dd>
                <dt className="text-muted-foreground">Version</dt>
                <dd className="font-mono">{data.harness.version}</dd>
                <dt className="text-muted-foreground">Model</dt>
                <dd className="font-mono">{data.harness.model}</dd>
              </dl>
            </Panel>
            <Panel title="Issue as the agent saw it">
              <TrustViewCard view={data.issue_snapshot} />
            </Panel>
            <Panel title="Output">
              <div className="flex flex-col gap-2 text-sm">
                <h3 className="text-muted-foreground text-xs font-medium">Pushes</h3>
                {data.pushes.length ? (
                  <ul className="flex flex-col gap-1">
                    {data.pushes.map((sha) => (
                      <li key={sha} className="flex items-center gap-1.5">
                        <HugeiconsIcon
                          icon={GitCommitIcon}
                          strokeWidth={2}
                          className="text-muted-foreground size-3.5"
                        />
                        <Link
                          to="/$owner/$repo/commit/$sha"
                          params={{ owner: r.owner, repo: r.repo, sha }}
                          className="font-mono text-xs hover:underline"
                          data-push={sha}
                        >
                          {sha.slice(0, 7)}
                        </Link>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="text-muted-foreground text-xs">No pushes.</p>
                )}
                <h3 className="text-muted-foreground mt-2 text-xs font-medium">Pull requests</h3>
                {data.prs.length ? (
                  <ul className="flex flex-col gap-1">
                    {data.prs.map((n) => (
                      <li key={n} className="flex items-center gap-1.5">
                        <HugeiconsIcon
                          icon={GitPullRequestIcon}
                          strokeWidth={2}
                          className="text-muted-foreground size-3.5"
                        />
                        <Link
                          to="/$owner/$repo/pull/$number"
                          params={{ owner: r.owner, repo: r.repo, number: n }}
                          className="text-primary text-xs hover:underline"
                          data-pr={n}
                        >
                          #{n}
                        </Link>
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="text-muted-foreground text-xs">No pull requests.</p>
                )}
              </div>
            </Panel>
          </aside>
        </div>
      )}
    </div>
  );
}

const KIND: Record<StepKind, { label: string; icon: typeof CodeIcon; tone: string }> = {
  model: { label: "Model", icon: AiBrain01Icon, tone: "text-info" },
  tool: { label: "Tool", icon: Wrench01Icon, tone: "text-muted-foreground" },
  test: { label: "Test", icon: TestTube01Icon, tone: "text-warning" },
  push: { label: "Push", icon: CloudUploadIcon, tone: "text-success" },
};

function failed(step: Step) {
  return step.kind === "test" && /FAILED|test failed|error\[/.test(step.output);
}

const language = (step: Step, text: string) =>
  text.trimStart().startsWith("{")
    ? "json"
    : text.startsWith("---") || text.startsWith("@@")
      ? "diff"
      : step.tool === "read_file"
        ? "rust"
        : step.kind === "test" || step.tool === "bash" || step.tool === "git"
          ? "shell"
          : undefined;

function Steps({ steps }: { steps: Step[] }) {
  return (
    <Timeline value={0} data-testid="steps">
      {steps.map((step, i) => {
        const k = KIND[step.kind];
        return (
          <TimelineItem
            key={i}
            step={i + 1}
            className="group-data-[orientation=vertical]/timeline:ms-10"
            data-step={step.kind}
          >
            <TimelineHeader>
              <TimelineSeparator className="bg-border group-data-[orientation=vertical]/timeline:-left-7 group-data-[orientation=vertical]/timeline:h-[calc(100%-1.5rem)] group-data-[orientation=vertical]/timeline:translate-y-6" />
              <TimelineIndicator className="bg-card flex size-6 items-center justify-center border group-data-[orientation=vertical]/timeline:-left-7">
                <HugeiconsIcon
                  icon={k.icon}
                  strokeWidth={2}
                  className={`size-3.5 ${k.tone}`}
                  aria-label={k.label}
                />
              </TimelineIndicator>
              <TimelineDate dateTime={step.at}>
                {new Date(step.at).toLocaleTimeString()}
              </TimelineDate>
              <TimelineTitle className="flex flex-wrap items-center gap-1.5">
                {k.label}
                {step.tool && (
                  <code className="bg-muted rounded-sm px-1 font-mono text-xs font-normal">
                    {step.tool}
                  </code>
                )}
                {failed(step) && (
                  <Badge variant="destructive-light" size="sm" className="dark:text-red-300">
                    Failed
                  </Badge>
                )}
                {step.truncated && (
                  <Badge variant="warning-light" size="sm" data-truncated>
                    Truncated
                  </Badge>
                )}
              </TimelineTitle>
            </TimelineHeader>
            <TimelineContent className="text-foreground mt-1 flex flex-col gap-1">
              <StepBlock
                title="Input"
                text={step.input}
                lang={language(step, step.input)}
                open={step.kind === "model"}
              />
              <StepBlock
                title={step.truncated ? "Output (cut at 64 KiB)" : "Output"}
                text={step.output}
                lang={language(step, step.output)}
                open={failed(step)}
              />
            </TimelineContent>
          </TimelineItem>
        );
      })}
    </Timeline>
  );
}

function StepBlock({
  title,
  text,
  lang,
  open,
}: {
  title: string;
  text: string;
  lang?: string;
  open?: boolean;
}) {
  const lines = text.split("\n").length;
  return (
    <Collapsible defaultOpen={open}>
      <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex items-center gap-1 text-xs font-medium">
        <HugeiconsIcon
          icon={ArrowRight01Icon}
          strokeWidth={2}
          className="size-3.5 transition-transform group-data-[panel-open]:rotate-90"
        />
        {title}
        <span className="font-normal">
          ({lines} line{lines === 1 ? "" : "s"})
        </span>
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-1">
        <CodeBlock code={text} language={lang} highlight={Boolean(lang)} wrap maxLines={24}>
          <CodeBlockExpandButton />
        </CodeBlock>
      </CollapsibleContent>
    </Collapsible>
  );
}
