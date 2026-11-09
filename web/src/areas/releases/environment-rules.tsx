import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon, UserGroupIcon, UserIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { UserAvatar } from "@/components/github";
import { Panel } from "@/components/panel";
import { Alert, AlertDescription } from "@/components/reui/alert";
import {
  NumberField,
  NumberFieldDecrement,
  NumberFieldGroup,
  NumberFieldIncrement,
  NumberFieldInput,
} from "@/components/reui/number-field";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { errorText, send } from "./api";
import { collaboratorsQuery, environmentsKey, environmentQuery, repoTeamsQuery } from "./queries";

type Env = { owner: string; repo: string; name: string };
type Environment = Schemas["environment"];

// A reviewer as the form holds it: who, and what GHES's PUT wants (type and id).
type Reviewer = { type: "User" | "Team"; id: number; label: string; avatar?: string };

// GHES returns no branch policy as null and one kind as exactly one true flag.
type BranchMode = "all" | "protected" | "custom";

function fromEnvironment(e: Environment) {
  const rules = e.protection_rules ?? [];
  const req = rules.find((r) => r.type === "required_reviewers");
  const timer = rules.find((r) => r.type === "wait_timer");
  const reviewers: Reviewer[] = [];
  if (req && "reviewers" in req) {
    for (const r of req.reviewers ?? []) {
      if (!r.reviewer) continue;
      reviewers.push(
        "login" in r.reviewer
          ? {
              type: "User",
              id: r.reviewer.id,
              label: r.reviewer.login,
              avatar: r.reviewer.avatar_url,
            }
          : { type: "Team", id: r.reviewer.id, label: r.reviewer.name },
      );
    }
  }
  const policy = e.deployment_branch_policy;
  return {
    reviewers,
    preventSelfReview: Boolean(req && "prevent_self_review" in req && req.prevent_self_review),
    waitTimer: timer && "wait_timer" in timer ? (timer.wait_timer ?? 0) : 0,
    waitEnabled: Boolean(timer),
    branchMode: (!policy
      ? "all"
      : policy.protected_branches
        ? "protected"
        : "custom") as BranchMode,
  };
}

