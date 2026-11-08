import { createFileRoute } from "@tanstack/react-router";
import { useForm } from "@tanstack/react-form";
import { useSuspenseQuery } from "@tanstack/react-query";
import { api } from "@/api/client";
import { actionsPermissionsQuery, type ActionsPermissions } from "@/areas/actions/queries";
import { fieldError, send, useWrite } from "@/areas/actions/write";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";

export const Route = createFileRoute("/$owner/$repo/settings/actions/general")({
  loader: ({ context, params }) =>
    void context.queryClient.prefetchQuery(actionsPermissionsQuery(params)),
  component: ActionsGeneral,
});

function ActionsGeneral() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Actions permissions"
        description="Which actions this repository's workflows may use, and what they may do."
      />
      <Region name="actions permissions" rows={8}>
        <PermissionsForm />
      </Region>
    </div>
  );
}

type Policy = "disabled" | "all" | "local_only" | "selected";

type Values = {
  policy: Policy;
  githubOwned: boolean;
  verified: boolean;
  patterns: string;
  shaPinning: boolean;
  token: "read" | "write";
  approveReviews: boolean;
  fork:
    | "first_time_contributors_new_to_github"
    | "first_time_contributors"
    | "all_external_contributors";
  retention: number;
};

const POLICIES: { value: Policy; label: string; help: string }[] = [
  {
    value: "all",
    label: "Allow all actions and reusable workflows",
    help: "Any action or reusable workflow can be used.",
  },
  {
    value: "local_only",
    label: "Allow graftyard actions and reusable workflows",
    help: "Only actions in repositories of this organization.",
  },
  {
    value: "selected",
    label: "Allow graftyard, and select non-graftyard, actions and reusable workflows",
    help: "Actions you list below, plus this organization's.",
  },
  { value: "disabled", label: "Disable actions", help: "No workflow in this repository runs." },
];

const FORK: { value: Values["fork"]; label: string }[] = [
  {
    value: "first_time_contributors_new_to_github",
    label: "Require approval for first-time contributors who are new to GitHub",
  },
  { value: "first_time_contributors", label: "Require approval for first-time contributors" },
  { value: "all_external_contributors", label: "Require approval for all external contributors" },
];

function toValues(p: ActionsPermissions): Values {
  return {
    policy: p.permissions.enabled ? (p.permissions.allowed_actions ?? "all") : "disabled",
    githubOwned: p.selected.github_owned_allowed ?? true,
    verified: p.selected.verified_allowed ?? false,
    patterns: (p.selected.patterns_allowed ?? []).join("\n"),
    shaPinning: p.permissions.sha_pinning_required ?? false,
    token: p.workflow.default_workflow_permissions,
    approveReviews: p.workflow.can_approve_pull_request_reviews,
    fork: p.fork.approval_policy,
    retention: p.retention.days,
  };
}

