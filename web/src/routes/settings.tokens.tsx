import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Delete02Icon,
  LockPasswordIcon,
  PlusSignIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import { userOrgsQuery, userQuery, userReposQuery } from "@/areas/core/queries";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  FieldErrors,
  formatDate,
  OneTimeSecret,
  SubmitButton,
  TextField,
  useOptimistic,
} from "@/areas/account/kit";
import { permissionSummary } from "@/areas/account/permissions";
import { call, tokensQuery } from "@/areas/account/queries";
import {
  ExpiryPicker,
  expiryFor,
  expiryText,
  normalizeScopes,
  PermissionMatrix,
  RepoPicker,
  ScopeTree,
  type ExpiryChoice,
} from "@/areas/account/token-fields";
import {
  ui,
  type AppPermissions,
  type IssuedToken,
  type NewToken,
  type Token,
} from "@/areas/account/ui-api";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["fine-grained", "classic"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/settings/tokens")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; new?: boolean } => ({
    tab: TABS.includes(s.tab as Tab) ? (s.tab as Tab) : undefined,
    new: s.new === true || s.new === "true" ? true : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(tokensQuery());
    void queryClient.prefetchQuery(userReposQuery());
  },
  component: TokensPage,
});

function TokensPage() {
  const { tab = "fine-grained", new: creating } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [issued, setIssued] = useState<IssuedToken>();

  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Personal access tokens"
        description="Tokens act as you over the API and git over HTTPS. Fine-grained tokens are limited to one owner, chosen repositories, and chosen permissions."
        actions={
          !creating && (
            <Button size="sm" onClick={() => void navigate({ search: { tab, new: true } })}>
              <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
              Generate new token
            </Button>
          )
        }
      />
      <Tabs value={tab} onValueChange={(t) => void navigate({ search: { tab: t as Tab } })}>
        <TabsList variant="line" className="border-b">
          <TabsTrigger value="fine-grained">Fine-grained tokens</TabsTrigger>
          <TabsTrigger value="classic">Tokens (classic)</TabsTrigger>
        </TabsList>
      </Tabs>
      {issued && (
        <OneTimeSecret
          title={`Your new token${issued.token.kind === "classic" ? ` "${issued.token.note}"` : ` "${issued.token.name}"`}`}
          value={issued.value}
          note="Copy it now. You won't be able to see it again; this page keeps only its last eight characters."
          onDismiss={() => setIssued(undefined)}
        />
      )}
      {creating ? (
        <Region name="new token" rows={8}>
          {tab === "fine-grained" ? (
            <FineGrainedForm
              onCreated={(t) => (setIssued(t), void navigate({ search: { tab } }))}
            />
          ) : (
            <ClassicForm onCreated={(t) => (setIssued(t), void navigate({ search: { tab } }))} />
          )}
          <Button
            size="sm"
            variant="ghost"
            className="self-start"
            onClick={() => void navigate({ search: { tab } })}
          >
            Cancel
          </Button>
        </Region>
      ) : (
        <Region name="tokens" rows={5}>
          <TokenList tab={tab} onIssued={setIssued} />
        </Region>
      )}
    </div>
  );
}

