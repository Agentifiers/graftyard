import { useEffect, useRef, useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Alert02Icon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  GitBranchIcon,
  GitMergeIcon,
  GitPullRequestClosedIcon,
} from "@hugeicons/core-free-icons";
import { repoQuery } from "@/areas/core/queries";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { BranchName } from "./bits";
import { runState, type Requirement } from "./merge-rules";
import {
  refreshPull,
  useAutoMerge,
  useHeadBranch,
  useMerge,
  useSetDraft,
  useUpdateBranch,
  useUpdatePull,
} from "./mutations";
import {
  checkRunsQuery,
  mergeRequirementsQuery,
  pullCommitsQuery,
  pullQuery,
  type Pull,
} from "./queries";
import type { MergeMethod } from "./ui-api";

const METHODS: {
  value: MergeMethod;
  flag: "allow_merge_commit" | "allow_squash_merge" | "allow_rebase_merge";
  label: string;
  verb: string;
}[] = [
  {
    value: "merge",
    flag: "allow_merge_commit",
    label: "Create a merge commit",
    verb: "Merge pull request",
  },
  {
    value: "squash",
    flag: "allow_squash_merge",
    label: "Squash and merge",
    verb: "Squash and merge",
  },
  {
    value: "rebase",
    flag: "allow_rebase_merge",
    label: "Rebase and merge",
    verb: "Rebase and merge",
  },
];

const LOOK: Record<Requirement["state"], { icon: typeof CheckmarkCircle02Icon; tone: string }> = {
  ok: { icon: CheckmarkCircle02Icon, tone: "text-success" },
  pending: { icon: Clock01Icon, tone: "text-warning" },
  failing: { icon: CancelCircleIcon, tone: "text-destructive" },
};

// The merge box: requirements from branch protection, the methods the repository allows,
// an editable commit message, update branch, auto-merge, and branch cleanup after merge.
export function MergeBox({ pull }: { pull: Pull }) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  const { data: req } = useSuspenseQuery(mergeRequirementsQuery(pull));
  // The requirements poll while a check runs; when the state moves (or auto-merge
  // lands), refresh the pull and its checks so the header and tabs agree.
  const qc = useQueryClient();
  const last = useRef(req.mergeable_state);
  useEffect(() => {
    if (last.current === req.mergeable_state) return;
    last.current = req.mergeable_state;
    void refreshPull(qc, pull);
  }, [req.mergeable_state, qc, pull]);
  const { data: repo } = useSuspenseQuery(repoQuery(pull));
  const updatePull = useUpdatePull(pull);
  const branch = useHeadBranch(pull);

  if (pr.merged)
    return (
      <Box
        tone="info"
        icon={GitMergeIcon}
        title="Pull request successfully merged and closed"
        testid="merge-box-merged"
      >
        <p className="text-muted-foreground text-sm">
          {pr.merged_by?.login} merged commit{" "}
          <code className="font-mono">{pr.merge_commit_sha?.slice(0, 7)}</code> into{" "}
          <BranchName>{pr.base.ref}</BranchName>.
        </p>
        <div className="flex items-center gap-2 text-sm">
          {req.head_exists ? (
            <>
              <span>
                The <BranchName>{pr.head.ref}</BranchName> branch can be safely deleted.
              </span>
              <Button
                size="sm"
                variant="outline"
                className="ml-auto"
                disabled={branch.isPending}
                onClick={() => branch.mutate({ action: "delete", ref: pr.head.ref })}
              >
                Delete branch
              </Button>
            </>
          ) : (
            <>
              <span className="text-muted-foreground">
                <BranchName>{pr.head.ref}</BranchName> was deleted.
              </span>
              <Button
                size="sm"
                variant="outline"
                className="ml-auto"
                disabled={branch.isPending}
                onClick={() =>
                  branch.mutate({ action: "restore", ref: pr.head.ref, sha: pr.head.sha })
                }
              >
                Restore branch
              </Button>
            </>
          )}
        </div>
      </Box>
    );

  if (pr.state === "closed")
    return (
      <Box
        tone="destructive"
        icon={GitPullRequestClosedIcon}
        title="Closed with unmerged commits"
        testid="merge-box-closed"
      >
        <p className="text-muted-foreground text-sm">
          This pull request is closed. Reopen it to merge.
        </p>
        <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            disabled={updatePull.isPending || !req.head_exists}
            onClick={() => updatePull.mutate({ state: "open" })}
          >
            Reopen pull request
          </Button>
          {req.head_exists ? (
            <Button
              size="sm"
              variant="ghost"
              disabled={branch.isPending}
              onClick={() => branch.mutate({ action: "delete", ref: pr.head.ref })}
            >
              Delete branch
            </Button>
          ) : (
            <Button
              size="sm"
              variant="ghost"
              disabled={branch.isPending}
              onClick={() =>
                branch.mutate({ action: "restore", ref: pr.head.ref, sha: pr.head.sha })
              }
            >
              Restore branch
            </Button>
          )}
        </div>
      </Box>
    );

  const allowed = METHODS.filter((m) => repo[m.flag]);
  const blocked = !["clean", "unstable", "has_hooks"].includes(req.mergeable_state);
  return (
    <section
      className="bg-card overflow-hidden rounded-sm border"
      data-testid="merge-box"
      data-mergeable-state={req.mergeable_state}
    >
      <ul className="divide-y">
        {req.requirements.map((r) => (
          <li
            key={r.key}
            className="flex items-start gap-3 px-4 py-2.5"
            data-requirement={r.key}
            data-state={r.state}
          >
            <HugeiconsIcon
              icon={LOOK[r.state].icon}
              strokeWidth={2}
              className={cn("mt-0.5 size-5 shrink-0", LOOK[r.state].tone)}
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-semibold">{r.title}</p>
              <p className="text-muted-foreground text-xs">{r.detail}</p>
              {r.key === "checks" && req.required_checks.length > 0 && (
                <RequiredChecks pull={pull} sha={pr.head.sha} names={req.required_checks} />
              )}
            </div>
            {r.key === "behind" && <UpdateBranchButton pull={pull} sha={pr.head.sha} />}
            {r.key === "draft" && <ReadyButton pull={pull} />}
          </li>
        ))}
      </ul>
      <div className="bg-muted/30 border-t px-4 py-3">
        {allowed.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            This repository allows no merge method. An admin can enable one in settings.
          </p>
        ) : pr.draft ? null : (
          <MergeForm
            pull={pull}
            allowed={allowed}
            blocked={blocked}
            autoMergeAllowed={repo.allow_auto_merge ?? false}
          />
        )}
      </div>
    </section>
  );
}