function PermissionsForm() {
  const { owner, repo } = Route.useParams();
  const { data } = useSuspenseQuery(actionsPermissionsQuery({ owner, repo }));
  const path = { params: { path: { owner, repo } } };

  // One save writes each GHES setting in turn; the first failure stops the rest.
  const save = useWrite({
    run: async (v: Values) => {
      await send(
        api.PUT("/repos/{owner}/{repo}/actions/permissions", {
          ...path,
          body: {
            enabled: v.policy !== "disabled",
            allowed_actions: v.policy === "disabled" ? undefined : v.policy,
            sha_pinning_required: v.shaPinning,
          },
        }),
      );
      if (v.policy === "selected")
        await send(
          api.PUT("/repos/{owner}/{repo}/actions/permissions/selected-actions", {
            ...path,
            body: {
              github_owned_allowed: v.githubOwned,
              verified_allowed: v.verified,
              patterns_allowed: v.patterns
                .split(/[\n,]/)
                .map((p) => p.trim())
                .filter(Boolean),
            },
          }),
        );
      await send(
        api.PUT("/repos/{owner}/{repo}/actions/permissions/workflow", {
          ...path,
          body: {
            default_workflow_permissions: v.token,
            can_approve_pull_request_reviews: v.approveReviews,
          },
        }),
      );
      await send(
        api.PUT("/repos/{owner}/{repo}/actions/permissions/fork-pr-contributor-approval", {
          ...path,
          body: { approval_policy: v.fork },
        }),
      );
      await send(
        api.PUT("/repos/{owner}/{repo}/actions/permissions/artifact-and-log-retention", {
          ...path,
          body: { days: v.retention },
        }),
      );
    },
    invalidates: () => [actionsPermissionsQuery({ owner, repo }).queryKey],
    success: () => "Actions settings saved",
    failure: "Could not save the Actions settings",
  });

  const form = useForm({
    defaultValues: toValues(data),
    // Saved: the values become the new baseline. Failed: the edits stay in the form, and
    // the refetched settings show what the server kept.
    onSubmit: async ({ value, formApi }) => {
      await save.mutateAsync(value).then(
        () => formApi.reset(value),
        () => undefined,
      );
    },
  });

  return (
    <form
      className="flex flex-col gap-4"
      data-testid="actions-permissions"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <Panel title="Actions permissions">
        <form.Field name="policy">
          {(field) => (
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as Policy)}
              className="gap-3"
            >
              {POLICIES.map((p) => (
                <Field key={p.value} orientation="horizontal">
                  <RadioGroupItem value={p.value} id={`policy-${p.value}`} />
                  <div>
                    <FieldLabel htmlFor={`policy-${p.value}`}>{p.label}</FieldLabel>
                    <FieldDescription className="text-xs">{p.help}</FieldDescription>
                  </div>
                </Field>
              ))}
            </RadioGroup>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => s.values.policy}>
          {(policy) =>
            policy === "selected" && (
              <div
                className="mt-4 ml-6 flex flex-col gap-3 border-l pl-4"
                data-testid="selected-actions"
              >
                <form.Field name="githubOwned">
                  {(f) => (
                    <Field orientation="horizontal">
                      <Checkbox
                        id="gh-owned"
                        checked={f.state.value}
                        onCheckedChange={(on) => f.handleChange(on === true)}
                      />
                      <FieldLabel htmlFor="gh-owned">Allow actions created by GitHub</FieldLabel>
                    </Field>
                  )}
                </form.Field>
                <form.Field name="verified">
                  {(f) => (
                    <Field orientation="horizontal">
                      <Checkbox
                        id="verified"
                        checked={f.state.value}
                        onCheckedChange={(on) => f.handleChange(on === true)}
                      />
                      <FieldLabel htmlFor="verified">
                        Allow actions by Marketplace verified creators
                      </FieldLabel>
                    </Field>
                  )}
                </form.Field>
                <form.Field
                  name="patterns"
                  validators={{
                    onChange: ({ value }) => {
                      const bad = value
                        .split(/[\n,]/)
                        .map((p) => p.trim())
                        .filter(Boolean)
                        .find((p) => !/^[\w.-]+(\/[\w.*-]+)*(@[\w.*-]+)?$/.test(p));
                      return bad ? `"${bad}" is not an owner/repo@ref pattern` : undefined;
                    },
                  }}
                >
                  {(f) => {
                    const error =
                      f.state.meta.errors[0] ?? fieldError(save.error, "patterns_allowed");
                    return (
                      <Field data-invalid={Boolean(error) || undefined}>
                        <FieldLabel htmlFor="patterns">
                          Allow specified actions and reusable workflows
                        </FieldLabel>
                        <Textarea
                          id="patterns"
                          className="min-h-20 rounded-sm font-mono text-xs"
                          value={f.state.value}
                          aria-invalid={Boolean(error) || undefined}
                          onChange={(e) => f.handleChange(e.target.value)}
                        />
                        <FieldDescription className="text-xs">
                          One per line or comma separated, such as <code>octo-org/octo-repo@*</code>
                          .
                        </FieldDescription>
                        {error && <FieldError>{String(error)}</FieldError>}
                      </Field>
                    );
                  }}
                </form.Field>
              </div>
            )
          }
        </form.Subscribe>
        <form.Field name="shaPinning">
          {(f) => (
            <Field orientation="horizontal" className="mt-4">
              <Checkbox
                id="sha-pin"
                checked={f.state.value}
                onCheckedChange={(on) => f.handleChange(on === true)}
              />
              <FieldLabel htmlFor="sha-pin">
                Require actions to be pinned to a full-length commit SHA
              </FieldLabel>
            </Field>
          )}
        </form.Field>
      </Panel>

      <Panel title="Workflow permissions">
        <form.Field name="token">
          {(f) => (
            <RadioGroup
              value={f.state.value}
              onValueChange={(v) => f.handleChange(v as Values["token"])}
              className="gap-3"
            >
              <Field orientation="horizontal">
                <RadioGroupItem value="write" id="token-write" />
                <div>
                  <FieldLabel htmlFor="token-write">Read and write permissions</FieldLabel>
                  <FieldDescription className="text-xs">
                    The GITHUB_TOKEN can read and write every scope.
                  </FieldDescription>
                </div>
              </Field>
              <Field orientation="horizontal">
                <RadioGroupItem value="read" id="token-read" />
                <div>
                  <FieldLabel htmlFor="token-read">
                    Read repository contents and packages permissions
                  </FieldLabel>
                  <FieldDescription className="text-xs">
                    A workflow asks for more with its own permissions key.
                  </FieldDescription>
                </div>
              </Field>
            </RadioGroup>
          )}
        </form.Field>
        <form.Field name="approveReviews">
          {(f) => (
            <Field orientation="horizontal" className="mt-4">
              <Checkbox
                id="approve"
                checked={f.state.value}
                onCheckedChange={(on) => f.handleChange(on === true)}
              />
              <FieldLabel htmlFor="approve">
                Allow GitHub Actions to create and approve pull requests
              </FieldLabel>
            </Field>
          )}
        </form.Field>
      </Panel>

      <Panel title="Fork pull request workflows from outside collaborators">
        <form.Field name="fork">
          {(f) => (
            <RadioGroup
              value={f.state.value}
              onValueChange={(v) => f.handleChange(v as Values["fork"])}
              className="gap-3"
            >
              {FORK.map((o) => (
                <Field key={o.value} orientation="horizontal">
                  <RadioGroupItem value={o.value} id={`fork-${o.value}`} />
                  <FieldLabel htmlFor={`fork-${o.value}`}>{o.label}</FieldLabel>
                </Field>
              ))}
            </RadioGroup>
          )}
        </form.Field>
      </Panel>

      <Panel title="Artifact and log retention">
        <form.Field
          name="retention"
          validators={{
            onChange: ({ value }) =>
              Number.isInteger(value) && value >= 1 && value <= data.retention.maximum_allowed_days
                ? undefined
                : `Enter a whole number of days from 1 to ${data.retention.maximum_allowed_days}`,
          }}
        >
          {(f) => {
            const error = f.state.meta.errors[0] ?? fieldError(save.error, "days");
            return (
              <Field data-invalid={Boolean(error) || undefined} className="max-w-xs">
                <FieldLabel htmlFor="retention">Days</FieldLabel>
                <Input
                  id="retention"
                  type="number"
                  className="rounded-sm"
                  value={Number.isNaN(f.state.value) ? "" : f.state.value}
                  aria-invalid={Boolean(error) || undefined}
                  onChange={(e) => f.handleChange(e.target.valueAsNumber)}
                />
                <FieldDescription className="text-xs">
                  At most {data.retention.maximum_allowed_days} days.
                </FieldDescription>
                {error && <FieldError>{String(error)}</FieldError>}
              </Field>
            );
          }}
        </form.Field>
      </Panel>

      <div className="flex justify-end gap-2">
        <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty, s.canSubmit] as const}>
          {([submitting, dirty, canSubmit]) => (
            <>
              <Button
                type="button"
                variant="outline"
                disabled={!dirty || submitting}
                onClick={() => form.reset()}
              >
                Discard changes
              </Button>
              <Button type="submit" disabled={!dirty || submitting || !canSubmit}>
                {submitting ? "Saving…" : "Save"}
              </Button>
            </>
          )}
        </form.Subscribe>
      </div>
    </form>
  );
}
