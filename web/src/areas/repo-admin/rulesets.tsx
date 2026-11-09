import { useMemo, useState } from "react";
import { useMutation, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { createColumnHelper, useTable } from "@tanstack/react-table";
import { Link, useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, Cancel01Icon, ShieldKeyIcon } from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { repoQuery } from "@/areas/core/queries";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
} from "@/components/reui/data-grid/data-grid";
import { DataGridColumnHeader } from "@/components/reui/data-grid/data-grid-column-header";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { call, failed, optimistic, refetch } from "./client";
import {
  refMatches,
  REPOSITORY_ROLE_IDS,
  RULE_KINDS,
  ruleLabel,
  type Rule,
  type RulesetTarget,
} from "./domain";
import {
  applyServerErrors,
  EmptyRow,
  FieldMessage,
  Suggest,
  TypedConfirm,
  type RepoParams,
} from "./kit";
import {
  branchesQuery,
  checkNamesQuery,
  repoTeamsQuery,
  rulesetHistoryQuery,
  rulesetQuery,
  rulesetsQuery,
  ruleSuiteQuery,
  ruleSuitesQuery,
  type SuiteFilters,
} from "./queries";

type Ruleset = Schemas["repository-ruleset"];
type BypassActor = Schemas["repository-ruleset-bypass-actor"];
type Enforcement = Schemas["repository-rule-enforcement"];

const ENFORCEMENT: Record<
  Enforcement,
  { label: string; variant: "success-light" | "warning-light" | "outline" }
> = {
  active: { label: "Active", variant: "success-light" },
  evaluate: { label: "Evaluate", variant: "warning-light" },
  disabled: { label: "Disabled", variant: "outline" },
};

// ---- List ----

