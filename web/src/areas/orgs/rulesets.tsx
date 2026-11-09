import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, Cancel01Icon, ShieldKeyIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import {
  RULE_KINDS,
  matchesPattern,
  ruleLabel,
  type Rule,
  type RulesetTarget,
} from "@/areas/repo-admin/domain";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { send } from "./api";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  FieldMessage,
  FormFailure,
  optimistic,
  refetch,
} from "./kit";
import {
  orgReposQuery,
  rulesetHistoryQuery,
  rulesetQuery,
  rulesetsQuery,
  teamsQuery,
} from "./queries";

// Organization rulesets: the same rules a repository ruleset has, aimed at repositories by
// name pattern as well as at refs.

type Ruleset = Schemas["repository-ruleset"];
type BypassActor = Schemas["repository-ruleset-bypass-actor"];
type Enforcement = Schemas["repository-rule-enforcement"];
type OrgRule = Schemas["org-rules"];

// An org ruleset takes every repository rule but merge_queue, which is repository-only.
const isOrgRule = (r: Rule): r is OrgRule => r.type !== "merge_queue";

const ENFORCEMENT: Record<
  Enforcement,
  { label: string; variant: "success-light" | "warning-light" | "outline" }
> = {
  active: { label: "Active", variant: "success-light" },
  evaluate: { label: "Evaluate", variant: "warning-light" },
  disabled: { label: "Disabled", variant: "outline" },
};