function TokenList({ tab, onIssued }: { tab: Tab; onIssued: (t: IssuedToken) => void }) {
  const { data } = useSuspenseQuery(tokensQuery());
  const [regenerating, setRegenerating] = useState<Token>();
  const tokens = data.filter((t) =>
    tab === "classic" ? t.kind === "classic" : t.kind === "fine_grained",
  );
  const remove = useOptimistic<Token[], number, void>({
    queryKey: tokensQuery().queryKey,
    what: "Deleting the token",
    mutationFn: (id) => call(ui.DELETE("/settings/tokens/{id}", { params: { path: { id } } })),
    apply: (old, id) => old.filter((t) => t.id !== id),
    onSuccess: () => toast.success("Token deleted. Anything using it stops working."),
  });

  return (
    <>
      <Panel flush>
        {tokens.length === 0 ? (
          <EmptyRow>
            No {tab === "classic" ? "classic" : "fine-grained"} tokens.{" "}
            <Link
              to="/settings/tokens"
              search={{ tab, new: true }}
              className="text-primary hover:underline"
            >
              Generate one
            </Link>
            .
          </EmptyRow>
        ) : (
          <ul className="divide-y" data-testid="token-list">
            {tokens.map((t) => {
              const expired =
                t.expires_at !== null && new Date(t.expires_at).getTime() < Date.now();
              const soon =
                !expired &&
                t.expires_at !== null &&
                new Date(t.expires_at).getTime() < Date.now() + 7 * 86_400_000;
              const label = t.kind === "classic" ? t.note : t.name;
              return (
                <li
                  key={t.id}
                  className="flex flex-wrap items-start gap-3 px-4 py-3"
                  data-token={label}
                >
                  <HugeiconsIcon
                    icon={LockPasswordIcon}
                    strokeWidth={2}
                    className="text-muted-foreground mt-0.5 size-4"
                  />
                  <div className="min-w-0 flex-1">
                    <p className="flex flex-wrap items-center gap-2 text-sm">
                      <span className="font-semibold">{label}</span>
                      <code
                        className="text-muted-foreground font-mono text-xs"
                        data-testid="last-eight"
                      >
                        {t.kind === "classic" ? "ghp_" : "github_pat_"}…{t.token_last_eight}
                      </code>
                      {expired && <Badge variant="destructive-light">Expired</Badge>}
                      {soon && <Badge variant="warning-light">Expires soon</Badge>}
                      {t.expires_at === null && (
                        <Badge variant="warning-outline">No expiration</Badge>
                      )}
                    </p>
                    <p className="text-muted-foreground text-xs">
                      {t.kind === "classic"
                        ? t.scopes.length
                          ? t.scopes.join(", ")
                          : "No scopes"
                        : `${t.resource_owner} · ${
                            t.repository_selection === "all"
                              ? "All repositories"
                              : t.repository_selection === "public"
                                ? "Public repositories"
                                : t.repositories.join(", ")
                          } · ${permissionSummary(t.permissions)}`}
                    </p>
                    <p className="text-muted-foreground text-xs">
                      {expired
                        ? `Expired on ${formatDate(t.expires_at)}`
                        : expiryText(t.expires_at)}{" "}
                      · {t.last_used_at ? `Last used ${formatDate(t.last_used_at)}` : "Never used"}
                    </p>
                  </div>
                  <div className="flex gap-1">
                    <Button size="sm" variant="outline" onClick={() => setRegenerating(t)}>
                      <HugeiconsIcon
                        icon={Refresh01Icon}
                        strokeWidth={2}
                        data-icon="inline-start"
                      />
                      Regenerate
                    </Button>
                    <Confirm
                      trigger={
                        <Button size="sm" variant="destructive" aria-label={`Delete ${label}`}>
                          <HugeiconsIcon
                            icon={Delete02Icon}
                            strokeWidth={2}
                            data-icon="inline-start"
                          />
                          Delete
                        </Button>
                      }
                      title={`Delete ${label}?`}
                      description="Any script or app using this token loses access at once."
                      action="Delete token"
                      onConfirm={() => remove.mutate(t.id)}
                    />
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </Panel>
      {regenerating && (
        <Regenerate
          token={regenerating}
          onClose={() => setRegenerating(undefined)}
          onIssued={(t) => {
            setRegenerating(undefined);
            onIssued(t);
          }}
        />
      )}
    </>
  );
}

function Regenerate({
  token,
  onClose,
  onIssued,
}: {
  token: Token;
  onClose: () => void;
  onIssued: (t: IssuedToken) => void;
}) {
  const queryClient = useQueryClient();
  const [choice, setChoice] = useState<ExpiryChoice>("30");
  const [custom, setCustom] = useState<Date>();
  const regen = useMutation({
    mutationFn: () =>
      call(
        ui.POST("/settings/tokens/{id}/regenerate", {
          params: { path: { id: token.id } },
          body: { expires_at: expiryFor(choice, custom) },
        }),
      ),
    onError: (e) => failed("Regenerating the token", e),
    onSuccess: (t) => {
      void queryClient.invalidateQueries({ queryKey: tokensQuery().queryKey });
      onIssued(t);
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            Regenerate {token.kind === "classic" ? token.note : token.name}?
          </DialogTitle>
          <DialogDescription>
            The current value stops working at once. Anything using it needs the new value.
          </DialogDescription>
        </DialogHeader>
        <ExpiryPicker choice={choice} custom={custom} onChoice={setChoice} onCustom={setCustom} />
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            onClick={() => regen.mutate()}
            disabled={regen.isPending || (choice === "custom" && !custom)}
          >
            {regen.isPending ? "Regenerating…" : "Regenerate token"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function useCreate(onCreated: (t: IssuedToken) => void) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: NewToken) => call(ui.POST("/settings/tokens", { body })),
    onSuccess: (t) => {
      queryClient.setQueryData<Token[]>(tokensQuery().queryKey, (old) => [t.token, ...(old ?? [])]);
      void queryClient.invalidateQueries({ queryKey: tokensQuery().queryKey });
      onCreated(t);
    },
  });
}

function FineGrainedForm({ onCreated }: { onCreated: (t: IssuedToken) => void }) {
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const { data: repos } = useSuspenseQuery(userReposQuery());
  const create = useCreate(onCreated);
  const form = useForm({
    defaultValues: {
      name: "",
      description: "",
      resource_owner: viewer.login,
      expiry: "30" as ExpiryChoice,
      custom: undefined as Date | undefined,
      repository_selection: "selected" as "public" | "all" | "selected",
      repositories: [] as string[],
      permissions: {} as AppPermissions,
    },
    onSubmit: async ({ value }) => {
      try {
        await create.mutateAsync({
          kind: "fine_grained" as const,
          name: value.name.trim(),
          description: value.description.trim(),
          resource_owner: value.resource_owner,
          expires_at: expiryFor(value.expiry, value.custom),
          repository_selection: value.repository_selection,
          repositories: value.repository_selection === "selected" ? value.repositories : [],
          permissions: value.permissions,
        });
      } catch (error) {
        const placed = applyServerErrors(form, error, {
          name: "name",
          expires_at: "custom",
          repositories: "repositories",
          resource_owner: "resource_owner",
        });
        if (!placed) failed("Generating the token", error);
      }
    },
  });

  return (
    <Panel title="New fine-grained personal access token">
      <form
        noValidate
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="fine-grained-form"
      >
        <form.Field
          name="name"
          validators={{
            onChange: ({ value }) =>
              value.length > 40 ? "Token name is too long (maximum is 40 characters)" : undefined,
            onSubmit: ({ value }) => (value.trim() ? undefined : "Token name can't be blank"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Token name"
              description="A unique name for this token."
              autoComplete="off"
            />
          )}
        </form.Field>
        <form.Field name="description">
          {(field) => (
            <TextField
              field={field}
              label="Description"
              multiline
              placeholder="What is this token for?"
            />
          )}
        </form.Field>
        <form.Field name="resource_owner">
          {(field) => (
            <Field>
              <FieldLabel htmlFor="f-owner">Resource owner</FieldLabel>
              <NativeSelect
                id="f-owner"
                value={field.state.value}
                onChange={(e) => {
                  field.handleChange(e.target.value);
                  form.setFieldValue("repositories", []);
                }}
              >
                <NativeSelectOption value={viewer.login}>{viewer.login}</NativeSelectOption>
                {orgs.map((o) => (
                  <NativeSelectOption key={o.login} value={o.login}>
                    {o.login}
                  </NativeSelectOption>
                ))}
              </NativeSelect>
              <FieldErrors field={field} />
            </Field>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => [s.values.expiry, s.values.custom] as const}>
          {([expiry, custom]) => (
            <form.Field
              name="custom"
              validators={{
                onSubmitAsync: async () =>
                  form.getFieldValue("expiry") === "custom" && !form.getFieldValue("custom")
                    ? "Pick an expiration date"
                    : undefined,
              }}
            >
              {(field) => (
                <Field>
                  <FieldLabel>Expiration</FieldLabel>
                  <ExpiryPicker
                    choice={expiry}
                    custom={custom}
                    onChoice={(c) => form.setFieldValue("expiry", c)}
                    onCustom={(d) => {
                      field.setErrorMap({ onServer: undefined, onSubmit: undefined });
                      field.handleChange(d);
                    }}
                  />
                  <FieldErrors field={field} />
                </Field>
              )}
            </form.Field>
          )}
        </form.Subscribe>
        <form.Field name="repository_selection">
          {(field) => (
            <FieldSet>
              <FieldLegend variant="label">Repository access</FieldLegend>
              <RadioGroup
                value={field.state.value}
                onValueChange={(v) => field.handleChange(v as "public" | "all" | "selected")}
              >
                {[
                  ["public", "Public repositories", "Read-only access to public repositories."],
                  ["all", "All repositories", "Current and future repositories the owner owns."],
                  ["selected", "Only select repositories", "At least one repository, at most 50."],
                ].map(([v, label, hint]) => (
                  <label key={v} className="flex items-start gap-2 text-sm">
                    <RadioGroupItem value={v} className="mt-0.5" />
                    <span>
                      <span className="font-medium">{label}</span>
                      <span className="text-muted-foreground block text-xs">{hint}</span>
                    </span>
                  </label>
                ))}
              </RadioGroup>
            </FieldSet>
          )}
        </form.Field>
        <form.Subscribe
          selector={(s) => [s.values.repository_selection, s.values.resource_owner] as const}
        >
          {([selection, owner]) =>
            selection === "selected" && (
              <form.Field
                name="repositories"
                validators={{
                  onSubmit: ({ value }) =>
                    form.getFieldValue("repository_selection") === "selected" && value.length === 0
                      ? "Select at least one repository"
                      : undefined,
                }}
              >
                {(field) => (
                  <div className="flex flex-col gap-1">
                    <RepoPicker
                      repos={repos.filter((r) => r.owner.login === owner)}
                      selected={field.state.value}
                      onChange={(names) => {
                        field.setErrorMap({ onServer: undefined, onSubmit: undefined });
                        field.handleChange(names);
                      }}
                    />
                    <FieldErrors field={field} />
                  </div>
                )}
              </form.Field>
            )
          }
        </form.Subscribe>
        <form.Field name="permissions">
          {(field) => (
            <FieldSet>
              <FieldLegend variant="label">Permissions</FieldLegend>
              <PermissionMatrix
                value={field.state.value}
                onChange={field.handleChange}
                groups={["repository", "account"]}
              />
            </FieldSet>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => [s.isSubmitting, s.values.permissions] as const}>
          {([submitting, permissions]) => (
            <div className="flex items-center gap-3 border-t pt-3">
              <SubmitButton pending={submitting}>Generate token</SubmitButton>
              <span className="text-muted-foreground text-xs">
                {permissionSummary(permissions)}
              </span>
            </div>
          )}
        </form.Subscribe>
      </form>
    </Panel>
  );
}

function ClassicForm({ onCreated }: { onCreated: (t: IssuedToken) => void }) {
  const create = useCreate(onCreated);
  const form = useForm({
    defaultValues: {
      note: "",
      expiry: "30" as ExpiryChoice,
      custom: undefined as Date | undefined,
      scopes: [] as string[],
    },
    onSubmit: async ({ value }) => {
      try {
        await create.mutateAsync({
          kind: "classic" as const,
          note: value.note.trim(),
          expires_at: expiryFor(value.expiry, value.custom),
          scopes: normalizeScopes(value.scopes),
        });
      } catch (error) {
        if (
          !applyServerErrors(form, error, { note: "note", expires_at: "custom", scopes: "scopes" })
        ) {
          failed("Generating the token", error);
        }
      }
    },
  });
  return (
    <Panel title="New personal access token (classic)">
      <form
        noValidate
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
        data-testid="classic-form"
      >
        <form.Field
          name="note"
          validators={{
            onSubmit: ({ value }) => (value.trim() ? undefined : "Note can't be blank"),
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Note"
              description="What's this token for?"
              autoComplete="off"
            />
          )}
        </form.Field>
        <form.Subscribe selector={(s) => [s.values.expiry, s.values.custom] as const}>
          {([expiry, custom]) => (
            <form.Field name="custom">
              {(field) => (
                <Field>
                  <FieldLabel>Expiration</FieldLabel>
                  <ExpiryPicker
                    choice={expiry}
                    custom={custom}
                    onChoice={(c) => form.setFieldValue("expiry", c)}
                    onCustom={(d) => {
                      field.setErrorMap({ onServer: undefined });
                      field.handleChange(d);
                    }}
                  />
                  <FieldErrors field={field} />
                </Field>
              )}
            </form.Field>
          )}
        </form.Subscribe>
        <form.Field name="scopes">
          {(field) => (
            <FieldSet>
              <FieldLegend variant="label">Select scopes</FieldLegend>
              <p className="text-muted-foreground -mt-1 text-xs">
                Scopes define the access for personal tokens. With no scope, a token reads public
                information only.
              </p>
              <ScopeTree value={field.state.value} onChange={field.handleChange} />
              <FieldErrors field={field} />
            </FieldSet>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => [s.isSubmitting, s.values.scopes] as const}>
          {([submitting, scopes]) => (
            <div className="flex items-center gap-3 border-t pt-3">
              <SubmitButton pending={submitting}>Generate token</SubmitButton>
              <span className="text-muted-foreground text-xs" data-testid="scope-summary">
                {normalizeScopes(scopes).join(", ") || "No scopes"}
              </span>
            </div>
          )}
        </form.Subscribe>
      </form>
    </Panel>
  );
}