export function RulesetList({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(rulesetsQuery(params));
  if (data.length === 0) {
    return (
      <EmptyRow>
        No rulesets yet. A ruleset protects branches or tags with rules anyone can read.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="rulesets">
      {data.map((r) => (
        <li key={r.id} className="flex items-center gap-3 px-4 py-2.5" data-ruleset={r.name}>
          <HugeiconsIcon
            icon={ShieldKeyIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <Link
              to="/$owner/$repo/settings/rules/$rulesetId"
              params={{ ...params, rulesetId: String(r.id) }}
              className="text-sm font-semibold hover:underline"
            >
              {r.name}
            </Link>
            <p className="text-muted-foreground text-xs capitalize">
              {r.target ?? "branch"} ruleset · updated{" "}
              {r.updated_at ? <Ago at={r.updated_at} /> : "never"}
            </p>
          </div>
          <Badge variant={ENFORCEMENT[r.enforcement].variant}>
            {ENFORCEMENT[r.enforcement].label}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

// ---- Builder ----

type Draft = {
  name: string;
  target: RulesetTarget;
  enforcement: Enforcement;
  include: string[];
  exclude: string[];
  bypass: BypassActor[];
  rules: Rule[];
};

const blank = (target: RulesetTarget): Draft => ({
  name: "",
  target,
  enforcement: "active",
  include: target === "branch" ? ["~DEFAULT_BRANCH"] : [],
  exclude: [],
  bypass: [],
  rules:
    target === "branch"
      ? [{ type: "deletion" }, { type: "non_fast_forward" }]
      : [{ type: "deletion" }],
});

function draftOf(r: Ruleset): Draft {
  return {
    name: r.name,
    target: r.target === "tag" ? "tag" : "branch",
    enforcement: r.enforcement,
    include: r.conditions?.ref_name?.include ?? [],
    exclude: r.conditions?.ref_name?.exclude ?? [],
    bypass: r.bypass_actors ?? [],
    rules: r.rules ?? [],
  };
}

const refLabel = (c: string) =>
  c === "~DEFAULT_BRANCH"
    ? "Default branch"
    : c === "~ALL"
      ? "All refs"
      : c.replace(/^refs\/(heads|tags)\//, "");

export function RulesetEditor({
  params,
  id,
  target,
}: {
  params: RepoParams;
  id: number | "new";
  target: RulesetTarget;
}) {
  const existing = useQuery({
    ...rulesetQuery({ ...params, id: id === "new" ? 0 : id }),
    enabled: id !== "new",
  });
  if (id !== "new" && existing.isPending)
    return <p className="text-muted-foreground px-4 py-6 text-sm">Loading…</p>;
  if (existing.error) throw existing.error;
  return (
    <Builder
      params={params}
      ruleset={existing.data}
      initial={existing.data ? draftOf(existing.data) : blank(target)}
    />
  );
}

function Builder({
  params,
  ruleset,
  initial,
}: {
  params: RepoParams;
  ruleset?: Ruleset;
  initial: Draft;
}) {
  const { data: repo } = useSuspenseQuery(repoQuery(params));
  const { data: branches } = useSuspenseQuery(branchesQuery(params));
  const { data: teams } = useSuspenseQuery(repoTeamsQuery(params));
  // Recent check names only feed suggestions; checks can be typed in, so a failed read
  // leaves the editor usable with no suggestions.
  const checks = useQuery(checkNamesQuery(params));
  const checkNames = checks.data ?? [];
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const listKey = rulesetsQuery(params).queryKey;

  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const body = {
        name: value.name.trim(),
        target: value.target,
        enforcement: value.enforcement,
        bypass_actors: value.bypass,
        conditions: { ref_name: { include: value.include, exclude: value.exclude } },
        rules: value.rules,
      };
      try {
        const saved = ruleset
          ? await call(
              api.PUT("/repos/{owner}/{repo}/rulesets/{ruleset_id}", {
                params: { path: { ...params, ruleset_id: ruleset.id } },
                body,
              }),
            )
          : await call(
              api.POST("/repos/{owner}/{repo}/rulesets", { params: { path: params }, body }),
            );
        toast.success(ruleset ? `Saved ${saved.name}` : `Created ${saved.name}`);
        await refetch(listKey, rulesetQuery({ ...params, id: saved.id }).queryKey);
        void navigate({ to: "/$owner/$repo/settings/rules", params });
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, { name: "name", conditions: "include", rules: "rules" }),
        );
        failed("Ruleset not saved")(e);
      }
    },
  });

  const del = useMutation({
    mutationFn: () =>
      call(
        api.DELETE("/repos/{owner}/{repo}/rulesets/{ruleset_id}", {
          params: { path: { ...params, ruleset_id: ruleset!.id } },
        }),
      ),
    onMutate: () =>
      optimistic<Ruleset[]>(listKey, (old) => old.filter((r) => r.id !== ruleset!.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Ruleset not deleted")(e);
    },
    onSuccess: () => {
      toast.success(`Deleted ${ruleset!.name}`);
      void navigate({ to: "/$owner/$repo/settings/rules", params });
    },
    onSettled: () => refetch(listKey),
  });

  const actorLabel = (a: BypassActor) =>
    a.actor_type === "RepositoryRole"
      ? (REPOSITORY_ROLE_IDS.find((r) => r.id === a.actor_id)?.label ?? "Repository role")
      : a.actor_type === "Team"
        ? `Team ${teams.find((t) => t.id === a.actor_id)?.slug ?? a.actor_id}`
        : a.actor_type === "OrganizationAdmin"
          ? "Organization admin"
          : a.actor_type === "DeployKey"
            ? "Deploy keys"
            : a.actor_type;
  const actorOptions: BypassActor[] = [
    ...REPOSITORY_ROLE_IDS.map((r) => ({
      actor_id: r.id,
      actor_type: "RepositoryRole" as const,
      bypass_mode: "always" as const,
    })),
    ...(repo.owner.type === "Organization"
      ? [{ actor_id: 1, actor_type: "OrganizationAdmin" as const, bypass_mode: "always" as const }]
      : []),
    ...teams.map((t) => ({
      actor_id: t.id,
      actor_type: "Team" as const,
      bypass_mode: "always" as const,
    })),
    { actor_id: null, actor_type: "DeployKey", bypass_mode: "always" },
  ];

  return (
    <form
      className="flex flex-col divide-y"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="ruleset-builder"
    >
      <div className="grid gap-3 px-4 py-3 sm:grid-cols-[minmax(0,1fr)_12rem]">
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) =>
              !value.trim()
                ? "Name the ruleset."
                : value.length > 100
                  ? "At most 100 characters."
                  : undefined,
          }}
        >
          {(field) => (
            <div className="flex flex-col gap-1">
              <label htmlFor="ruleset-name" className="text-sm font-medium">
                Ruleset name
              </label>
              <Input
                id="ruleset-name"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
                aria-invalid={field.state.meta.errors.length > 0 || undefined}
              />
              <FieldMessage field={field} />
            </div>
          )}
        </form.Field>
        <form.Field name="enforcement">
          {(field) => (
            <label className="flex flex-col gap-1 text-sm font-medium">
              Enforcement status
              <NativeSelect
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value as Enforcement)}
                className="w-full"
              >
                <NativeSelectOption value="active">Active</NativeSelectOption>
                <NativeSelectOption value="evaluate">Evaluate (report only)</NativeSelectOption>
                <NativeSelectOption value="disabled">Disabled</NativeSelectOption>
              </NativeSelect>
            </label>
          )}
        </form.Field>
      </div>

      <section className="flex flex-col gap-2 px-4 py-3">
        <h3 className="text-sm font-semibold">Bypass list</h3>
        <p className="text-muted-foreground text-xs">
          These roles, teams, and keys can bypass the ruleset.
        </p>
        <form.Field name="bypass">
          {(field) => (
            <>
              <ul className="flex flex-col divide-y rounded-sm border" data-testid="bypass-actors">
                {field.state.value.length === 0 && (
                  <li className="text-muted-foreground px-3 py-2 text-xs">Bypass list is empty.</li>
                )}
                {field.state.value.map((a, i) => (
                  <li
                    key={`${a.actor_type}:${a.actor_id}`}
                    className="flex items-center gap-2 px-3 py-1.5 text-sm"
                  >
                    <span className="flex-1">{actorLabel(a)}</span>
                    <NativeSelect
                      size="sm"
                      aria-label={`Bypass mode for ${actorLabel(a)}`}
                      value={a.bypass_mode}
                      onChange={(e) =>
                        field.handleChange(
                          field.state.value.map((x, j) =>
                            j === i
                              ? { ...x, bypass_mode: e.target.value as BypassActor["bypass_mode"] }
                              : x,
                          ),
                        )
                      }
                    >
                      <NativeSelectOption value="always">Always allow</NativeSelectOption>
                      <NativeSelectOption value="pull_request">
                        For pull requests only
                      </NativeSelectOption>
                    </NativeSelect>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-xs"
                      aria-label={`Remove ${actorLabel(a)}`}
                      onClick={() =>
                        field.handleChange(field.state.value.filter((_, j) => j !== i))
                      }
                    >
                      <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} />
                    </Button>
                  </li>
                ))}
              </ul>
              <NativeSelect
                size="sm"
                aria-label="Add bypass"
                value=""
                onChange={(e) => {
                  const a = actorOptions[Number(e.target.value)];
                  if (a) field.handleChange([...field.state.value, a]);
                }}
              >
                <NativeSelectOption value="">Add bypass…</NativeSelectOption>
                {actorOptions.map((a, i) =>
                  field.state.value.some(
                    (x) => x.actor_type === a.actor_type && x.actor_id === a.actor_id,
                  ) ? null : (
                    <NativeSelectOption key={i} value={i}>
                      {actorLabel(a)}
                    </NativeSelectOption>
                  ),
                )}
              </NativeSelect>
            </>
          )}
        </form.Field>
      </section>

      <form.Subscribe selector={(s) => s.values.target}>
        {(target) => (
          <section className="flex flex-col gap-3 px-4 py-3">
            <h3 className="text-sm font-semibold">
              Target {target === "tag" ? "tags" : "branches"}
            </h3>
            <form.Field
              name="include"
              validators={{
                onSubmit: ({ value }) =>
                  value.length === 0 ? "Include at least one target." : undefined,
              }}
            >
              {(field) => (
                <RefList
                  label="Include"
                  target={target}
                  values={field.state.value}
                  onChange={field.handleChange}
                  presets={target === "branch" ? ["~DEFAULT_BRANCH", "~ALL"] : ["~ALL"]}
                  error={<FieldMessage field={field} />}
                />
              )}
            </form.Field>
            <form.Field name="exclude">
              {(field) => (
                <RefList
                  label="Exclude"
                  target={target}
                  values={field.state.value}
                  onChange={field.handleChange}
                  presets={[]}
                />
              )}
            </form.Field>
            <form.Subscribe selector={(s) => [s.values.include, s.values.exclude] as const}>
              {([include, exclude]) => {
                const names = target === "branch" ? branches.map((b) => b.name) : [];
                const hit = names.filter(
                  (n) =>
                    include.some((c) => refMatches(c, n, repo.default_branch, target)) &&
                    !exclude.some((c) => refMatches(c, n, repo.default_branch, target)),
                );
                return target === "branch" ? (
                  <p className="text-muted-foreground text-xs" data-testid="ruleset-targets">
                    Applies to {hit.length} of {names.length} branches
                    {hit.length ? `: ${hit.join(", ")}` : ""}.
                  </p>
                ) : null;
              }}
            </form.Subscribe>
          </section>
        )}
      </form.Subscribe>

      <section className="flex flex-col gap-2 px-4 py-3">
        <h3 className="text-sm font-semibold">Rules</h3>
        <form.Subscribe selector={(s) => s.values.target}>
          {(target) => (
            <form.Field name="rules">
              {(field) => (
                <>
                  <ul className="flex flex-col divide-y rounded-sm border" data-testid="rules">
                    {RULE_KINDS.filter((k) => k.targets.includes(target)).map((k) => {
                      const at = field.state.value.findIndex((r) => r.type === k.type);
                      const rule = at >= 0 ? field.state.value[at] : undefined;
                      const set = (next: Rule | undefined) =>
                        field.handleChange(
                          next
                            ? at >= 0
                              ? field.state.value.map((r, i) => (i === at ? next : r))
                              : [...field.state.value, next]
                            : field.state.value.filter((r) => r.type !== k.type),
                        );
                      return (
                        <li key={k.type} className="flex flex-col gap-2 px-3 py-2">
                          <label className="flex items-start gap-2 text-sm">
                            <Checkbox
                              className="mt-0.5"
                              checked={Boolean(rule)}
                              onCheckedChange={(on) => set(on ? k.make() : undefined)}
                              data-rule={k.type}
                            />
                            <span>
                              <span className="font-medium">{k.label}</span>
                              <span className="text-muted-foreground block text-xs">
                                {k.description}
                              </span>
                            </span>
                          </label>
                          {rule && (
                            <RuleParams
                              rule={rule}
                              onChange={set}
                              checkNames={checkNames}
                              checksError={checks.error?.message}
                            />
                          )}
                        </li>
                      );
                    })}
                  </ul>
                  <FieldMessage field={field} />
                </>
              )}
            </form.Field>
          )}
        </form.Subscribe>
      </section>

      <div className="flex flex-wrap items-center gap-2 px-4 py-3">
        {failure && (
          <p className="text-destructive w-full text-xs" role="alert">
            {failure}
          </p>
        )}
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting}>
              {submitting ? "Saving…" : ruleset ? "Save changes" : "Create"}
            </Button>
          )}
        </form.Subscribe>
        <Button
          variant="outline"
          render={<Link to="/$owner/$repo/settings/rules" params={params} />}
        >
          Cancel
        </Button>
        {ruleset && (
          <Button
            type="button"
            variant="destructive"
            className="ml-auto"
            onClick={() => setConfirmDelete(true)}
          >
            Delete ruleset
          </Button>
        )}
      </div>
      {ruleset && (
        <TypedConfirm
          open={confirmDelete}
          onOpenChange={setConfirmDelete}
          title={`Delete ${ruleset.name}`}
          description="Its rules stop applying at once."
          phrase={ruleset.name}
          action="Delete ruleset"
          pending={del.isPending}
          onConfirm={() => del.mutate()}
        />
      )}
    </form>
  );
}