const refLabel = (c: string) =>
  c === "~DEFAULT_BRANCH"
    ? "Default branch"
    : c === "~ALL"
      ? "All"
      : c.replace(/^refs\/(heads|tags)\//, "");

type Conditions = {
  ref_name?: { include?: string[]; exclude?: string[] };
  repository_name?: { include?: string[]; exclude?: string[] };
};
const conditionsOf = (r: Ruleset) => (r.conditions ?? {}) as Conditions;

export function RulesetList({ org }: { org: string }) {
  const { data } = useSuspenseQuery(rulesetsQuery(org));
  if (!data.length) {
    return (
      <EmptyRow>
        No rulesets. An org ruleset protects branches or tags across many repositories at once.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="rulesets">
      {data.map((r) => {
        const repos = conditionsOf(r).repository_name?.include ?? [];
        return (
          <li key={r.id} className="flex items-center gap-3 px-4 py-2.5" data-ruleset={r.name}>
            <HugeiconsIcon
              icon={ShieldKeyIcon}
              strokeWidth={2}
              className="text-muted-foreground size-4"
            />
            <div className="min-w-0 flex-1">
              <Link
                to="/orgs/$org/settings/rules/$rulesetId"
                params={{ org, rulesetId: String(r.id) }}
                className="text-sm font-semibold hover:underline"
              >
                {r.name}
              </Link>
              <p className="text-muted-foreground text-xs">
                <span className="capitalize">{r.target ?? "branch"}</span> ruleset ·{" "}
                {r.rules?.length ?? 0} rules ·{" "}
                {repos.includes("~ALL")
                  ? "every repository"
                  : `${repos.length} repository patterns`}{" "}
                · updated {r.updated_at ? <Ago at={r.updated_at} /> : "never"}
              </p>
            </div>
            <Badge variant={ENFORCEMENT[r.enforcement].variant}>
              {ENFORCEMENT[r.enforcement].label}
            </Badge>
          </li>
        );
      })}
    </ul>
  );
}

type Draft = {
  name: string;
  target: RulesetTarget;
  enforcement: Enforcement;
  repoInclude: string[];
  repoExclude: string[];
  refInclude: string[];
  refExclude: string[];
  bypass: BypassActor[];
  rules: OrgRule[];
};

const blank = (target: RulesetTarget): Draft => ({
  name: "",
  target,
  enforcement: "evaluate",
  repoInclude: ["~ALL"],
  repoExclude: [],
  refInclude: target === "branch" ? ["~DEFAULT_BRANCH"] : ["refs/tags/v*"],
  refExclude: [],
  bypass: [{ actor_id: 1, actor_type: "OrganizationAdmin", bypass_mode: "always" }],
  rules:
    target === "branch"
      ? [{ type: "deletion" }, { type: "non_fast_forward" }]
      : [{ type: "deletion" }],
});

function draftOf(r: Ruleset): Draft {
  const c = conditionsOf(r);
  return {
    name: r.name,
    target: r.target === "tag" ? "tag" : "branch",
    enforcement: r.enforcement,
    repoInclude: c.repository_name?.include ?? [],
    repoExclude: c.repository_name?.exclude ?? [],
    refInclude: c.ref_name?.include ?? [],
    refExclude: c.ref_name?.exclude ?? [],
    bypass: r.bypass_actors ?? [],
    rules: (r.rules ?? []).filter(isOrgRule),
  };
}

export function RulesetEditor({
  org,
  id,
  target,
}: {
  org: string;
  id: number | "new";
  target: RulesetTarget;
}) {
  if (id === "new") return <Builder org={org} initial={blank(target)} />;
  return <Existing org={org} id={id} />;
}

function Existing({ org, id }: { org: string; id: number }) {
  const { data } = useSuspenseQuery(rulesetQuery(org, id));
  return <Builder org={org} ruleset={data} initial={draftOf(data)} />;
}

function Builder({ org, ruleset, initial }: { org: string; ruleset?: Ruleset; initial: Draft }) {
  const { data: teams } = useSuspenseQuery(teamsQuery(org));
  const { data: repos } = useSuspenseQuery(orgReposQuery(org));
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const listKey = rulesetsQuery(org).queryKey;
  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const body = {
        name: value.name.trim(),
        target: value.target,
        enforcement: value.enforcement,
        bypass_actors: value.bypass,
        conditions: {
          ref_name: { include: value.refInclude, exclude: value.refExclude },
          repository_name: { include: value.repoInclude, exclude: value.repoExclude },
        },
        rules: value.rules,
      };
      try {
        const saved = ruleset
          ? await send(
              api.PUT("/orgs/{org}/rulesets/{ruleset_id}", {
                params: { path: { org, ruleset_id: ruleset.id } },
                body,
              }),
            )
          : await send(api.POST("/orgs/{org}/rulesets", { params: { path: { org } }, body }));
        toast.success(ruleset ? `Saved ${saved.name}` : `Created ${saved.name}`);
        await refetch(listKey, ["orgs", org, "rulesets", saved.id]);
        void navigate({ to: "/orgs/$org/settings/rules", params: { org } });
      } catch (e) {
        setFailure(applyServerErrors(formApi, e, { name: "name", bypass_actors: "bypass" }));
        failed("Ruleset not saved")(e);
      }
    },
  });
  const del = useMutation({
    mutationFn: () =>
      send(
        api.DELETE("/orgs/{org}/rulesets/{ruleset_id}", {
          params: { path: { org, ruleset_id: ruleset!.id } },
        }),
      ),
    onMutate: () =>
      optimistic<Ruleset[]>(listKey, (old) => old.filter((r) => r.id !== ruleset!.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Ruleset not deleted")(e);
    },
    onSuccess: () => {
      setConfirmDelete(false);
      toast.success(`Deleted ${ruleset!.name}`);
      void navigate({ to: "/orgs/$org/settings/rules", params: { org } });
    },
    onSettled: () => refetch(listKey),
  });
  const actorLabel = (a: BypassActor) =>
    a.actor_type === "Team"
      ? `Team ${teams.find((t) => t.id === a.actor_id)?.name ?? a.actor_id}`
      : a.actor_type === "OrganizationAdmin"
        ? "Organization owners"
        : a.actor_type === "DeployKey"
          ? "Deploy keys"
          : a.actor_type;
  const actorOptions: BypassActor[] = [
    { actor_id: 1, actor_type: "OrganizationAdmin", bypass_mode: "always" },
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
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="ruleset-name">Ruleset name</Label>
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
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="ruleset-enforcement">Enforcement status</Label>
              <NativeSelect
                id="ruleset-enforcement"
                className="w-full"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value as Enforcement)}
              >
                <NativeSelectOption value="active">Active</NativeSelectOption>
                <NativeSelectOption value="evaluate">Evaluate (report only)</NativeSelectOption>
                <NativeSelectOption value="disabled">Disabled</NativeSelectOption>
              </NativeSelect>
            </div>
          )}
        </form.Field>
      </div>

      <section className="flex flex-col gap-2 px-4 py-3">
        <h3 className="text-sm font-semibold">Bypass list</h3>
        <form.Field name="bypass">
          {(field) => (
            <>
              <ul className="flex flex-col divide-y rounded-sm border" data-testid="bypass-actors">
                {field.state.value.length === 0 && (
                  <li className="text-muted-foreground px-3 py-2 text-xs">
                    Nobody can bypass this ruleset.
                  </li>
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
              <FieldMessage field={field} />
            </>
          )}
        </form.Field>
      </section>

      <section className="flex flex-col gap-3 px-4 py-3">
        <h3 className="text-sm font-semibold">Target repositories</h3>
        <form.Field
          name="repoInclude"
          validators={{
            onSubmit: ({ value }) =>
              value.length ? undefined : "Include at least one repository pattern.",
          }}
        >
          {(field) => (
            <PatternList
              label="Include"
              values={field.state.value}
              onChange={field.handleChange}
              placeholder="edge or *-service"
              presets={["~ALL"]}
              format={(v) => (v === "~ALL" ? "All repositories" : v)}
              error={<FieldMessage field={field} />}
            />
          )}
        </form.Field>
        <form.Field name="repoExclude">
          {(field) => (
            <PatternList
              label="Exclude"
              values={field.state.value}
              onChange={field.handleChange}
              placeholder="sandbox-*"
              presets={[]}
              format={(v) => v}
            />
          )}
        </form.Field>
        <form.Subscribe selector={(s) => [s.values.repoInclude, s.values.repoExclude] as const}>
          {([include, exclude]) => {
            const hit = repos.filter(
              (r) =>
                include.some((p) => p === "~ALL" || matchesPattern(p, r.name)) &&
                !exclude.some((p) => matchesPattern(p, r.name)),
            );
            return (
              <p
                className="text-muted-foreground text-xs"
                data-testid="ruleset-repos"
                data-count={hit.length}
              >
                Applies to {hit.length} of {repos.length} repositories
                {hit.length ? `: ${hit.map((r) => r.name).join(", ")}` : ""}.
              </p>
            );
          }}
        </form.Subscribe>
      </section>

      <form.Subscribe selector={(s) => s.values.target}>
        {(target) => (
          <section className="flex flex-col gap-3 px-4 py-3">
            <h3 className="text-sm font-semibold">
              Target {target === "tag" ? "tags" : "branches"}
            </h3>
            <form.Field
              name="refInclude"
              validators={{
                onSubmit: ({ value }) =>
                  value.length ? undefined : "Include at least one target.",
              }}
            >
              {(field) => (
                <PatternList
                  label="Include"
                  values={field.state.value}
                  onChange={field.handleChange}
                  placeholder={target === "tag" ? "v*" : "release/*"}
                  presets={target === "branch" ? ["~DEFAULT_BRANCH", "~ALL"] : ["~ALL"]}
                  normalize={(v) =>
                    v.startsWith("~") || v.startsWith("refs/")
                      ? v
                      : `refs/${target === "tag" ? "tags" : "heads"}/${v}`
                  }
                  format={refLabel}
                  error={<FieldMessage field={field} />}
                />
              )}
            </form.Field>
            <form.Field name="refExclude">
              {(field) => (
                <PatternList
                  label="Exclude"
                  values={field.state.value}
                  onChange={field.handleChange}
                  placeholder={target === "tag" ? "v*-rc*" : "dependabot/**"}
                  presets={[]}
                  normalize={(v) =>
                    v.startsWith("refs/") ? v : `refs/${target === "tag" ? "tags" : "heads"}/${v}`
                  }
                  format={refLabel}
                />
              )}
            </form.Field>
          </section>
        )}
      </form.Subscribe>

      <section className="flex flex-col gap-2 px-4 py-3">
        <h3 className="text-sm font-semibold">Rules</h3>
        <form.Subscribe selector={(s) => s.values.target}>
          {(target) => (
            <form.Field name="rules">
              {(field) => (
                <ul className="flex flex-col divide-y rounded-sm border" data-testid="rules">
                  {RULE_KINDS.filter((k) => k.targets.includes(target)).map((k) => {
                    const on = field.state.value.some((r) => r.type === k.type);
                    return (
                      <li key={k.type} className="px-3 py-2">
                        <label className="flex items-start gap-2 text-sm">
                          <Checkbox
                            className="mt-0.5"
                            checked={on}
                            onCheckedChange={(v) =>
                              field.handleChange(
                                v
                                  ? [...field.state.value, k.make()].filter(isOrgRule)
                                  : field.state.value.filter((r) => r.type !== k.type),
                              )
                            }
                            data-rule={k.type}
                          />
                          <span>
                            <span className="font-medium">{k.label}</span>
                            <span className="text-muted-foreground block text-xs">
                              {k.description}
                            </span>
                          </span>
                        </label>
                      </li>
                    );
                  })}
                </ul>
              )}
            </form.Field>
          )}
        </form.Subscribe>
      </section>

      <div className="flex flex-wrap items-center gap-2 px-4 py-3">
        <FormFailure>{failure}</FormFailure>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-testid="save-ruleset">
              {submitting ? "Saving…" : ruleset ? "Save changes" : "Create"}
            </Button>
          )}
        </form.Subscribe>
        <Button variant="outline" render={<Link to="/orgs/$org/settings/rules" params={{ org }} />}>
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
        <Confirm
          open={confirmDelete}
          onOpenChange={setConfirmDelete}
          title={`Delete ${ruleset.name}?`}
          description="Its rules stop applying to every repository at once."
          phrase={ruleset.name}
          action="Delete ruleset"
          pending={del.isPending}
          onConfirm={() => del.mutate()}
        />
      )}
    </form>
  );
}

