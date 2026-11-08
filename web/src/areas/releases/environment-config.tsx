import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Delete02Icon,
  GitBranchIcon,
  Key01Icon,
  PencilEdit02Icon,
  Tag01Icon,
  VariableIcon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Panel } from "@/components/panel";
import { Button } from "@/components/ui/button";
import { Field, FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { errorText, send, serverError, WriteError } from "./api";
import {
  branchPoliciesQuery,
  environmentQuery,
  envPublicKeyQuery,
  envSecretsQuery,
  envVariablesQuery,
} from "./queries";
import { useOptimisticWrite } from "./write";

type Env = { owner: string; repo: string; name: string };
type Policy = Schemas["deployment-branch-policy"];
type Secret = Schemas["actions-secret"];
type Variable = Schemas["actions-variable"];

const path = (env: Env) => ({ owner: env.owner, repo: env.repo, environment_name: env.name });
const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const nameProblem = (v: string) =>
  !v.trim()
    ? "Name it."
    : !NAME.test(v.trim())
      ? "Use letters, digits, and underscores, and do not start with a digit."
      : v.trim().toUpperCase().startsWith("GITHUB_")
        ? "Names cannot start with GITHUB_."
        : undefined;

// The message under a field for a 422, or undefined when the failure is not a field's.
function fieldMessage(e: unknown, field: string, subject: string) {
  if (!(e instanceof WriteError) || e.status !== 422) return undefined;
  const m = e.fields[field];
  return m ? `${subject} ${m}.` : undefined;
}

function Errors({ errors }: { errors: unknown[] }) {
  const list = errors.filter(Boolean).map(String);
  return list.length > 0 ? <FieldError>{list.join(" ")}</FieldError> : null;
}

// ---- Deployment branch policies ----

export function BranchPolicies(env: Env) {
  const { data: environment } = useSuspenseQuery(environmentQuery(env));
  if (!environment.deployment_branch_policy?.custom_branch_policies) return null;
  return <PolicyList {...env} />;
}

function PolicyList(env: Env) {
  const query = branchPoliciesQuery(env);
  const { data: policies } = useSuspenseQuery(query);
  const [editing, setEditing] = useState<number>();
  const remove = useOptimisticWrite({
    key: query.queryKey,
    invalidate: [query.queryKey],
    failure: "Rule not deleted",
    apply: (list: Policy[], p: Policy) => list.filter((x) => x.id !== p.id),
    write: (p: Policy) =>
      send(
        api.DELETE(
          "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies/{branch_policy_id}",
          { params: { path: { ...path(env), branch_policy_id: p.id! } } },
        ),
      ),
  });
  return (
    <Panel flush title="Allowed branches and tags" className="">
      <ul className="divide-y text-sm" data-testid="branch-policies">
        {policies.length === 0 && (
          <li className="text-muted-foreground px-3 py-3">
            No rules yet, so nothing can deploy. Add a branch or tag name pattern.
          </li>
        )}
        {policies.map((p) =>
          editing === p.id ? (
            <li key={p.id} className="px-3 py-2">
              <PolicyForm {...env} policy={p} onDone={() => setEditing(undefined)} />
            </li>
          ) : (
            <li key={p.id} className="flex items-center gap-2 px-3 py-2" data-policy={p.name}>
              <HugeiconsIcon
                icon={p.type === "tag" ? Tag01Icon : GitBranchIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <code className="min-w-0 flex-1 truncate font-mono text-xs">{p.name}</code>
              <span className="text-muted-foreground text-xs capitalize">{p.type}</span>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Edit ${p.name}`}
                onClick={() => setEditing(p.id)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Delete ${p.name}`}
                onClick={() => remove.mutate(p)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ),
        )}
        <li className="px-3 py-2">
          <PolicyForm {...env} />
        </li>
      </ul>
    </Panel>
  );
}

// Adds a pattern, or renames one when `policy` is given. GHES cannot change a rule's type.
function PolicyForm({ policy, onDone, ...env }: Env & { policy?: Policy; onDone?: () => void }) {
  const queryClient = useQueryClient();
  const key = branchPoliciesQuery(env).queryKey;
  const form = useForm({
    defaultValues: {
      name: policy?.name ?? "",
      type: (policy?.type ?? "branch") as "branch" | "tag",
    },
    onSubmit: async ({ value, formApi }) => {
      const name = value.name.trim();
      const before = queryClient.getQueryData<Policy[]>(key);
      queryClient.setQueryData<Policy[]>(key, (list = []) =>
        policy
          ? list.map((p) => (p.id === policy.id ? { ...p, name } : p))
          : [...list, { id: -Date.now(), name, type: value.type }],
      );
      try {
        if (policy) {
          await send(
            api.PUT(
              "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies/{branch_policy_id}",
              { params: { path: { ...path(env), branch_policy_id: policy.id! } }, body: { name } },
            ),
          );
        } else {
          await send(
            api.POST(
              "/repos/{owner}/{repo}/environments/{environment_name}/deployment-branch-policies",
              {
                params: { path: path(env) },
                body: { name, type: value.type },
              },
            ),
          );
          formApi.reset();
        }
        onDone?.();
      } catch (e) {
        queryClient.setQueryData(key, before);
        const m = fieldMessage(e, "name", "A rule for this pattern");
        if (m) formApi.setFieldMeta("name", serverError(m));
        toast.error(policy ? "Rule not saved" : "Rule not added", { description: errorText(e) });
      } finally {
        void queryClient.invalidateQueries({ queryKey: key });
      }
    },
  });
  return (
    <form
      className="flex flex-wrap items-start gap-2"
      data-testid={policy ? "policy-edit" : "policy-add"}
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      {!policy && (
        <form.Field name="type">
          {(field) => (
            <NativeSelect
              size="sm"
              aria-label="Rule type"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value as "branch" | "tag")}
            >
              <NativeSelectOption value="branch">Branch</NativeSelectOption>
              <NativeSelectOption value="tag">Tag</NativeSelectOption>
            </NativeSelect>
          )}
        </form.Field>
      )}
      <form.Field
        name="name"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Enter a name pattern."),
        }}
      >
        {(field) => (
          <Field
            className="min-w-48 flex-1"
            data-invalid={field.state.meta.errors.length > 0 || undefined}
          >
            <Input
              aria-label="Name pattern"
              placeholder="e.g. release/*"
              className="h-7 font-mono text-xs"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <Errors errors={field.state.meta.errors} />
          </Field>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <Button type="submit" size="sm" variant="outline" disabled={submitting}>
            {policy ? "Save" : "Add rule"}
          </Button>
        )}
      </form.Subscribe>
      {policy && (
        <Button type="button" size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      )}
    </form>
  );
}

