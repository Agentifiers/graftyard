import { createFileRoute, Link } from "@tanstack/react-router";
import { useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { LockKeyIcon } from "@hugeicons/core-free-icons";
import { preReceiveHooksQuery, repoPreReceiveQuery, staffReposQuery } from "@/areas/admin/queries";
import type { StaffRepo } from "@/areas/admin/stafftools-api";
import { columns, Grid, StateBadge, when } from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";

export const Route = createFileRoute("/stafftools/repositories")({
  validateSearch: (s: Record<string, unknown>): { q?: string; repo?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    repo: typeof s.repo === "string" && s.repo.includes("/") ? s.repo : undefined,
  }),
  loader: ({ context: { queryClient }, location }) => {
    void queryClient.prefetchQuery(staffReposQuery());
    const repo = new URLSearchParams(location.searchStr).get("repo");
    if (repo?.includes("/")) {
      const [owner, name] = repo.split("/") as [string, string];
      void queryClient.prefetchQuery(repoPreReceiveQuery(owner, name));
    }
  },
  component: Repos,
});

function Repos() {
  const { repo } = Route.useSearch();
  return (
    <>
      <PageHeader
        title="Repositories"
        description="Every repository on the instance, with migration locks. Pick one to see its pre-receive hooks."
      />
      <div className="flex flex-col gap-4">
        <Region name="repositories" rows={5}>
          <RepoGrid />
        </Region>
        {repo && (
          <Region name="repository pre-receive hooks" rows={3}>
            <RepoHooks key={repo} fullName={repo} />
          </Region>
        )}
      </div>
    </>
  );
}

function RepoGrid() {
  const { data } = useSuspenseQuery(staffReposQuery());
  const { q = "", repo } = Route.useSearch();
  const navigate = Route.useNavigate();
  const col = columns<StaffRepo>();
  const cols = [
    col.accessor("full_name", {
      header: "Repository",
      cell: ({ row }) => (
        <span className="flex items-center gap-2">
          <Link
            to="/$owner/$repo"
            params={{ owner: row.original.owner.login, repo: row.original.name }}
            className="font-semibold hover:underline"
            onClick={(e) => e.stopPropagation()}
          >
            {row.original.full_name}
          </Link>
          {row.original.locked_by_migration && (
            <Badge
              variant="warning-light"
              size="sm"
              title={`Locked by migration ${row.original.locked_by_migration}`}
            >
              <HugeiconsIcon icon={LockKeyIcon} strokeWidth={2} />
              locked
            </Badge>
          )}
          {row.original.full_name === repo && <Badge size="sm">selected</Badge>}
        </span>
      ),
    }),
    col.accessor((r) => r.visibility ?? "", {
      id: "visibility",
      header: "Visibility",
      cell: ({ getValue }) => (
        <StateBadge
          tone={
            getValue() === "public"
              ? "success-light"
              : getValue() === "internal"
                ? "info-light"
                : "outline"
          }
        >
          {getValue()}
        </StateBadge>
      ),
    }),
    col.accessor((r) => r.language ?? "", { id: "language", header: "Language" }),
    col.accessor((r) => r.size ?? 0, {
      id: "size",
      header: "Size",
      cell: ({ getValue }) => `${(getValue() / 1024).toFixed(1)} MB`,
    }),
    col.accessor((r) => r.pushed_at ?? "", {
      id: "pushed",
      header: "Pushed",
      cell: ({ getValue }) => when(getValue()),
    }),
  ];
  return (
    <Grid
      rows={data}
      cols={cols}
      rowId={(r) => String(r.id)}
      search={q}
      onSearch={(v) => navigate({ search: (p) => ({ ...p, q: v || undefined }), replace: true })}
      placeholder="Search repositories"
      empty="No repositories match."
      onRowClick={(r) => navigate({ search: (p) => ({ ...p, repo: r.full_name }) })}
    />
  );
}

// What each instance hook does on this repository. A repository admin overrides a hook
// that allows it, in the repository's settings.
function RepoHooks({ fullName }: { fullName: string }) {
  const [owner, repo] = fullName.split("/") as [string, string];
  const { data } = useSuspenseQuery(repoPreReceiveQuery(owner, repo));
  const { data: hooks } = useSuspenseQuery(preReceiveHooksQuery());
  return (
    <Panel
      title={`Pre-receive hooks on ${fullName}`}
      actions={
        <Link to="/stafftools/pre-receive-hooks" className="text-xs hover:underline">
          Edit instance hooks
        </Link>
      }
      flush
    >
      {data.length === 0 ? (
        <p className="text-muted-foreground p-4 text-sm">No pre-receive hooks on this instance.</p>
      ) : (
        <ul className="divide-y" data-testid="repo-hooks">
          {data.map((h) => {
            const global = hooks.find((g) => g.id === h.id);
            return (
              <li key={h.id} className="flex flex-wrap items-center gap-3 px-4 py-2 text-sm">
                <span className="min-w-0 flex-1 font-medium">{h.name}</span>
                <span className="text-muted-foreground text-xs">
                  {global?.allow_downstream_configuration
                    ? "Repository admins may override this."
                    : "Repositories cannot override this."}
                </span>
                <StateBadge
                  tone={
                    h.enforcement === "enabled"
                      ? "success-light"
                      : h.enforcement === "testing"
                        ? "warning-light"
                        : "outline"
                  }
                >
                  {h.enforcement}
                </StateBadge>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}
