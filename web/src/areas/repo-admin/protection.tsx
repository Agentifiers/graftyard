import { useState } from "react";
import { useMutation, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { Link, useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Cancel01Icon, GitBranchIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { AdminError, call, failed, optimistic, refetch } from "./client";
import { matchesPattern } from "./domain";
import { EmptyRow, FieldMessage, Suggest, TypedConfirm, type RepoParams } from "./kit";
import type { ProtectionRule } from "./private-api";
import { branchesQuery, checkNamesQuery, protectionRulesQuery, usersQuery } from "./queries";

// ---- The rules list ----

function summary(p: Schemas["branch-protection"]) {
  const out: string[] = [];
  const reviews = p.required_pull_request_reviews;
  if (reviews)
    out.push(
      `${reviews.required_approving_review_count ?? 1} approving review${reviews.required_approving_review_count === 1 ? "" : "s"}`,
    );
  if (p.required_status_checks)
    out.push(`${p.required_status_checks.contexts.length} required checks`);
  if (p.required_linear_history?.enabled) out.push("linear history");
  if (p.required_signatures?.enabled) out.push("signed commits");
  if (p.restrictions) out.push("restricted pushes");
  return out.join(" · ") || "No requirements";
}

export function ProtectionRules({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(protectionRulesQuery(params));
  if (data.length === 0) {
    return (
      <EmptyRow>
        No branch protection rules. Protect important branches from force pushes and deletion.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="protection-rules">
      {data.map((r) => (
        <li
          key={r.pattern}
          className="flex items-center gap-3 px-4 py-2.5"
          data-pattern={r.pattern}
        >
          <HugeiconsIcon
            icon={GitBranchIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <Link
              to="/$owner/$repo/settings/branches/$pattern"
              params={{ ...params, pattern: r.pattern }}
              className="font-mono text-sm font-semibold hover:underline"
            >
              {r.pattern}
            </Link>
            <p className="text-muted-foreground text-xs">
              {summary(r.protection)} · Applies to {r.matching_branches.length} branch
              {r.matching_branches.length === 1 ? "" : "es"} · created <Ago at={r.created_at} />
            </p>
          </div>
          <Button
            variant="outline"
            size="sm"
            render={
              <Link
                to="/$owner/$repo/settings/branches/$pattern"
                params={{ ...params, pattern: r.pattern }}
              />
            }
          >
            Edit
          </Button>
        </li>
      ))}
    </ul>
  );
}

// ---- The rule editor ----

type Values = {
  pattern: string;
  requireReviews: boolean;
  reviewCount: number;
  dismissStale: boolean;
  codeOwners: boolean;
  lastPush: boolean;
  requireChecks: boolean;
  strict: boolean;
  checks: string[];
  conversations: boolean;
  signatures: boolean;
  linear: boolean;
  enforceAdmins: boolean;
  restrict: boolean;
  pushUsers: string[];
  forcePushes: boolean;
  deletions: boolean;
};

function valuesOf(rule: ProtectionRule | undefined): Values {
  const p = rule?.protection;
  const reviews = p?.required_pull_request_reviews;
  return {
    pattern: rule?.pattern ?? "",
    requireReviews: Boolean(reviews) || !rule,
    reviewCount: reviews?.required_approving_review_count ?? 1,
    dismissStale: reviews?.dismiss_stale_reviews ?? false,
    codeOwners: reviews?.require_code_owner_reviews ?? false,
    lastPush: reviews?.require_last_push_approval ?? false,
    requireChecks: Boolean(p?.required_status_checks),
    strict: p?.required_status_checks?.strict ?? false,
    checks: p?.required_status_checks?.contexts ?? [],
    conversations: p?.required_conversation_resolution?.enabled ?? false,
    signatures: p?.required_signatures?.enabled ?? false,
    linear: p?.required_linear_history?.enabled ?? false,
    enforceAdmins: p?.enforce_admins?.enabled ?? false,
    restrict: Boolean(p?.restrictions),
    pushUsers: p?.restrictions?.users.flatMap((u) => (u.login ? [u.login] : [])) ?? [],
    forcePushes: p?.allow_force_pushes?.enabled ?? false,
    deletions: p?.allow_deletions?.enabled ?? false,
  };
}

async function save(params: RepoParams, v: Values, previous: string | undefined) {
  const branch = v.pattern.trim();
  await call(
    api.PUT("/repos/{owner}/{repo}/branches/{branch}/protection", {
      params: { path: { ...params, branch } },
      body: {
        required_status_checks: v.requireChecks ? { strict: v.strict, contexts: v.checks } : null,
        enforce_admins: v.enforceAdmins,
        required_pull_request_reviews: v.requireReviews
          ? {
              required_approving_review_count: v.reviewCount,
              dismiss_stale_reviews: v.dismissStale,
              require_code_owner_reviews: v.codeOwners,
              require_last_push_approval: v.lastPush,
            }
          : null,
        restrictions: v.restrict ? { users: v.pushUsers, teams: [] } : null,
        required_linear_history: v.linear,
        allow_force_pushes: v.forcePushes,
        allow_deletions: v.deletions,
        required_conversation_resolution: v.conversations,
      },
    }),
  );
  // Signed commits have their own endpoint.
  const sig = { params: { path: { ...params, branch } } };
  await call(
    v.signatures
      ? api.POST("/repos/{owner}/{repo}/branches/{branch}/protection/required_signatures", sig)
      : api.DELETE("/repos/{owner}/{repo}/branches/{branch}/protection/required_signatures", sig),
  );
  // A renamed pattern replaces the old rule.
  if (previous && previous !== branch) {
    await call(
      api.DELETE("/repos/{owner}/{repo}/branches/{branch}/protection", {
        params: { path: { ...params, branch: previous } },
      }),
    );
  }
}

export function ProtectionEditor({ params, pattern }: { params: RepoParams; pattern: string }) {
  const { data: rules } = useSuspenseQuery(protectionRulesQuery(params));
  const { data: branches } = useSuspenseQuery(branchesQuery(params));
  // Recent check names only feed suggestions; checks can be typed in, so a failed read
  // leaves the editor usable with no suggestions.
  const checks = useQuery(checkNamesQuery(params));
  const checkNames = checks.data ?? [];
  const { data: people } = useSuspenseQuery(usersQuery());
  const navigate = useNavigate();
  const isNew = pattern === "new";
  const rule = rules.find((r) => r.pattern === pattern);
  const [failure, setFailure] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const listKey = protectionRulesQuery(params).queryKey;

  const form = useForm({
    defaultValues: valuesOf(rule),
    onSubmit: async ({ value }) => {
      setFailure(undefined);
      try {
        await save(params, value, rule?.pattern);
        toast.success(isNew ? `Protected ${value.pattern}` : `Saved ${value.pattern}`);
        await refetch(listKey, branchesQuery(params).queryKey);
        void navigate({ to: "/$owner/$repo/settings/branches", params });
      } catch (e) {
        setFailure(e instanceof AdminError ? e.message : String(e));
        failed("Rule not saved")(e);
        void refetch(listKey);
      }
    },
  });

  const del = useMutation({
    mutationFn: () =>
      call(
        api.DELETE("/repos/{owner}/{repo}/branches/{branch}/protection", {
          params: { path: { ...params, branch: pattern } },
        }),
      ),
    onMutate: () =>
      optimistic<ProtectionRule[]>(listKey, (old) => old.filter((r) => r.pattern !== pattern)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Rule not deleted")(e);
    },
    onSuccess: () => {
      toast.success(`Deleted the rule for ${pattern}`);
      void navigate({ to: "/$owner/$repo/settings/branches", params });
    },
    onSettled: () => refetch(listKey),
  });

  if (!isNew && !rule) {
    return (
      <EmptyRow>
        No rule protects <code className="font-mono">{pattern}</code>.{" "}
        <Link
          to="/$owner/$repo/settings/branches/$pattern"
          params={{ ...params, pattern: "new" }}
          className="text-primary hover:underline"
        >
          Add one
        </Link>
        .
      </EmptyRow>
    );
  }

  const branchNames = branches.map((b) => b.name);
  return (
    <form
      className="flex flex-col gap-0 divide-y"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="protection-editor"
    >
      <div className="flex flex-col gap-2 px-4 py-3">
        <form.Field
          name="pattern"
          validators={{
            onChange: ({ value }) =>
              !value.trim()
                ? "A branch name pattern is required."
                : rules.some((r) => r.pattern === value.trim() && r.pattern !== rule?.pattern)
                  ? "A rule for this pattern already exists."
                  : undefined,
          }}
        >
          {(field) => {
            const matches = field.state.value.trim()
              ? branchNames.filter((b) => matchesPattern(field.state.value.trim(), b))
              : [];
            return (
              <>
                <label htmlFor="pattern" className="text-sm font-medium">
                  Branch name pattern
                </label>
                <Input
                  id="pattern"
                  className="max-w-md font-mono"
                  placeholder="main or release/*"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                  aria-invalid={field.state.meta.errors.length > 0 || undefined}
                />
                <FieldMessage field={field} />
                <p className="text-muted-foreground text-xs" data-testid="pattern-matches">
                  {field.state.value.trim()
                    ? matches.length
                      ? `Applies to ${matches.length} branch${matches.length === 1 ? "" : "es"}: ${matches.join(", ")}`
                      : "Applies to no branches yet."
                    : "Use * to match within a path segment and ** across segments."}
                </p>
              </>
            );
          }}
        </form.Field>
      </div>

      <Section>
        <Check
          form={form}
          name="requireReviews"
          title="Require a pull request before merging"
          description="Commits go to a branch other than this one and merge through a pull request."
        >
          <form.Subscribe selector={(s) => s.values.requireReviews}>
            {(on) =>
              on && (
                <div className="flex flex-col gap-2 pl-6">
                  <form.Field
                    name="reviewCount"
                    validators={{
                      onChange: ({ value }) =>
                        value < 0 || value > 6 ? "Choose 0 to 6 approvals." : undefined,
                    }}
                  >
                    {(field) => (
                      <label className="flex items-center gap-2 text-sm">
                        Required approvals
                        <NativeSelect
                          size="sm"
                          aria-label="Required approvals"
                          value={String(field.state.value)}
                          onChange={(e) => field.handleChange(Number(e.target.value))}
                        >
                          {[0, 1, 2, 3, 4, 5, 6].map((n) => (
                            <NativeSelectOption key={n} value={n}>
                              {n}
                            </NativeSelectOption>
                          ))}
                        </NativeSelect>
                      </label>
                    )}
                  </form.Field>
                  <Check
                    form={form}
                    name="dismissStale"
                    title="Dismiss stale pull request approvals when new commits are pushed"
                  />
                  <Check
                    form={form}
                    name="codeOwners"
                    title="Require review from Code Owners"
                    description="An approval from a code owner is needed when a change touches files they own in CODEOWNERS."
                  />
                  <Check
                    form={form}
                    name="lastPush"
                    title="Require approval of the most recent reviewable push"
                  />
                </div>
              )
            }
          </form.Subscribe>
        </Check>
      </Section>

      <Section>
        <Check
          form={form}
          name="requireChecks"
          title="Require status checks to pass before merging"
        >
          <form.Subscribe selector={(s) => s.values.requireChecks}>
            {(on) =>
              on && (
                <div className="flex flex-col gap-2 pl-6">
                  <Check
                    form={form}
                    name="strict"
                    title="Require branches to be up to date before merging"
                  />
                  <form.Field
                    name="checks"
                    validators={{
                      onSubmit: ({ value, fieldApi }) =>
                        fieldApi.form.getFieldValue("requireChecks") && value.length === 0
                          ? "Add at least one required check."
                          : undefined,
                    }}
                  >
                    {(field) => (
                      <div className="flex flex-col gap-2">
                        <div className="max-w-md">
                          <Suggest
                            label="Search for status checks"
                            placeholder="Search for status checks in the last week"
                            items={checkNames.filter((c) => !field.state.value.includes(c))}
                            allowFree
                            unavailable={checks.error?.message}
                            onPick={(v) => field.handleChange([...field.state.value, v])}
                          />
                        </div>
                        <ul className="flex flex-wrap gap-1.5" data-testid="required-checks">
                          {field.state.value.map((c) => (
                            <li key={c}>
                              <Badge variant="outline" size="lg" className="gap-1 font-mono">
                                {c}
                                <button
                                  type="button"
                                  aria-label={`Remove check ${c}`}
                                  onClick={() =>
                                    field.handleChange(field.state.value.filter((x) => x !== c))
                                  }
                                >
                                  <HugeiconsIcon
                                    icon={Cancel01Icon}
                                    strokeWidth={2}
                                    className="size-3"
                                  />
                                </button>
                              </Badge>
                            </li>
                          ))}
                        </ul>
                        <FieldMessage field={field} />
                      </div>
                    )}
                  </form.Field>
                </div>
              )
            }
          </form.Subscribe>
        </Check>
      </Section>

      <Section>
        <Check
          form={form}
          name="conversations"
          title="Require conversation resolution before merging"
          description="Every review thread must be resolved."
        />
        <Check
          form={form}
          name="signatures"
          title="Require signed commits"
          description="Commits pushed to matching branches must carry verified signatures."
        />
        <Check
          form={form}
          name="linear"
          title="Require linear history"
          description="Prevent merge commits from being pushed to matching branches."
        />
        <Check
          form={form}
          name="enforceAdmins"
          title="Do not allow bypassing the above settings"
          description="Administrators are held to the rule too."
        />
      </Section>

      <Section>
        <Check form={form} name="restrict" title="Restrict who can push to matching branches">
          <form.Subscribe selector={(s) => s.values.restrict}>
            {(on) =>
              on && (
                <form.Field name="pushUsers">
                  {(field) => (
                    <div className="flex flex-col gap-2 pl-6">
                      <div className="max-w-md">
                        <Suggest
                          label="Add people who can push"
                          placeholder="Search for people"
                          items={people
                            .map((p) => p.login)
                            .filter((l) => !field.state.value.includes(l))}
                          onPick={(v) => field.handleChange([...field.state.value, v])}
                        />
                      </div>
                      <ul className="flex flex-wrap gap-1.5" data-testid="push-users">
                        {field.state.value.length === 0 && (
                          <li className="text-muted-foreground text-xs">Only admins can push.</li>
                        )}
                        {field.state.value.map((l) => (
                          <li key={l}>
                            <Badge variant="outline" size="lg" className="gap-1">
                              {l}
                              <button
                                type="button"
                                aria-label={`Remove ${l}`}
                                onClick={() =>
                                  field.handleChange(field.state.value.filter((x) => x !== l))
                                }
                              >
                                <HugeiconsIcon
                                  icon={Cancel01Icon}
                                  strokeWidth={2}
                                  className="size-3"
                                />
                              </button>
                            </Badge>
                          </li>
                        ))}
                      </ul>
                    </div>
                  )}
                </form.Field>
              )
            }
          </form.Subscribe>
        </Check>
      </Section>

      <Section title="Rules applied to everyone, including administrators">
        <Check
          form={form}
          name="forcePushes"
          title="Allow force pushes"
          description="Anyone with push access can force push to matching branches."
        />
        <Check
          form={form}
          name="deletions"
          title="Allow deletions"
          description="Anyone with push access can delete matching branches."
        />
      </Section>

      <div className="flex flex-wrap items-center gap-2 px-4 py-3">
        {failure && (
          <p className="text-destructive w-full text-xs" role="alert">
            {failure}
          </p>
        )}
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting}>
              {submitting ? "Saving…" : isNew ? "Create" : "Save changes"}
            </Button>
          )}
        </form.Subscribe>
        <Button
          variant="outline"
          render={<Link to="/$owner/$repo/settings/branches" params={params} />}
        >
          Cancel
        </Button>
        {!isNew && (
          <Button
            type="button"
            variant="destructive"
            className="ml-auto"
            onClick={() => setConfirmDelete(true)}
          >
            Delete rule
          </Button>
        )}
      </div>
      <TypedConfirm
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        title="Delete this branch protection rule"
        description="Matching branches lose every requirement in this rule at once."
        phrase={pattern}
        action="Delete rule"
        pending={del.isPending}
        onConfirm={() => del.mutate()}
      />
    </form>
  );
}

function Section({ title, children }: { title?: string; children: React.ReactNode }) {
  return (
    <fieldset className="flex flex-col gap-3 px-4 py-3">
      {title && <legend className="text-muted-foreground mb-1 text-xs font-medium">{title}</legend>}
      {children}
    </fieldset>
  );
}

type BoolField = { [K in keyof Values]: Values[K] extends boolean ? K : never }[keyof Values];

function Check({
  form,
  name,
  title,
  description,
  children,
}: {
  // The form is this editor's; its field names are Values' boolean keys.
  form: {
    Field: React.ComponentType<{
      name: BoolField;
      children: (f: {
        state: { value: boolean };
        handleChange: (v: boolean) => void;
      }) => React.ReactNode;
    }>;
  };
  name: BoolField;
  title: string;
  description?: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-2">
      <form.Field name={name}>
        {(field) => (
          <label className="flex items-start gap-2 text-sm">
            <Checkbox
              className="mt-0.5"
              checked={field.state.value}
              onCheckedChange={(v) => field.handleChange(Boolean(v))}
              data-check={name}
            />
            <span>
              <span className="font-medium">{title}</span>
              {description && (
                <span className="text-muted-foreground block text-xs">{description}</span>
              )}
            </span>
          </label>
        )}
      </form.Field>
      {children}
    </div>
  );
}
