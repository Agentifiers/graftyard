import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { CheckmarkCircle02Icon, PlugSocketIcon } from "@hugeicons/core-free-icons";
import { userOrgsQuery, userQuery, userReposQuery } from "@/areas/core/queries";
import { RepositorySelection } from "@/areas/account/install-fields";
import { failed } from "@/areas/account/kit";
import { PERMISSIONS } from "@/areas/account/permissions";
import { appQuery, call, installationsQuery, ValidationError } from "@/areas/account/queries";
import { ui } from "@/areas/account/ui-api";
import { UserAvatar } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";

// Installing a GitHub App: pick the account, then all or selected repositories, then
// review what the App gets. Top-level, outside the settings sidebar, like GitHub's.
export const Route = createFileRoute("/apps/$slug/installations/new")({
  validateSearch: (s: Record<string, unknown>): { account?: string } => ({
    account: typeof s.account === "string" && s.account ? s.account : undefined,
  }),
  loader: ({ context: { queryClient }, params }) => {
    void queryClient.prefetchQuery(appQuery(params.slug));
    void queryClient.prefetchQuery(installationsQuery());
    void queryClient.prefetchQuery(userReposQuery());
  },
  component: InstallPage,
});

function InstallPage() {
  return (
    <main className="mx-auto max-w-3xl p-4 sm:p-6">
      <Region name="GitHub app" rows={8}>
        <Install />
      </Region>
    </main>
  );
}

function Install() {
  const { slug } = Route.useParams();
  const { account } = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const { data: app } = useSuspenseQuery(appQuery(slug));
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: orgs } = useSuspenseQuery(userOrgsQuery());
  const { data: installed } = useSuspenseQuery(installationsQuery());
  const { data: repos } = useSuspenseQuery(userReposQuery());
  const [selection, setSelection] = useState<"all" | "selected">("all");
  const [selected, setSelected] = useState<string[]>([]);
  const [error, setError] = useState<string>();
  const [done, setDone] = useState<string>();

  const owner = app && "login" in app.owner ? app.owner.login : "";
  const accounts = [
    { login: viewer.login, avatar_url: viewer.avatar_url, kind: "Personal account" },
    ...orgs.map((o) => ({ login: o.login, avatar_url: o.avatar_url, kind: "Organization" })),
  ].map((a) => ({
    ...a,
    installed: installed.installations.find(
      (i) =>
        i.app_slug === slug && i.account && "login" in i.account && i.account.login === a.login,
    ),
  }));
  const target = accounts.find((a) => a.login === account);
  const ownRepos = repos.filter((r) => r.owner.login === target?.login);

  const install = useMutation({
    mutationFn: () =>
      call(
        ui.POST("/settings/installations", {
          body: {
            app_slug: slug,
            account: target!.login,
            repository_selection: selection,
            repository_ids: ownRepos.filter((r) => selected.includes(r.full_name)).map((r) => r.id),
          },
        }),
      ),
    onSuccess: (i) => {
      void queryClient.invalidateQueries({ queryKey: installationsQuery().queryKey });
      setDone(
        `Installed ${app?.name} on ${i.account && "login" in i.account ? i.account.login : target?.login}.`,
      );
    },
    onError: (e) => {
      if (e instanceof ValidationError) setError(e.message);
      else failed("Installing the App", e);
    },
  });

  if (!app) return <p className="text-sm">No such app.</p>;
  const perms = PERMISSIONS.filter((p) => app.permissions[p.key]);

  return (
    <div className="flex flex-col gap-4" data-testid="install-app">
      <PageHeader
        title={`Install ${app.name}`}
        description={
          <>
            {app.description} By <span className="font-medium">{owner}</span>.
          </>
        }
      />
      {done ? (
        <Alert variant="success" data-testid="install-done">
          <HugeiconsIcon icon={CheckmarkCircle02Icon} strokeWidth={2} />
          <AlertTitle>{done}</AlertTitle>
          <AlertDescription>
            <Link to="/settings/installations" className="text-primary hover:underline">
              Manage installed GitHub Apps
            </Link>
          </AlertDescription>
        </Alert>
      ) : !target ? (
        <Panel title="Where do you want to install it?" flush>
          <ul className="divide-y" data-testid="install-accounts">
            {accounts.map((a) => (
              <li key={a.login} className="flex items-center gap-3 px-4 py-3">
                <UserAvatar user={a} size="default" />
                <div className="min-w-0 flex-1 text-sm">
                  <p className="font-semibold">{a.login}</p>
                  <p className="text-muted-foreground text-xs">{a.kind}</p>
                </div>
                {a.installed ? (
                  <Button
                    size="sm"
                    variant="outline"
                    render={
                      <Link to="/settings/installations" search={{ configure: a.installed.id }} />
                    }
                  >
                    Configure
                  </Button>
                ) : (
                  <Button size="sm" onClick={() => void navigate({ search: { account: a.login } })}>
                    <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} data-icon="inline-start" />
                    Install
                  </Button>
                )}
              </li>
            ))}
          </ul>
        </Panel>
      ) : (
        <Panel title={`Install on ${target.login}`}>
          <div className="flex flex-col gap-4">
            <RepositorySelection
              selection={selection}
              onSelection={(s) => (setSelection(s), setError(undefined))}
              repos={ownRepos}
              selected={selected}
              onSelected={(n) => (setSelected(n), setError(undefined))}
              owner={target.login}
            />
            <div>
              <p className="mb-1 text-sm font-medium">With these permissions</p>
              <ul className="flex flex-wrap gap-1" data-testid="install-permissions">
                {perms.map((p) => (
                  <li key={p.key}>
                    <Badge
                      variant={app.permissions[p.key] === "write" ? "warning-light" : "outline"}
                    >
                      {p.label}:{" "}
                      {app.permissions[p.key] === "write"
                        ? "read and write"
                        : app.permissions[p.key]}
                    </Badge>
                  </li>
                ))}
              </ul>
              {app.events.length > 0 && (
                <p className="text-muted-foreground mt-2 text-xs">
                  It receives these events: {app.events.join(", ")}.
                </p>
              )}
            </div>
            {error && (
              <Alert variant="destructive" data-testid="install-error">
                <AlertTitle>{error}</AlertTitle>
              </Alert>
            )}
            <div className="flex gap-2">
              <Button
                size="sm"
                onClick={() => install.mutate()}
                disabled={install.isPending || (selection === "selected" && selected.length === 0)}
              >
                {install.isPending ? "Installing…" : "Install"}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void navigate({ search: {} })}>
                Cancel
              </Button>
            </div>
          </div>
        </Panel>
      )}
    </div>
  );
}
