import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { DashboardSquare01Icon, PlugSocketIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { Confirm, EmptyRow, formatDate, Section, useOptimistic } from "@/areas/account/kit";
import { permissionSummary } from "@/areas/account/permissions";
import {
  authorizedAppsQuery,
  call,
  grantsQuery,
  installationsQuery,
} from "@/areas/account/queries";
import { ui, type AuthorizedApp } from "@/areas/account/ui-api";
import { UserAvatar } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/settings/applications")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(grantsQuery());
    void queryClient.prefetchQuery(authorizedAppsQuery());
    void queryClient.prefetchQuery(installationsQuery());
  },
  component: ApplicationsPage,
});

function ApplicationsPage() {
  return (
    <div className="flex flex-col gap-8">
      <PageHeader
        title="Applications"
        description="Apps you have allowed to act as you, and the GitHub Apps installed on your accounts."
        actions={
          <Button size="sm" variant="outline" render={<Link to="/settings/installations" />}>
            <HugeiconsIcon icon={PlugSocketIcon} strokeWidth={2} data-icon="inline-start" />
            Installed GitHub Apps
          </Button>
        }
      />
      <Section
        title="Authorized OAuth Apps"
        description="Each one holds a token for the scopes you granted."
      >
        <Region name="authorized OAuth apps" rows={3}>
          <Grants />
        </Region>
      </Section>
      <Section
        title="Authorized GitHub Apps"
        description="Each one can act as you within its permissions, on the accounts it is installed on."
      >
        <Region name="authorized GitHub apps" rows={2}>
          <AuthorizedApps />
        </Region>
      </Section>
    </div>
  );
}

type Grant = Schemas["application-grant"];

function Grants() {
  const { data } = useSuspenseQuery(grantsQuery());
  const revoke = useOptimistic<Grant[], number, void>({
    queryKey: grantsQuery().queryKey,
    what: "Revoking the app",
    mutationFn: (id) =>
      call(api.DELETE("/applications/grants/{grant_id}", { params: { path: { grant_id: id } } })),
    apply: (old, id) => old.filter((g) => g.id !== id),
    onSuccess: () => toast.success("Access revoked. The app's tokens no longer work."),
  });
  return (
    <Panel flush>
      {data.length === 0 ? (
        <EmptyRow>No OAuth apps are authorized.</EmptyRow>
      ) : (
        <ul className="divide-y" data-testid="grants">
          {data.map((g) => (
            <li
              key={g.id}
              className="flex flex-wrap items-center gap-3 px-4 py-2.5"
              data-grant={g.app.name}
            >
              <HugeiconsIcon
                icon={DashboardSquare01Icon}
                strokeWidth={2}
                className="text-muted-foreground size-5"
              />
              <div className="min-w-0 flex-1 text-sm">
                <p className="font-semibold">
                  <a href={g.app.url} target="_blank" rel="noreferrer" className="hover:underline">
                    {g.app.name}
                  </a>
                </p>
                <p className="flex flex-wrap items-center gap-1 text-xs">
                  {g.scopes.length === 0 ? (
                    <span className="text-muted-foreground">Public information only</span>
                  ) : (
                    g.scopes.map((s) => (
                      <Badge key={s} variant="outline" size="sm">
                        {s}
                      </Badge>
                    ))
                  )}
                </p>
                <p className="text-muted-foreground text-xs">
                  Authorized {formatDate(g.created_at)}
                </p>
              </div>
              <Confirm
                trigger={
                  <Button size="sm" variant="destructive">
                    Revoke
                  </Button>
                }
                title={`Revoke ${g.app.name}?`}
                description="It loses access to your account, and every token it holds stops working."
                action="Revoke access"
                onConfirm={() => revoke.mutate(g.id)}
              />
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function AuthorizedApps() {
  const { data } = useSuspenseQuery(authorizedAppsQuery());
  const revoke = useOptimistic<AuthorizedApp[], string, void>({
    queryKey: authorizedAppsQuery().queryKey,
    what: "Revoking the app",
    mutationFn: (slug) =>
      call(ui.DELETE("/settings/applications/apps/{slug}", { params: { path: { slug } } })),
    apply: (old, slug) => old.filter((a) => a.slug !== slug),
    onSuccess: () => toast.success("Access revoked"),
  });
  return (
    <Panel flush>
      {data.length === 0 ? (
        <EmptyRow>No GitHub Apps are authorized to act as you.</EmptyRow>
      ) : (
        <ul className="divide-y" data-testid="authorized-apps">
          {data.map((a) => (
            <li
              key={a.slug}
              className="flex flex-wrap items-center gap-3 px-4 py-2.5"
              data-app={a.slug}
            >
              <UserAvatar user={a.owner} />
              <div className="min-w-0 flex-1 text-sm">
                <p className="font-semibold">
                  {a.name}{" "}
                  <span className="text-muted-foreground font-normal">by {a.owner.login}</span>
                </p>
                <p className="text-muted-foreground text-xs">{permissionSummary(a.permissions)}</p>
                <p className="text-muted-foreground text-xs">
                  Authorized {formatDate(a.authorized_at)} ·{" "}
                  {a.last_used_at ? `Last used ${formatDate(a.last_used_at)}` : "Never used"}
                </p>
              </div>
              <Confirm
                trigger={
                  <Button size="sm" variant="destructive">
                    Revoke
                  </Button>
                }
                title={`Revoke ${a.name}?`}
                description="It can no longer act as you. Its installations stay until you uninstall them."
                action="Revoke access"
                onConfirm={() => revoke.mutate(a.slug)}
              />
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}
