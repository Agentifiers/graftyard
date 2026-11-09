import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Download04Icon,
  Key02Icon,
  PlugSocketIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { AppForm, type AppFormValues } from "@/areas/account/app-form";
import {
  Confirm,
  download,
  EmptyRow,
  failed,
  formatDate,
  Section,
  useOptimistic,
} from "@/areas/account/kit";
import { permissionSummary } from "@/areas/account/permissions";
import {
  appDeliveriesQuery,
  appDeliveryQuery,
  appInstallationsQuery,
  asApp,
  call,
  devAppQuery,
  devAppsQuery,
} from "@/areas/account/queries";
import { ui, type AppManifest, type DeveloperApp } from "@/areas/account/ui-api";
import { Ago } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

const TABS = ["general", "keys", "installations", "advanced"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/settings/apps/$slug")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; delivery?: number } => ({
    tab: TABS.includes(s.tab as Tab) ? (s.tab as Tab) : undefined,
    delivery: Number(s.delivery) > 0 ? Number(s.delivery) : undefined,
  }),
  loader: ({ context: { queryClient }, params }) =>
    void queryClient.prefetchQuery(devAppQuery(params.slug)),
  component: AppPage,
});

function AppPage() {
  const { slug } = Route.useParams();
  return (
    <Region name="GitHub app" rows={8}>
      <AppBody slug={slug} />
    </Region>
  );
}

function AppBody({ slug }: { slug: string }) {
  const { tab = "general" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data: app } = useSuspenseQuery(devAppQuery(slug));
  return (
    <div className="flex flex-col gap-4" data-testid="dev-app" data-app-id={app.id}>
      <PageHeader
        title={app.name}
        description={
          <>
            <Link to="/settings/developers" className="text-primary hover:underline">
              GitHub Apps
            </Link>{" "}
            / {app.slug} · App ID <code className="font-mono">{app.id}</code> · Client ID{" "}
            <code className="font-mono">{app.client_id}</code>
          </>
        }
        actions={
          <Button
            size="sm"
            variant="outline"
            render={<Link to="/apps/$slug/installations/new" params={{ slug }} />}
          >
            <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} data-icon="inline-start" />
            Install App
          </Button>
        }
      />
      <Tabs value={tab} onValueChange={(t) => void navigate({ search: { tab: t as Tab } })}>
        <TabsList variant="line" className="border-b">
          <TabsTrigger value="general">General</TabsTrigger>
          <TabsTrigger value="keys">Private keys</TabsTrigger>
          <TabsTrigger value="installations">Installations</TabsTrigger>
          <TabsTrigger value="advanced">Advanced</TabsTrigger>
        </TabsList>
      </Tabs>
      {tab === "general" && <General app={app} />}
      {tab === "keys" && <Keys app={app} />}
      {tab === "installations" && (
        <Region name="app installations" rows={3}>
          <Installations app={app} />
        </Region>
      )}
      {tab === "advanced" && (
        <Region name="webhook deliveries" rows={4}>
          <Deliveries app={app} />
        </Region>
      )}
    </div>
  );
}

