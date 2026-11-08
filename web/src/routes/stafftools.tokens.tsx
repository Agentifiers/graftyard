import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon } from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { keysQuery, tokensQuery } from "@/areas/admin/queries";
import { columns, Grid, send, useWrite, when } from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Badge } from "@/components/reui/badge";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/stafftools/tokens")({
  validateSearch: (s: Record<string, unknown>): { q?: string; keys?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
    keys: typeof s.keys === "string" && s.keys ? s.keys : undefined,
  }),
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(tokensQuery());
    void queryClient.prefetchQuery(keysQuery());
  },
  component: () => (
    <>
      <PageHeader
        title="Tokens and keys"
        description="Personal access tokens, OAuth and impersonation tokens, and SSH keys across the instance. Deleting one revokes it at once."
      />
      <div className="flex flex-col gap-4">
        <Panel title="Tokens">
          <Region name="tokens" rows={6}>
            <Tokens />
          </Region>
        </Panel>
        <Panel title="SSH keys">
          <Region name="SSH keys" rows={6}>
            <Keys />
          </Region>
        </Panel>
      </div>
    </>
  ),
});

type Auth = Schemas["authorization"];
type Key = Schemas["public-key-full"];

// Removes the row at once, puts it back if the DELETE fails.
function useOptimisticDelete<T extends { id: number }>(
  key: readonly unknown[],
  run: (row: T) => Promise<unknown>,
  label: (row: T) => string,
) {
  const queryClient = useQueryClient();
  return useWrite({
    run: async (row: T) => {
      const before = queryClient.getQueryData(key);
      queryClient.setQueryData<T[]>(key, (old) => old?.filter((r) => r.id !== row.id));
      try {
        return await run(row);
      } catch (e) {
        queryClient.setQueryData(key, before);
        throw e;
      }
    },
    invalidate: [key as unknown[]],
    success: (_, row) => `Revoked ${label(row)}`,
  });
}

function Tokens() {
  const { data } = useSuspenseQuery(tokensQuery());
  const { q = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [confirm, setConfirm] = useState<Auth>();
  const remove = useOptimisticDelete<Auth>(
    tokensQuery().queryKey,
    (t) => send(api.DELETE("/admin/tokens/{token_id}", { params: { path: { token_id: t.id } } })),
    (t) => t.note ?? `token …${t.token_last_eight}`,
  );
  const col = columns<Auth>();
  const cols = [
    col.accessor((t) => t.user?.login ?? "", {
      id: "user",
      header: "User",
      cell: ({ getValue }) => (
        <Link
          to="/stafftools/users/$username"
          params={{ username: getValue() }}
          className="font-medium hover:underline"
        >
          {getValue()}
        </Link>
      ),
    }),
    col.accessor((t) => t.note ?? "", { id: "note", header: "Note" }),
    col.accessor((t) => t.app.name, {
      id: "app",
      header: "Kind",
      cell: ({ getValue }) => (
        <Badge
          variant={getValue().includes("Administrator") ? "warning-light" : "outline"}
          size="sm"
        >
          {getValue()}
        </Badge>
      ),
    }),
    col.accessor((t) => (t.scopes ?? []).join(" "), {
      id: "scopes",
      header: "Scopes",
      cell: ({ getValue }) => <code className="text-xs">{getValue()}</code>,
    }),
    col.accessor((t) => t.token_last_eight ?? "", {
      id: "last8",
      header: "Ends in",
      cell: ({ getValue }) => <code className="text-xs">…{getValue()}</code>,
    }),
    col.accessor((t) => t.expires_at ?? "", {
      id: "expires",
      header: "Expires",
      cell: ({ getValue }) => (getValue() ? when(getValue()) : "Never"),
    }),
    col.display({
      id: "actions",
      header: "",
      cell: ({ row }) => (
        <Button
          size="xs"
          variant="destructive"
          aria-label={`Revoke ${row.original.note}`}
          onClick={() => setConfirm(row.original)}
        >
          <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
          Revoke
        </Button>
      ),
    }),
  ];
  return (
    <>
      <Grid
        rows={data}
        cols={cols}
        rowId={(t) => String(t.id)}
        search={q}
        onSearch={(v) => navigate({ search: (p) => ({ ...p, q: v || undefined }), replace: true })}
        placeholder="Search tokens by user, note, or scope"
        empty="No tokens match."
      />
      <Confirm
        what={confirm ? `${confirm.note ?? "this token"} for ${confirm.user?.login}` : ""}
        open={!!confirm}
        onCancel={() => setConfirm(undefined)}
        onConfirm={() => {
          remove.mutate(confirm!);
          setConfirm(undefined);
        }}
      />
    </>
  );
}

function Keys() {
  const { data } = useSuspenseQuery(keysQuery());
  const { keys = "" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [confirm, setConfirm] = useState<Key>();
  const remove = useOptimisticDelete<Key>(
    keysQuery().queryKey,
    (k) =>
      send(api.DELETE("/admin/keys/{key_ids}", { params: { path: { key_ids: String(k.id) } } })),
    (k) => k.title,
  );
  const col = columns<Key>();
  const cols = [
    col.accessor("title", {
      header: "Title",
      cell: ({ getValue }) => <span className="font-medium">{getValue()}</span>,
    }),
    col.accessor((k) => (k.repository_id ? "deploy key" : (k.added_by ?? "")), {
      id: "owner",
      header: "Owner",
    }),
    col.accessor("key", {
      header: "Key",
      cell: ({ getValue }) => <code className="block max-w-56 truncate text-xs">{getValue()}</code>,
    }),
    col.accessor((k) => (k.read_only ? "read" : "read, write"), { id: "access", header: "Access" }),
    col.accessor((k) => k.last_used ?? "", {
      id: "last_used",
      header: "Last used",
      cell: ({ getValue }) => when(getValue()),
    }),
    col.display({
      id: "actions",
      header: "",
      cell: ({ row }) => (
        <Button
          size="xs"
          variant="destructive"
          aria-label={`Delete ${row.original.title}`}
          onClick={() => setConfirm(row.original)}
        >
          <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
          Delete
        </Button>
      ),
    }),
  ];
  return (
    <>
      <Grid
        rows={data}
        cols={cols}
        rowId={(k) => String(k.id)}
        search={keys}
        onSearch={(v) =>
          navigate({ search: (p) => ({ ...p, keys: v || undefined }), replace: true })
        }
        placeholder="Search keys by title or owner"
        empty="No keys match."
      />
      <Confirm
        what={confirm?.title ?? ""}
        open={!!confirm}
        onCancel={() => setConfirm(undefined)}
        onConfirm={() => {
          remove.mutate(confirm!);
          setConfirm(undefined);
        }}
      />
    </>
  );
}

function Confirm({
  what,
  open,
  onCancel,
  onConfirm,
}: {
  what: string;
  open: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog open={open} onOpenChange={(o) => !o && onCancel()}>
      <AlertDialogContent className="rounded-sm">
        <AlertDialogHeader>
          <AlertDialogTitle>Revoke {what}?</AlertDialogTitle>
          <AlertDialogDescription>
            Anything using it stops working on its next request.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
          <Button variant="destructive" size="sm" onClick={onConfirm}>
            Revoke
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