// ---- Secrets ----

// A secret's value is sealed to the environment's public key before it leaves the browser.
// The mock's key is not a real X25519 key, so this stands in for libsodium's sealed box with
// the same inputs (key id and key) and the same base64 output shape.
async function seal(value: string, key: Schemas["actions-public-key"]) {
  const data = new TextEncoder().encode(`${key.key}:${value}`);
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", data));
  return btoa(
    String.fromCharCode(...digest, ...new TextEncoder().encode(value).map((b) => b ^ 0x5a)),
  );
}

export function Secrets(env: Env) {
  const query = envSecretsQuery(env);
  const { data: secrets } = useSuspenseQuery(query);
  const [editing, setEditing] = useState<string>();
  const remove = useOptimisticWrite({
    key: query.queryKey,
    invalidate: [query.queryKey],
    failure: "Secret not deleted",
    apply: (list: Secret[], s: Secret) => list.filter((x) => x.name !== s.name),
    write: (s: Secret) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/environments/{environment_name}/secrets/{secret_name}", {
          params: { path: { ...path(env), secret_name: s.name } },
        }),
      ),
    onSuccess: (_r, s) => toast.success(`Deleted secret ${s.name}`),
  });
  return (
    <Panel flush title="Environment secrets">
      <ul className="divide-y text-sm" data-testid="secrets">
        {secrets.length === 0 && <li className="text-muted-foreground px-3 py-3">No secrets.</li>}
        {secrets.map((s) =>
          editing === s.name ? (
            <li key={s.name} className="px-3 py-2">
              <SecretForm {...env} existing={s.name} onDone={() => setEditing(undefined)} />
            </li>
          ) : (
            <li key={s.name} className="flex items-center gap-2 px-3 py-2" data-secret={s.name}>
              <HugeiconsIcon
                icon={Key01Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <code className="min-w-0 flex-1 truncate font-mono text-xs">{s.name}</code>
              <span className="text-muted-foreground text-xs">
                Updated <Ago at={s.updated_at} />
              </span>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Update ${s.name}`}
                onClick={() => setEditing(s.name)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Delete ${s.name}`}
                onClick={() => remove.mutate(s)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ),
        )}
        <li className="px-3 py-2">
          <SecretForm {...env} />
        </li>
      </ul>
    </Panel>
  );
}