function RefList({
  label,
  target,
  values,
  onChange,
  presets,
  error,
}: {
  label: string;
  target: RulesetTarget;
  values: string[];
  onChange: (v: string[]) => void;
  presets: string[];
  error?: React.ReactNode;
}) {
  const [draft, setDraft] = useState("");
  const prefix = target === "tag" ? "refs/tags/" : "refs/heads/";
  const add = (v: string) => {
    const full = v.startsWith("~") || v.startsWith("refs/") ? v : `${prefix}${v}`;
    if (!values.includes(full)) onChange([...values, full]);
  };
  return (
    <div className="flex flex-col gap-1.5" data-testid={`refs-${label.toLowerCase()}`}>
      <div className="text-xs font-medium">{label}</div>
      <ul className="flex flex-wrap gap-1.5">
        {values.length === 0 && <li className="text-muted-foreground text-xs">None.</li>}
        {values.map((v) => (
          <li key={v}>
            <Badge variant="outline" size="lg" className="gap-1 font-mono">
              {refLabel(v)}
              <button
                type="button"
                aria-label={`Remove ${refLabel(v)} from ${label}`}
                onClick={() => onChange(values.filter((x) => x !== v))}
              >
                <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} className="size-3" />
              </button>
            </Badge>
          </li>
        ))}
      </ul>
      <div className="flex flex-wrap items-center gap-2">
        <Input
          className="h-7 max-w-64 font-mono text-xs"
          aria-label={`${label} pattern`}
          placeholder={target === "tag" ? "v*" : "release/*"}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && draft.trim()) {
              e.preventDefault();
              add(draft.trim());
              setDraft("");
            }
          }}
        />
        <Button
          type="button"
          size="xs"
          variant="outline"
          disabled={!draft.trim()}
          onClick={() => {
            add(draft.trim());
            setDraft("");
          }}
        >
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Add pattern
        </Button>
        {presets
          .filter((p) => !values.includes(p))
          .map((p) => (
            <Button key={p} type="button" size="xs" variant="ghost" onClick={() => add(p)}>
              Include {refLabel(p).toLowerCase()}
            </Button>
          ))}
      </div>
      {error}
    </div>
  );
}

