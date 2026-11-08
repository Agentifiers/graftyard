import { useMemo, useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { createColumnHelper, useTable } from "@tanstack/react-table";
import { HugeiconsIcon } from "@hugeicons/react";
import { Download04Icon, Tag01Icon } from "@hugeicons/core-free-icons";
import type { Schemas } from "@/api/client";
import { gitTagQuery, tagRefsQuery, tagsQuery } from "@/areas/code/queries";
import { CopyButton, short } from "@/areas/code/refs";
import { Ago } from "@/components/github";
import { PageHeader } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import {
  DataGrid,
  DataGridContainer,
  dataGridFeatures,
} from "@/components/reui/data-grid/data-grid";
import { DataGridColumnHeader } from "@/components/reui/data-grid/data-grid-column-header";
import { DataGridPagination } from "@/components/reui/data-grid/data-grid-pagination";
import { DataGridTable } from "@/components/reui/data-grid/data-grid-table";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Input } from "@/components/ui/input";

// Tags, newest first, in a sortable grid. An annotated tag's message and tagger come from
// GET /git/tags/{sha}, which the ref names; a lightweight tag points at its commit.

export const Route = createFileRoute("/$owner/$repo/tags")({
  loader: ({ context, params }) => {
    void context.queryClient.prefetchQuery(tagsQuery(params));
    void context.queryClient.prefetchQuery(tagRefsQuery(params));
  },
  component: TagsPage,
});

function TagsPage() {
  return (
    <div className="flex flex-col gap-4">
      <PageHeader title="Tags" description="Every tag in the repository, and what it points at." />
      <Region name="tags" rows={6}>
        <TagGrid />
      </Region>
    </div>
  );
}

type Row = Schemas["tag"] & { object: Schemas["git-ref"]["object"] | undefined };

const helper = createColumnHelper<typeof dataGridFeatures, Row>();

function TagGrid() {
  const params = Route.useParams();
  const { data: tags } = useSuspenseQuery(tagsQuery(params));
  const { data: refs } = useSuspenseQuery(tagRefsQuery(params));
  const [q, setQ] = useState("");
  const rows = useMemo<Row[]>(
    () =>
      tags
        .filter((t) => t.name.toLowerCase().includes(q.toLowerCase()))
        .map((t) => ({ ...t, object: refs.find((r) => r.ref === `refs/tags/${t.name}`)?.object })),
    [tags, refs, q],
  );
  const columns = useMemo(
    () =>
      helper.columns([
        helper.accessor("name", {
          header: ({ column }) => <DataGridColumnHeader column={column} title="Tag" />,
          cell: ({ row }) => (
            <span className="flex items-center gap-2">
              <HugeiconsIcon
                icon={Tag01Icon}
                strokeWidth={2}
                className="text-muted-foreground size-4"
              />
              <Link
                to="/$owner/$repo/tree/$"
                params={{ ...params, _splat: row.original.name }}
                className="font-mono font-semibold hover:underline"
              >
                {row.original.name}
              </Link>
              {row.original.object?.type === "tag" ? (
                <Badge variant="outline" size="sm">
                  Annotated
                </Badge>
              ) : (
                <Badge variant="secondary" size="sm">
                  Lightweight
                </Badge>
              )}
            </span>
          ),
          size: 260,
        }),
        helper.display({
          id: "message",
          header: "Message",
          cell: ({ row }) => (
            <TagMessage
              sha={row.original.object?.type === "tag" ? row.original.object.sha : undefined}
            />
          ),
          size: 320,
        }),
        helper.accessor((r) => r.commit.sha, {
          id: "commit",
          header: "Commit",
          cell: ({ row }) => (
            <span className="flex items-center gap-1">
              <Link
                to="/$owner/$repo/commit/$sha"
                params={{ ...params, sha: row.original.commit.sha }}
                className="font-mono text-xs hover:underline"
              >
                {short(row.original.commit.sha)}
              </Link>
              <CopyButton value={row.original.commit.sha} label="Copy the commit SHA" />
            </span>
          ),
          size: 140,
        }),
        helper.display({
          id: "downloads",
          header: "Downloads",
          cell: ({ row }) => (
            <span className="flex items-center gap-3 text-xs">
              <a
                href={row.original.zipball_url}
                className="text-primary flex items-center gap-1 hover:underline"
              >
                <HugeiconsIcon icon={Download04Icon} strokeWidth={2} className="size-3.5" />
                zip
              </a>
              <a
                href={row.original.tarball_url}
                className="text-primary flex items-center gap-1 hover:underline"
              >
                <HugeiconsIcon icon={Download04Icon} strokeWidth={2} className="size-3.5" />
                tar.gz
              </a>
            </span>
          ),
          size: 160,
        }),
      ]),
    [params],
  );
  const table = useTable({
    features: dataGridFeatures,
    columns,
    data: rows,
    getRowId: (r) => r.name,
    initialState: { pagination: { pageIndex: 0, pageSize: 10 } },
  });
  if (tags.length === 0) {
    return (
      <Empty className="border py-12">
        <EmptyHeader>
          <EmptyTitle>No tags yet</EmptyTitle>
          <EmptyDescription>
            Tags mark release points. Push one with git tag and git push --tags.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <div className="flex flex-col gap-2" data-testid="tags">
      <Input
        type="search"
        aria-label="Filter tags"
        placeholder="Find a tag"
        className="max-w-xs"
        value={q}
        onChange={(e) => setQ(e.target.value)}
      />
      <DataGrid
        table={table}
        recordCount={rows.length}
        emptyMessage="No tags match."
        tableLayout={{ dense: true, headerBackground: true }}
      >
        <DataGridContainer className="rounded-sm border">
          <DataGridTable />
        </DataGridContainer>
        <DataGridPagination sizes={[10, 25, 50]} />
      </DataGrid>
    </div>
  );
}

function TagMessage({ sha }: { sha?: string }) {
  const params = Route.useParams();
  const { data, isPending, error } = useQuery({
    ...gitTagQuery({ ...params, sha: sha ?? "" }),
    enabled: Boolean(sha),
  });
  if (!sha)
    return <span className="text-muted-foreground text-xs">Points straight at a commit</span>;
  if (isPending) return <span className="text-muted-foreground text-xs">Loading</span>;
  if (error)
    return (
      <span className="text-destructive text-xs">Could not load the tag: {error.message}</span>
    );
  return (
    <span className="flex min-w-0 flex-col text-xs">
      <span className="truncate">{data.message}</span>
      <span className="text-muted-foreground">
        {data.tagger.name} tagged <Ago at={data.tagger.date} />
      </span>
    </span>
  );
}
