import { useState } from "react";
import { useNavigate } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowDown01Icon,
  Delete02Icon,
  Refresh01Icon,
  StopCircleIcon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { runQuery } from "./queries";
import { send, useWrite } from "./write";

type Ref = { owner: string; repo: string; runId: number };
type Run = Schemas["workflow-run"];

const runKeys = ({ owner, repo, runId }: Ref) => [
  ["repos", owner, repo, "actions", "runs"],
  ["_t", "ui", "repos", owner, repo, "actions", "runs", runId],
];

// Shows a run as re-queued or cancelling the moment the button is pressed.
function useRunPatch(ref: Ref) {
  const queryClient = useQueryClient();
  const key = runQuery(ref).queryKey;
  return (patch: Partial<Run>) => {
    const before = queryClient.getQueryData(key);
    queryClient.setQueryData(key, (r) => r && { ...r, ...patch });
    return () => queryClient.setQueryData(key, before);
  };
}

export function RunActions({ run, failed, ...ref }: Ref & { run: Run; failed: boolean }) {
  const navigate = useNavigate();
  const patch = useRunPatch(ref);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const path = { params: { path: { owner: ref.owner, repo: ref.repo, run_id: ref.runId } } };

  const rerun = useWrite({
    run: (which: "all" | "failed") =>
      send(
        which === "all"
          ? api.POST("/repos/{owner}/{repo}/actions/runs/{run_id}/rerun", { ...path, body: {} })
          : api.POST("/repos/{owner}/{repo}/actions/runs/{run_id}/rerun-failed-jobs", {
              ...path,
              body: {},
            }),
      ),
    invalidates: () => runKeys(ref),
    optimistic: () =>
      patch({ status: "queued", conclusion: null, run_attempt: (run.run_attempt ?? 1) + 1 }),
    success: (which) => (which === "all" ? "Re-running all jobs" : "Re-running failed jobs"),
    failure: "Could not re-run this workflow",
  });

  const cancel = useWrite({
    run: () => send(api.POST("/repos/{owner}/{repo}/actions/runs/{run_id}/cancel", path)),
    invalidates: () => runKeys(ref),
    optimistic: () => patch({ status: "completed", conclusion: "cancelled" }),
    success: () => "Cancelling this run",
    failure: "Could not cancel this run",
  });

  const remove = useWrite({
    run: () => send(api.DELETE("/repos/{owner}/{repo}/actions/runs/{run_id}", path)),
    invalidates: () => [["repos", ref.owner, ref.repo, "actions", "runs"]],
    success: () => `Deleted run #${run.run_number}`,
    failure: "Could not delete this run",
  });

  const running = run.status !== "completed";
  return (
    <div className="flex flex-wrap items-center gap-2" data-testid="run-actions">
      {running ? (
        <Button
          variant="outline"
          size="sm"
          onClick={() => cancel.mutate(undefined)}
          disabled={cancel.isPending}
        >
          <HugeiconsIcon icon={StopCircleIcon} strokeWidth={2} data-icon="inline-start" />
          Cancel workflow
        </Button>
      ) : (
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button variant="outline" size="sm" disabled={rerun.isPending}>
                <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                Re-run jobs
                <HugeiconsIcon icon={ArrowDown01Icon} strokeWidth={2} data-icon="inline-end" />
              </Button>
            }
          />
          <DropdownMenuContent align="end" className="w-52">
            <DropdownMenuItem disabled={!failed} onClick={() => rerun.mutate("failed")}>
              Re-run failed jobs
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => rerun.mutate("all")}>Re-run all jobs</DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <Button
        variant="ghost"
        size="icon-sm"
        aria-label="Delete workflow run"
        disabled={running}
        title={running ? "Cancel the run before deleting it" : "Delete workflow run"}
        onClick={() => setConfirmDelete(true)}
      >
        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
      </Button>
      <AlertDialog open={confirmDelete} onOpenChange={setConfirmDelete}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete workflow run #{run.run_number}?</AlertDialogTitle>
            <AlertDialogDescription>
              Its logs and artifacts are deleted with it. This cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter className="rounded-b-sm">
            <AlertDialogCancel>Keep it</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() =>
                remove.mutate(undefined, {
                  onSuccess: () => void navigate({ to: "/$owner/$repo/actions", params: ref }),
                  onSettled: () => setConfirmDelete(false),
                })
              }
            >
              Delete run
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
