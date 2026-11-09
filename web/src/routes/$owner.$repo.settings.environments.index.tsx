import { useState } from "react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CloudServerIcon,
  Delete02Icon,
  GitBranchIcon,
  PlusSignIcon,
  Timer02Icon,
  UserGroupIcon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { errorText, send } from "@/areas/releases/api";
import { environmentsKey, environmentsQuery } from "@/areas/releases/queries";
import { useOptimisticWrite } from "@/areas/releases/write";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription } from "@/components/reui/alert";
import {
  AlertDialog,
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
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";

export const Route = createFileRoute("/$owner/$repo/settings/environments/")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(environmentsQuery(params)),
  component: EnvironmentsPage,
});

type Environment = Schemas["environment"];

function EnvironmentsPage() {
  const [creating, setCreating] = useState(false);
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Environments"
        description="Environments are deployment targets, each with its own protection rules, secrets, and variables."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
            New environment
          </Button>
        }
      />
      <Region name="environments" rows={4}>
        <EnvironmentList />
      </Region>
      {creating && <CreateEnvironment onClose={() => setCreating(false)} />}
    </div>
  );
}

function rulesOf(e: Environment) {
  const rules = e.protection_rules ?? [];
  const reviewers = rules.find((r) => r.type === "required_reviewers");
  const timer = rules.find((r) => r.type === "wait_timer");
  return {
    reviewers: reviewers && "reviewers" in reviewers ? (reviewers.reviewers?.length ?? 0) : 0,
    wait: timer && "wait_timer" in timer ? (timer.wait_timer ?? 0) : 0,
    branches: e.deployment_branch_policy
      ? e.deployment_branch_policy.protected_branches
        ? "Protected branches"
        : "Selected branches and tags"
      : null,
  };
}