function PatternList({
  label,
  values,
  onChange,
  placeholder,
  presets,
  format,
  normalize = (v) => v,
  error,
}: {
  label: string;
  values: string[];
  onChange: (v: string[]) => void;
  placeholder: string;
  presets: string[];
  format: (v: string) => string;
  normalize?: (v: string) => string;
  error?: React.ReactNode;
}) {
  const [draft, setDraft] = useState("");
  const add = (v: string) => {
    const full = normalize(v);
    if (!values.includes(full)) onChange([...values, full]);
    setDraft("");
  };
  return (
    <div className="flex flex-col gap-1.5">
      <div className="text-xs font-medium">{label}</div>
      <ul className="flex flex-wrap gap-1.5">
        {values.length === 0 && <li className="text-muted-foreground text-xs">None.</li>}
        {values.map((v) => (
          <li key={v}>
            <Badge variant="outline" size="lg" className="gap-1 font-mono" data-pattern={v}>
              {format(v)}
              <button
                type="button"
                aria-label={`Remove ${format(v)} from ${label}`}
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
          placeholder={placeholder}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && draft.trim()) {
              e.preventDefault();
              add(draft.trim());
            }
          }}
        />
        <Button
          type="button"
          size="xs"
          variant="outline"
          disabled={!draft.trim()}
          onClick={() => add(draft.trim())}
        >
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Add pattern
        </Button>
        {presets
          .filter((p) => !values.includes(p))
          .map((p) => (
            <Button key={p} type="button" size="xs" variant="ghost" onClick={() => add(p)}>
              Include {format(p).toLowerCase()}
            </Button>
          ))}
      </div>
      {error}
    </div>
  );
}

export function RulesetHistory({ org, id }: { org: string; id: number }) {
  const { data } = useSuspenseQuery(rulesetHistoryQuery(org, id));
  const { data: ruleset } = useSuspenseQuery(rulesetQuery(org, id));
  return (
    <ol className="divide-y" data-testid="ruleset-history">
      {data.map((v) => (
        <li key={v.version_id} className="flex items-center gap-2 px-4 py-2 text-sm">
          <Badge variant="outline" size="sm" className="font-mono">
            v{v.version_id}
          </Badge>
          <span className="text-muted-foreground">
            {v.actor.type ?? "User"} {v.actor.id} saved it <Ago at={v.updated_at} />
          </span>
        </li>
      ))}
      <li className="text-muted-foreground px-4 py-2 text-xs">
        Current rules: {(ruleset.rules ?? []).map((r) => ruleLabel(r.type)).join(", ") || "none"}.
      </li>
    </ol>
  );
}