function RuleParams({
  rule,
  onChange,
  checkNames,
  checksError,
}: {
  rule: Rule;
  onChange: (r: Rule) => void;
  checkNames: string[];
  checksError?: string;
}) {
  switch (rule.type) {
    case "update":
      return (
        <label className="flex items-center gap-2 pl-6 text-xs">
          <Checkbox
            checked={rule.parameters?.update_allows_fetch_and_merge ?? false}
            onCheckedChange={(v) =>
              onChange({ ...rule, parameters: { update_allows_fetch_and_merge: Boolean(v) } })
            }
          />
          Allow fetch and merge from the upstream
        </label>
      );
    case "pull_request": {
      const p = rule.parameters!;
      const set = (patch: Partial<typeof p>) =>
        onChange({ ...rule, parameters: { ...p, ...patch } });
      return (
        <div className="flex flex-col gap-1.5 pl-6 text-xs">
          <label className="flex items-center gap-2">
            Required approvals
            <NativeSelect
              size="sm"
              aria-label="Ruleset required approvals"
              value={String(p.required_approving_review_count)}
              onChange={(e) => set({ required_approving_review_count: Number(e.target.value) })}
            >
              {Array.from({ length: 11 }, (_, n) => (
                <NativeSelectOption key={n} value={n}>
                  {n}
                </NativeSelectOption>
              ))}
            </NativeSelect>
          </label>
          {(
            [
              [
                "dismiss_stale_reviews_on_push",
                "Dismiss stale approvals when new commits are pushed",
              ],
              ["require_code_owner_review", "Require review from Code Owners"],
              ["require_last_push_approval", "Require approval of the most recent push"],
              ["required_review_thread_resolution", "Require conversation resolution"],
            ] as const
          ).map(([k, label]) => (
            <label key={k} className="flex items-center gap-2">
              <Checkbox checked={p[k]} onCheckedChange={(v) => set({ [k]: Boolean(v) })} />
              {label}
            </label>
          ))}
          <div className="flex items-center gap-3">
            Allowed merge methods:
            {(["merge", "squash", "rebase"] as const).map((m) => (
              <label key={m} className="flex items-center gap-1 capitalize">
                <Checkbox
                  checked={p.allowed_merge_methods?.includes(m) ?? true}
                  onCheckedChange={(v) => {
                    const cur = p.allowed_merge_methods ?? ["merge", "squash", "rebase"];
                    const next = v ? [...new Set([...cur, m])] : cur.filter((x) => x !== m);
                    if (next.length) set({ allowed_merge_methods: next });
                  }}
                />
                {m}
              </label>
            ))}
          </div>
        </div>
      );
    }
    case "required_status_checks": {
      const p = rule.parameters!;
      const names = p.required_status_checks.map((c) => c.context);
      const set = (patch: Partial<typeof p>) =>
        onChange({ ...rule, parameters: { ...p, ...patch } });
      return (
        <div className="flex flex-col gap-1.5 pl-6 text-xs">
          <label className="flex items-center gap-2">
            <Checkbox
              checked={p.strict_required_status_checks_policy}
              onCheckedChange={(v) => set({ strict_required_status_checks_policy: Boolean(v) })}
            />
            Require branches to be up to date before merging
          </label>
          <div className="max-w-sm">
            <Suggest
              label="Add a required check"
              placeholder="Add checks"
              items={checkNames.filter((c) => !names.includes(c))}
              allowFree
              unavailable={checksError}
              onPick={(c) =>
                set({ required_status_checks: [...p.required_status_checks, { context: c }] })
              }
            />
          </div>
          <ul className="flex flex-wrap gap-1.5">
            {names.map((c) => (
              <li key={c}>
                <Badge variant="outline" className="gap-1 font-mono">
                  {c}
                  <button
                    type="button"
                    aria-label={`Remove ruleset check ${c}`}
                    onClick={() =>
                      set({
                        required_status_checks: p.required_status_checks.filter(
                          (x) => x.context !== c,
                        ),
                      })
                    }
                  >
                    <HugeiconsIcon icon={Cancel01Icon} strokeWidth={2} className="size-3" />
                  </button>
                </Badge>
              </li>
            ))}
          </ul>
        </div>
      );
    }
    case "commit_message_pattern":
    case "branch_name_pattern":
    case "tag_name_pattern": {
      const p = rule.parameters!;
      const set = (patch: Partial<typeof p>) =>
        onChange({ ...rule, parameters: { ...p, ...patch } } as Rule);
      return (
        <div className="flex flex-wrap items-center gap-2 pl-6 text-xs">
          <NativeSelect
            size="sm"
            aria-label={`${ruleLabel(rule.type)} operator`}
            value={p.operator}
            onChange={(e) => set({ operator: e.target.value as typeof p.operator })}
          >
            <NativeSelectOption value="starts_with">Starts with</NativeSelectOption>
            <NativeSelectOption value="ends_with">Ends with</NativeSelectOption>
            <NativeSelectOption value="contains">Contains</NativeSelectOption>
            <NativeSelectOption value="regex">Matches regex</NativeSelectOption>
          </NativeSelect>
          <Input
            className="h-7 max-w-64 font-mono text-xs"
            aria-label={`${ruleLabel(rule.type)} pattern`}
            value={p.pattern}
            onChange={(e) => set({ pattern: e.target.value })}
          />
          <label className="flex items-center gap-1">
            <Checkbox
              checked={p.negate ?? false}
              onCheckedChange={(v) => set({ negate: Boolean(v) })}
            />
            Must not match
          </label>
        </div>
      );
    }
    default:
      return null;
  }
}