const CHIP = {
  success: { variant: "success-light", label: "passed" },
  failure: { variant: "destructive-light", label: "failed" },
  pending: { variant: "warning-light", label: "pending" },
  none: { variant: "outline", label: "not reported" },
} as const;

function RequiredChecks({ pull, sha, names }: { pull: Pull; sha: string; names: string[] }) {
  const { data } = useQuery(checkRunsQuery({ ...pull, ref: sha }));
  const byName = new Map(data?.check_runs.map((r) => [r.name, runState(r)]));
  return (
    <p className="mt-1 flex flex-wrap gap-1" data-testid="required-checks">
      {names.map((c) => {
        const chip = CHIP[byName.get(c) ?? "none"];
        return (
          <Badge
            key={c}
            variant={chip.variant}
            size="default"
            data-check={c}
            className={chip.variant === "destructive-light" ? "dark:text-red-300" : undefined}
          >
            {c}: {chip.label}
          </Badge>
        );
      })}
    </p>
  );
}

function Box({
  tone,
  icon,
  title,
  testid,
  children,
}: {
  tone: "info" | "destructive";
  icon: typeof GitMergeIcon;
  title: string;
  testid: string;
  children: React.ReactNode;
}) {
  return (
    <section className="bg-card flex gap-3 rounded-sm border p-4" data-testid={testid}>
      <HugeiconsIcon
        icon={icon}
        strokeWidth={2}
        className={cn("mt-0.5 size-6 shrink-0", tone === "info" ? "text-info" : "text-destructive")}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <p className="font-semibold">{title}</p>
        {children}
      </div>
    </section>
  );
}

function UpdateBranchButton({ pull, sha }: { pull: Pull; sha: string }) {
  const update = useUpdateBranch(pull);
  return (
    <Button
      size="sm"
      variant="outline"
      disabled={update.isPending}
      onClick={() => update.mutate(sha)}
    >
      {update.isPending ? "Updating" : "Update branch"}
    </Button>
  );
}

function ReadyButton({ pull }: { pull: Pull }) {
  const ready = useSetDraft(pull);
  return (
    <Button
      size="sm"
      variant="outline"
      disabled={ready.isPending}
      onClick={() => ready.mutate(false)}
    >
      Ready for review
    </Button>
  );
}