function General({ app }: { app: DeveloperApp }) {
  const queryClient = useQueryClient();
  const navigate = Route.useNavigate();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const save = useMutation({
    mutationFn: (m: AppManifest) =>
      call(
        ui.PATCH("/settings/apps/{slug}", {
          params: { path: { slug: app.slug } },
          body: { ...m, hook_attributes: m.hook_attributes },
        }),
      ),
    onSuccess: (a) => {
      queryClient.setQueryData(devAppQuery(app.slug).queryKey, a);
      void queryClient.invalidateQueries({ queryKey: devAppsQuery().queryKey });
      toast.success("App updated");
    },
  });
  const remove = useMutation({
    mutationFn: () =>
      call(ui.DELETE("/settings/apps/{slug}", { params: { path: { slug: app.slug } } })),
    onError: (e) => failed("Deleting the App", e),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: devAppsQuery().queryKey });
      toast.success(`Deleted ${app.name}`);
      void navigate({ to: "/settings/developers" });
    },
  });
  const initial: AppFormValues = {
    name: app.name,
    description: app.description,
    homepage_url: app.homepage_url,
    callback_urls: app.callback_urls.join("\n"),
    request_oauth_on_install: app.request_oauth_on_install,
    setup_url: app.setup_url,
    setup_on_update: app.setup_on_update,
    hook_active: app.webhook.active,
    hook_url: app.webhook.url,
    hook_secret: "",
    permissions: app.permissions,
    events: app.events,
    public: app.public,
  };
  return (
    <div className="flex flex-col gap-8">
      <AppForm
        key={app.updated_at}
        initial={initial}
        submit={(m) => save.mutateAsync(m)}
        label="Save changes"
        editing
        ownerLogin={viewer.login}
      />
      <Section title="Danger zone">
        <div className="border-destructive/40 flex items-center gap-3 border p-3 text-sm">
          <p className="flex-1">
            Deleting the App uninstalls it from {app.installations_count} account
            {app.installations_count === 1 ? "" : "s"} and revokes every token it holds.
          </p>
          <Confirm
            trigger={
              <Button size="sm" variant="destructive">
                Delete GitHub App
              </Button>
            }
            title={`Delete ${app.name}?`}
            description="This cannot be undone."
            action="Delete GitHub App"
            onConfirm={() => remove.mutate()}
          />
        </div>
      </Section>
    </div>
  );
}

