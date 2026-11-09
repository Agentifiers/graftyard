import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  ArrowLeft02Icon,
  Delete02Icon,
  SentIcon,
  WebhookIcon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { hookDeliveriesQuery, hookQuery, hooksQuery } from "@/areas/admin/queries";
import type { HookDelivery } from "@/areas/admin/stafftools-api";
import { send, showFieldErrors, StateBadge, TextField, useWrite, when } from "@/areas/admin/ui";
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
import { Checkbox } from "@/components/ui/checkbox";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { Field, FieldError, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Switch } from "@/components/ui/switch";

export const Route = createFileRoute("/stafftools/hooks")({
  validateSearch: (s: Record<string, unknown>): { hook?: number | "new" } => ({
    hook: s.hook === "new" ? "new" : Number(s.hook) > 0 ? Number(s.hook) : undefined,
  }),
  loader: ({ context: { queryClient }, location }) => {
    const id = Number(new URLSearchParams(location.searchStr).get("hook"));
    if (id > 0) {
      void queryClient.prefetchQuery(hookQuery(id));
      void queryClient.prefetchQuery(hookDeliveriesQuery(id));
    } else void queryClient.prefetchQuery(hooksQuery());
  },
  component: Page,
});

type Hook = Schemas["global-hook"];
const HOOKS = hooksQuery().queryKey;
const EVENTS = [
  ["user", "Users created, renamed, deleted, suspended, or promoted"],
  ["organization", "Organizations created, renamed, or deleted"],
  ["repository", "Repositories created, renamed, transferred, or deleted"],
  ["team", "Teams created, renamed, or deleted"],
  ["membership", "Users added to or removed from teams"],
  ["member", "Users added to or removed from organizations"],
] as const;

function Page() {
  const { hook } = Route.useSearch();
  return (
    <>
      <PageHeader
        title="Global webhooks"
        description="Instance-wide events, delivered as JSON or form posts to each URL here."
        actions={
          !hook && (
            <Button size="sm" render={<Link from={Route.fullPath} search={{ hook: "new" }} />}>
              <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
              Add webhook
            </Button>
          )
        }
      />
      {hook === undefined ? (
        <Region name="global webhooks" rows={4}>
          <HookList />
        </Region>
      ) : hook === "new" ? (
        <HookDetail hook={undefined} />
      ) : (
        <Region key={hook} name="webhook" rows={6}>
          <SavedHook id={hook} />
        </Region>
      )}
    </>
  );
}

