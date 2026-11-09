import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import { Delete02Icon, McpServerIcon } from "@hugeicons/core-free-icons";
import { call, gy, type Grant } from "@/areas/agents/api";
import { grantsQuery } from "@/areas/agents/queries";
import { Ago } from "@/components/github";
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
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";

export const Route = createFileRoute("/settings/mcp")({
  loader: ({ context }) => void context.queryClient.prefetchQuery(grantsQuery()),
  component: () => (
    <div className="flex flex-col gap-4">
      <PageHeader
        title="MCP clients"
        description="AI tools you approved to use Graftyard MCP as you. Revoking one signs it out everywhere."
      />
      <Panel title="Approved clients" flush>
        <Region name="MCP grants" rows={3}>
          <Grants />
        </Region>
      </Panel>
    </div>
  ),
});

function Grants() {
  const client = useQueryClient();
  const key = grantsQuery().queryKey;
  const { data } = useSuspenseQuery(grantsQuery());
  const [confirm, setConfirm] = useState<Grant>();
  const revoke = useMutation({
    mutationFn: (id: string) => call(gy.DELETE("/mcp/grants/{id}", { params: { path: { id } } })),
    onMutate: async (id) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData(key);
      client.setQueryData<Grant[]>(key, (old) => old?.filter((g) => g.id !== id));
      return { before };
    },
    onError: (e, _id, ctx) => {
      if (ctx?.before) client.setQueryData(key, ctx.before);
      toast.error("Grant not revoked", { description: e.message });
    },
    onSuccess: () => toast.success("Client revoked"),
    onSettled: () => client.invalidateQueries({ queryKey: key }),
  });

  if (!data.length) {
    return (
      <Empty data-grants-empty>
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <HugeiconsIcon icon={McpServerIcon} strokeWidth={2} />
          </EmptyMedia>
          <EmptyTitle>No MCP clients</EmptyTitle>
          <EmptyDescription>
            When you approve an AI tool on the consent page at mcp.&lt;host&gt;, it shows up here.
          </EmptyDescription>
        </EmptyHeader>
      </Empty>
    );
  }
  return (
    <>
      <ul className="divide-y" data-testid="grant-list">
        {data.map((g) => (
          <li key={g.id} className="flex flex-wrap items-start gap-3 px-4 py-3" data-grant={g.id}>
            <HugeiconsIcon
              icon={McpServerIcon}
              strokeWidth={2}
              className="text-muted-foreground mt-0.5 size-4"
            />
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="truncate font-mono text-sm font-medium" title={g.client}>
                {g.client}
              </span>
              <span className="flex flex-wrap items-center gap-1 text-xs">
                <span className="text-muted-foreground">Redirects to</span>
                {g.redirect_hosts.map((h) => (
                  <Badge key={h} variant="outline" size="sm" className="font-mono">
                    {h}
                  </Badge>
                ))}
              </span>
              <span className="text-muted-foreground text-xs">
                Approved <Ago at={g.created_at} />. Refresh generation {g.generation}.{" "}
                <code className="font-mono">{g.id}</code>
              </span>
            </div>
            <Button
              size="sm"
              variant="destructive"
              onClick={() => setConfirm(g)}
              data-revoke={g.id}
            >
              <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
              Revoke
            </Button>
          </li>
        ))}
      </ul>
      <AlertDialog open={!!confirm} onOpenChange={(open) => !open && setConfirm(undefined)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Revoke this client?</AlertDialogTitle>
            <AlertDialogDescription>
              <span className="font-mono break-all">{confirm?.client}</span> loses access at its
              next refresh. Its tokens stop working within the hour.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                if (confirm) revoke.mutate(confirm.id);
                setConfirm(undefined);
              }}
            >
              Revoke
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
