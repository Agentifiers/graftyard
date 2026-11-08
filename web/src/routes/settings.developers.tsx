import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, Key02Icon, PlusSignIcon } from "@hugeicons/core-free-icons";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  formatDate,
  OneTimeSecret,
  Section,
  SubmitButton,
  TextField,
  useOptimistic,
} from "@/areas/account/kit";
import { call, devAppsQuery, oauthAppQuery, oauthAppsQuery } from "@/areas/account/queries";
import { ui, type OAuthApp, type OAuthAppInput } from "@/areas/account/ui-api";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["github-apps", "oauth-apps"] as const;
type Tab = (typeof TABS)[number];

// The OAuth app editor is a search param (`?oauth=<client_id>` or `?oauth=new`) so one
// route holds the whole OAuth Apps CRUD; GitHub Apps have their own pages.
export const Route = createFileRoute("/settings/developers")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; oauth?: string } => ({
    tab: TABS.includes(s.tab as Tab) ? (s.tab as Tab) : undefined,
    oauth: typeof s.oauth === "string" && s.oauth ? s.oauth : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(devAppsQuery());
    void queryClient.prefetchQuery(oauthAppsQuery());
  },
  component: DevelopersPage,
});

function DevelopersPage() {
  const { tab = "github-apps", oauth } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Developer settings" description="Apps you build on Graftyard's API." />
      <Tabs value={tab} onValueChange={(t) => void navigate({ search: { tab: t as Tab } })}>
        <TabsList variant="line" className="border-b">
          <TabsTrigger value="github-apps">GitHub Apps</TabsTrigger>
          <TabsTrigger value="oauth-apps">OAuth Apps</TabsTrigger>
        </TabsList>
      </Tabs>
      {tab === "github-apps" ? (
        <Region name="GitHub apps" rows={3}>
          <GitHubApps />
        </Region>
      ) : oauth === "new" ? (
        <NewOAuthApp />
      ) : oauth ? (
        <Region name="OAuth app" rows={6}>
          <EditOAuthApp clientId={oauth} />
        </Region>
      ) : (
        <Region name="OAuth apps" rows={3}>
          <OAuthApps />
        </Region>
      )}
    </div>
  );
}

