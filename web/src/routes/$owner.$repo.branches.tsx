import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Delete02Icon,
  Edit02Icon,
  GitBranchIcon,
  GitPullRequestIcon,
  LockIcon,
  Search01Icon,
} from "@hugeicons/core-free-icons";
import { repoQuery } from "@/areas/core/queries";
import {
  createBranch,
  deleteBranch,
  errorText,
  invalidateCode,
  renameBranch,
  WriteError,
  type FieldErrors,
} from "@/areas/code/mutations";
import { branchSummariesQuery } from "@/areas/code/queries";
import { Author, CopyButton, RefPicker, short } from "@/areas/code/refs";
import type { BranchSummary } from "@/areas/code/ui-api";
import { Ago } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

// Branches with ahead and behind counts against the default branch, filter tabs, and
// create, rename, and delete. Each write shows at once and rolls back on failure.

const TABS = ["overview", "yours", "active", "stale", "all"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/$owner/$repo/branches")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; q?: string } => ({
    tab: TABS.includes(s.tab as Tab) && s.tab !== "overview" ? (s.tab as Tab) : undefined,
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(branchSummariesQuery(params)),
  component: BranchesPage,
});

const VIEWER = "mira";
// A branch untouched for three months is stale, as GHES counts it.
const STALE_MS = 90 * 86400_000;
const isStale = (b: BranchSummary) => Date.now() - Date.parse(b.updated_at) > STALE_MS;

function BranchesPage() {
  const { tab = "overview", q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [creating, setCreating] = useState(false);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Branches"
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} data-icon="inline-start" />
            New branch
          </Button>
        }
      />
      <div className="flex flex-wrap items-center gap-2">
        <nav
          className="bg-background flex rounded-sm border p-0.5 text-sm"
          role="tablist"
          aria-label="Branch filter"
        >
          {TABS.map((t) => (
            <Link
              key={t}
              from={Route.fullPath}
              search={(prev) => ({ ...prev, tab: t === "overview" ? undefined : t })}
              role="tab"
              aria-selected={tab === t}
              className="text-muted-foreground aria-selected:bg-muted aria-selected:text-foreground rounded-sm px-2.5 py-1 font-medium capitalize"
            >
              {t}
            </Link>
          ))}
        </nav>
        <label className="relative ml-auto w-full max-w-xs">
          <HugeiconsIcon
            icon={Search01Icon}
            strokeWidth={2}
            className="text-muted-foreground absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2"
          />
          <Input
            type="search"
            aria-label="Search branches"
            placeholder="Search branches"
            className="pl-8"
            defaultValue={q}
            onChange={(e) =>
              void navigate({
                search: (prev) => ({ ...prev, q: e.target.value || undefined }),
                replace: true,
              })
            }
          />
        </label>
      </div>
      <Region name="branches" rows={6}>
        <BranchLists tab={tab} q={q} />
      </Region>
      {creating && (
        <Region name="the branch form" fallback={null}>
          <CreateBranchDialog onClose={() => setCreating(false)} />
        </Region>
      )}
    </div>
  );
}