function HookList() {
  const { data } = useSuspenseQuery(hooksQuery());
  if (data.length === 0) {
    return (
      <Panel>
        <p className="text-muted-foreground p-6 text-center text-sm">
          No global webhooks. Add one to stream instance events.
        </p>
      </Panel>
    );
  }
  return (
    <Panel flush>
      <ul className="divide-y" data-testid="global-hooks">
        {data.map((h) => (
          <li key={h.id} className="flex items-center gap-3 px-4 py-2.5 text-sm">
            <HugeiconsIcon
              icon={WebhookIcon}
              strokeWidth={2}
              className="text-muted-foreground size-4"
            />
            <Link
              from={Route.fullPath}
              search={{ hook: h.id }}
              className="min-w-0 flex-1 truncate font-medium hover:underline"
            >
              {h.config?.url}
            </Link>
            <span className="text-muted-foreground hidden text-xs sm:inline">
              {h.events?.join(", ")}
            </span>
            <StateBadge tone={h.active ? "success-light" : "outline"}>
              {h.active ? "active" : "inactive"}
            </StateBadge>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function SavedHook({ id }: { id: number }) {
  const { data: hook } = useSuspenseQuery(hookQuery(id));
  if (!hook) {
    return (
      <Panel>
        <p className="text-sm">
          That webhook no longer exists.{" "}
          <Link from={Route.fullPath} search={{}} className="underline">
            Back to the list
          </Link>
        </p>
      </Panel>
    );
  }
  return <HookDetail hook={hook} />;
}

// The form for a new hook, or a saved hook with its deliveries.
function HookDetail({ hook }: { hook: Hook | undefined }) {
  const navigate = Route.useNavigate();
  return (
    <div className="flex flex-col gap-4">
      <Link
        from={Route.fullPath}
        search={{}}
        className="text-muted-foreground flex items-center gap-1 text-sm hover:underline"
      >
        <HugeiconsIcon icon={ArrowLeft02Icon} strokeWidth={2} className="size-4" />
        All webhooks
      </Link>
      <HookForm hook={hook} onSaved={(h) => navigate({ search: { hook: h.id } })} />
      {hook && (
        <Region name="deliveries" rows={4}>
          <Deliveries hook={hook} />
        </Region>
      )}
      {hook && <DeleteHook hook={hook} />}
    </div>
  );
}

function HookForm({
  hook,
  onSaved,
}: {
  hook?: Hook;
  onSaved: (h: Schemas["global-hook-2"]) => void;
}) {
  const save = useWrite({
    run: (v: {
      url: string;
      content_type: string;
      secret: string;
      insecure: boolean;
      events: string[];
      active: boolean;
    }) => {
      const config = {
        url: v.url,
        content_type: v.content_type,
        insecure_ssl: v.insecure ? "1" : "0",
        ...(v.secret ? { secret: v.secret } : {}),
      };
      return hook
        ? send(
            api.PATCH("/admin/hooks/{hook_id}", {
              params: { path: { hook_id: hook.id! } },
              body: { config, events: v.events, active: v.active },
            }),
          )
        : send(
            api.POST("/admin/hooks", {
              body: { name: "web", config, events: v.events, active: v.active },
            }),
          );
    },
    invalidate: [HOOKS, ["enterprise"]],
    success: hook ? "Webhook saved" : "Webhook added",
    onSuccess: (h) => {
      form.setFieldValue("secret", "");
      onSaved(h);
    },
    onError: (e) => showFieldErrors(e, form, { "config.url": "url", events: "events" }),
  });
  const form = useForm({
    defaultValues: {
      url: hook?.config?.url ?? "",
      content_type: hook?.config?.content_type ?? "json",
      secret: "",
      insecure: hook?.config?.insecure_ssl === "1",
      events: hook?.events ?? ["user", "organization"],
      active: hook?.active ?? true,
    },
    onSubmit: ({ value }) => save.mutateAsync({ ...value, url: value.url.trim() }).catch(() => {}),
  });
  return (
    <Panel title={hook ? "Webhook" : "Add a webhook"}>
      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field
          name="url"
          validators={{
            onSubmit: ({ value }) =>
              !/^https?:\/\/\S+$/.test(value.trim()) ? "Enter an http or https URL." : undefined,
          }}
        >
          {(field) => (
            <TextField
              field={field}
              label="Payload URL"
              placeholder="https://audit.example.com/hooks"
            />
          )}
        </form.Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <form.Field name="content_type">
            {(field) => (
              <Field>
                <FieldLabel htmlFor="content-type">Content type</FieldLabel>
                <NativeSelect
                  id="content-type"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                >
                  <NativeSelectOption value="json">application/json</NativeSelectOption>
                  <NativeSelectOption value="form">
                    application/x-www-form-urlencoded
                  </NativeSelectOption>
                </NativeSelect>
              </Field>
            )}
          </form.Field>
          <form.Field name="secret">
            {(field) => (
              <TextField
                field={field}
                label="Secret"
                type="password"
                description={
                  hook?.config?.secret
                    ? "A secret is set. Leave empty to keep it."
                    : "Signs each delivery in X-Hub-Signature-256."
                }
              />
            )}
          </form.Field>
        </div>
        <form.Field name="insecure">
          {(field) => (
            <Field orientation="horizontal">
              <Checkbox
                id="insecure"
                checked={field.state.value}
                onCheckedChange={(v) => field.handleChange(!!v)}
              />
              <FieldLabel htmlFor="insecure">Skip SSL verification (not recommended)</FieldLabel>
            </Field>
          )}
        </form.Field>
        <form.Field
          name="events"
          validators={{
            onSubmit: ({ value }) => (value.length === 0 ? "Pick at least one event." : undefined),
          }}
        >
          {(field) => (
            <FieldSet>
              <FieldLegend variant="label">Events</FieldLegend>
              <div className="grid gap-2 sm:grid-cols-2">
                {EVENTS.map(([name, help]) => (
                  <Field key={name} orientation="horizontal" className="items-start">
                    <Checkbox
                      id={`ev-${name}`}
                      checked={field.state.value.includes(name)}
                      onCheckedChange={(v) =>
                        field.handleChange(
                          v
                            ? [...field.state.value, name]
                            : field.state.value.filter((x) => x !== name),
                        )
                      }
                    />
                    <FieldLabel htmlFor={`ev-${name}`} className="flex-col items-start gap-0">
                      <span className="font-mono text-xs">{name}</span>
                      <span className="text-muted-foreground text-xs font-normal">{help}</span>
                    </FieldLabel>
                  </Field>
                ))}
              </div>
              {field.state.meta.errors.length > 0 && (
                <FieldError>{field.state.meta.errors.join(". ")}</FieldError>
              )}
            </FieldSet>
          )}
        </form.Field>
        <form.Field name="active">
          {(field) => (
            <Field orientation="horizontal">
              <Switch
                id="active"
                checked={field.state.value}
                onCheckedChange={(v) => field.handleChange(v)}
              />
              <FieldLabel htmlFor="active">Active: deliver events to this URL</FieldLabel>
            </Field>
          )}
        </form.Field>
        <div className="flex justify-end">
          <Button size="sm" type="submit" disabled={save.isPending}>
            {hook ? "Update webhook" : "Add webhook"}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function Deliveries({ hook }: { hook: Hook }) {
  const { data } = useSuspenseQuery(hookDeliveriesQuery(hook.id!));
  const queryClient = useQueryClient();
  const ping = useWrite({
    run: () =>
      send(api.POST("/admin/hooks/{hook_id}/pings", { params: { path: { hook_id: hook.id! } } })),
    invalidate: [hookDeliveriesQuery(hook.id!).queryKey],
    success: "Ping sent",
    onSuccess: () =>
      void queryClient.invalidateQueries({ queryKey: hookDeliveriesQuery(hook.id!).queryKey }),
  });
  return (
    <Panel
      title={`Recent deliveries (${data.length})`}
      actions={
        <Button
          size="sm"
          variant="outline"
          disabled={ping.isPending}
          onClick={() => ping.mutate(undefined)}
        >
          <HugeiconsIcon icon={SentIcon} strokeWidth={2} data-icon="inline-start" />
          Ping
        </Button>
      }
      flush
    >
      {data.length === 0 ? (
        <p className="text-muted-foreground p-4 text-sm">
          No deliveries yet. Ping the hook, or make a change it subscribes to, such as creating a
          user.
        </p>
      ) : (
        <ul className="divide-y" data-testid="deliveries">
          {data.map((d) => (
            <DeliveryRow key={d.id} d={d} />
          ))}
        </ul>
      )}
    </Panel>
  );
}

function DeliveryRow({ d }: { d: HookDelivery }) {
  const ok = d.status_code < 300;
  return (
    <li>
      <Collapsible>
        <CollapsibleTrigger className="hover:bg-muted/40 flex w-full items-center gap-3 px-4 py-2 text-left text-sm">
          <StateBadge tone={ok ? "success-light" : "destructive-light"}>{d.status_code}</StateBadge>
          <code className="text-muted-foreground truncate text-xs">{d.guid}</code>
          <Badge variant="outline" size="sm" className="font-mono">
            {d.event}
            {d.action ? `.${d.action}` : ""}
          </Badge>
          <span className="text-muted-foreground ml-auto text-xs">
            {when(d.delivered_at)} · {d.duration.toFixed(2)} s
          </span>
        </CollapsibleTrigger>
        <CollapsibleContent className="bg-muted/30 grid gap-3 border-t px-4 py-3 text-xs md:grid-cols-2">
          <div>
            <h4 className="mb-1 font-semibold">Request</h4>
            <pre className="bg-background overflow-auto rounded-sm border p-2">
              {Object.entries(d.request.headers)
                .map(([k, v]) => `${k}: ${v}`)
                .join("\n")}
            </pre>
            <pre className="bg-background mt-2 max-h-64 overflow-auto rounded-sm border p-2">
              {JSON.stringify(d.request.payload, null, 2)}
            </pre>
          </div>
          <div>
            <h4 className="mb-1 font-semibold">
              Response {d.status_code} {d.status}
            </h4>
            <pre className="bg-background overflow-auto rounded-sm border p-2">
              {Object.entries(d.response.headers)
                .map(([k, v]) => `${k}: ${v}`)
                .join("\n")}
            </pre>
            <pre className="bg-background mt-2 overflow-auto rounded-sm border p-2">
              {d.response.body}
            </pre>
          </div>
        </CollapsibleContent>
      </Collapsible>
    </li>
  );
}

function DeleteHook({ hook }: { hook: Hook }) {
  const [open, setOpen] = useState(false);
  const navigate = Route.useNavigate();
  const remove = useWrite({
    run: () =>
      send(api.DELETE("/admin/hooks/{hook_id}", { params: { path: { hook_id: hook.id! } } })),
    invalidate: [HOOKS, ["enterprise"]],
    success: "Webhook deleted",
    onSuccess: () => {
      setOpen(false);
      void navigate({ search: {} });
    },
  });
  return (
    <Panel title="Danger zone" className="border-destructive/40">
      <div className="flex items-center gap-3 text-sm">
        <span className="flex-1">Delete this webhook. Its delivery history goes with it.</span>
        <Button size="sm" variant="destructive" onClick={() => setOpen(true)}>
          <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
          Delete webhook
        </Button>
      </div>
      <AlertDialog open={open} onOpenChange={setOpen}>
        <AlertDialogContent className="rounded-sm">
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this webhook?</AlertDialogTitle>
            <AlertDialogDescription>
              {hook.config?.url} stops receiving events.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel size="sm">Cancel</AlertDialogCancel>
            <Button
              size="sm"
              variant="destructive"
              disabled={remove.isPending}
              onClick={() => remove.mutate(undefined)}
            >
              Delete
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Panel>
  );
}