function GitHubApps() {
  const { data } = useSuspenseQuery(devAppsQuery());
  return (
    <Section
      title="GitHub Apps"
      description="Apps that act on their own behalf, with fine-grained permissions and their own webhooks."
      actions={
        <Button size="sm" render={<Link to="/settings/apps/new" />}>
          <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
          New GitHub App
        </Button>
      }
    >
      <Panel flush>
        {data.length === 0 ? (
          <EmptyRow>You have not registered a GitHub App.</EmptyRow>
        ) : (
          <ul className="divide-y" data-testid="dev-apps">
            {data.map((a) => (
              <li key={a.slug} className="flex items-center gap-3 px-4 py-2.5">
                <div className="min-w-0 flex-1 text-sm">
                  <Link
                    to="/settings/apps/$slug"
                    params={{ slug: a.slug }}
                    className="font-semibold hover:underline"
                  >
                    {a.name}
                  </Link>
                  <p className="text-muted-foreground text-xs">
                    {a.description || "No description"} · {a.installations_count} installation
                    {a.installations_count === 1 ? "" : "s"}
                  </p>
                </div>
                <Badge variant="outline">{a.public ? "Any account" : "Only on this account"}</Badge>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </Section>
  );
}

function OAuthApps() {
  const { data } = useSuspenseQuery(oauthAppsQuery());
  return (
    <Section
      title="OAuth Apps"
      description="Apps that act as the people who authorize them, through scopes."
      actions={
        <Button
          size="sm"
          render={<Link to="/settings/developers" search={{ tab: "oauth-apps", oauth: "new" }} />}
        >
          <HugeiconsIcon icon={PlusSignIcon} strokeWidth={2} data-icon="inline-start" />
          New OAuth App
        </Button>
      }
    >
      <Panel flush>
        {data.length === 0 ? (
          <EmptyRow>You have not registered an OAuth App.</EmptyRow>
        ) : (
          <ul className="divide-y" data-testid="oauth-apps">
            {data.map((a) => (
              <li
                key={a.client_id}
                className="flex items-center gap-3 px-4 py-2.5"
                data-oauth-app={a.name}
              >
                <div className="min-w-0 flex-1 text-sm">
                  <Link
                    to="/settings/developers"
                    search={{ tab: "oauth-apps", oauth: a.client_id }}
                    className="font-semibold hover:underline"
                  >
                    {a.name}
                  </Link>
                  <p className="text-muted-foreground text-xs">
                    <code className="font-mono">{a.client_id}</code> · {a.users} user
                    {a.users === 1 ? "" : "s"}
                  </p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </Section>
  );
}

const SERVER_FIELDS = { name: "name", homepage_url: "homepage_url", callback_url: "callback_url" };

function OAuthAppForm({
  initial,
  submit,
  label,
}: {
  initial: OAuthAppInput;
  submit: (v: OAuthAppInput) => Promise<unknown>;
  label: string;
}) {
  const form = useForm({
    defaultValues: initial,
    onSubmit: async ({ value, formApi }) => {
      try {
        await submit({ ...value, name: value.name.trim() });
        formApi.reset(value);
      } catch (error) {
        if (!applyServerErrors(form, error, SERVER_FIELDS)) failed(label, error);
      }
    },
  });
  const url = (v: string) => (/^https?:\/\/\S+/.test(v) ? undefined : "Enter an http or https URL");
  return (
    <form
      noValidate
      className="flex max-w-xl flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="oauth-app-form"
    >
      <form.Field
        name="name"
        validators={{
          onSubmit: ({ value }) => (value.trim() ? undefined : "Application name can't be blank"),
        }}
      >
        {(field) => (
          <TextField
            field={field}
            label="Application name"
            description="Something people will recognize and trust."
          />
        )}
      </form.Field>
      <form.Field name="homepage_url" validators={{ onSubmit: ({ value }) => url(value) }}>
        {(field) => (
          <TextField
            field={field}
            label="Homepage URL"
            type="url"
            placeholder="https://example.com"
          />
        )}
      </form.Field>
      <form.Field name="description">
        {(field) => (
          <TextField
            field={field}
            label="Application description"
            multiline
            placeholder="Shown to people who authorize it"
          />
        )}
      </form.Field>
      <form.Field
        name="callback_url"
        validators={{
          onSubmit: ({ value }) =>
            /^[a-z][a-z0-9+.-]*:\/\/\S+/.test(value) ? undefined : "Enter a callback URL",
        }}
      >
        {(field) => (
          <TextField
            field={field}
            label="Authorization callback URL"
            placeholder="https://example.com/auth/callback"
            description="Where people return after they authorize. A redirect_uri must share its host and path."
          />
        )}
      </form.Field>
      <form.Field name="device_flow">
        {(field) => (
          <label className="flex items-start gap-2 text-sm">
            <Checkbox
              checked={field.state.value}
              onCheckedChange={field.handleChange}
              className="mt-0.5"
            />
            <span>
              <span className="font-medium">Enable Device Flow</span>
              <span className="text-muted-foreground block text-xs">
                Lets a CLI or headless app sign people in with a code they enter at /login/device.
              </span>
            </span>
          </label>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => [s.isSubmitting, s.isDirty] as const}>
        {([submitting, dirty]) => (
          <SubmitButton pending={submitting} disabled={submitting || !dirty} className="self-start">
            {label}
          </SubmitButton>
        )}
      </form.Subscribe>
    </form>
  );
}

function NewOAuthApp() {
  const queryClient = useQueryClient();
  const navigate = Route.useNavigate();
  const create = useMutation({
    mutationFn: (body: OAuthAppInput) => call(ui.POST("/settings/oauth-apps", { body })),
    onSuccess: (a) => {
      void queryClient.invalidateQueries({ queryKey: oauthAppsQuery().queryKey });
      toast.success(`Registered ${a.name}`);
      void navigate({ search: { tab: "oauth-apps", oauth: a.client_id } });
    },
  });
  return (
    <Section title="Register a new OAuth app">
      <OAuthAppForm
        initial={{
          name: "",
          homepage_url: "",
          description: "",
          callback_url: "",
          device_flow: false,
        }}
        submit={(v) => create.mutateAsync(v)}
        label="Register application"
      />
    </Section>
  );
}

function EditOAuthApp({ clientId }: { clientId: string }) {
  const queryClient = useQueryClient();
  const navigate = Route.useNavigate();
  const { data: app } = useSuspenseQuery(oauthAppQuery(clientId));
  const [secret, setSecret] = useState<string>();
  const key = oauthAppQuery(clientId).queryKey;

  const save = useMutation({
    mutationFn: (body: OAuthAppInput) =>
      call(
        ui.PATCH("/settings/oauth-apps/{client_id}", {
          params: { path: { client_id: clientId } },
          body,
        }),
      ),
    onSuccess: (a) => {
      queryClient.setQueryData(key, a);
      void queryClient.invalidateQueries({ queryKey: oauthAppsQuery().queryKey });
      toast.success("Application updated");
    },
  });
  const generate = useMutation({
    mutationFn: () =>
      call(
        ui.POST("/settings/oauth-apps/{client_id}/secrets", {
          params: { path: { client_id: clientId } },
        }),
      ),
    onError: (e) => failed("Generating a client secret", e),
    onSuccess: (r) => {
      queryClient.setQueryData<OAuthApp>(
        key,
        (old) => old && { ...old, secrets: [...old.secrets, r.secret] },
      );
      setSecret(r.value);
    },
  });
  const deleteSecret = useOptimistic<OAuthApp, number, void>({
    queryKey: key,
    what: "Deleting the client secret",
    mutationFn: (id) =>
      call(
        ui.DELETE("/settings/oauth-apps/{client_id}/secrets/{id}", {
          params: { path: { client_id: clientId, id } },
        }),
      ),
    apply: (old, id) => ({ ...old, secrets: old.secrets.filter((s) => s.id !== id) }),
    onSuccess: () => toast.success("Client secret deleted"),
  });
  const remove = useMutation({
    mutationFn: () =>
      call(
        ui.DELETE("/settings/oauth-apps/{client_id}", {
          params: { path: { client_id: clientId } },
        }),
      ),
    onError: (e) => failed("Deleting the application", e),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: oauthAppsQuery().queryKey });
      toast.success(`Deleted ${app.name}`);
      void navigate({ search: { tab: "oauth-apps" } });
    },
  });

  return (
    <div className="flex flex-col gap-6" data-testid="oauth-app">
      <div className="flex items-center gap-2 text-sm">
        <Link
          to="/settings/developers"
          search={{ tab: "oauth-apps" }}
          className="text-primary hover:underline"
        >
          OAuth Apps
        </Link>
        <span className="text-muted-foreground">/</span>
        <span className="font-semibold">{app.name}</span>
      </div>
      <Section
        title="Client ID"
        description={`${app.users} user${app.users === 1 ? "" : "s"} have authorized it.`}
      >
        <code
          className="bg-muted self-start rounded-sm border px-2 py-1 font-mono text-sm"
          data-testid="client-id"
        >
          {app.client_id}
        </code>
      </Section>
      <Section
        title="Client secrets"
        description="Your app uses a client secret to trade an authorization code for a token. Keep two while you rotate."
        actions={
          <Button
            size="sm"
            variant="outline"
            onClick={() => generate.mutate()}
            disabled={generate.isPending}
          >
            <HugeiconsIcon icon={Key02Icon} strokeWidth={2} data-icon="inline-start" />
            Generate a new client secret
          </Button>
        }
      >
        {secret && (
          <OneTimeSecret
            title="New client secret"
            value={secret}
            note="Copy it now. It is shown once; afterwards only its last eight characters appear."
            onDismiss={() => setSecret(undefined)}
          />
        )}
        <Panel flush>
          {app.secrets.length === 0 ? (
            <EmptyRow>No client secrets. Generate one before your app can exchange codes.</EmptyRow>
          ) : (
            <ul className="divide-y" data-testid="client-secrets">
              {app.secrets.map((s) => (
                <li key={s.id} className="flex items-center gap-3 px-4 py-2 text-sm">
                  <code className="font-mono text-xs">*****{s.last_eight}</code>
                  <span className="text-muted-foreground text-xs">
                    Added {formatDate(s.created_at)} ·{" "}
                    {s.last_used_at ? `Last used ${formatDate(s.last_used_at)}` : "Never used"}
                  </span>
                  <Confirm
                    trigger={
                      <Button
                        size="icon-xs"
                        variant="ghost"
                        className="ml-auto"
                        aria-label="Delete client secret"
                      >
                        <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                      </Button>
                    }
                    title="Delete this client secret?"
                    description="Your app can no longer exchange codes with it."
                    action="Delete"
                    onConfirm={() => deleteSecret.mutate(s.id)}
                  />
                </li>
              ))}
            </ul>
          )}
        </Panel>
      </Section>
      <Section title="Application">
        <OAuthAppForm
          key={app.updated_at}
          initial={{
            name: app.name,
            homepage_url: app.homepage_url,
            description: app.description,
            callback_url: app.callback_url,
            device_flow: app.device_flow,
          }}
          submit={(v) => save.mutateAsync(v)}
          label="Update application"
        />
      </Section>
      <Section title="Delete application">
        <div className="border-destructive/40 flex items-center gap-3 border p-3 text-sm">
          <p className="flex-1">
            Every token it issued stops working, and everyone who authorized it is signed out of it.
          </p>
          <Confirm
            trigger={
              <Button size="sm" variant="destructive">
                Delete application
              </Button>
            }
            title={`Delete ${app.name}?`}
            description="This cannot be undone."
            action="Delete application"
            onConfirm={() => remove.mutate()}
          />
        </div>
      </Section>
    </div>
  );
}
