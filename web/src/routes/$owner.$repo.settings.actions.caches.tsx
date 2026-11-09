import { useMemo } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createColumnHelper, useTable } from "@tanstack/react-table";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, GitBranchIcon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { branchesQuery, cacheUsageQuery, cachesQuery } from "@/areas/actions/queries";
import { formatBytes } from "@/areas/actions/status";
import { send, useWrite } from "@/areas/actions/write";
import { Ago } from "@/components/github";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
  type DataGridFeatures,
} from "@/components/reui/data-grid/data-grid";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Progress } from "@/components/ui/progress";

type Sort = "last_accessed_at" | "created_at" | "size_in_bytes";
type Search = { ref?: string; sort?: Sort; direction?: "asc" };

const SORTS: Record<Sort, string> = {
  last_accessed_at: "Last used",
  created_at: "Created",
  size_in_bytes: "Size",
};

export const Route = createFileRoute("/$owner/$repo/settings/actions/caches")({
  validateSearch: (s: Record<string, unknown>): Search => ({
    ref: typeof s.ref === "string" && s.ref.startsWith("refs/") ? s.ref : undefined,
    sort: s.sort === "created_at" || s.sort === "size_in_bytes" ? s.sort : undefined,
    direction: s.direction === "asc" ? "asc" : undefined,
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) => {
    void context.queryClient.prefetchQuery(cacheUsageQuery(params));
    void context.queryClient.prefetchQuery(
      cachesQuery({
        ...params,
        ref: deps.ref,
        sort: deps.sort ?? "last_accessed_at",
        direction: deps.direction ?? "desc",
      }),
    );
  },
  component: CachesPage,
});

function CachesPage() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="Caches"
        description="Dependencies and build outputs saved by actions/cache. The oldest unused caches go first when the repository passes its limit."
      />
      <Region name="cache usage" rows={1}>
        <Usage />
      </Region>
      <Region name="caches" rows={6}>
        <Caches />
      </Region>
    </div>
  );
}

function Usage() {
  const params = Route.useParams();
  const { data } = useSuspenseQuery(cacheUsageQuery(params));
  const limit = data.policy.repo_cache_size_limit_in_gb * 1024 ** 3;
  const used = data.usage.active_caches_size_in_bytes;
  return (
    <div
      className="bg-card flex flex-col gap-2 rounded-sm border px-4 py-3 text-sm"
      data-testid="cache-usage"
    >
      <div className="flex justify-between">
        <span>
          <span className="font-semibold">{formatBytes(used)}</span> of{" "}
          {data.policy.repo_cache_size_limit_in_gb} GB used
        </span>
        <span className="text-muted-foreground">{data.usage.active_caches_count} caches</span>
      </div>
      <Progress value={Math.min(100, (used / limit) * 100)} />
    </div>
  );
}

type Cache = Schemas["actions-cache-list"]["actions_caches"][number];
const column = createColumnHelper<DataGridFeatures, Cache>();

