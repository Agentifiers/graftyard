import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { LockIcon } from "@hugeicons/core-free-icons";
import {
  environmentSecretsQuery,
  repoOrgSecretsQuery,
  repoOrgVariablesQuery,
} from "@/areas/actions/queries";
import { secretsQuery, variablesQuery } from "@/areas/actions/scope";
import { ConfigList } from "@/areas/actions/secrets";
import { Ago } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";

type Tab = "secrets" | "variables";

export const Route = createFileRoute("/$owner/$repo/settings/secrets/actions")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab } => ({
    tab: s.tab === "variables" ? "variables" : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    const scope = { kind: "repo" as const, ...params };
    const q = context.queryClient;
    if (deps.tab === "variables") {
      void q.prefetchQuery(variablesQuery(scope));
      void q.prefetchQuery(repoOrgVariablesQuery(params));
    } else {
      void q.prefetchQuery(secretsQuery(scope));
      void q.prefetchQuery(environmentSecretsQuery(params));
      void q.prefetchQuery(repoOrgSecretsQuery(params));
    }
  },
  component: SecretsPage,
});

function SecretsPage() {
  const params = Route.useParams();
  const { tab = "secrets" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const scope = { kind: "repo" as const, ...params };
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Actions secrets and variables"
        description="Secrets are encrypted and only exposed to workflows. Variables are plain text configuration."
      />
      <Tabs
        value={tab}
        onValueChange={(v) =>
          navigate({ search: { tab: v === "variables" ? "variables" : undefined } })
        }
      >
        <TabsList>
          <TabsTrigger value="secrets">Secrets</TabsTrigger>
          <TabsTrigger value="variables">Variables</TabsTrigger>
        </TabsList>
      </Tabs>
      {tab === "secrets" ? (
        <>
          <Region name="environment secrets" rows={2}>
            <EnvironmentSecrets />
          </Region>
          <Region name="repository secrets" rows={3}>
            <ConfigList scope={scope} kind="secret" />
          </Region>
          <Region name="organization secrets" rows={2}>
            <Inherited kind="secret" />
          </Region>
        </>
      ) : (
        <>
          <Region name="repository variables" rows={3}>
            <ConfigList scope={scope} kind="variable" />
          </Region>
          <Region name="organization variables" rows={2}>
            <Inherited kind="variable" />
          </Region>
        </>
      )}
    </div>
  );
}

// Read only here: environment secrets are edited on the environment's page.
function EnvironmentSecrets() {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(environmentSecretsQuery(params));
  return (
    <Panel title="Environment secrets" flush>
      {data.length === 0 ? (
        <p className="text-muted-foreground px-4 py-4 text-sm">
          This repository has no environments.
        </p>
      ) : (
        <ul className="divide-y" data-testid="environment-secrets">
          {data.map(({ env, secrets }) => (
            <li key={env.id} className="px-4 py-2 text-sm">
              <div className="flex items-center gap-2">
                <Badge variant="outline" className="font-mono">
                  {env.name}
                </Badge>
                <span className="text-muted-foreground text-xs">
                  {secrets.length} secret{secrets.length === 1 ? "" : "s"}
                </span>
                {/* The releases area owns the environment page; it is not in this worktree's route tree yet. */}
                <a
                  href={`/${params.owner}/${params.repo}/settings/environments/${encodeURIComponent(env.name)}`}
                  className="text-primary ml-auto text-xs hover:underline"
                >
                  Manage environment
                </a>
              </div>
              <div className="mt-1.5 flex flex-wrap gap-1.5">
                {secrets.map((s) => (
                  <code
                    key={s.name}
                    className="bg-muted flex items-center gap-1 rounded-sm px-1.5 py-0.5 font-mono text-xs"
                  >
                    <HugeiconsIcon icon={LockIcon} strokeWidth={2} className="size-3" />
                    {s.name}
                  </code>
                ))}
              </div>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function Inherited({ kind }: { kind: "secret" | "variable" }) {
  const params = Route.useParams();
  const secrets = useSuspenseQuery(repoOrgSecretsQuery(params)).data;
  const variables = useSuspenseQuery(repoOrgVariablesQuery(params)).data;
  const list =
    kind === "secret"
      ? secrets.secrets.map((s) => ({ ...s, value: null }))
      : variables.variables.map((v) => ({ ...v, value: v.value as string | null }));
  return (
    <Panel title={`Organization ${kind}s`} flush>
      {list.length === 0 ? (
        <p className="text-muted-foreground px-4 py-4 text-sm">
          No organization {kind}s are shared with this repository.
        </p>
      ) : (
        <ul className="divide-y" data-testid={`org-${kind}s`}>
          {list.map((x) => (
            <li key={x.name} className="flex items-center gap-3 px-4 py-2 text-sm">
              <code className="font-mono text-xs font-semibold">{x.name}</code>
              <code className="text-muted-foreground min-w-0 flex-1 truncate font-mono text-xs">
                {x.value}
              </code>
              <span className="text-muted-foreground text-xs">
                Updated <Ago at={x.updated_at} />
              </span>
            </li>
          ))}
        </ul>
      )}
      <p className="text-muted-foreground border-t px-4 py-2 text-xs">
        Organization {kind}s are managed in{" "}
        <Link
          to={
            kind === "secret"
              ? "/orgs/$org/settings/secrets/actions"
              : "/orgs/$org/settings/variables/actions"
          }
          params={{ org: params.owner }}
          className="text-primary hover:underline"
        >
          the organization's settings
        </Link>
        .
      </p>
    </Panel>
  );
}
