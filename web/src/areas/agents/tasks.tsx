import { useMemo, useState } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createColumnHelper, useTable } from "@tanstack/react-table";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { GitPullRequestIcon, MinusSignCircleIcon, RecordIcon } from "@hugeicons/core-free-icons";
import { Ago, UserAvatar } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
} from "@/components/reui/data-grid/data-grid";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { call, GraftyardError, gy, type Task } from "./api";
import { agentAppsQuery, tasksKey, tasksQuery, type TaskFilters } from "./queries";
import { TaskStateBadge, TaskStateIcon } from "./task-state";
import { FINAL_STATES, shortId } from "./states";

type Repo = { owner: string; repo: string };

// Cancels a task. The row reads Cancelled at once and rolls back on failure. A final task
// answers 409 with its recorded or derived state, which the toast names.
export function useCancelTask(r: Repo) {
  const client = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      call(gy.POST("/repos/{owner}/{repo}/tasks/{id}/cancel", { params: { path: { ...r, id } } })),
    onMutate: async (id) => {
      await client.cancelQueries({ queryKey: tasksKey(r) });
      const before = client.getQueriesData<Task[] | Task>({ queryKey: tasksKey(r) });
      const cancelled = (t: Task) => (t.id === id ? { ...t, state: "cancelled" as const } : t);
      client.setQueriesData<Task[] | Task>({ queryKey: tasksKey(r) }, (old) =>
        Array.isArray(old) ? old.map(cancelled) : old && "id" in old ? cancelled(old) : old,
      );
      return { before };
    },
    onError: (e, _id, ctx) => {
      for (const [key, data] of ctx?.before ?? []) client.setQueryData(key, data);
      if (e instanceof GraftyardError && e.status === 409) {
        toast.error("This task cannot be cancelled", {
          description: `${e.message} The server reports it as ${String(e.body.state).replace("_", " ")}.`,
        });
      } else {
        toast.error("Task not cancelled", { description: e.message });
      }
    },
    onSuccess: (t) => toast.success(`Cancelled task ${shortId(t.id)}`),
    onSettled: () => client.invalidateQueries({ queryKey: tasksKey(r) }),
  });
}

export function CancelTaskButton({ task, ...r }: Repo & { task: Task }) {
  const [open, setOpen] = useState(false);
  const cancel = useCancelTask(r);
  // The dialog stays mounted when the task turns final under it, so confirming then
  // shows the server's 409 instead of the dialog vanishing.
  return (
    <>
      {(open || !FINAL_STATES.has(task.state)) && (
        <Button
          size="xs"
          variant="outline"
          onClick={() => setOpen(true)}
          disabled={cancel.isPending}
          data-cancel-task={task.id}
        >
          <HugeiconsIcon icon={MinusSignCircleIcon} strokeWidth={2} data-icon="inline-start" />
          Cancel
        </Button>
      )}
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Cancel this task?</AlertDialogTitle>
            <AlertDialogDescription>
              The agent stops at its next step and its leases are released. A cancelled task cannot
              be resumed.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep running</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                setOpen(false);
                cancel.mutate(task.id);
              }}
            >
              Cancel task
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}

const col = createColumnHelper<typeof dataGridFeatures, Task>();