function Caches() {
  const params = Route.useParams();
  const search = Route.useSearch();
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const options = cachesQuery({
    ...params,
    ref: search.ref,
    sort: search.sort ?? "last_accessed_at",
    direction: search.direction ?? "desc",
  });
  const { data } = useSuspenseQuery(options);
  const { data: branches } = useSuspenseQuery(branchesQuery(params));

  const remove = useWrite({
    run: (c: Cache) =>
      send(
        api.DELETE("/repos/{owner}/{repo}/actions/caches/{cache_id}", {
          params: { path: { ...params, cache_id: c.id ?? 0 } },
        }),
      ),
    invalidates: () => [
      ["repos", params.owner, params.repo, "actions", "caches"],
      cacheUsageQuery(params).queryKey,
    ],
    optimistic: (c) => {
      const before = queryClient.getQueryData(options.queryKey);
      queryClient.setQueryData(
        options.queryKey,
        (d) =>
          d && {
            total_count: d.total_count - 1,
            actions_caches: d.actions_caches.filter((x) => x.id !== c.id),
          },
      );
      return () => queryClient.setQueryData(options.queryKey, before);
    },
    success: (c) => `Deleted cache ${c.key}`,
    failure: (c) => `Could not delete cache ${c.key}`,
  });

  const columns = useMemo(
    () =>
      column.columns([
        column.accessor("key", {
          header: "Key",
          size: 380,
          cell: ({ row }) => (
            <div className="min-w-0">
              <code className="block truncate font-mono text-xs font-semibold">
                {row.original.key}
              </code>
              <Badge variant="info-light" size="sm" className="mt-1 font-mono">
                <HugeiconsIcon icon={GitBranchIcon} strokeWidth={2} />
                {row.original.ref?.replace("refs/heads/", "")}
              </Badge>
            </div>
          ),
        }),
        column.accessor("size_in_bytes", {
          header: "Size",
          size: 90,
          cell: ({ getValue }) => (
            <span className="tabular-nums">{formatBytes(getValue() ?? 0)}</span>
          ),
        }),
        column.accessor("created_at", {
          header: "Created",
          size: 110,
          cell: ({ getValue }) => (
            <span className="text-muted-foreground">{getValue() && <Ago at={getValue()!} />}</span>
          ),
        }),
        column.accessor("last_accessed_at", {
          header: "Last used",
          size: 110,
          cell: ({ getValue }) => (
            <span className="text-muted-foreground">{getValue() && <Ago at={getValue()!} />}</span>
          ),
        }),
        column.display({
          id: "delete",
          header: "",
          size: 48,
          cell: ({ row }) => (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={`Delete cache ${row.original.key}`}
              onClick={() => remove.mutate(row.original)}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
            </Button>
          ),
        }),
      ]),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const table = useTable({
    features: dataGridFeatures,
    columns,
    data: data.actions_caches,
    getRowId: (c) => String(c.id),
    initialState: { pagination: { pageIndex: 0, pageSize: 100 } },
  });

  return (
    <Panel
      title={`${data.total_count} cache${data.total_count === 1 ? "" : "s"}`}
      flush
      actions={
        <>
          <NativeSelect
            size="sm"
            aria-label="Branch"
            value={search.ref ?? ""}
            onChange={(e) =>
              navigate({
                search: (p) => ({ ...p, ref: e.target.value || undefined }),
                replace: true,
              })
            }
          >
            <NativeSelectOption value="">All branches</NativeSelectOption>
            {branches.map((b) => (
              <NativeSelectOption key={b.name} value={`refs/heads/${b.name}`}>
                {b.name}
              </NativeSelectOption>
            ))}
          </NativeSelect>
          <NativeSelect
            size="sm"
            aria-label="Sort"
            value={`${search.sort ?? "last_accessed_at"}:${search.direction ?? "desc"}`}
            onChange={(e) => {
              const [sort, direction] = e.target.value.split(":") as [Sort, "asc" | "desc"];
              void navigate({
                search: (p) => ({
                  ...p,
                  sort: sort === "last_accessed_at" ? undefined : sort,
                  direction: direction === "asc" ? "asc" : undefined,
                }),
                replace: true,
              });
            }}
          >
            {(Object.keys(SORTS) as Sort[]).flatMap((s) =>
              (["desc", "asc"] as const).map((d) => (
                <NativeSelectOption key={`${s}:${d}`} value={`${s}:${d}`}>
                  {SORTS[s]},{" "}
                  {s === "size_in_bytes"
                    ? d === "desc"
                      ? "largest"
                      : "smallest"
                    : d === "desc"
                      ? "newest"
                      : "oldest"}{" "}
                  first
                </NativeSelectOption>
              )),
            )}
          </NativeSelect>
        </>
      }
    >
      <DataGrid
        table={table}
        recordCount={data.actions_caches.length}
        emptyMessage="No caches match."
        tableLayout={{ dense: true, headerBackground: true, width: "auto" }}
      >
        <DataGridContainer className="border-0" data-testid="caches">
          <DataGridTable />
        </DataGridContainer>
      </DataGrid>
    </Panel>
  );
}
