import { useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Add01Icon, Delete02Icon, LockIcon, PencilEdit02Icon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { Ago } from "@/components/github";
import { Panel } from "@/components/panel";
import { Region } from "@/components/region";
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
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Textarea } from "@/components/ui/textarea";
import { orgReposQuery, orgSecretReposQuery, orgVariableReposQuery } from "./queries";
import { sealSecret } from "./sealed-box";
import {
  publicKeyQuery,
  scopeKey,
  secretsQuery,
  variablesQuery,
  type ConfigRow,
  type Scope,
} from "./scope";
import { fieldError, send, useWrite } from "./write";

type Visibility = "all" | "private" | "selected";
type Kind = "secret" | "variable";

const NAME = /^[A-Za-z_][A-Za-z0-9_]*$/;
const validateName = (v: string) => {
  const n = v.trim();
  if (!n) return "Name is required";
  if (!NAME.test(n)) return "Use letters, digits, and underscores, not starting with a digit";
  if (/^GITHUB_/i.test(n)) return "Names may not start with GITHUB_";
  return undefined;
};

const VISIBILITY: Record<Visibility, string> = {
  all: "All repositories",
  private: "Private and internal repositories",
  selected: "Selected repositories",
};

// One list for secrets or variables, at a repository or the organization.
export function ConfigList({ scope, kind }: { scope: Scope; kind: Kind }) {
  const query = kind === "secret" ? secretsQuery(scope) : variablesQuery(scope);
  const { data } = useSuspenseQuery(query);
  const [editing, setEditing] = useState<{
    name?: string;
    value?: string;
    visibility?: Visibility;
  } | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const queryClient = useQueryClient();
  const title = `${scope.kind === "org" ? "Organization" : "Repository"} ${kind}s`;

  const remove = useWrite({
    run: (name: string) =>
      send(
        scope.kind === "repo"
          ? kind === "secret"
            ? api.DELETE("/repos/{owner}/{repo}/actions/secrets/{secret_name}", {
                params: { path: { owner: scope.owner, repo: scope.repo, secret_name: name } },
              })
            : api.DELETE("/repos/{owner}/{repo}/actions/variables/{name}", {
                params: { path: { owner: scope.owner, repo: scope.repo, name } },
              })
          : kind === "secret"
            ? api.DELETE("/orgs/{org}/actions/secrets/{secret_name}", {
                params: { path: { org: scope.org, secret_name: name } },
              })
            : api.DELETE("/orgs/{org}/actions/variables/{name}", {
                params: { path: { org: scope.org, name } },
              }),
      ),
    invalidates: () => [query.queryKey],
    optimistic: (name) => {
      const before = queryClient.getQueryData(query.queryKey);
      queryClient.setQueryData(query.queryKey, (list) => list?.filter((x) => x.name !== name));
      return () => queryClient.setQueryData(query.queryKey, before);
    },
    success: (name) => `Deleted ${name}`,
    failure: (name) => `Could not delete ${name}`,
  });

  return (
    <Panel
      title={title}
      flush
      actions={
        <Button size="sm" onClick={() => setEditing({})}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          New {kind}
        </Button>
      }
    >
      {data.length === 0 ? (
        <p
          className="text-muted-foreground px-4 py-6 text-center text-sm"
          data-testid={`${kind}s-empty`}
        >
          There are no {kind}s for this {scope.kind === "org" ? "organization" : "repository"}.
        </p>
      ) : (
        <ul className="divide-y" data-testid={`${kind}s`}>
          {data.map((x) => (
            <li
              key={x.name}
              className="flex items-center gap-3 px-4 py-2 text-sm"
              data-name={x.name}
            >
              {kind === "secret" && (
                <HugeiconsIcon
                  icon={LockIcon}
                  strokeWidth={2}
                  className="text-muted-foreground size-4"
                />
              )}
              <code className="font-mono text-xs font-semibold">{x.name}</code>
              <code className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">
                {x.value}
              </code>
              {x.visibility && (
                <Badge variant="outline" size="sm">
                  {VISIBILITY[x.visibility]}
                </Badge>
              )}
              <span className="text-muted-foreground hidden text-xs sm:inline">
                Updated <Ago at={x.updated_at} />
              </span>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Edit ${x.name}`}
                onClick={() =>
                  setEditing({
                    name: x.name,
                    value: x.value ?? undefined,
                    visibility: x.visibility ?? undefined,
                  })
                }
              >
                <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={`Delete ${x.name}`}
                onClick={() => setConfirm(x.name)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {editing && (
        <Region name={`${kind} form`} fallback={null}>
          <ConfigDialog
            scope={scope}
            kind={kind}
            initial={editing}
            onClose={() => setEditing(null)}
          />
        </Region>
      )}
      <Dialog open={confirm !== null} onOpenChange={(o) => !o && setConfirm(null)}>
        <DialogContent className="rounded-sm">
          <DialogHeader>
            <DialogTitle>Delete {confirm}?</DialogTitle>
            <DialogDescription>
              Workflows that read it get an empty value from now on. This cannot be undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter className="rounded-b-sm">
            <Button variant="outline" onClick={() => setConfirm(null)}>
              Cancel
            </Button>
            <Button
              variant="destructive"
              onClick={() => {
                if (confirm) remove.mutate(confirm);
                setConfirm(null);
              }}
            >
              Delete {kind}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </Panel>
  );
}

type Draft = { name: string; value: string; visibility: Visibility; repos: number[] };

function ConfigDialog({
  scope,
  kind,
  initial,
  onClose,
}: {
  scope: Scope;
  kind: Kind;
  initial: { name?: string; value?: string; visibility?: Visibility };
  onClose: () => void;
}) {
  const editing = initial.name !== undefined;
  const queryClient = useQueryClient();
  // An org entry with selected access starts from the repositories it has now.
  const { data: selected } = useSuspenseQuery({
    queryKey: [...scopeKey(scope), `${kind}s`, initial.name ?? "", "repositories", "seed"],
    queryFn: async () => {
      if (scope.kind !== "org" || !initial.name || initial.visibility !== "selected") return [];
      const q =
        kind === "secret"
          ? orgSecretReposQuery(scope.org, initial.name)
          : orgVariableReposQuery(scope.org, initial.name);
      return (await queryClient.fetchQuery(q)).repositories.map((r) => r.id);
    },
    staleTime: 0,
  });
  const [sealedNote, setSealedNote] = useState<string>();
  const listKey = (kind === "secret" ? secretsQuery(scope) : variablesQuery(scope)).queryKey;

  const save = useWrite({
    run: async (d: Draft) => {
      const name = d.name.trim().toUpperCase();
      const org = scope.kind === "org" ? scope.org : "";
      if (kind === "secret") {
        // The GHES flow: read the scope's public key, seal the value to it in the
        // browser, and send only the box. The value never leaves unsealed.
        const key = await queryClient.fetchQuery(publicKeyQuery(scope));
        const encrypted_value = sealSecret(d.value, key.key);
        setSealedNote(
          `Sealed to key ${key.key_id}: ${encrypted_value.length} base64 characters sent.`,
        );
        if (scope.kind === "repo")
          return send(
            api.PUT("/repos/{owner}/{repo}/actions/secrets/{secret_name}", {
              params: { path: { owner: scope.owner, repo: scope.repo, secret_name: name } },
              body: { encrypted_value, key_id: key.key_id },
            }),
          );
        return send(
          api.PUT("/orgs/{org}/actions/secrets/{secret_name}", {
            params: { path: { org, secret_name: name } },
            body: {
              encrypted_value,
              key_id: key.key_id,
              visibility: d.visibility,
              selected_repository_ids: d.visibility === "selected" ? d.repos : undefined,
            },
          }),
        );
      }
      if (scope.kind === "repo")
        return editing
          ? send(
              api.PATCH("/repos/{owner}/{repo}/actions/variables/{name}", {
                params: { path: { owner: scope.owner, repo: scope.repo, name: initial.name! } },
                body: { name, value: d.value },
              }),
            )
          : send(
              api.POST("/repos/{owner}/{repo}/actions/variables", {
                params: { path: { owner: scope.owner, repo: scope.repo } },
                body: { name, value: d.value },
              }),
            );
      const selected = d.visibility === "selected" ? d.repos : undefined;
      return editing
        ? send(
            api.PATCH("/orgs/{org}/actions/variables/{name}", {
              params: { path: { org, name: initial.name! } },
              body: {
                name,
                value: d.value,
                visibility: d.visibility,
                selected_repository_ids: selected,
              },
            }),
          )
        : send(
            api.POST("/orgs/{org}/actions/variables", {
              params: { path: { org } },
              body: {
                name,
                value: d.value,
                visibility: d.visibility,
                selected_repository_ids: selected,
              },
            }),
          );
    },
    invalidates: () => [listKey, [...scopeKey(scope)]],
    // A variable shows its new value at once; a secret has no value to show.
    optimistic: (d) => {
      const before = queryClient.getQueryData(listKey);
      const now = new Date().toISOString();
      const name = d.name.trim().toUpperCase();
      queryClient.setQueryData(listKey, (list) => {
        if (!list) return list;
        const old = list.find((x) => x.name === (initial.name ?? name));
        const row: ConfigRow = {
          name,
          value: kind === "secret" ? null : d.value,
          visibility: scope.kind === "org" ? d.visibility : null,
          created_at: old?.created_at ?? now,
          updated_at: now,
        };
        return [...list.filter((x) => x !== old), row].sort((a, b) => a.name.localeCompare(b.name));
      });
      return () => queryClient.setQueryData(listKey, before);
    },
    success: (d) =>
      `${kind === "secret" ? "Secret" : "Variable"} ${d.name.trim().toUpperCase()} ${editing ? "updated" : "added"}`,
    failure: (d) => `Could not save ${d.name.trim().toUpperCase() || kind}`,
  });

  const form = useForm({
    defaultValues: {
      name: initial.name ?? "",
      value: kind === "variable" ? (initial.value ?? "") : "",
      visibility: initial.visibility ?? "all",
      repos: selected,
    } as Draft,
    onSubmit: async ({ value }) => {
      await save.mutateAsync(value).then(onClose, () => undefined);
    },
  });

  const serverName = fieldError(save.error, "name");
  const serverValue = fieldError(save.error, kind === "secret" ? "encrypted_value" : "value");
  const serverRepos = fieldError(save.error, "selected_repository_ids");

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            {editing
              ? `Update ${initial.name}`
              : `New ${scope.kind === "org" ? "organization" : "repository"} ${kind}`}
          </DialogTitle>
          <DialogDescription>
            {kind === "secret"
              ? "The value is encrypted in your browser with a sealed box before it is sent. No one can read it back, including you."
              : "Variables are stored and shown as plain text. Do not put credentials in them."}
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-3"
          data-testid={`${kind}-form`}
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field name="name" validators={{ onSubmit: ({ value }) => validateName(value) }}>
            {(field) => {
              const error = field.state.meta.errors[0] ?? serverName;
              return (
                <Field data-invalid={Boolean(error) || undefined}>
                  <FieldLabel htmlFor={`${kind}-name`}>Name</FieldLabel>
                  <Input
                    id={`${kind}-name`}
                    className="rounded-sm font-mono uppercase"
                    placeholder="YOUR_NAME"
                    readOnly={editing && kind === "secret"}
                    value={field.state.value}
                    aria-invalid={Boolean(error) || undefined}
                    onChange={(e) => field.handleChange(e.target.value)}
                  />
                  {error && <FieldError>{String(error)}</FieldError>}
                </Field>
              );
            }}
          </form.Field>
          <form.Field
            name="value"
            validators={{
              onSubmit: ({ value }) =>
                value ? undefined : `${kind === "secret" ? "Secret" : "Value"} is required`,
            }}
          >
            {(field) => {
              const error = field.state.meta.errors[0] ?? serverValue;
              return (
                <Field data-invalid={Boolean(error) || undefined}>
                  <FieldLabel htmlFor={`${kind}-value`}>
                    {kind === "secret" ? "Secret" : "Value"}
                  </FieldLabel>
                  <Textarea
                    id={`${kind}-value`}
                    className="min-h-20 rounded-sm font-mono text-xs"
                    placeholder={
                      editing && kind === "secret"
                        ? "Enter a new value. The current one is never shown."
                        : ""
                    }
                    value={field.state.value}
                    aria-invalid={Boolean(error) || undefined}
                    onChange={(e) => field.handleChange(e.target.value)}
                  />
                  {error && <FieldError>{String(error)}</FieldError>}
                </Field>
              );
            }}
          </form.Field>
          {scope.kind === "org" && (
            <form.Field name="visibility">
              {(field) => (
                <Field>
                  <FieldLabel>Repository access</FieldLabel>
                  <RadioGroup
                    value={field.state.value}
                    onValueChange={(v) => field.handleChange(v as Visibility)}
                  >
                    {(Object.keys(VISIBILITY) as Visibility[]).map((v) => (
                      <Field key={v} orientation="horizontal">
                        <RadioGroupItem value={v} id={`vis-${v}`} />
                        <FieldLabel htmlFor={`vis-${v}`} className="font-normal">
                          {VISIBILITY[v]}
                        </FieldLabel>
                      </Field>
                    ))}
                  </RadioGroup>
                </Field>
              )}
            </form.Field>
          )}
          {scope.kind === "org" && (
            <form.Subscribe selector={(s) => s.values.visibility}>
              {(visibility) =>
                visibility === "selected" && (
                  <form.Field
                    name="repos"
                    validators={{
                      onSubmit: ({ value }) =>
                        value.length ? undefined : "Choose at least one repository",
                    }}
                  >
                    {(field) => (
                      <RepoPicker
                        org={scope.org}
                        value={field.state.value}
                        onChange={field.handleChange}
                        error={field.state.meta.errors[0] ?? serverRepos}
                      />
                    )}
                  </form.Field>
                )
              }
            </form.Subscribe>
          )}
          {kind === "secret" && sealedNote && (
            <FieldDescription className="font-mono text-xs" data-testid="sealed-note">
              {sealedNote}
            </FieldDescription>
          )}
          <DialogFooter className="rounded-b-sm">
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <form.Subscribe selector={(s) => s.isSubmitting}>
              {(submitting) => (
                <Button type="submit" disabled={submitting}>
                  {submitting
                    ? kind === "secret"
                      ? "Encrypting…"
                      : "Saving…"
                    : editing
                      ? `Update ${kind}`
                      : `Add ${kind}`}
                </Button>
              )}
            </form.Subscribe>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// The organization's repositories with a checkbox each.
export function RepoPicker({
  org,
  value,
  onChange,
  error,
}: {
  org: string;
  value: number[];
  onChange: (ids: number[]) => void;
  error?: unknown;
}) {
  const { data: repos } = useSuspenseQuery(orgReposQuery(org));
  return (
    <Field data-invalid={Boolean(error) || undefined}>
      <FieldLabel>Repositories</FieldLabel>
      <div className="flex flex-col gap-1.5 rounded-sm border p-2" data-testid="repo-picker">
        {repos.map((r) => (
          <Field key={r.id} orientation="horizontal">
            <Checkbox
              id={`repo-${r.id}`}
              checked={value.includes(r.id)}
              onCheckedChange={(on) =>
                onChange(on ? [...value, r.id] : value.filter((x) => x !== r.id))
              }
            />
            <FieldLabel htmlFor={`repo-${r.id}`} className="font-normal">
              {r.full_name}
              <Badge variant="outline" size="sm" className="capitalize">
                {r.visibility}
              </Badge>
            </FieldLabel>
          </Field>
        ))}
      </div>
      {Boolean(error) && <FieldError>{String(error)}</FieldError>}
    </Field>
  );
}