export function RulesetHistory({ params, id }: { params: RepoParams; id: number }) {
  const { data } = useSuspenseQuery(rulesetHistoryQuery({ ...params, id }));
  return (
    <ol className="divide-y text-sm" data-testid="ruleset-history">
      {data.map((v) => (
        <li key={v.version_id} className="flex items-center gap-2 px-4 py-2">
          <Badge variant="outline">v{v.version_id}</Badge>
          <span className="text-muted-foreground">
            changed <Ago at={v.updated_at} />
          </span>
        </li>
      ))}
    </ol>
  );
}

// ---- Insights ----

type Suite = Schemas["rule-suites"][number];
const col = createColumnHelper<typeof dataGridFeatures, Suite>();
const RESULT: Record<string, "success-light" | "destructive-light" | "warning-light"> = {
  pass: "success-light",
  fail: "destructive-light",
  bypass: "warning-light",
};

export function RuleInsights({ params, filters }: { params: RepoParams; filters: SuiteFilters }) {
  const { data } = useSuspenseQuery(ruleSuitesQuery({ ...params, ...filters }));
  const [open, setOpen] = useState<number>();
  const columns = useMemo(
    () =>
      col.columns([
        col.accessor("result", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Result" />,
          cell: ({ getValue }) => (
            <Badge variant={RESULT[getValue() ?? "pass"]} className="capitalize">
              {getValue()}
            </Badge>
          ),
          size: 90,
        }),
        col.accessor("ref", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Ref" />,
          cell: ({ getValue }) => (
            <code className="font-mono text-xs">
              {getValue()?.replace(/^refs\/(heads|tags)\//, "")}
            </code>
          ),
          size: 240,
        }),
        col.accessor("actor_name", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Actor" />,
          size: 110,
        }),
        col.accessor("evaluation_result", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Evaluate" />,
          cell: ({ getValue }) => (
            <span className="text-muted-foreground text-xs capitalize">{getValue() ?? "n/a"}</span>
          ),
          size: 90,
        }),
        col.accessor("after_sha", {
          header: "Commit",
          cell: ({ getValue }) => (
            <code className="font-mono text-xs">{getValue()?.slice(0, 7)}</code>
          ),
          size: 90,
          enableSorting: false,
        }),
        col.accessor("pushed_at", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Pushed" />,
          cell: ({ getValue }) => <Ago at={getValue() ?? ""} />,
          size: 120,
        }),
      ]),
    [],
  );
  const table = useTable({
    features: dataGridFeatures,
    columns,
    data,
    getRowId: (r) => String(r.id),
    initialState: { sorting: [{ id: "pushed_at", desc: true }] },
  });
  return (
    <>
      <DataGrid
        table={table}
        recordCount={data.length}
        onRowClick={(r) => setOpen(r.id)}
        emptyMessage="No pushes evaluated in this window."
        tableLayout={{ dense: true, headerBackground: true }}
      >
        <DataGridContainer>
          <div data-testid="rule-insights">
            <DataGridTable />
          </div>
        </DataGridContainer>
      </DataGrid>
      <Sheet open={open !== undefined} onOpenChange={(o) => !o && setOpen(undefined)}>
        <SheetContent className="sm:max-w-lg">
          {open !== undefined && <SuiteDetail params={params} id={open} />}
        </SheetContent>
      </Sheet>
    </>
  );
}

