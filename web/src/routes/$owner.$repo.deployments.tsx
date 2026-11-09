import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  ArrowUpRight01Icon,
  Delete02Icon,
  Rocket01Icon,
  Settings02Icon,
  Timer02Icon,
  UserGroupIcon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api } from "@/api/client";
import { errorText, send, serverError, WriteError, type PendingReview } from "@/areas/releases/api";
import { DeploymentBadge } from "@/areas/releases/parts";
import {
  branchesQuery,
  deploymentHistoryQuery,
  deploymentsKey,
  environmentsQuery,
  isFinal,
  pendingReviewsQuery,
  tagsQuery,
  type DeploymentRow,
} from "@/areas/releases/queries";
import { useOptimisticWrite } from "@/areas/releases/write";
import { Ago, UserAvatar } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  NativeSelect,
  NativeSelectOptGroup,
  NativeSelectOption,
} from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";

export const Route = createFileRoute("/$owner/$repo/deployments")({
  validateSearch: (search: Record<string, unknown>): { environment?: string } => ({
    environment:
      typeof search.environment === "string" && search.environment ? search.environment : undefined,
  }),
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(environmentsQuery(params));
    void context.queryClient.prefetchQuery(deploymentHistoryQuery(params));
    void context.queryClient.prefetchQuery(pendingReviewsQuery(params));
  },
  component: DeploymentsPage,
});

type Repo = { owner: string; repo: string };

function DeploymentsPage() {
  const params = Route.useParams();
  const [open, setOpen] = useState(false);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Deployments"
        description="Every deployment of this repository, by environment."
        actions={
          <>
            <Button
              size="sm"
              variant="outline"
              render={<Link to="/$owner/$repo/settings/environments" params={params} />}
            >
              <HugeiconsIcon icon={Settings02Icon} strokeWidth={2} data-icon="inline-start" />
              Environments
            </Button>
            <Button size="sm" onClick={() => setOpen(true)}>
              <HugeiconsIcon icon={Rocket01Icon} strokeWidth={2} data-icon="inline-start" />
              New deployment
            </Button>
          </>
        }
      />
      <Region name="pending deployments" rows={2}>
        <PendingReviews />
      </Region>
      <div className="grid items-start gap-4 md:grid-cols-[13rem_minmax(0,1fr)]">
        <Region name="environments" rows={4}>
          <EnvironmentFilter />
        </Region>
        <Region name="deployments" rows={8}>
          <History />
        </Region>
      </div>
      {open && (
        <Region name="deployment form" rows={3}>
          <NewDeployment {...params} onClose={() => setOpen(false)} />
        </Region>
      )}
    </div>
  );
}

function EnvironmentFilter() {
  const params = Route.useParams();
  const { environment } = Route.useSearch();
  const { data: environments } = useSuspenseQuery(environmentsQuery(params));
  const { data: history } = useSuspenseQuery(deploymentHistoryQuery(params));
  const entries = [
    { name: undefined, label: "All environments" },
    ...environments.map((e) => ({ name: e.name, label: e.name })),
  ];
  return (
    <nav
      aria-label="Environments"
      className="flex flex-col gap-0.5 text-sm"
      data-testid="environment-filter"
    >
      {entries.map((e) => {
        const active = environment === e.name;
        const last = e.name ? history.find((r) => r.deployment.environment === e.name) : undefined;
        return (
          <Link
            key={e.label}
            from={Route.fullPath}
            search={{ environment: e.name }}
            data-active={active || undefined}
            className="text-muted-foreground hover:bg-muted hover:text-foreground data-active:bg-muted data-active:text-foreground data-active:border-primary flex items-center gap-2 rounded-sm border-l-2 border-transparent px-2 py-1.5 font-medium"
          >
            <span className="min-w-0 flex-1 truncate">{e.label}</span>
            {last && <StateDot state={last.statuses[0]?.state} />}
          </Link>
        );
      })}
    </nav>
  );
}

function StateDot({ state }: { state: string | undefined }) {
  const tone =
    state === "success"
      ? "bg-success"
      : state === "failure" || state === "error"
        ? "bg-destructive"
        : state === "in_progress" || state === "queued"
          ? "bg-warning"
          : "bg-muted-foreground/40";
  return <span className={cn("size-2 shrink-0 rounded-sm", tone)} aria-label={state} />;
}