function EnvironmentList() {
  const params = Route.useParams();
  const query = environmentsQuery(params);
  const { data: environments } = useSuspenseQuery(query);
  const [deleting, setDeleting] = useState<Environment>();
  const remove = useOptimisticWrite({
    key: query.queryKey,
    invalidate: [environmentsKey(params), ["repos", params.owner, params.repo, "deployments"]],
    failure: "Environment not deleted",
    apply: (list: Environment[], e: Environment) => list.filter((x) => x.id !== e.id),
    write: (e: Environment) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/environments/{environment_name}", {
          params: { path: { ...params, environment_name: e.name } },
        }),
      ),
    onSuccess: (_r, e) => toast.success(`Deleted environment ${e.name}`),
  });

  if (environments.length === 0) {
    return (
      <p
        className="text-muted-foreground border px-4 py-10 text-center text-sm"
        data-testid="environments-empty"
      >
        There are no environments yet. A deployment creates its environment, or add one here.
      </p>
    );
  }
  return (
    <Panel flush title={`${environments.length} environments`}>
      <ul className="divide-y" data-testid="environment-list">
        {environments.map((e) => {
          const r = rulesOf(e);
          return (
            <li
              key={e.id}
              className="flex items-center gap-3 px-3 py-2.5 text-sm"
              data-environment={e.name}
            >
              <HugeiconsIcon
                icon={CloudServerIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <div className="min-w-0 flex-1">
                <Link
                  to="/$owner/$repo/settings/environments/$name"
                  params={{ ...params, name: e.name }}
                  className="font-semibold hover:underline"
                >
                  {e.name}
                </Link>
                <div className="text-muted-foreground mt-0.5 flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
                  {r.reviewers > 0 && (
                    <span className="flex items-center gap-1">
                      <HugeiconsIcon icon={UserGroupIcon} strokeWidth={2} className="size-3.5" />
                      {r.reviewers} required {r.reviewers === 1 ? "reviewer" : "reviewers"}
                    </span>
                  )}
                  {r.wait > 0 && (
                    <span className="flex items-center gap-1">
                      <HugeiconsIcon icon={Timer02Icon} strokeWidth={2} className="size-3.5" />
                      Wait {r.wait} min
                    </span>
                  )}
                  {r.branches && (
                    <span className="flex items-center gap-1">
                      <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} className="size-3.5" />
                      {r.branches}
                    </span>
                  )}
                  {!r.reviewers && !r.wait && !r.branches && <span>No protection rules</span>}
                </div>
              </div>
              <Button
                size="icon-sm"
                variant="ghost"
                aria-label={`Delete ${e.name}`}
                onClick={() => setDeleting(e)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          );
        })}
      </ul>
      <AlertDialog open={Boolean(deleting)} onOpenChange={(o) => !o && setDeleting(undefined)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete {deleting?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This deletes the environment, its protection rules, secrets, and variables, and
              cancels any deployment waiting for its review.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                if (deleting) remove.mutate(deleting);
                setDeleting(undefined);
              }}
            >
              Delete environment
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}

const BAD_NAME = /[/:"<>|?*\\,']/;

function CreateEnvironment({ onClose }: { onClose: () => void }) {
  const params = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [failure, setFailure] = useState<string>();
  const form = useForm({
    defaultValues: { name: "" },
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const name = value.name.trim();
      const existing = queryClient.getQueryData<Environment[]>(environmentsQuery(params).queryKey);
      if (existing?.some((e) => e.name.toLowerCase() === name.toLowerCase())) {
        formApi.setFieldMeta("name", (m) => ({
          ...m,
          errorMap: { ...m.errorMap, onSubmit: "An environment with this name exists." },
        }));
        return;
      }
      try {
        const created = await send(
          api.PUT("/repos/{owner}/{repo}/environments/{environment_name}", {
            params: { path: { ...params, environment_name: name } },
            body: {},
          }),
        );
        await queryClient.invalidateQueries({ queryKey: environmentsKey(params) });
        toast.success(`Created environment ${created.name}`);
        void navigate({
          to: "/$owner/$repo/settings/environments/$name",
          params: { ...params, name: created.name },
        });
      } catch (e) {
        // GHES answers a bad environment name with a basic-error 422 and no field list.
        const status = e instanceof Error && "status" in e ? e.status : 0;
        if (status === 422) {
          formApi.setFieldMeta("name", (m) => ({
            ...m,
            errorMap: { ...m.errorMap, onSubmit: errorText(e) },
          }));
        } else setFailure(errorText(e));
        toast.error("Environment not created", { description: errorText(e) });
      }
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <form
          className="flex flex-col gap-4"
          data-testid="create-environment"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <DialogHeader>
            <DialogTitle>New environment</DialogTitle>
            <DialogDescription>You set its protection rules on the next page.</DialogDescription>
          </DialogHeader>
          <form.Field
            name="name"
            validators={{
              onSubmit: ({ value }) =>
                !value.trim()
                  ? "Name the environment."
                  : value.length > 255
                    ? "Keep the name under 256 characters."
                    : undefined,
              onChange: ({ value }) =>
                BAD_NAME.test(value)
                  ? "A name cannot contain / : \" < > | ? * \\ , or '."
                  : undefined,
            }}
          >
            {(field) => {
              const errors = field.state.meta.errors.filter(Boolean).map(String);
              return (
                <Field data-invalid={errors.length > 0 || undefined}>
                  <FieldLabel htmlFor="env-name">Name</FieldLabel>
                  <Input
                    id="env-name"
                    autoFocus
                    placeholder="e.g. production"
                    value={field.state.value}
                    aria-invalid={errors.length > 0 || undefined}
                    onBlur={field.handleBlur}
                    onChange={(e) => field.handleChange(e.target.value)}
                  />
                  {errors.length > 0 ? (
                    <FieldError>{errors.join(" ")}</FieldError>
                  ) : (
                    <FieldDescription>Names are not case sensitive.</FieldDescription>
                  )}
                </Field>
              );
            }}
          </form.Field>
          {failure && (
            <Alert variant="destructive">
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
                  {submitting ? "Creating…" : "Configure environment"}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