function Keys({ app }: { app: DeveloperApp }) {
  const queryClient = useQueryClient();
  const key = devAppQuery(app.slug).queryKey;
  const generate = useMutation({
    mutationFn: () =>
      call(ui.POST("/settings/apps/{slug}/keys", { params: { path: { slug: app.slug } } })),
    onError: (e) => failed("Generating a private key", e),
    onSuccess: (r) => {
      queryClient.setQueryData<DeveloperApp>(
        key,
        (old) => old && { ...old, keys: [...old.keys, r.key] },
      );
      const stamp = new Date(r.key.created_at).toISOString().slice(0, 10);
      download(`${app.slug}.${stamp}.private-key.pem`, r.pem);
      toast.success("Private key generated and downloaded", { description: r.key.fingerprint });
    },
  });
  const remove = useOptimistic<DeveloperApp, number, void>({
    queryKey: key,
    what: "Deleting the private key",
    mutationFn: (id) =>
      call(
        ui.DELETE("/settings/apps/{slug}/keys/{id}", { params: { path: { slug: app.slug, id } } }),
      ),
    apply: (old, id) => ({ ...old, keys: old.keys.filter((k) => k.id !== id) }),
    onSuccess: () => toast.success("Private key deleted"),
  });
  return (
    <Section
      title="Private keys"
      description="The App signs the JWTs it authenticates with using one of these. Graftyard keeps only the public half."
      actions={
        <Button size="sm" onClick={() => generate.mutate()} disabled={generate.isPending}>
          <HugeiconsIcon icon={Key02Icon} strokeWidth={2} data-icon="inline-start" />
          {generate.isPending ? "Generating…" : "Generate a private key"}
        </Button>
      }
    >
      <Panel flush>
        <ul className="divide-y" data-testid="app-keys">
          {app.keys.map((k) => (
            <li key={k.id} className="flex flex-wrap items-center gap-3 px-4 py-2.5 text-sm">
              <HugeiconsIcon
                icon={Key02Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <code className="font-mono text-xs break-all" data-testid="key-fingerprint">
                {k.fingerprint}
              </code>
              <span className="text-muted-foreground text-xs">
                Added {formatDate(k.created_at)}
              </span>
              <Confirm
                trigger={
                  <Button size="sm" variant="ghost" className="ml-auto">
                    Delete
                  </Button>
                }
                title="Delete this private key?"
                description="JWTs signed with it stop working at once."
                action="Delete key"
                onConfirm={() => remove.mutate(k.id)}
              />
            </li>
          ))}
        </ul>
      </Panel>
      <p className="text-muted-foreground text-xs">
        <HugeiconsIcon icon={Download04Icon} strokeWidth={2} className="inline size-3" /> A new key
        downloads once as a .pem file.
      </p>
    </Section>
  );
}

type Installation = Schemas["installation"];

function Installations({ app }: { app: DeveloperApp }) {
  const queryClient = useQueryClient();
  const { data } = useSuspenseQuery(appInstallationsQuery(app.id));
  const key = appInstallationsQuery(app.id).queryKey;
  const [token, setToken] = useState<{ id: number; token: string; expires_at: string }>();
  const suspend = useOptimistic<Installation[], { id: number; on: boolean }, void>({
    queryKey: key,
    what: "Changing the suspension",
    mutationFn: ({ id, on }) =>
      on
        ? call(
            api.PUT("/app/installations/{installation_id}/suspended", {
              params: { path: { installation_id: id } },
              headers: asApp(app.id),
            }),
          )
        : call(
            api.DELETE("/app/installations/{installation_id}/suspended", {
              params: { path: { installation_id: id } },
              headers: asApp(app.id),
            }),
          ),
    apply: (old, { id, on }) =>
      old.map((i) =>
        i.id === id ? { ...i, suspended_at: on ? new Date().toISOString() : null } : i,
      ),
  });
  const uninstall = useOptimistic<Installation[], number, void>({
    queryKey: key,
    what: "Uninstalling",
    mutationFn: (id) =>
      call(
        api.DELETE("/app/installations/{installation_id}", {
          params: { path: { installation_id: id } },
          headers: asApp(app.id),
        }),
      ),
    apply: (old, id) => old.filter((i) => i.id !== id),
    invalidate: [devAppQuery(app.slug).queryKey],
    onSuccess: () => toast.success("Uninstalled"),
  });
  const mint = useMutation({
    mutationFn: (id: number) =>
      call(
        api.POST("/app/installations/{installation_id}/access_tokens", {
          params: { path: { installation_id: id } },
          headers: asApp(app.id),
        }),
      ),
    onError: (e) => failed("Creating an installation token", e),
    onSuccess: (t, id) => setToken({ id, token: t.token, expires_at: t.expires_at }),
  });
  return (
    <Section
      title="Installations"
      description="Accounts this App is installed on, from GET /app/installations as the App."
      actions={
        <Button
          size="sm"
          variant="ghost"
          onClick={() => void queryClient.invalidateQueries({ queryKey: key })}
        >
          <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
          Refresh
        </Button>
      }
    >
      <Panel flush>
        {data.length === 0 ? (
          <EmptyRow>
            Not installed anywhere yet.{" "}
            <Link
              to="/apps/$slug/installations/new"
              params={{ slug: app.slug }}
              className="text-primary hover:underline"
            >
              Install it
            </Link>
            .
          </EmptyRow>
        ) : (
          <ul className="divide-y" data-testid="app-installations">
            {data.map((i) => {
              const login = i.account && "login" in i.account ? i.account.login : "unknown";
              return (
                <li
                  key={i.id}
                  className="flex flex-col gap-2 px-4 py-2.5 text-sm"
                  data-installation-account={login}
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="font-semibold">{login}</span>
                    <Badge variant="outline" size="sm">
                      {i.target_type}
                    </Badge>
                    <Badge variant="outline" size="sm">
                      {i.repository_selection === "all"
                        ? "All repositories"
                        : "Selected repositories"}
                    </Badge>
                    {i.suspended_at && <Badge variant="destructive-light">Suspended</Badge>}
                    <span className="text-muted-foreground text-xs">
                      Installed {formatDate(i.created_at)}
                    </span>
                    <div className="ml-auto flex gap-1">
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() => mint.mutate(i.id)}
                        disabled={mint.isPending}
                      >
                        Create token
                      </Button>
                      <Button
                        size="xs"
                        variant="outline"
                        onClick={() => suspend.mutate({ id: i.id, on: !i.suspended_at })}
                      >
                        {i.suspended_at ? "Unsuspend" : "Suspend"}
                      </Button>
                      <Confirm
                        trigger={
                          <Button size="xs" variant="destructive">
                            Uninstall
                          </Button>
                        }
                        title={`Uninstall from ${login}?`}
                        description="The App loses access to that account's repositories."
                        action="Uninstall"
                        onConfirm={() => uninstall.mutate(i.id)}
                      />
                    </div>
                  </div>
                  <p className="text-muted-foreground text-xs">
                    {permissionSummary(i.permissions)}
                  </p>
                  {token?.id === i.id && (
                    <p
                      className="bg-muted border p-2 font-mono text-xs break-all"
                      data-testid="installation-token"
                    >
                      {token.token}
                      <span className="text-muted-foreground block font-sans">
                        Expires <Ago at={token.expires_at} />
                      </span>
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </Panel>
    </Section>
  );
}

function Deliveries({ app }: { app: DeveloperApp }) {
  const queryClient = useQueryClient();
  const { delivery } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data } = useSuspenseQuery(appDeliveriesQuery(app.id));
  const redeliver = useMutation({
    mutationFn: (id: number) =>
      call(
        api.POST("/app/hook/deliveries/{delivery_id}/attempts", {
          params: { path: { delivery_id: id } },
          headers: asApp(app.id),
        }),
      ),
    onError: (e) => failed("Redelivering", e),
    onSuccess: () => {
      toast.success("Redelivery queued");
      void queryClient.invalidateQueries({ queryKey: appDeliveriesQuery(app.id).queryKey });
    },
  });
  return (
    <Section
      title="Recent deliveries"
      description={app.webhook.active ? app.webhook.url : "The webhook is inactive."}
    >
      <Panel flush>
        {data.length === 0 ? (
          <EmptyRow>No deliveries yet.</EmptyRow>
        ) : (
          <ul className="divide-y" data-testid="deliveries">
            {data.map((d) => (
              <li key={d.id} className="text-sm">
                <button
                  type="button"
                  className="hover:bg-muted/40 flex w-full flex-wrap items-center gap-2 px-4 py-2 text-left"
                  onClick={() =>
                    void navigate({
                      search: { tab: "advanced", delivery: delivery === d.id ? undefined : d.id },
                      replace: true,
                    })
                  }
                >
                  <Badge
                    variant={
                      d.status_code >= 200 && d.status_code < 300
                        ? "success-light"
                        : "destructive-light"
                    }
                  >
                    {d.status_code || "—"}
                  </Badge>
                  <code className="font-mono text-xs">{d.guid}</code>
                  <span>
                    {d.event}
                    {d.action && `.${d.action}`}
                  </span>
                  {d.redelivery && (
                    <Badge variant="outline" size="sm">
                      redelivery
                    </Badge>
                  )}
                  <span className="text-muted-foreground ml-auto text-xs">
                    <Ago at={d.delivered_at} />
                  </span>
                </button>
                {delivery === d.id && (
                  <Region name="delivery" rows={3}>
                    <Delivery
                      appId={app.id}
                      id={d.id}
                      onRedeliver={() => redeliver.mutate(d.id)}
                      pending={redeliver.isPending}
                    />
                  </Region>
                )}
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </Section>
  );
}

function Delivery({
  appId,
  id,
  onRedeliver,
  pending,
}: {
  appId: number;
  id: number;
  onRedeliver: () => void;
  pending: boolean;
}) {
  const { data: d } = useSuspenseQuery(appDeliveryQuery(appId, id));
  return (
    <div className="flex flex-col gap-2 border-t px-4 py-3" data-testid="delivery-detail">
      <div className="flex items-center gap-2">
        <span className="text-xs">
          {d.status} · {d.duration.toFixed(2)} s
        </span>
        <Button
          size="xs"
          variant="outline"
          className="ml-auto"
          onClick={onRedeliver}
          disabled={pending}
        >
          Redeliver
        </Button>
      </div>
      <p className="text-xs font-semibold">Request headers</p>
      <pre className="bg-muted overflow-x-auto border p-2 text-xs">
        {JSON.stringify(d.request.headers, null, 2)}
      </pre>
      <p className="text-xs font-semibold">Payload</p>
      <pre className="bg-muted overflow-x-auto border p-2 text-xs">
        {JSON.stringify(d.request.payload, null, 2)}
      </pre>
      <p className="text-xs font-semibold">Response</p>
      <pre className="bg-muted overflow-x-auto border p-2 text-xs">
        {d.response.payload ?? "(no body)"}
      </pre>
    </div>
  );
}