// The deployments, grouped by environment, newest first, with each one's status trail.
function History() {
  const params = Route.useParams();
  const { environment } = Route.useSearch();
  const { data: rows } = useSuspenseQuery(deploymentHistoryQuery(params));
  const shown = rows.filter((r) => !environment || r.deployment.environment === environment);
  const groups = new Map<string, DeploymentRow[]>();
  for (const r of [...shown].sort((a, b) =>
    b.deployment.created_at.localeCompare(a.deployment.created_at),
  )) {
    groups.set(r.deployment.environment, [...(groups.get(r.deployment.environment) ?? []), r]);
  }
  if (shown.length === 0) {
    return (
      <p
        className="text-muted-foreground border px-4 py-10 text-center text-sm"
        data-testid="deployments-empty"
      >
        {environment
          ? `Nothing has been deployed to ${environment} yet.`
          : "Nothing has been deployed yet."}
      </p>
    );
  }
  const live = rows.some((r) => !isFinal(r));
  return (
    <div className="flex flex-col gap-4" data-testid="deployments" data-live={live || undefined}>
      {[...groups].map(([env, list]) => {
        const active = list.find((r) => r.statuses[0]?.state === "success");
        const url = active?.statuses.find((s) => s.environment_url)?.environment_url;
        return (
          <Panel
            key={env}
            flush
            title={
              <span className="flex items-center gap-2">
                {env}
                <span className="text-muted-foreground font-normal tabular-nums">
                  {list.length}
                </span>
              </span>
            }
            actions={
              url ? (
                <a
                  href={url}
                  target="_blank"
                  rel="noreferrer"
                  className="text-primary flex items-center gap-1 text-xs hover:underline"
                  data-testid={`active-url-${env}`}
                >
                  {url.replace(/^https:\/\//, "")}
                  <HugeiconsIcon icon={ArrowUpRight01Icon} strokeWidth={2} className="size-3.5" />
                </a>
              ) : (
                <span className="text-muted-foreground text-xs">No active deployment</span>
              )
            }
          >
            <ul className="divide-y" data-environment={env}>
              {list.map((row) => (
                <DeploymentItem key={row.deployment.id} row={row} active={row === active} />
              ))}
            </ul>
          </Panel>
        );
      })}
    </div>
  );
}

function DeploymentItem({ row, active }: { row: DeploymentRow; active: boolean }) {
  const params = Route.useParams();
  const [expanded, setExpanded] = useState(false);
  const { deployment: d, statuses } = row;
  const latest = statuses[0];
  const key = deploymentHistoryQuery(params).queryKey;
  const remove = useOptimisticWrite({
    key,
    invalidate: [deploymentsKey(params)],
    failure: "Deployment not deleted",
    apply: (list: DeploymentRow[]) => list.filter((r) => r.deployment.id !== d.id),
    write: () =>
      send(
        api.DELETE("/repos/{owner}/{repo}/deployments/{deployment_id}", {
          params: { path: { ...params, deployment_id: d.id } },
        }),
      ),
    onSuccess: () => toast.success(`Deleted deployment ${d.id}`),
  });
  const deactivate = useOptimisticWrite({
    key,
    invalidate: [deploymentsKey(params)],
    failure: "Deployment not marked inactive",
    apply: (list: DeploymentRow[]) =>
      list.map((r) =>
        r.deployment.id === d.id && r.statuses[0]
          ? {
              ...r,
              statuses: [
                {
                  ...r.statuses[0],
                  id: -1,
                  state: "inactive" as const,
                  description: "Marking inactive…",
                },
                ...r.statuses,
              ],
            }
          : r,
      ),
    write: () =>
      send(
        api.POST("/repos/{owner}/{repo}/deployments/{deployment_id}/statuses", {
          params: { path: { ...params, deployment_id: d.id } },
          body: { state: "inactive", description: "Marked inactive by @mira" },
        }),
      ),
  });
  return (
    <li className="px-3 py-2 text-sm" data-deployment={d.id} data-state={latest?.state}>
      <div className="flex flex-wrap items-center gap-2">
        <DeploymentBadge state={latest?.state} />
        {active && (
          <Badge variant="success-light" size="sm">
            Latest active
          </Badge>
        )}
        <code className="bg-muted rounded-sm px-1.5 py-0.5 font-mono text-xs">{d.ref}</code>
        <span className="text-muted-foreground font-mono text-xs">{d.sha.slice(0, 7)}</span>
        <span className="min-w-0 flex-1 truncate">{d.description}</span>
        <span className="text-muted-foreground flex w-44 shrink-0 items-center gap-1.5 text-xs">
          <UserAvatar user={d.creator} />
          {d.creator?.login} <Ago at={d.created_at} />
        </span>
        <span className="flex w-44 shrink-0 items-center justify-end gap-1">
          {latest?.state === "success" && (
            <Button size="xs" variant="outline" onClick={() => deactivate.mutate(undefined)}>
              Mark inactive
            </Button>
          )}
          {latest && isFinal(row) && latest.state !== "success" && (
            <Button
              size="icon-xs"
              variant="ghost"
              aria-label={`Delete deployment ${d.id}`}
              onClick={() => remove.mutate(undefined)}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
            </Button>
          )}
          <Button
            size="xs"
            variant="ghost"
            aria-expanded={expanded}
            onClick={() => setExpanded((v) => !v)}
          >
            {expanded ? "Hide" : "History"}
          </Button>
        </span>
      </div>
      {latest?.description && (
        <p className="text-muted-foreground mt-1 text-xs" data-testid="status-description">
          {latest.description}
        </p>
      )}
      {expanded && (
        <ol className="mt-2 flex flex-col gap-1 border-l pl-3 text-xs" data-testid="status-trail">
          {[...statuses].reverse().map((s) => (
            <li key={s.id} className="flex flex-wrap items-center gap-2">
              <DeploymentBadge state={s.state} />
              <span>{s.description}</span>
              <span className="text-muted-foreground">
                by {s.creator?.login} <Ago at={s.created_at} />
              </span>
            </li>
          ))}
        </ol>
      )}
    </li>
  );
}

// ---- Review deployments ----

function PendingReviews() {
  const params = Route.useParams();
  const { data: pending } = useSuspenseQuery(pendingReviewsQuery(params));
  const [reviewing, setReviewing] = useState(false);
  if (pending.length === 0) return null;
  const canReview = pending.some((p) => p.current_user_can_approve);
  return (
    <>
      <Alert variant="warning" data-testid="pending-reviews">
        <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} />
        <AlertTitle>
          {pending.length} {pending.length === 1 ? "deployment is" : "deployments are"} waiting for
          review
        </AlertTitle>
        <AlertDescription>
          <ul className="flex flex-col gap-0.5">
            {pending.map((p) => (
              <li key={p.deployment.id}>
                <code className="font-mono">{p.deployment.ref}</code> to{" "}
                <span className="font-medium">{p.environment.name}</span>, requested by{" "}
                {p.deployment.creator?.login} <Ago at={p.deployment.created_at} />
              </li>
            ))}
          </ul>
          <Button
            size="sm"
            className="mt-1 w-fit"
            disabled={!canReview}
            onClick={() => setReviewing(true)}
          >
            Review deployments
          </Button>
          {!canReview && (
            <span className="text-xs">You are not a required reviewer for these.</span>
          )}
        </AlertDescription>
      </Alert>
      {reviewing && (
        <ReviewDialog {...params} pending={pending} onClose={() => setReviewing(false)} />
      )}
    </>
  );
}

function ReviewDialog({
  pending,
  onClose,
  ...repo
}: Repo & { pending: PendingReview[]; onClose: () => void }) {
  const queryClient = useQueryClient();
  const reviewable = pending.filter((p) => p.current_user_can_approve);
  const [chosen, setChosen] = useState(() => new Set(reviewable.map((p) => p.deployment.id)));
  const [comment, setComment] = useState("");
  const pendingKey = pendingReviewsQuery(repo).queryKey;
  const review = useMutation({
    mutationFn: async (verdict: "approve" | "reject") => {
      for (const id of chosen) {
        await send(
          api.POST("/repos/{owner}/{repo}/deployments/{deployment_id}/statuses", {
            params: { path: { ...repo, deployment_id: id } },
            body:
              verdict === "approve"
                ? { state: "in_progress", description: comment.trim() }
                : {
                    state: "failure",
                    description: comment.trim()
                      ? `Rejected by @mira: ${comment.trim()}`
                      : "Rejected by @mira",
                  },
          }),
        );
      }
      return verdict;
    },
    onMutate: async () => {
      await queryClient.cancelQueries({ queryKey: pendingKey });
      const before = queryClient.getQueryData<PendingReview[]>(pendingKey);
      queryClient.setQueryData<PendingReview[]>(pendingKey, (list) =>
        list?.filter((p) => !chosen.has(p.deployment.id)),
      );
      return { before };
    },
    onSuccess: (verdict) => {
      toast.success(verdict === "approve" ? "Deployments approved" : "Deployments rejected");
      onClose();
    },
    onError: (e, _v, ctx) => {
      if (ctx?.before) queryClient.setQueryData(pendingKey, ctx.before);
      toast.error("Review not submitted", { description: errorText(e) });
    },
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: pendingKey }),
        queryClient.invalidateQueries({ queryKey: deploymentsKey(repo) }),
      ]),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md" data-testid="review-dialog">
        <DialogHeader>
          <DialogTitle>Review pending deployments</DialogTitle>
          <DialogDescription>
            Approve or reject the deployments you are a reviewer for.
          </DialogDescription>
        </DialogHeader>
        <ul className="flex flex-col gap-2 text-sm">
          {pending.map((p) => (
            <li key={p.deployment.id}>
              <Field
                orientation="horizontal"
                data-disabled={!p.current_user_can_approve || undefined}
              >
                <Checkbox
                  id={`review-${p.deployment.id}`}
                  disabled={!p.current_user_can_approve}
                  checked={chosen.has(p.deployment.id)}
                  onCheckedChange={(v) =>
                    setChosen((s) => {
                      const next = new Set(s);
                      if (v) next.add(p.deployment.id);
                      else next.delete(p.deployment.id);
                      return next;
                    })
                  }
                />
                <FieldLabel
                  htmlFor={`review-${p.deployment.id}`}
                  className="flex-col items-start gap-0.5"
                >
                  <span>
                    {p.environment.name}{" "}
                    <code className="font-mono text-xs">{p.deployment.ref}</code>
                  </span>
                  <span className="text-muted-foreground text-xs font-normal">
                    Reviewers:{" "}
                    {p.reviewers
                      .map((r) =>
                        r.reviewer && "login" in r.reviewer ? r.reviewer.login : r.reviewer?.name,
                      )
                      .join(", ")}
                    {p.wait_timer > 0 && ` · then waits ${p.wait_timer} min`}
                  </span>
                </FieldLabel>
              </Field>
            </li>
          ))}
        </ul>
        <Field>
          <FieldLabel htmlFor="review-comment">Comment</FieldLabel>
          <Textarea
            id="review-comment"
            placeholder="Leave a comment (optional)"
            value={comment}
            onChange={(e) => setComment(e.target.value)}
          />
        </Field>
        <DialogFooter>
          <Button
            variant="destructive"
            disabled={chosen.size === 0 || review.isPending}
            onClick={() => review.mutate("reject")}
          >
            Reject
          </Button>
          <Button
            disabled={chosen.size === 0 || review.isPending}
            onClick={() => review.mutate("approve")}
          >
            Approve and deploy
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---- New deployment ----