function SuiteDetail({ params, id }: { params: RepoParams; id: number }) {
  const { data, isPending, error } = useQuery(ruleSuiteQuery({ ...params, id }));
  return (
    <>
      <SheetHeader>
        <SheetTitle>Rule evaluation</SheetTitle>
        <SheetDescription>
          {data ? `${data.actor_name} pushed ${data.ref}` : "Loading…"}
        </SheetDescription>
      </SheetHeader>
      <div className="flex flex-col gap-2 px-4 text-sm" data-testid="suite-detail">
        {isPending && <p className="text-muted-foreground">Loading…</p>}
        {error && (
          <p className="text-destructive" role="alert">
            Could not load: {error.message}
          </p>
        )}
        {data?.rule_evaluations?.map((e, i) => (
          <div key={i} className="rounded-sm border p-2">
            <div className="flex items-center gap-2">
              <Badge
                variant={e.result === "pass" ? "success-light" : "destructive-light"}
                className="capitalize"
              >
                {e.result}
              </Badge>
              <span className="font-medium">{ruleLabel(e.rule_type ?? "")}</span>
              <span className="text-muted-foreground ml-auto text-xs">
                {e.rule_source?.name} · {e.enforcement}
              </span>
            </div>
            {e.details && <p className="text-muted-foreground mt-1 text-xs">{e.details}</p>}
          </div>
        ))}
      </div>
    </>
  );
}