function SecretForm({
  existing,
  onDone,
  ...env
}: Env & { existing?: string; onDone?: () => void }) {
  const queryClient = useQueryClient();
  const key = envSecretsQuery(env).queryKey;
  const form = useForm({
    defaultValues: { name: existing ?? "", value: "" },
    onSubmit: async ({ value, formApi }) => {
      const name = value.name.trim().toUpperCase();
      const before = queryClient.getQueryData<Secret[]>(key);
      const now = new Date().toISOString();
      queryClient.setQueryData<Secret[]>(key, (list = []) =>
        list.some((s) => s.name === name)
          ? list.map((s) => (s.name === name ? { ...s, updated_at: now } : s))
          : [...list, { name, created_at: now, updated_at: now }],
      );
      try {
        const publicKey = await queryClient.fetchQuery(envPublicKeyQuery(env));
        await send(
          api.PUT("/repos/{owner}/{repo}/environments/{environment_name}/secrets/{secret_name}", {
            params: { path: { ...path(env), secret_name: name } },
            body: { encrypted_value: await seal(value.value, publicKey), key_id: publicKey.key_id },
          }),
        );
        toast.success(existing ? `Updated secret ${name}` : `Added secret ${name}`);
        formApi.reset();
        onDone?.();
      } catch (e) {
        queryClient.setQueryData(key, before);
        const m = fieldMessage(e, "name", "Secret name");
        if (m) formApi.setFieldMeta("name", serverError(m));
        toast.error("Secret not saved", { description: errorText(e) });
      } finally {
        void queryClient.invalidateQueries({ queryKey: key });
      }
    },
  });
  return (
    <form
      className="flex flex-col gap-2"
      data-testid={existing ? "secret-edit" : "secret-add"}
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <div className="flex flex-wrap items-start gap-2">
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) => (value ? nameProblem(value) : undefined),
            onSubmit: ({ value }) => nameProblem(value),
          }}
        >
          {(field) => (
            <Field className="w-56" data-invalid={field.state.meta.errors.length > 0 || undefined}>
              <Input
                aria-label="Secret name"
                placeholder="SECRET_NAME"
                className="h-7 font-mono text-xs uppercase"
                disabled={Boolean(existing)}
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
              />
              <Errors errors={field.state.meta.errors} />
            </Field>
          )}
        </form.Field>
        <form.Field
          name="value"
          validators={{ onSubmit: ({ value }) => (value ? undefined : "Enter a value.") }}
        >
          {(field) => (
            <Field
              className="min-w-56 flex-1"
              data-invalid={field.state.meta.errors.length > 0 || undefined}
            >
              <Textarea
                aria-label="Secret value"
                placeholder={existing ? "New value. The old value is never shown." : "Value"}
                className="min-h-8 font-mono text-xs"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
              />
              <Errors errors={field.state.meta.errors} />
            </Field>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" size="sm" variant="outline" disabled={submitting}>
              {existing ? "Update secret" : "Add secret"}
            </Button>
          )}
        </form.Subscribe>
        {existing && (
          <Button type="button" size="sm" variant="ghost" onClick={onDone}>
            Cancel
          </Button>
        )}
      </div>
    </form>
  );
}

// ---- Variables ----