function NewDeployment({ onClose, ...repo }: Repo & { onClose: () => void }) {
  const queryClient = useQueryClient();
  const { data: environments } = useSuspenseQuery(environmentsQuery(repo));
  const { data: branches } = useSuspenseQuery(branchesQuery(repo));
  const { data: tags } = useSuspenseQuery(tagsQuery(repo));
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: {
      ref: "main",
      environment: environments[0]?.name ?? "production",
      description: "",
    },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      try {
        const d = await send(
          api.POST("/repos/{owner}/{repo}/deployments", {
            params: { path: repo },
            body: {
              ref: value.ref,
              environment: value.environment.trim(),
              description: value.description.trim() || null,
              auto_merge: false,
              required_contexts: [],
            },
          }),
        );
        // A 202 means GHES merged the default branch into the ref instead of deploying it.
        if (!("id" in d)) toast.info(d.message ?? "Merged the default branch; deploy again.");
        else toast.success(`Deployment ${d.id} to ${d.environment} queued`);
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: deploymentsKey(repo) }),
          queryClient.invalidateQueries({ queryKey: ["_t", "ui", ...deploymentsKey(repo)] }),
          queryClient.invalidateQueries({
            queryKey: ["repos", repo.owner, repo.repo, "environments"],
          }),
        ]);
        onClose();
      } catch (e) {
        if (e instanceof WriteError && e.fields.ref) {
          formApi.setFieldMeta(
            "ref",
            serverError(e.message === "Validation Failed" ? `Ref ${e.fields.ref}.` : e.message),
          );
        } else {
          setFailure(errorText(e));
        }
        toast.error("Deployment not created", { description: errorText(e) });
      }
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md" data-testid="deploy-dialog">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle>New deployment</DialogTitle>
            <DialogDescription>
              The environment's protection rules apply: reviewers hold it, a wait timer delays it.
            </DialogDescription>
          </DialogHeader>
          <form.Field name="ref">
            {(field) => {
              const errors = field.state.meta.errors.filter(Boolean).map(String);
              return (
                <Field data-invalid={errors.length > 0 || undefined}>
                  <FieldLabel htmlFor="deploy-ref">Ref</FieldLabel>
                  <NativeSelect
                    id="deploy-ref"
                    className="w-full"
                    value={field.state.value}
                    aria-invalid={errors.length > 0 || undefined}
                    onChange={(e) => field.handleChange(e.target.value)}
                  >
                    <NativeSelectOptGroup label="Branches">
                      {branches.map((b) => (
                        <NativeSelectOption key={b.name} value={b.name}>
                          {b.name}
                        </NativeSelectOption>
                      ))}
                    </NativeSelectOptGroup>
                    <NativeSelectOptGroup label="Tags">
                      {tags.map((t) => (
                        <NativeSelectOption key={t.name} value={t.name}>
                          {t.name}
                        </NativeSelectOption>
                      ))}
                    </NativeSelectOptGroup>
                  </NativeSelect>
                  {errors.length > 0 && <FieldError>{errors.join(" ")}</FieldError>}
                </Field>
              );
            }}
          </form.Field>
          <form.Field
            name="environment"
            validators={{
              onSubmit: ({ value }) => (value.trim() ? undefined : "Name an environment."),
            }}
          >
            {(field) => {
              const errors = field.state.meta.errors.filter(Boolean).map(String);
              const env = environments.find((e) => e.name === field.state.value);
              const rules = env?.protection_rules ?? [];
              return (
                <Field data-invalid={errors.length > 0 || undefined}>
                  <FieldLabel htmlFor="deploy-env">Environment</FieldLabel>
                  <Input
                    id="deploy-env"
                    list="deploy-env-names"
                    value={field.state.value}
                    onBlur={field.handleBlur}
                    onChange={(e) => field.handleChange(e.target.value)}
                  />
                  <datalist id="deploy-env-names">
                    {environments.map((e) => (
                      <option key={e.id} value={e.name} />
                    ))}
                  </datalist>
                  {errors.length > 0 ? (
                    <FieldError>{errors.join(" ")}</FieldError>
                  ) : (
                    <p className="text-muted-foreground flex flex-wrap gap-2 text-xs">
                      {!env &&
                        field.state.value.trim() &&
                        "A new environment is created on first deploy."}
                      {rules.map((r) =>
                        r.type === "required_reviewers" ? (
                          <span key={r.id} className="flex items-center gap-1">
                            <HugeiconsIcon
                              icon={UserGroupIcon}
                              strokeWidth={2}
                              className="size-3.5"
                            />
                            Needs review
                          </span>
                        ) : r.type === "wait_timer" && "wait_timer" in r ? (
                          <span key={r.id} className="flex items-center gap-1">
                            <HugeiconsIcon
                              icon={Timer02Icon}
                              strokeWidth={2}
                              className="size-3.5"
                            />
                            Waits {r.wait_timer} min
                          </span>
                        ) : r.type === "branch_policy" ? (
                          <span key={r.id}>Branch policy applies</span>
                        ) : null,
                      )}
                    </p>
                  )}
                </Field>
              );
            }}
          </form.Field>
          <form.Field name="description">
            {(field) => (
              <Field>
                <FieldLabel htmlFor="deploy-description">Description</FieldLabel>
                <Input
                  id="deploy-description"
                  placeholder="Optional"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                />
              </Field>
            )}
          </form.Field>
          {failure && (
            <Alert variant="destructive">
              <AlertTitle>Deployment not created</AlertTitle>
              <AlertDescription>{failure}</AlertDescription>
            </Alert>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" disabled={submitting}>
                  {submitting ? "Deploying…" : "Deploy"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
