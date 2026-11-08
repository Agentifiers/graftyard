import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { PlugSocketIcon, Settings01Icon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { userReposQuery } from "@/areas/core/queries";
import { RepositorySelection } from "@/areas/account/install-fields";
import { Confirm, EmptyRow, failed, formatDate, Section, useOptimistic } from "@/areas/account/kit";
import { permissionSummary } from "@/areas/account/permissions";
import { call, installationReposQuery, installationsQuery } from "@/areas/account/queries";
import { ui, type AppPermissions } from "@/areas/account/ui-api";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/settings/installations")({
  validateSearch: (s: Record<string, unknown>): { configure?: number } => ({
    configure: Number(s.configure) > 0 ? Number(s.configure) : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(installationsQuery());
    void queryClient.prefetchQuery(userReposQuery());
  },
  component: InstallationsPage,
});

type Installation = Schemas["installation"];
type InstallationList = { total_count: number; installations: Installation[] };

const accountLogin = (i: Installation) =>
  i.account && "login" in i.account ? i.account.login : "";

function InstallationsPage() {
  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="Installed GitHub Apps"
        description="GitHub Apps on your personal account and on the organizations you own. Each one reaches only the repositories you chose."
      />
      <Region name="installations" rows={4}>
        <Installations />
      </Region>
    </div>
  );
}

function Installations() {
  const { configure } = Route.useSearch();
  const navigate = Route.useNavigate();
  const { data } = useSuspenseQuery(installationsQuery());
  const uninstall = useOptimistic<InstallationList, number, void>({
    queryKey: installationsQuery().queryKey,
    what: "Uninstalling the app",
    mutationFn: (id) =>
      call(ui.DELETE("/settings/installations/{id}", { params: { path: { id } } })),
    apply: (old, id) => ({
      total_count: old.total_count - 1,
      installations: old.installations.filter((i) => i.id !== id),
    }),
    onSuccess: () => toast.success("App uninstalled"),
  });

  if (data.installations.length === 0) {
    return (
      <Panel>
        <EmptyRow>No GitHub Apps are installed on your accounts.</EmptyRow>
      </Panel>
    );
  }
  const groups = new Map<string, Installation[]>();
  for (const i of data.installations)
    groups.set(accountLogin(i), [...(groups.get(accountLogin(i)) ?? []), i]);
  return (
    <div className="flex flex-col gap-6">
      {[...groups].map(([login, list]) => (
        <Section
          key={login}
          title={login}
          description={
            list[0]?.target_type === "Organization" ? "Organization" : "Personal account"
          }
        >
          <Panel flush>
            <ul className="divide-y" data-testid={`installations-${login}`}>
              {list.map((i) => (
                <li key={i.id} className="px-4 py-3" data-installation={i.app_slug}>
                  <div className="flex flex-wrap items-center gap-3">
                    <HugeiconsIcon
                      icon={PlugSocketIcon}
                      strokeWidth={2}
                      className="text-muted-foreground size-5"
                    />
                    <div className="min-w-0 flex-1 text-sm">
                      <p className="flex items-center gap-2 font-semibold">
                        {i.app_slug}
                        <Badge variant="outline" size="sm">
                          {i.repository_selection === "all"
                            ? "All repositories"
                            : "Selected repositories"}
                        </Badge>
                        {i.suspended_at && <Badge variant="destructive-light">Suspended</Badge>}
                      </p>
                      <p className="text-muted-foreground text-xs">
                        Installed {formatDate(i.created_at)} ·{" "}
                        {permissionSummary(i.permissions as AppPermissions)}
                      </p>
                    </div>
                    <Button
                      size="sm"
                      variant={configure === i.id ? "secondary" : "outline"}
                      onClick={() =>
                        void navigate({
                          search: { configure: configure === i.id ? undefined : i.id },
                          replace: true,
                        })
                      }
                    >
                      <HugeiconsIcon
                        icon={Settings01Icon}
                        strokeWidth={2}
                        data-icon="inline-start"
                      />
                      Configure
                    </Button>
                    <Confirm
                      trigger={
                        <Button size="sm" variant="destructive">
                          Uninstall
                        </Button>
                      }
                      title={`Uninstall ${i.app_slug} from ${login}?`}
                      description="It loses access to every repository on this account at once."
                      action="Uninstall"
                      onConfirm={() => uninstall.mutate(i.id)}
                    />
                  </div>
                  {configure === i.id && (
                    <Region name="installation repositories" rows={3}>
                      <Configure installation={i} />
                    </Region>
                  )}
                </li>
              ))}
            </ul>
          </Panel>
        </Section>
      ))}
    </div>
  );
}

// Changes are sent one at a time, as the REST routes take them: the selection mode, then
// PUT or DELETE per repository.
function Configure({ installation }: { installation: Installation }) {
  const queryClient = useQueryClient();
  const owner = accountLogin(installation);
  const { data: all } = useSuspenseQuery(userReposQuery());
  const { data: current } = useSuspenseQuery(installationReposQuery(installation.id));
  const repos = all.filter((r) => r.owner.login === owner);
  const [selection, setSelection] = useState(installation.repository_selection);
  const [selected, setSelected] = useState(
    installation.repository_selection === "selected"
      ? current.repositories.map((r) => r.full_name)
      : [],
  );
  const save = useMutation({
    mutationFn: async () => {
      if (selection !== installation.repository_selection) {
        await call(
          ui.PATCH("/settings/installations/{id}", {
            params: { path: { id: installation.id } },
            body: { repository_selection: selection },
          }),
        );
      }
      if (selection === "selected") {
        const before = new Set(
          installation.repository_selection === "selected"
            ? current.repositories.map((r) => r.id)
            : [],
        );
        const after = new Set(repos.filter((r) => selected.includes(r.full_name)).map((r) => r.id));
        const path = (repository_id: number) => ({
          params: { path: { installation_id: installation.id, repository_id } },
        });
        for (const id of after) {
          if (!before.has(id))
            await call(
              api.PUT(
                "/user/installations/{installation_id}/repositories/{repository_id}",
                path(id),
              ),
            );
        }
        for (const id of before) {
          if (!after.has(id))
            await call(
              api.DELETE(
                "/user/installations/{installation_id}/repositories/{repository_id}",
                path(id),
              ),
            );
        }
      }
    },
    onError: (e) => {
      failed("Saving the installation", e);
      void queryClient.invalidateQueries({
        queryKey: installationReposQuery(installation.id).queryKey,
      });
    },
    onSuccess: () => toast.success("Installation saved"),
    onSettled: () =>
      Promise.all([
        queryClient.invalidateQueries({ queryKey: installationsQuery().queryKey }),
        queryClient.invalidateQueries({
          queryKey: installationReposQuery(installation.id).queryKey,
        }),
      ]),
  });
  return (
    <div className="mt-3 flex flex-col gap-3 border-t pt-3" data-testid="configure-installation">
      <p className="text-sm font-medium">Repository access</p>
      <RepositorySelection
        selection={selection}
        onSelection={setSelection}
        repos={repos}
        selected={selected}
        onSelected={setSelected}
        owner={owner}
      />
      <p className="text-muted-foreground text-xs" data-testid="current-repos">
        Reaches now: {current.repositories.map((r) => r.full_name).join(", ") || "no repositories"}
      </p>
      <div>
        <Button
          size="sm"
          onClick={() => save.mutate()}
          disabled={save.isPending || (selection === "selected" && selected.length === 0)}
        >
          {save.isPending ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