function useColumns(r: Repo) {
  const { data: apps = [] } = useQuery(agentAppsQuery());
  return useMemo(
    () =>
      col.columns([
        col.accessor("state", {
          header: "State",
          size: 150,
          cell: ({ row }) => <TaskStateBadge state={row.original.state} />,
        }),
        col.display({
          id: "work",
          header: "Task",
          size: 420,
          cell: ({ row }) => <TaskSummary task={row.original} {...r} />,
        }),
        col.accessor("app_id", {
          header: "App",
          size: 170,
          cell: ({ row }) => {
            const app = apps.find((a) => a.app_id === row.original.app_id);
            return (
              <span className="flex flex-wrap items-center gap-1">
                <span className="font-mono text-xs">{app?.slug ?? row.original.app_id}</span>
                {row.original.kind === "external" && (
                  <Badge variant="outline" size="xs">
                    External
                  </Badge>
                )}
              </span>
            );
          },
        }),
        col.display({
          id: "requester",
          header: "Requester",
          size: 130,
          cell: ({ row }) =>
            row.original.requester ? (
              <span className="flex items-center gap-1.5 text-xs">
                <UserAvatar user={row.original.requester} />
                {row.original.requester.login}
              </span>
            ) : (
              <span className="text-muted-foreground text-xs">None</span>
            ),
        }),
        col.accessor("updated_at", {
          header: "Updated",
          size: 110,
          cell: ({ row }) => (
            <span className="text-muted-foreground text-xs">
              <Ago at={row.original.updated_at} />
            </span>
          ),
        }),
        col.display({
          id: "actions",
          header: "",
          size: 90,
          cell: ({ row }) => <CancelTaskButton task={row.original} {...r} />,
        }),
      ]),
    [apps, r.owner, r.repo],
  );
}

export function TaskSummary({ task, owner, repo }: Repo & { task: Task }) {
  return (
    <div className="flex min-w-0 flex-col gap-0.5 whitespace-normal">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <Link
          to="/$owner/$repo/_graftyard/sessions/$uuid"
          params={{ owner, repo, uuid: task.id }}
          className="truncate font-mono text-xs font-semibold hover:underline"
          data-session-link={task.id}
        >
          {shortId(task.id)}
        </Link>
        {task.issue_number !== null && (
          <Link
            to="/$owner/$repo/issues/$number"
            params={{ owner, repo, number: task.issue_number }}
            className="text-muted-foreground flex items-center gap-0.5 text-xs hover:underline"
          >
            <HugeiconsIcon icon={RecordIcon} strokeWidth={2} className="size-3" />#
            {task.issue_number}
          </Link>
        )}
        {task.pr_number !== null && (
          <Link
            to="/$owner/$repo/pull/$number"
            params={{ owner, repo, number: task.pr_number }}
            className="text-primary flex items-center gap-0.5 text-xs hover:underline"
            data-pr-link={task.pr_number}
          >
            <HugeiconsIcon icon={GitPullRequestIcon} strokeWidth={2} className="size-3" />#
            {task.pr_number}
          </Link>
        )}
      </div>
      <p className="text-muted-foreground line-clamp-2 text-xs">
        {task.summary || task.prompt || "No summary yet."}
      </p>
    </div>
  );
}

export function TaskGrid({ filters, ...r }: Repo & { filters: TaskFilters }) {
  const { data } = useSuspenseQuery(tasksQuery(r, filters));
  const columns = useColumns(r);
  const table = useTable({
    features: dataGridFeatures,
    columns,
    data,
    getRowId: (t) => t.id,
    initialState: { pagination: { pageIndex: 0, pageSize: 100 } },
  });
  const filtered = Object.values(filters).some((v) => v !== undefined);
  return (
    <DataGrid
      table={table}
      recordCount={data.length}
      emptyMessage={
        filtered ? "No tasks match these filters." : "No agent tasks in this repository yet."
      }
      tableLayout={{ dense: true, headerBackground: true, width: "auto" }}
    >
      <DataGridContainer className="border-0" data-testid="task-grid">
        <DataGridTable />
      </DataGridContainer>
    </DataGrid>
  );
}

// A compact list for sidebars and the home page.
export function TaskList({ tasks, ...r }: Repo & { tasks: Task[] }) {
  return (
    <ul className="divide-y">
      {tasks.map((t) => (
        <li key={t.id} className="flex items-start gap-2 py-2">
          <TaskStateIcon state={t.state} className="mt-0.5" />
          <TaskSummary task={t} {...r} />
        </li>
      ))}
    </ul>
  );
}
