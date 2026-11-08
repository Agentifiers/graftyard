import { useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  Delete02Icon,
  InformationCircleIcon,
  PencilEdit02Icon,
  Robot01Icon,
} from "@hugeicons/core-free-icons";
import { graftyard, type AgentAppRow, type AgentKind } from "@/areas/admin/graftyard-api";
import { agentAppsQuery, installAppsQuery } from "@/areas/admin/queries";
import { send, showFieldErrors, StateBadge, TextField, useWrite } from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
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
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

export const Route = createFileRoute("/stafftools/agents/apps")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(agentAppsQuery());
    void queryClient.prefetchQuery(installAppsQuery());
  },
  component: Page,
});

const KEY = agentAppsQuery().queryKey;
const KINDS: Record<AgentKind, { label: string; help: string }> = {
  graftyard_run: {
    label: "Graftyard-run",
    help: "One of Graftyard's own agent Apps. Always ActorKind::Agent.",
  },
  third_party: {
    label: "Third party",
    help: "A vendor agent. With confine set it is ActorKind::Agent and the host refuses it outside the prefix.",
  },
  mcp: {
    label: "MCP",
    help: "The install's MCP server. Acts with its user's rights; approve and merge stay refused.",
  },
};

// What decide makes of a row (design/AGENT-LAYER.md, "Agent actors in the layer").
const actor = (r: AgentAppRow) =>
  r.kind === "graftyard_run" || (r.kind === "third_party" && r.confine) ? "Agent" : "Bot";

function Page() {
  return (
    <>
      <PageHeader
        title="Agent apps"
        description="GitHub Apps flagged as agents. Their commands take the agent admission class, their pull requests need a fresh human approval to merge, and a confine prefix keeps their pushes inside it."
      />
      <Region name="agent apps" rows={5}>
        <AppsTable />
      </Region>
    </>
  );
}

