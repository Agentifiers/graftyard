import { useState, type ComponentType } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useQueries, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Robot01Icon } from "@hugeicons/core-free-icons";
import { userQuery, userReposQuery } from "@/areas/core/queries";
import { checkRunsQuery } from "@/areas/pulls/queries";
import type { SlotEntry, SlotName, SlotProps } from "@/areas/slots";
import { Panel } from "@/components/panel";
import { Button } from "@/components/ui/button";
import { GraftyardError, type Task } from "./api";
import { VerdictBadge } from "./activity";
import { PolicyRefusal, useCreateTask } from "./new-task";
import { agentAppsQuery, agentPullQuery, tasksQuery } from "./queries";
import { FINAL_STATES, shortId } from "./states";
import { TaskStateBadge, TaskStateIcon } from "./task-state";
import { TaskList } from "./tasks";
import { AuthorBadges, HiddenDisclosure } from "./trust-view";

type Repo = { owner: string; repo: string };

// The agent session behind a pull request, and the opt-in trust view of its author.
function PullAgentCard({
  owner,
  repo,
  number,
  headSha,
}: Repo & { number: number; headSha: string }) {
  const { data: pr } = useSuspenseQuery(agentPullQuery({ owner, repo, number }));
  const { data: apps = [] } = useQuery(agentAppsQuery());
  const checks = useQuery(checkRunsQuery({ owner, repo, ref: headSha }));
  const session = pr.graftyard_agent?.session ?? null;
  const task = useQuery({
    ...tasksQuery({ owner, repo }),
    enabled: Boolean(session),
    select: (ts) => ts.find((t) => t.id === session),
  });
  const insight = (checks.data?.check_runs ?? []).filter((c) => c.name.startsWith("graftyard/"));
  const app = apps.find((a) => a.app_id === pr.graftyard_agent?.app_id);
  const overlaps = pr.graftyard_overlaps.with;
  return (
    <Panel
      title={
        <span className="flex items-center gap-1.5">
          <HugeiconsIcon icon={Robot01Icon} strokeWidth={2} className="size-4" />
          {pr.graftyard_agent ? "Agent session" : "Author trust"}
        </span>
      }
    >
      <div className="flex flex-col gap-3 text-sm" data-agent-card={session ?? "none"}>
        {pr.graftyard_agent && (
          <div className="flex flex-col gap-1.5">
            <div className="flex flex-wrap items-center gap-2">
              {task.data && <TaskStateBadge state={task.data.state} />}
              <span className="font-mono text-xs">
                {app?.slug ?? `App ${pr.graftyard_agent.app_id}`}
              </span>
            </div>
            {pr.graftyard_agent.requester && (
              <span className="text-muted-foreground text-xs">
                Requested by {pr.graftyard_agent.requester.login}
              </span>
            )}
            {session && (
              <Link
                to="/$owner/$repo/_graftyard/sessions/$uuid"
                params={{ owner, repo, uuid: session }}
                className="text-primary text-xs font-medium hover:underline"
                data-session-link={session}
              >
                View session log
              </Link>
            )}
          </div>
        )}
        {(insight.length > 0 || overlaps.length > 0) && (
          <div className="flex flex-col gap-1.5">
            <h3 className="text-muted-foreground text-xs font-medium">Insight checks</h3>
            {insight.map((c) => (
              <div key={c.id} className="flex flex-col gap-0.5" data-insight={c.name}>
                <span className="flex items-center gap-1.5 text-xs">
                  <code className="font-mono">{c.name}</code>
                  <span className="text-muted-foreground">{c.conclusion}</span>
                </span>
                {/* Paths and trailer values in check output are untrusted text. */}
                <p className="text-muted-foreground text-xs break-words">{c.output.summary}</p>
              </div>
            ))}
            {overlaps.map((o) => (
              <span key={o.ref} className="flex flex-wrap items-center gap-1.5 text-xs">
                <VerdictBadge verdict={o.verdict} />
                with <code className="font-mono">{o.ref.replace("refs/heads/", "")}</code>
                {o.pr_number && <>#{o.pr_number}</>}
              </span>
            ))}
          </div>
        )}
        <div className="flex flex-col gap-1.5">
          <h3 className="text-muted-foreground text-xs font-medium">Description author</h3>
          <AuthorBadges author={pr.graftyard_trust.author} />
          <HiddenDisclosure view={pr.graftyard_trust} />
        </div>
      </div>
    </Panel>
  );
}

function AssignToAgent({ owner, repo, number }: Repo & { number: number }) {
  const { data: tasks } = useSuspenseQuery(tasksQuery({ owner, repo }, { issue_number: number }));
  const create = useCreateTask({ owner, repo });
  const navigate = useNavigate();
  const [refusal, setRefusal] = useState<GraftyardError>();
  const open = tasks.filter((t) => !FINAL_STATES.has(t.state));
  return (
    <Panel title="Agents">
      <div className="flex flex-col gap-2 text-sm">
        <Button
          size="sm"
          variant="outline"
          disabled={create.isPending}
          data-assign-agent
          onClick={() => {
            setRefusal(undefined);
            create.mutate(
              { issue_number: number },
              {
                onSuccess: (t) => {
                  toast.success("Assigned to graftyard-agent", {
                    description: `Task ${shortId(t.id)} is queued.`,
                    action: {
                      label: "Open",
                      onClick: () =>
                        navigate({
                          to: "/$owner/$repo/_graftyard/sessions/$uuid",
                          params: { owner, repo, uuid: t.id },
                        }),
                    },
                  });
                },
                onError: (e) => {
                  if (e instanceof GraftyardError && (e.status === 422 || e.status === 403))
                    setRefusal(e);
                  else toast.error("Not assigned", { description: e.message });
                },
              },
            );
          }}
        >
          <HugeiconsIcon icon={Robot01Icon} strokeWidth={2} data-icon="inline-start" />
          {create.isPending ? "Assigning…" : "Assign to agent"}
        </Button>
        {refusal && <PolicyRefusal error={refusal} owner={owner} />}
        {open.length ? (
          <TaskList tasks={open} owner={owner} repo={repo} />
        ) : (
          <p className="text-muted-foreground text-xs">No open agent tasks for this issue.</p>
        )}
        {tasks.length > open.length && (
          <Link
            to="/$owner/$repo/agents"
            params={{ owner, repo }}
            search={{ issue: number }}
            className="text-muted-foreground text-xs hover:underline"
          >
            {tasks.length - open.length} finished task{tasks.length - open.length === 1 ? "" : "s"}
          </Link>
        )}
      </div>
    </Panel>
  );
}

// The viewer's tasks across every repository they can see.
function YourTasks() {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: repos } = useSuspenseQuery(userReposQuery());
  const results = useQueries({
    queries: repos.map((r) =>
      tasksQuery({ owner: r.owner.login, repo: r.name }, { requester: viewer.login }),
    ),
  });
  const failed = results.find((q) => q.isError);
  if (failed) throw failed.error;
  if (results.some((q) => q.isPending))
    return <p className="text-muted-foreground p-3 text-xs">Loading tasks…</p>;
  const rows: { repo: Repo & { full: string }; task: Task }[] = results
    .flatMap((q, i) =>
      (q.data ?? []).map((task) => ({
        repo: { owner: repos[i].owner.login, repo: repos[i].name, full: repos[i].full_name },
        task,
      })),
    )
    .sort((a, b) => b.task.updated_at.localeCompare(a.task.updated_at))
    .slice(0, 8);
  return (
    <Panel title="Your agent tasks" flush>
      {rows.length ? (
        <ul className="divide-y" data-testid="your-tasks">
          {rows.map(({ repo, task }) => (
            <li key={task.id} className="flex items-start gap-2 px-3 py-2">
              <TaskStateIcon state={task.state} className="mt-0.5" />
              <div className="flex min-w-0 flex-col gap-0.5">
                <Link
                  to="/$owner/$repo/_graftyard/sessions/$uuid"
                  params={{ owner: repo.owner, repo: repo.repo, uuid: task.id }}
                  className="truncate text-xs font-medium hover:underline"
                >
                  {repo.full}
                  {task.issue_number !== null ? ` #${task.issue_number}` : ""}
                </Link>
                <span className="text-muted-foreground line-clamp-2 text-xs">{task.summary}</span>
              </div>
            </li>
          ))}
        </ul>
      ) : (
        <p className="text-muted-foreground p-3 text-xs">
          You have no agent tasks. Assign one from any issue.
        </p>
      )}
    </Panel>
  );
}

// src/areas/slots.tsx imports this file eagerly, so its `slot` helper is not yet
// initialized when this module runs. This is the same identity cast with type-only imports.
const slot = <K extends SlotName>(entry: {
  slot: K;
  region: string;
  order: number;
  Component: ComponentType<SlotProps[K]>;
}) => entry as SlotEntry;

export const slots: SlotEntry[] = [
  slot({ slot: "pull-sidebar", region: "agent session", order: 30, Component: PullAgentCard }),
  slot({
    slot: "issue-sidebar",
    region: "agent tasks for this issue",
    order: 30,
    Component: AssignToAgent,
  }),
  slot({ slot: "home", region: "your agent tasks", order: 20, Component: YourTasks }),
];
