import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowRight01Icon, Download04Icon } from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import { userQuery } from "@/areas/core/queries";
import { AppForm, emptyApp, type AppFormValues } from "@/areas/account/app-form";
import { CopyButton, download, Section } from "@/areas/account/kit";
import { call, devAppsQuery } from "@/areas/account/queries";
import { ui, type AppManifest } from "@/areas/account/ui-api";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Button } from "@/components/ui/button";

// The manifest flow's page (`design/AGENT-LAYER.md`, "GHES compatibility"). A vendor's
// setup page sends people here with a manifest; GitHub takes it as a form POST, which a
// static app cannot receive, so this page also reads it from `?manifest=` (URL-encoded
// JSON) and `?state=`. Registering returns a one-time code; the page shows the redirect it
// would make to the manifest's redirect_url, then trades the code itself at
// POST /app-manifests/{code}/conversions to show the credentials once.
type Search = { manifest?: string; state?: string };

export const Route = createFileRoute("/settings/apps/new")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    manifest:
      typeof s.manifest === "string"
        ? s.manifest
        : s.manifest && typeof s.manifest === "object"
          ? JSON.stringify(s.manifest)
          : undefined,
    state: typeof s.state === "string" ? s.state : undefined,
  }),
  component: NewAppPage,
});

type Incoming = Partial<AppManifest> & {
  url?: string;
  redirect_url?: string;
  hook_attributes?: { url?: string; active?: boolean };
  default_permissions?: AppManifest["permissions"];
  default_events?: string[];
};

function fromManifest(raw: string | undefined): {
  values: AppFormValues;
  redirect?: string;
  error?: string;
} {
  if (!raw) return { values: emptyApp };
  try {
    const m = JSON.parse(raw) as Incoming;
    return {
      redirect: m.redirect_url,
      values: {
        ...emptyApp,
        name: m.name ?? "",
        description: m.description ?? "",
        homepage_url: m.url ?? m.homepage_url ?? "",
        callback_urls: (m.callback_urls ?? []).join("\n"),
        request_oauth_on_install: m.request_oauth_on_install ?? false,
        setup_url: m.setup_url ?? "",
        setup_on_update: m.setup_on_update ?? false,
        hook_active: m.hook_attributes?.active ?? Boolean(m.hook_attributes?.url),
        hook_url: m.hook_attributes?.url ?? "",
        permissions: { metadata: "read", ...(m.default_permissions ?? m.permissions ?? {}) },
        events: m.default_events ?? m.events ?? [],
        public: m.public ?? false,
      },
    };
  } catch {
    return {
      values: emptyApp,
      error: "The manifest in the link is not valid JSON, so the form starts empty.",
    };
  }
}

type Created = { slug: string; code: string; redirect?: string };

function NewAppPage() {
  const { manifest, state } = Route.useSearch();
  const incoming = fromManifest(manifest);
  const [created, setCreated] = useState<Created>();
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Register new GitHub App"
        description={
          <>
            <Link to="/settings/developers" className="text-primary hover:underline">
              Developer settings
            </Link>{" "}
            / New GitHub App
          </>
        }
      />
      {incoming.error && (
        <Alert variant="warning">
          <AlertTitle>{incoming.error}</AlertTitle>
        </Alert>
      )}
      {manifest && !incoming.error && !created && (
        <Alert variant="info" data-testid="manifest-banner">
          <AlertTitle>Prefilled from a manifest</AlertTitle>
          <AlertDescription>
            Review every field, then register.{" "}
            {incoming.redirect && `You will return to ${new URL(incoming.redirect).host}.`}
          </AlertDescription>
        </Alert>
      )}
      <Region name="new GitHub app" rows={8}>
        {created ? (
          <Credentials created={created} state={state} />
        ) : (
          <Form
            initial={incoming.values}
            onCreated={(c) => setCreated({ ...c, redirect: incoming.redirect })}
          />
        )}
      </Region>
    </div>
  );
}