function MergeForm({
  pull,
  allowed,
  blocked,
  autoMergeAllowed,
}: {
  pull: Pull;
  allowed: typeof METHODS;
  blocked: boolean;
  autoMergeAllowed: boolean;
}) {
  const { data: pr } = useSuspenseQuery(pullQuery(pull));
  // The commit list only seeds the squash message, so the form works without it.
  const commits = useQuery(pullCommitsQuery(pull)).data ?? [];
  const merge = useMerge(pull);
  const auto = useAutoMerge(pull);
  const [method, setMethod] = useState<MergeMethod>(
    allowed.find((m) => m.value === "squash")?.value ?? allowed[0].value,
  );
  const [editing, setEditing] = useState<"merge" | "auto" | null>(null);
  const meta = allowed.find((m) => m.value === method) ?? allowed[0];

  const defaults = (m: MergeMethod) =>
    m === "squash"
      ? {
          title: `${pr.title} (#${pr.number})`,
          message: commits.map((c) => `* ${c.commit.message.split("\n")[0]}`).join("\n"),
        }
      : m === "merge"
        ? {
            title: `Merge pull request #${pr.number} from ${pr.head.label.replace(":", "/")}`,
            message: pr.title,
          }
        : { title: "", message: "" };

  const form = useForm({
    defaultValues: defaults(method),
    onSubmit: async ({ value }) => {
      if (editing === "auto")
        await auto.mutateAsync({ method, title: value.title, message: value.message });
      else
        await merge.mutateAsync({
          method,
          title: value.title,
          message: value.message,
          sha: pr.head.sha,
        });
      setEditing(null);
    },
  });

  if (pr.auto_merge)
    return (
      <div className="flex flex-wrap items-center gap-3" data-testid="auto-merge-on">
        <HugeiconsIcon icon={Clock01Icon} strokeWidth={2} className="text-info size-5" />
        <div className="min-w-0 flex-1 text-sm">
          <p className="font-semibold">Auto-merge enabled</p>
          <p className="text-muted-foreground text-xs">
            {pr.auto_merge.enabled_by.login} enabled auto-merge ({pr.auto_merge.merge_method}). This
            pull request merges when every requirement passes.
          </p>
        </div>
        <Button
          size="sm"
          variant="outline"
          disabled={auto.isPending}
          onClick={() => auto.mutate(null)}
        >
          Disable auto-merge
        </Button>
      </div>
    );

  if (editing)
    return (
      <form
        className="flex flex-col gap-2"
        data-testid="merge-form"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
      >
        {method !== "rebase" ? (
          <>
            <form.Field name="title">
              {(f) => (
                <Input
                  aria-label="Commit title"
                  className="rounded-sm font-mono text-sm"
                  value={f.state.value}
                  onChange={(e) => f.handleChange(e.target.value)}
                />
              )}
            </form.Field>
            <form.Field name="message">
              {(f) => (
                <Textarea
                  aria-label="Commit message"
                  className="min-h-24 rounded-sm font-mono text-xs"
                  value={f.state.value}
                  onChange={(e) => f.handleChange(e.target.value)}
                />
              )}
            </form.Field>
          </>
        ) : (
          <p className="text-muted-foreground text-sm">
            The {pr.commits === 1 ? "commit" : `${pr.commits} commits`} from this branch will be
            rebased and added to the base branch.
          </p>
        )}
        <div className="flex gap-2">
          <form.Subscribe selector={(s) => s.isSubmitting}>
            {(busy) => (
              <Button type="submit" size="sm" disabled={busy}>
                {editing === "auto"
                  ? `Confirm auto-merge (${meta.label.toLowerCase()})`
                  : `Confirm ${meta.verb.toLowerCase()}`}
              </Button>
            )}
          </form.Subscribe>
          <Button type="button" variant="ghost" size="sm" onClick={() => setEditing(null)}>
            Cancel
          </Button>
        </div>
      </form>
    );

  return (
    <div className="flex flex-wrap items-center gap-2">
      <label className="flex items-center gap-1.5 text-sm">
        <span className="text-muted-foreground">Method</span>
        <NativeSelect
          size="sm"
          aria-label="Merge method"
          value={method}
          onChange={(e) => {
            const m = e.target.value as MergeMethod;
            setMethod(m);
            const d = defaults(m);
            form.setFieldValue("title", d.title);
            form.setFieldValue("message", d.message);
          }}
          className="[&_select]:rounded-sm"
        >
          {allowed.map((m) => (
            <NativeSelectOption key={m.value} value={m.value}>
              {m.label}
            </NativeSelectOption>
          ))}
        </NativeSelect>
      </label>
      <Button
        size="sm"
        variant={blocked ? "outline" : "default"}
        disabled={blocked}
        onClick={() => setEditing("merge")}
        data-testid="merge-button"
      >
        <HugeiconsIcon icon={GitMergeIcon} strokeWidth={2} data-icon="inline-start" />
        {meta.verb}
      </Button>
      {blocked && autoMergeAllowed && (
        <Button
          size="sm"
          variant="outline"
          onClick={() => setEditing("auto")}
          data-testid="enable-auto-merge"
        >
          Enable auto-merge
        </Button>
      )}
      {blocked && (
        <p className="text-muted-foreground flex items-center gap-1 text-xs">
          <HugeiconsIcon icon={Alert02Icon} strokeWidth={2} className="size-3.5" />
          Merging is blocked until the requirements above pass.
        </p>
      )}
      <p className="text-muted-foreground ml-auto flex items-center gap-1 text-xs">
        <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className="size-3.5" />
        {pr.head.ref} into {pr.base.ref}
      </p>
    </div>
  );
}