export function Variables(env: Env) {
  const query = envVariablesQuery(env);
  const { data: variables } = useSuspenseQuery(query);
  const [editing, setEditing] = useState<string>();
  const remove = useOptimisticWrite({
    key: query.queryKey,
    invalidate: [query.queryKey],
    failure: "Variable not deleted",
    apply: (list: Variable[], v: Variable) => list.filter((x) => x.name !== v.name),
    write: (v: Variable) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/environments/{environment_name}/variables/{name}", {
          params: { path: { ...path(env), name: v.name } },
        }),
      ),
    onSuccess: (_r, v) => toast.success(`Deleted variable ${v.name}`),
  });
  return (
    <Panel flush title="Environment variables">
      <ul className="divide-y text-sm" data-testid="variables">
        {variables.length === 0 && (
          <li className="text-muted-foreground px-3 py-3">No variables.</li>
        )}
        {variables.map((v) =>
          editing === v.name ? (
            <li key={v.name} className="px-3 py-2">
              <VariableForm {...env} existing={v} onDone={() => setEditing(undefined)} />
            </li>
          ) : (
            <li key={v.name} className="flex items-center gap-2 px-3 py-2" data-variable={v.name}>
              <HugeiconsIcon
                icon={VariableIcon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <code className="w-48 shrink-0 truncate font-mono text-xs">{v.name}</code>
              <code className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">
                {v.value}
              </code>
              <span className="text-muted-foreground text-xs">
                <Ago at={v.updated_at} />
              </span>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Edit ${v.name}`}
                onClick={() => setEditing(v.name)}
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                size="icon-xs"
                variant="ghost"
                aria-label={`Delete ${v.name}`}
                onClick={() => remove.mutate(v)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ),
        )}
        <li className="px-3 py-2">
          <VariableForm {...env} />
        </li>
      </ul>
    </Panel>
  );
}

function VariableForm({
  existing,
  onDone,
  ...env
}: Env & { existing?: Variable; onDone?: () => void }) {
  const queryClient = useQueryClient();
  const key = envVariablesQuery(env).queryKey;
  const form = useForm({
    defaultValues: { name: existing?.name ?? "", value: existing?.value ?? "" },
    onSubmit: async ({ value, formApi }) => {
      const name = value.name.trim().toUpperCase();
      const before = queryClient.getQueryData<Variable[]>(key);
      const now = new Date().toISOString();
      queryClient.setQueryData<Variable[]>(key, (list = []) =>
        existing
          ? list.map((v) =>
              v.name === existing.name ? { ...v, name, value: value.value, updated_at: now } : v,
            )
          : [...list, { name, value: value.value, created_at: now, updated_at: now }],
      );
      try {
        if (existing) {
          await send(
            api.PATCH("/repos/{owner}/{repo}/environments/{environment_name}/variables/{name}", {
              params: { path: { ...path(env), name: existing.name } },
              body: { name, value: value.value },
            }),
          );
        } else {
          await send(
            api.POST("/repos/{owner}/{repo}/environments/{environment_name}/variables", {
              params: { path: path(env) },
              body: { name, value: value.value },
            }),
          );
          formApi.reset();
        }
        toast.success(existing ? `Updated variable ${name}` : `Added variable ${name}`);
        onDone?.();
      } catch (e) {
        queryClient.setQueryData(key, before);
        const m = fieldMessage(e, "name", "A variable with this name");
        if (m) formApi.setFieldMeta("name", serverError(m));
        toast.error("Variable not saved", { description: errorText(e) });
      } finally {
        void queryClient.invalidateQueries({ queryKey: key });
      }
    },
  });
  return (
    <form
      className="flex flex-wrap items-start gap-2"
      data-testid={existing ? "variable-edit" : "variable-add"}
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <form.Field
        name="name"
        validators={{
          onChange: ({ value }) => (value ? nameProblem(value) : undefined),
          onSubmit: ({ value }) => nameProblem(value),
        }}
      >
        {(field) => (
          <Field className="w-56" data-invalid={field.state.meta.errors.length > 0 || undefined}>
            <Input
              aria-label="Variable name"
              placeholder="VARIABLE_NAME"
              className="h-7 font-mono text-xs uppercase"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <Errors errors={field.state.meta.errors} />
          </Field>
        )}
      </form.Field>
      <form.Field
        name="value"
        validators={{ onSubmit: ({ value }) => (value ? undefined : "Enter a value.") }}
      >
        {(field) => (
          <Field
            className="min-w-48 flex-1"
            data-invalid={field.state.meta.errors.length > 0 || undefined}
          >
            <Input
              aria-label="Variable value"
              placeholder="Value"
              className="h-7 font-mono text-xs"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <Errors errors={field.state.meta.errors} />
          </Field>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.isSubmitting}>
        {(submitting) => (
          <Button type="submit" size="sm" variant="outline" disabled={submitting}>
            {existing ? "Save" : "Add variable"}
          </Button>
        )}
      </form.Subscribe>
      {existing && (
        <Button type="button" size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
      )}
    </form>
  );
}