function AppsTable() {
  const { data } = useSuspenseQuery(agentAppsQuery());
  const [editing, setEditing] = useState<AgentAppRow | "new">();
  const [removing, setRemoving] = useState<AgentAppRow>();
  const queryClient = useQueryClient();
  const unflag = useWrite({
    run: async (row: AgentAppRow) => {
      const before = queryClient.getQueryData(KEY);
      queryClient.setQueryData(KEY, (old) => old?.filter((r) => r.app_id !== row.app_id));
      try {
        return await send(
          graftyard.DELETE("/agents/apps/{app_id}", { params: { path: { app_id: row.app_id } } }),
        );
      } catch (e) {
        queryClient.setQueryData(KEY, before);
        throw e;
      }
    },
    invalidate: [KEY],
    success: (_, r) => `${r.slug} is no longer an agent app`,
    onSuccess: () => setRemoving(undefined),
  });
  return (
    <Panel
      title={`${data.length} flagged Apps`}
      actions={
        <Button size="sm" onClick={() => setEditing("new")}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Flag an App
        </Button>
      }
      flush
    >
      {data.length === 0 ? (
        <p className="text-muted-foreground p-6 text-center text-sm">
          No Apps are flagged as agents.
        </p>
      ) : (
        <table className="w-full text-sm" data-testid="agent-apps">
          <thead className="bg-muted/50 text-muted-foreground text-left text-xs">
            <tr>
              <th className="px-4 py-2 font-medium">App</th>
              <th className="px-4 py-2 font-medium">Kind</th>
              <th className="px-4 py-2 font-medium">Confine</th>
              <th className="px-4 py-2 font-medium">Acts as</th>
              <th />
            </tr>
          </thead>
          <tbody className="divide-y">
            {data.map((r) => (
              <tr key={r.app_id} data-app={r.slug}>
                <td className="px-4 py-2">
                  <span className="flex items-center gap-2">
                    <HugeiconsIcon
                      icon={Robot01Icon}
                      strokeWidth={2}
                      className="text-muted-foreground size-4"
                    />
                    <span className="font-medium">{r.slug}</span>
                    <span className="text-muted-foreground text-xs">#{r.app_id}</span>
                  </span>
                </td>
                <td className="px-4 py-2">
                  <Badge variant={r.kind === "third_party" ? "outline" : "info-light"} size="sm">
                    {KINDS[r.kind].label}
                  </Badge>
                </td>
                <td className="px-4 py-2">
                  {r.confine ? (
                    <code className="text-xs">{r.confine}</code>
                  ) : (
                    <span className="text-muted-foreground">none</span>
                  )}
                </td>
                <td className="px-4 py-2">
                  <StateBadge tone={actor(r) === "Agent" ? "warning-light" : "outline"}>
                    {actor(r)}
                  </StateBadge>
                </td>
                <td className="px-4 py-2 text-right whitespace-nowrap">
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`Edit ${r.slug}`}
                    onClick={() => setEditing(r)}
                  >
                    <HugeiconsIcon icon={PencilEdit02Icon} strokeWidth={2} />
                  </Button>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={`Unflag ${r.slug}`}
                    onClick={() => setRemoving(r)}
                  >
                    <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <Alert variant="info" className="m-3 w-auto">
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        <AlertTitle>A flagged App without confine is a Bot</AlertTitle>
        <AlertDescription>
          It still takes the agent admission class and its pull requests still carry AgentMeta, but
          the host does not refuse its pushes. Set confine, such as refs/heads/claude/**, to keep it
          inside a prefix.
        </AlertDescription>
      </Alert>
      {editing && (
        <Region name="install apps" fallback={null}>
          <AppForm
            row={editing === "new" ? undefined : editing}
            flagged={data}
            onClose={() => setEditing(undefined)}
          />
        </Region>
      )}
      <AlertDialog open={!!removing} onOpenChange={(o) => !o && setRemoving(undefined)}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Unflag {removing?.slug}?</AlertDialogTitle>
            <AlertDialogDescription>
              From the next decision on, it is an ordinary App: its pushes are no longer confined
              and its PRs no longer need AgentGate.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <Button
              size="sm"
              variant="destructive"
              disabled={unflag.isPending}
              onClick={() => unflag.mutate(removing!)}
            >
              Unflag
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}

function AppForm({
  row,
  flagged,
  onClose,
}: {
  row?: AgentAppRow;
  flagged: AgentAppRow[];
  onClose: () => void;
}) {
  const { data: apps } = useSuspenseQuery(installAppsQuery());
  const choices = row
    ? apps.filter((a) => a.app_id === row.app_id)
    : apps.filter((a) => !flagged.some((f) => f.app_id === a.app_id));
  const queryClient = useQueryClient();
  const save = useWrite({
    run: async (v: { app_id: number; kind: AgentKind; confine: string }) => {
      const body = { kind: v.kind, confine: v.confine.trim() || null };
      const { data, error, response } = await graftyard.PUT("/agents/apps/{app_id}", {
        params: { path: { app_id: v.app_id } },
        body,
      });
      // The 422 message names the rule (an App the install does not own).
      if (response.status === 422 && error) {
        form.setFieldMeta(error.errors?.[0]?.field === "confine" ? "confine" : "kind", (m) => ({
          ...m,
          errorMap: {
            ...m.errorMap,
            onServer:
              error.errors?.[0]?.field === "confine"
                ? "Use a refs/heads/ pattern, such as refs/heads/claude/**."
                : error.message,
          },
        }));
        throw Object.assign(new Error(error.message), { handled: true });
      }
      return send(Promise.resolve({ data, error, response }));
    },
    invalidate: [KEY],
    success: (r) => `${r.slug} saved as ${KINDS[r.kind].label}`,
    onSuccess: (r) => {
      queryClient.setQueryData(KEY, (old) => [
        ...(old ?? []).filter((x) => x.app_id !== r.app_id),
        r,
      ]);
      onClose();
    },
    onError: (e) =>
      (e instanceof Error && "handled" in e) ||
      showFieldErrors(e, form, { kind: "kind", confine: "confine" }),
  });
  const form = useForm({
    defaultValues: {
      app_id: row?.app_id ?? choices[0]?.app_id ?? 0,
      kind: row?.kind ?? ("third_party" as AgentKind),
      confine: row?.confine ?? "",
    },
    onSubmit: ({ value }) => save.mutateAsync(value).catch(() => {}),
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{row ? `Edit ${row.slug}` : "Flag an App as an agent"}</DialogTitle>
          <DialogDescription>
            PUT /api/graftyard/v1/agents/apps/{"{app_id}"}. It takes effect on the next decision.
          </DialogDescription>
        </DialogHeader>
        {choices.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            Every App on this install is already flagged.
          </p>
        ) : (
          <form
            id="agent-app"
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              void form.handleSubmit();
            }}
          >
            <form.Field name="app_id">
              {(field) => (
                <Field>
                  <FieldLabel htmlFor="app">App</FieldLabel>
                  <NativeSelect
                    id="app"
                    value={field.state.value}
                    disabled={!!row}
                    onChange={(e) => field.handleChange(Number(e.target.value))}
                  >
                    {choices.map((a) => (
                      <NativeSelectOption key={a.app_id} value={a.app_id}>
                        {a.name} ({a.slug}){a.owned ? ", owned by this install" : ""}
                      </NativeSelectOption>
                    ))}
                  </NativeSelect>
                  <FieldDescription>
                    {apps.find((a) => a.app_id === field.state.value)?.description}
                  </FieldDescription>
                </Field>
              )}
            </form.Field>
            <form.Field name="kind">
              {(field) => (
                <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                  <FieldLabel htmlFor="kind">Kind</FieldLabel>
                  <NativeSelect
                    id="kind"
                    value={field.state.value}
                    onChange={(e) => {
                      field.setMeta((m) => ({
                        ...m,
                        errorMap: { ...m.errorMap, onServer: undefined },
                      }));
                      field.handleChange(e.target.value as AgentKind);
                    }}
                  >
                    {(Object.keys(KINDS) as AgentKind[]).map((k) => (
                      <NativeSelectOption key={k} value={k}>
                        {KINDS[k].label}
                      </NativeSelectOption>
                    ))}
                  </NativeSelect>
                  <FieldDescription>{KINDS[field.state.value].help}</FieldDescription>
                  {field.state.meta.errors.length > 0 && (
                    <FieldError data-testid="kind-error">
                      {field.state.meta.errors.join(". ")}
                    </FieldError>
                  )}
                </Field>
              )}
            </form.Field>
            <form.Field
              name="confine"
              validators={{
                onSubmit: ({ value }) =>
                  value.trim() && !/^refs\/heads\/[\w./*-]+$/.test(value.trim())
                    ? "Use a refs/heads/ pattern, such as refs/heads/claude/**."
                    : undefined,
              }}
            >
              {(field) => (
                <TextField
                  field={field}
                  label="Confine"
                  placeholder="refs/heads/claude/**"
                  description="Leave empty for no prefix. The host refuses any ref update outside it, naming the rule."
                />
              )}
            </form.Field>
          </form>
        )}
        <DialogFooter>
          <Button size="sm" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            size="sm"
            type="submit"
            form="agent-app"
            disabled={save.isPending || choices.length === 0}
          >
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