export function ProtectionRules(env: Env) {
  const repo = { owner: env.owner, repo: env.repo };
  const query = environmentQuery(env);
  const { data: environment } = useSuspenseQuery(query);
  const { data: collaborators } = useSuspenseQuery(collaboratorsQuery(repo));
  const { data: teams } = useSuspenseQuery(repoTeamsQuery(repo));
  const queryClient = useQueryClient();
  const [failure, setFailure] = useState<string>();
  const initial = fromEnvironment(environment);

  const candidates: Reviewer[] = [
    ...collaborators.map((c) => ({
      type: "User" as const,
      id: c.id,
      label: c.login,
      avatar: c.avatar_url,
    })),
    ...teams.map((t) => ({ type: "Team" as const, id: t.id, label: t.name })),
  ];

  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const before = queryClient.getQueryData<Environment>(query.queryKey);
      // Show the saved rules at once; the PUT's answer replaces them.
      if (before) {
        queryClient.setQueryData<Environment>(query.queryKey, {
          ...before,
          deployment_branch_policy:
            value.branchMode === "all"
              ? null
              : {
                  protected_branches: value.branchMode === "protected",
                  custom_branch_policies: value.branchMode === "custom",
                },
        });
      }
      try {
        const saved = await send(
          api.PUT("/repos/{owner}/{repo}/environments/{environment_name}", {
            params: { path: { ...repo, environment_name: env.name } },
            body: {
              wait_timer: value.waitEnabled ? value.waitTimer : 0,
              prevent_self_review: value.preventSelfReview,
              reviewers: value.reviewers.map((r) => ({ type: r.type, id: r.id })),
              deployment_branch_policy:
                value.branchMode === "all"
                  ? null
                  : {
                      protected_branches: value.branchMode === "protected",
                      custom_branch_policies: value.branchMode === "custom",
                    },
            },
          }),
        );
        queryClient.setQueryData(query.queryKey, saved);
        formApi.reset(fromEnvironment(saved));
        toast.success("Protection rules saved");
      } catch (e) {
        if (before) queryClient.setQueryData(query.queryKey, before);
        setFailure(errorText(e));
        toast.error("Protection rules not saved", { description: errorText(e) });
      } finally {
        void queryClient.invalidateQueries({ queryKey: environmentsKey(repo) });
      }
    },
  });

  return (
    <form
      data-testid="protection-rules"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <Panel
        title="Deployment protection rules"
        actions={
          <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
            {([submitting, dirty]) => (
              <Button type="submit" size="sm" disabled={submitting || !dirty}>
                {submitting ? "Saving…" : "Save protection rules"}
              </Button>
            )}
          </form.Subscribe>
        }
      >
        <div className="flex flex-col gap-5">
          <form.Field
            name="reviewers"
            validators={{
              onChange: ({ value }) =>
                value.length > 6 ? "An environment can have at most 6 reviewers." : undefined,
            }}
          >
            {(field) => {
              const errors = field.state.meta.errors.filter(Boolean).map(String);
              const chosen = field.state.value;
              const key = (r: Reviewer) => `${r.type}:${r.id}`;
              const open = candidates.filter((c) => !chosen.some((r) => key(r) === key(c)));
              return (
                <Field data-invalid={errors.length > 0 || undefined}>
                  <FieldLabel>Required reviewers</FieldLabel>
                  <FieldDescription>
                    Up to 6 people or teams. One of them must approve a deployment before it runs.
                  </FieldDescription>
                  {chosen.length > 0 && (
                    <ul className="flex flex-wrap gap-1.5" data-testid="reviewers">
                      {chosen.map((r) => (
                        <li
                          key={key(r)}
                          className="bg-muted flex items-center gap-1.5 rounded-sm py-0.5 pr-0.5 pl-1.5 text-xs font-medium"
                          data-reviewer={r.label}
                        >
                          {r.type === "User" ? (
                            <UserAvatar user={{ login: r.label, avatar_url: r.avatar ?? "" }} />
                          ) : (
                            <HugeiconsIcon
                              icon={UserGroupIcon}
                              strokeWidth={2}
                              className="size-3.5"
                            />
                          )}
                          {r.label}
                          <Button
                            type="button"
                            size="icon-xs"
                            variant="ghost"
                            aria-label={`Remove ${r.label}`}
                            onClick={() =>
                              field.handleChange(chosen.filter((x) => key(x) !== key(r)))
                            }
                          >
                            <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
                          </Button>
                        </li>
                      ))}
                    </ul>
                  )}
                  <Combobox<Reviewer>
                    items={open}
                    value={null}
                    itemToStringLabel={(r) => r.label}
                    onValueChange={(r) => {
                      if (r) field.handleChange([...chosen, r]);
                    }}
                  >
                    <ComboboxInput
                      placeholder="Search for people or teams"
                      aria-label="Add a reviewer"
                      className="w-full max-w-sm"
                    />
                    <ComboboxContent>
                      <ComboboxEmpty>No one else to add.</ComboboxEmpty>
                      <ComboboxList>
                        {(r: Reviewer) => (
                          <ComboboxItem key={key(r)} value={r}>
                            <HugeiconsIcon
                              icon={r.type === "User" ? UserIcon : UserGroupIcon}
                              strokeWidth={2}
                            />
                            {r.label}
                            <span className="text-muted-foreground ml-auto text-xs">
                              {r.type === "User" ? "Person" : "Team"}
                            </span>
                          </ComboboxItem>
                        )}
                      </ComboboxList>
                    </ComboboxContent>
                  </Combobox>
                  {errors.length > 0 && <FieldError>{errors.join(" ")}</FieldError>}
                </Field>
              );
            }}
          </form.Field>

          <form.Field name="preventSelfReview">
            {(field) => (
              <Field orientation="horizontal">
                <Checkbox
                  id="env-self-review"
                  checked={field.state.value}
                  onCheckedChange={(v) => field.handleChange(v)}
                />
                <FieldLabel htmlFor="env-self-review" className="flex-col items-start gap-0.5">
                  Prevent self-review
                  <span className="text-muted-foreground text-xs font-normal">
                    Whoever starts a deployment cannot approve it.
                  </span>
                </FieldLabel>
              </Field>
            )}
          </form.Field>

          <div className="flex flex-col gap-2">
            <form.Field name="waitEnabled">
              {(field) => (
                <Field orientation="horizontal">
                  <Checkbox
                    id="env-wait"
                    checked={field.state.value}
                    onCheckedChange={(v) => field.handleChange(v)}
                  />
                  <FieldLabel htmlFor="env-wait" className="flex-col items-start gap-0.5">
                    Wait timer
                    <span className="text-muted-foreground text-xs font-normal">
                      Hold each deployment for a set time before it runs.
                    </span>
                  </FieldLabel>
                </Field>
              )}
            </form.Field>
            <form.Subscribe selector={(s) => s.values.waitEnabled}>
              {(enabled) =>
                enabled && (
                  <form.Field
                    name="waitTimer"
                    validators={{
                      onChange: ({ value }) =>
                        !Number.isInteger(value) || value < 0 || value > 43200
                          ? "Wait from 0 to 43,200 minutes (30 days)."
                          : undefined,
                    }}
                  >
                    {(field) => {
                      const errors = field.state.meta.errors.filter(Boolean).map(String);
                      return (
                        <Field
                          className="ml-6 max-w-48"
                          data-invalid={errors.length > 0 || undefined}
                        >
                          <NumberField
                            value={field.state.value}
                            min={0}
                            max={43200}
                            step={1}
                            onValueChange={(v) => field.handleChange(v ?? 0)}
                          >
                            <NumberFieldGroup>
                              <NumberFieldDecrement />
                              <NumberFieldInput aria-label="Wait timer in minutes" />
                              <NumberFieldIncrement />
                            </NumberFieldGroup>
                          </NumberField>
                          {errors.length > 0 ? (
                            <FieldError>{errors.join(" ")}</FieldError>
                          ) : (
                            <FieldDescription>Minutes</FieldDescription>
                          )}
                        </Field>
                      );
                    }}
                  </form.Field>
                )
              }
            </form.Subscribe>
          </div>

          <form.Field name="branchMode">
            {(field) => (
              <Field>
                <FieldLabel>Deployment branches and tags</FieldLabel>
                <RadioGroup
                  value={field.state.value}
                  onValueChange={(v) => field.handleChange(v as BranchMode)}
                  aria-label="Deployment branches and tags"
                >
                  {(
                    [
                      ["all", "No restriction", "Any branch or tag can deploy."],
                      [
                        "protected",
                        "Protected branches only",
                        "Only branches with protection rules.",
                      ],
                      [
                        "custom",
                        "Selected branches and tags",
                        "Only refs that match the name patterns below.",
                      ],
                    ] as const
                  ).map(([v, title, hint]) => (
                    <Field key={v} orientation="horizontal">
                      <RadioGroupItem value={v} id={`env-branches-${v}`} />
                      <FieldLabel
                        htmlFor={`env-branches-${v}`}
                        className="flex-col items-start gap-0.5"
                      >
                        {title}
                        <span className="text-muted-foreground text-xs font-normal">{hint}</span>
                      </FieldLabel>
                    </Field>
                  ))}
                </RadioGroup>
              </Field>
            )}
          </form.Field>

          {failure && (
            <Alert variant="destructive" data-testid="rules-failure">
              <AlertDescription>{failure}</AlertDescription>
            </Alert>
          )}
        </div>
      </Panel>
    </form>
  );
}