function Form({
  initial,
  onCreated,
}: {
  initial: AppFormValues;
  onCreated: (c: { slug: string; code: string }) => void;
}) {
  const queryClient = useQueryClient();
  const { data: viewer } = useSuspenseQuery(userQuery());
  const create = useMutation({
    mutationFn: (body: AppManifest) => call(ui.POST("/settings/apps", { body })),
    onSuccess: (r) => {
      void queryClient.invalidateQueries({ queryKey: devAppsQuery().queryKey });
      onCreated(r);
    },
  });
  return (
    <AppForm
      initial={initial}
      submit={(m) => create.mutateAsync(m)}
      label="Create GitHub App"
      ownerLogin={viewer.login}
    />
  );
}

function Credentials({ created, state }: { created: Created; state?: string }) {
  const convert = useMutation({
    mutationFn: () =>
      call(
        api.POST("/app-manifests/{code}/conversions", { params: { path: { code: created.code } } }),
      ),
  });
  const redirect = created.redirect && new URL(created.redirect);
  if (redirect) {
    redirect.searchParams.set("code", created.code);
    if (state) redirect.searchParams.set("state", state);
  }
  const creds = convert.data;
  return (
    <div className="flex flex-col gap-4" data-testid="app-created">
      <Alert variant="success">
        <AlertTitle>Registered {created.slug}</AlertTitle>
        <AlertDescription>
          {redirect ? (
            <>
              The manifest flow would now send you to{" "}
              <code className="font-mono text-xs break-all">{redirect.toString()}</code>. The mock
              does not leave the app; trade the code here instead.
            </>
          ) : creds ? (
            "Its credentials are below, once."
          ) : (
            "Trade the one-time code for the App's credentials. The code works once, within an hour."
          )}
        </AlertDescription>
      </Alert>
      {!creds ? (
        <div className="flex flex-col gap-2">
          <p className="text-sm">
            Code <code className="font-mono text-xs">{created.code}</code>
          </p>
          <Button
            className="self-start"
            size="sm"
            onClick={() => convert.mutate()}
            disabled={convert.isPending}
          >
            {convert.isPending ? "Converting…" : "Get the App's credentials"}
          </Button>
          {convert.error && (
            <p className="text-destructive text-xs" role="alert">
              {convert.error.message}. The code may already be spent; open the App page to generate
              a new key.
            </p>
          )}
        </div>
      ) : (
        <Section
          title="Credentials"
          description="Shown once. Store them now; afterwards the App page shows only fingerprints."
        >
          <dl
            className="grid gap-2 text-sm sm:grid-cols-[10rem_minmax(0,1fr)]"
            data-testid="credentials"
          >
            <dt className="text-muted-foreground">App ID</dt>
            <dd className="font-mono">{creds.id}</dd>
            <dt className="text-muted-foreground">Client ID</dt>
            <dd className="font-mono">{creds.client_id}</dd>
            <dt className="text-muted-foreground">Client secret</dt>
            <dd className="flex items-center gap-2">
              <code className="font-mono text-xs break-all">{creds.client_secret}</code>
              <CopyButton value={creds.client_secret} />
            </dd>
            <dt className="text-muted-foreground">Webhook secret</dt>
            <dd className="font-mono text-xs">{creds.webhook_secret ?? "None"}</dd>
            <dt className="text-muted-foreground">Private key</dt>
            <dd>
              <Button
                size="sm"
                variant="outline"
                onClick={() => download(`${created.slug}.private-key.pem`, creds.pem)}
              >
                <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
                Download {created.slug}.private-key.pem
              </Button>
            </dd>
          </dl>
          <Button
            size="sm"
            className="self-start"
            render={<Link to="/settings/apps/$slug" params={{ slug: created.slug }} />}
          >
            Go to the App
            <HugeiconsIcon icon={ArrowRight01Icon} strokeWidth={2} data-icon="inline-end" />
          </Button>
        </Section>
      )}
    </div>
  );
}