function BranchLists({ tab, q }: { tab: Tab; q: string }) {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(branchSummariesQuery(params));
  const match = data.filter((b) => b.name.toLowerCase().includes(q.toLowerCase()));
  const def = match.filter((b) => b.default);
  const others = match.filter((b) => !b.default);
  const yours = others.filter((b) => b.author.login === VIEWER);
  const active = others.filter((b) => !isStale(b));
  const stale = others.filter(isStale);
  const sections: [string, BranchSummary[]][] =
    tab === "overview"
      ? [
          ["Default", def],
          ["Your branches", yours],
          ["Active branches", active.filter((b) => b.author.login !== VIEWER).slice(0, 5)],
        ]
      : tab === "yours"
        ? [["Your branches", yours]]
        : tab === "active"
          ? [["Active branches", active]]
          : tab === "stale"
            ? [["Stale branches", stale]]
            : [["All branches", [...def, ...others]]];
  const shown = sections.filter(([, list]) => list.length > 0);
  if (shown.length === 0) {
    return (
      <Empty className="border py-12">
        <EmptyHeader>
          <EmptyTitle>No branches here</EmptyTitle>
          <EmptyDescription>
            {q
              ? `Nothing matches "${q}".`
              : tab === "stale"
                ? "Every branch has a commit in the last three months."
                : "Create one to start."}
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  const max = Math.max(1, ...match.map((b) => Math.max(b.ahead, b.behind)));
  return (
    <div className="flex flex-col gap-4" data-testid="branches">
      {shown.map(([title, list]) => (
        <Panel key={title} title={title} flush>
          <ul className="divide-y">
            {list.map((b) => (
              <BranchRow key={b.name} branch={b} max={max} />
            ))}
          </ul>
        </Panel>
      ))}
    </div>
  );
}

const STATUS_LOOK = {
  success: { icon: CheckmarkCircle02Icon, tone: "text-success", label: "Checks passed" },
  failure: { icon: CancelCircleIcon, tone: "text-destructive", label: "Checks failed" },
  pending: { icon: Clock01Icon, tone: "text-warning", label: "Checks pending" },
} as const;

function BranchRow({ branch: b, max }: { branch: BranchSummary; max: number }) {
  const params = Route.useParams();
  const [renaming, setRenaming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const pending = b.sha === "";
  const status = b.status ? STATUS_LOOK[b.status] : null;
  return (
    <li
      className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2 text-sm"
      data-branch={b.name}
      data-pending={pending || undefined}
    >
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Link
          to="/$owner/$repo/tree/$"
          params={{ ...params, _splat: b.name }}
          className="bg-muted text-primary truncate rounded-sm px-1.5 py-0.5 font-mono text-xs hover:underline"
        >
          {b.name}
        </Link>
        <CopyButton value={b.name} label="Copy the branch name" />
        {b.default && (
          <Badge variant="outline" size="sm">
            Default
          </Badge>
        )}
        {b.protected && (
          <Badge variant="info-light" size="sm" title="Protected branch">
            <HugeiconsIcon icon={LockIcon} strokeWidth={2} />
            Protected
          </Badge>
        )}
        {pending && (
          <Badge variant="warning-light" size="sm">
            Saving
          </Badge>
        )}
        {status && (
          <HugeiconsIcon
            icon={status.icon}
            strokeWidth={2}
            className={`size-4 ${status.tone}`}
            aria-label={status.label}
          />
        )}
      </div>
      <span className="text-muted-foreground flex items-center gap-1.5 text-xs">
        {!pending && <Author author={b.author} />}
        updated <Ago at={b.updated_at} />
      </span>
      {!b.default && (
        <span
          className="flex w-28 items-center text-[11px] tabular-nums"
          title={`${b.behind} behind, ${b.ahead} ahead of the default branch`}
          data-testid="ahead-behind"
        >
          <span className="flex w-1/2 flex-col items-end gap-0.5 border-r pr-1">
            <span className="text-muted-foreground">{b.behind}</span>
            <span
              className="bg-muted-foreground/40 h-1"
              style={{ width: `${(b.behind / max) * 100}%` }}
            />
          </span>
          <span className="flex w-1/2 flex-col items-start gap-0.5 pl-1">
            <span className="text-muted-foreground">{b.ahead}</span>
            <span className="bg-primary/60 h-1" style={{ width: `${(b.ahead / max) * 100}%` }} />
          </span>
        </span>
      )}
      <span className="w-16 text-right">
        {b.pull && (
          <Link
            to="/$owner/$repo/pull/$number"
            params={{ ...params, number: b.pull.number }}
            className="text-muted-foreground hover:text-foreground inline-flex items-center gap-1 text-xs capitalize hover:underline"
          >
            <HugeiconsIcon icon={GitPullRequestIcon} strokeWidth={2} className="size-3.5" />#
            {b.pull.number}
          </Link>
        )}
      </span>
      <span className="flex items-center gap-0.5">
        <Button
          size="icon-xs"
          variant="ghost"
          aria-label={`Rename ${b.name}`}
          onClick={() => setRenaming(true)}
          disabled={pending}
        >
          <HugeiconsIcon icon={Edit02Icon} strokeWidth={2} />
        </Button>
        <Button
          size="icon-xs"
          variant="ghost"
          className="text-destructive"
          aria-label={`Delete ${b.name}`}
          onClick={() => setDeleting(true)}
          disabled={pending || b.default || b.protected}
          title={
            b.default
              ? "The default branch cannot be deleted"
              : b.protected
                ? "Protected branches cannot be deleted"
                : undefined
          }
        >
          <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
        </Button>
      </span>
      {renaming && <RenameDialog branch={b} onClose={() => setRenaming(false)} />}
      <DeleteDialog branch={b} open={deleting} onOpenChange={setDeleting} />
    </li>
  );
}

function useSummaries() {
  const params = Route.useParams();
  const queryClient = useQueryClient();
  const key = branchSummariesQuery(params).queryKey;
  return {
    params,
    key,
    // Applies an optimistic edit and returns what to put back on failure.
    async edit(change: (list: BranchSummary[]) => BranchSummary[]) {
      await queryClient.cancelQueries({ queryKey: key });
      const previous = queryClient.getQueryData<BranchSummary[]>(key);
      if (previous) queryClient.setQueryData(key, change(previous));
      return previous;
    },
    restore(previous: BranchSummary[] | undefined) {
      if (previous) queryClient.setQueryData(key, previous);
    },
  };
}

function CreateBranchDialog({ onClose }: { onClose: () => void }) {
  const { params, edit, restore } = useSummaries();
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const { data: branches } = useSuspenseQuery(branchSummariesQuery(params));
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const create = useMutation({
    mutationFn: (v: { name: string; from: string }) => {
      const sha = branches.find((b) => b.name === v.from)?.sha ?? v.from;
      return createBranch(params, { name: v.name, sha });
    },
    onMutate: (v) =>
      edit((list) => [
        {
          name: v.name,
          sha: "",
          ahead: 0,
          behind: 0,
          updated_at: new Date().toISOString(),
          author: { login: VIEWER, name: "Mira Okafor", avatar_url: null },
          protected: false,
          default: false,
          status: null,
          pull: null,
        },
        ...list,
      ]),
    onError: (e, v, previous) => {
      restore(previous);
      setServerErrors(e instanceof WriteError ? e.fields : {});
      toast.error(`Could not create ${v.name}: ${errorText(e)}`);
    },
    onSuccess: (_r, v) => {
      toast.success(`Created ${v.name} from ${v.from}`);
      onClose();
    },
    onSettled: () => invalidateCode(params),
  });
  const form = useForm({
    defaultValues: { name: "", from: repo.default_branch },
    onSubmit: ({ value }) => {
      setServerErrors({});
      return create.mutateAsync({ name: value.name.trim(), from: value.from }).catch(() => {});
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md" data-testid="create-branch">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Create a branch</DialogTitle>
            <DialogDescription>
              The new branch starts at the source branch's latest commit.
            </DialogDescription>
          </DialogHeader>
          <form.Field
            name="name"
            validators={{
              onChange: ({ value }) =>
                /\s/.test(value)
                  ? "A branch name cannot contain spaces."
                  : /\.\.|~|\^|:|\\/.test(value)
                    ? "Remove .., ~, ^, : and \\ from the name."
                    : undefined,
              onSubmit: ({ value }) => (value.trim() ? undefined : "Name the branch."),
            }}
          >
            {(field) => (
              <Field>
                <FieldLabel htmlFor="branch-name">New branch name</FieldLabel>
                <Input
                  id="branch-name"
                  autoFocus
                  className="font-mono"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={
                    field.state.meta.errors.length > 0 || Boolean(serverErrors.ref) || undefined
                  }
                />
                <FieldError
                  errors={[
                    ...field.state.meta.errors,
                    serverErrors.ref && `Branch ${serverErrors.ref.replace(/^ref /, "")}`,
                  ]
                    .filter(Boolean)
                    .map((m) => ({ message: String(m) }))}
                />
              </Field>
            )}
          </form.Field>
          <form.Field name="from">
            {(field) => (
              <Field>
                <FieldLabel>Source</FieldLabel>
                <RefPicker
                  owner={params.owner}
                  repo={params.repo}
                  current={field.state.value}
                  onPick={field.handleChange}
                  label="Pick a source branch or tag"
                />
                <FieldDescription>
                  At{" "}
                  <code className="font-mono">
                    {short(
                      branches.find((b) => b.name === field.state.value)?.sha ?? field.state.value,
                    )}
                  </code>
                </FieldDescription>
                {serverErrors.sha && (
                  <FieldError errors={[{ message: "The source commit does not exist." }]} />
                )}
              </Field>
            )}
          </form.Field>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" size="sm" disabled={submitting}>
                  {submitting ? "Creating" : "Create branch"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function RenameDialog({ branch, onClose }: { branch: BranchSummary; onClose: () => void }) {
  const { params, edit, restore } = useSummaries();
  const queryClient = useQueryClient();
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const rename = useMutation({
    mutationFn: (to: string) => renameBranch(params, { from: branch.name, to }),
    onMutate: (to) =>
      edit((list) => list.map((b) => (b.name === branch.name ? { ...b, name: to } : b))),
    onError: (e, _to, previous) => {
      restore(previous);
      setServerErrors(e instanceof WriteError ? e.fields : {});
      toast.error(`Could not rename ${branch.name}: ${errorText(e)}`);
    },
    onSuccess: (_r, to) => {
      toast.success(`Renamed ${branch.name} to ${to}`);
      onClose();
    },
    // The repository's default branch and its pull requests may follow the rename.
    onSettled: () =>
      Promise.all([
        invalidateCode(params),
        queryClient.invalidateQueries({ queryKey: ["repos", params.owner, params.repo, "pulls"] }),
      ]),
  });
  const form = useForm({
    defaultValues: { name: branch.name },
    onSubmit: ({ value }) => {
      setServerErrors({});
      return rename.mutateAsync(value.name.trim()).catch(() => {});
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md" data-testid="rename-branch">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Rename this branch</DialogTitle>
            <DialogDescription>
              Open pull requests from and into <code className="font-mono">{branch.name}</code>{" "}
              follow the new name.
              {branch.default && " It stays the default branch."}
            </DialogDescription>
          </DialogHeader>
          <form.Field
            name="name"
            validators={{
              onSubmit: ({ value }) =>
                !value.trim()
                  ? "Name the branch."
                  : value.trim() === branch.name
                    ? "Pick a different name."
                    : /\s/.test(value)
                      ? "A branch name cannot contain spaces."
                      : undefined,
            }}
          >
            {(field) => (
              <Field>
                <FieldLabel htmlFor="rename-to">Rename {branch.name} to</FieldLabel>
                <Input
                  id="rename-to"
                  autoFocus
                  className="font-mono"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={
                    field.state.meta.errors.length > 0 ||
                    Boolean(serverErrors.new_name) ||
                    undefined
                  }
                />
                <FieldError
                  errors={[
                    ...field.state.meta.errors,
                    serverErrors.new_name &&
                      `Branch ${serverErrors.new_name.replace(/^new name /, "")}`,
                  ]
                    .filter(Boolean)
                    .map((m) => ({ message: String(m) }))}
                />
              </Field>
            )}
          </form.Field>
          <DialogFooter>
            <Button type="button" variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" size="sm" disabled={submitting}>
                  {submitting ? "Renaming" : "Rename branch"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DeleteDialog({
  branch,
  open,
  onOpenChange,
}: {
  branch: BranchSummary;
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const { params, edit, restore } = useSummaries();
  const remove = useMutation({
    mutationFn: () => deleteBranch(params, branch.name),
    onMutate: () => edit((list) => list.filter((b) => b.name !== branch.name)),
    onError: (e, _v, previous) => {
      restore(previous);
      toast.error(`Could not delete ${branch.name}: ${errorText(e)}`);
    },
    onSuccess: () => toast.success(`Deleted ${branch.name}`),
    onSettled: () => invalidateCode(params),
  });
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent data-testid="delete-branch">
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {branch.name}?</AlertDialogTitle>
          <AlertDialogDescription>
            {branch.ahead > 0
              ? `${branch.ahead} ${branch.ahead === 1 ? "commit is" : "commits are"} only on this branch. Deleting it drops the ref; the commits stay reachable by SHA.`
              : "Every commit on this branch is already on the default branch."}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant="destructive"
            onClick={() => {
              onOpenChange(false);
              remove.mutate();
            }}
          >
            Delete branch
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
