import { useState } from "react";
import { useMutation, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { Link, useNavigate } from "@tanstack/react-router";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  CheckmarkCircle02Icon,
  AlertCircleIcon,
  WebhookIcon,
  Refresh01Icon,
} from "@hugeicons/core-free-icons";
import { toast } from "sonner";
import { api, type Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { call, failed, optimistic, refetch } from "./client";
import { HOOK_EVENTS } from "./domain";
import { applyServerErrors, EmptyRow, FieldMessage, TypedConfirm, type RepoParams } from "./kit";
import { deliveriesQuery, deliveryQuery, hookQuery, hooksQuery } from "./queries";

type Hook = Schemas["hook"];

function Status({ hook }: { hook: Hook }) {
  const r = hook.last_response;
  if (!hook.active) return <Badge variant="outline">Inactive</Badge>;
  if (r.code === null) return <Badge variant="outline">Not delivered yet</Badge>;
  return r.code >= 200 && r.code < 300 ? (
    <Badge variant="success-light">Last delivery {r.code}</Badge>
  ) : (
    <Badge variant="destructive-light">Last delivery {r.code}</Badge>
  );
}

export function HookList({ params }: { params: RepoParams }) {
  const { data } = useSuspenseQuery(hooksQuery(params));
  if (data.length === 0) {
    return (
      <EmptyRow>
        No webhooks. A webhook POSTs an event to a URL you choose when something happens here.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="hooks">
      {data.map((h) => (
        <li key={h.id} className="flex items-center gap-3 px-4 py-2.5" data-hook-url={h.config.url}>
          <HugeiconsIcon
            icon={WebhookIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <Link
              to="/$owner/$repo/settings/hooks/$hookId"
              params={{ ...params, hookId: String(h.id) }}
              className="truncate font-mono text-sm font-semibold hover:underline"
            >
              {h.config.url}
            </Link>
            <p className="text-muted-foreground text-xs">
              {h.events.includes("*") ? "Every event" : h.events.join(", ")} ·{" "}
              {h.config.content_type}
            </p>
          </div>
          <Status hook={h} />
          <Button
            variant="outline"
            size="sm"
            render={
              <Link
                to="/$owner/$repo/settings/hooks/$hookId"
                params={{ ...params, hookId: String(h.id) }}
              />
            }
          >
            Edit
          </Button>
        </li>
      ))}
    </ul>
  );
}

// ---- Create and edit ----

type Values = {
  url: string;
  content_type: "json" | "form";
  secret: string;
  insecure: boolean;
  mode: "push" | "all" | "pick";
  events: string[];
  active: boolean;
};

function valuesOf(h: Hook | undefined): Values {
  const events = h?.events ?? ["push"];
  return {
    url: h?.config.url ?? "",
    content_type: h?.config.content_type === "form" ? "form" : "json",
    secret: "",
    insecure: h ? String(h.config.insecure_ssl) === "1" : false,
    mode: events.includes("*")
      ? "all"
      : events.length === 1 && events[0] === "push"
        ? "push"
        : "pick",
    events: events.filter((e) => e !== "*"),
    active: h?.active ?? true,
  };
}

export function HookEditor({ params, id }: { params: RepoParams; id: number | "new" }) {
  const hook = useQuery({
    ...hookQuery({ ...params, id: id === "new" ? 0 : id }),
    enabled: id !== "new",
  });
  if (id !== "new" && hook.isPending)
    return <p className="text-muted-foreground px-4 py-6 text-sm">Loading…</p>;
  if (hook.error) throw hook.error;
  return <HookForm params={params} hook={hook.data} />;
}

function HookForm({ params, hook }: { params: RepoParams; hook?: Hook }) {
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const listKey = hooksQuery(params).queryKey;
  const form = useForm({
    defaultValues: valuesOf(hook),
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const events = value.mode === "all" ? ["*"] : value.mode === "push" ? ["push"] : value.events;
      const config = {
        url: value.url.trim(),
        content_type: value.content_type,
        insecure_ssl: value.insecure ? "1" : "0",
        ...(value.secret ? { secret: value.secret } : {}),
      };
      try {
        const saved = hook
          ? await call(
              api.PATCH("/repos/{owner}/{repo}/hooks/{hook_id}", {
                params: { path: { ...params, hook_id: hook.id } },
                body: { config, events, active: value.active },
              }),
            )
          : await call(
              api.POST("/repos/{owner}/{repo}/hooks", {
                params: { path: params },
                body: { name: "web", config, events, active: value.active },
              }),
            );
        toast.success(hook ? "Webhook updated" : "Webhook created; GHES sent it a ping");
        await refetch(
          listKey,
          hookQuery({ ...params, id: saved.id }).queryKey,
          deliveriesQuery({ ...params, id: saved.id }).queryKey,
        );
        if (!hook)
          void navigate({
            to: "/$owner/$repo/settings/hooks/$hookId",
            params: { ...params, hookId: String(saved.id) },
          });
        else formApi.reset({ ...value, secret: "" });
      } catch (e) {
        setFailure(
          applyServerErrors(formApi, e, {
            url: "url",
            content_type: "content_type",
            events: "events",
          }),
        );
        failed("Webhook not saved")(e);
      }
    },
  });
  const del = useMutation({
    mutationFn: () =>
      call(
        api.DELETE("/repos/{owner}/{repo}/hooks/{hook_id}", {
          params: { path: { ...params, hook_id: hook!.id } },
        }),
      ),
    onMutate: () => optimistic<Hook[]>(listKey, (old) => old.filter((h) => h.id !== hook!.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Webhook not deleted")(e);
    },
    onSuccess: () => {
      toast.success("Webhook deleted");
      void navigate({ to: "/$owner/$repo/settings/hooks", params });
    },
    onSettled: () => refetch(listKey),
  });
  return (
    <form
      className="flex flex-col gap-4 px-4 py-3"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
      data-testid="hook-form"
    >
      <form.Field
        name="url"
        validators={{
          onChange: ({ value }) =>
            !value.trim()
              ? "A payload URL is required."
              : !/^https?:\/\/\S+$/.test(value.trim())
                ? "Use an http:// or https:// URL."
                : undefined,
        }}
      >
        {(field) => (
          <label className="flex max-w-xl flex-col gap-1 text-sm font-medium">
            Payload URL
            <Input
              className="font-mono"
              placeholder="https://example.com/postreceive"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </label>
        )}
      </form.Field>
      <form.Field name="content_type">
        {(field) => (
          <label className="flex max-w-xs flex-col gap-1 text-sm font-medium">
            Content type
            <NativeSelect
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value as Values["content_type"])}
              className="w-full"
            >
              <NativeSelectOption value="json">application/json</NativeSelectOption>
              <NativeSelectOption value="form">
                application/x-www-form-urlencoded
              </NativeSelectOption>
            </NativeSelect>
          </label>
        )}
      </form.Field>
      <form.Field name="secret">
        {(field) => (
          <label className="flex max-w-xl flex-col gap-1 text-sm font-medium">
            Secret
            <Input
              type="password"
              autoComplete="new-password"
              placeholder={hook?.config.secret ? "Unchanged (a secret is set)" : ""}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <span className="text-muted-foreground text-xs font-normal">
              Signs each payload in the X-Hub-Signature-256 header.
            </span>
          </label>
        )}
      </form.Field>
      <form.Field name="insecure">
        {(field) => (
          <fieldset className="flex flex-col gap-1 text-sm">
            <legend className="mb-1 font-medium">SSL verification</legend>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="ssl"
                checked={!field.state.value}
                onChange={() => field.handleChange(false)}
              />{" "}
              Enable SSL verification
            </label>
            <label className="flex items-center gap-2">
              <input
                type="radio"
                name="ssl"
                checked={field.state.value}
                onChange={() => field.handleChange(true)}
              />{" "}
              Disable (not recommended)
            </label>
          </fieldset>
        )}
      </form.Field>
      <form.Field name="mode">
        {(field) => (
          <fieldset className="flex flex-col gap-1 text-sm">
            <legend className="mb-1 font-medium">Which events should trigger this webhook?</legend>
            {(
              [
                ["push", "Just the push event"],
                ["all", "Send me everything"],
                ["pick", "Let me select individual events"],
              ] as const
            ).map(([v, label]) => (
              <label key={v} className="flex items-center gap-2">
                <input
                  type="radio"
                  name="mode"
                  value={v}
                  checked={field.state.value === v}
                  onChange={() => field.handleChange(v)}
                />{" "}
                {label}
              </label>
            ))}
          </fieldset>
        )}
      </form.Field>
      <form.Subscribe selector={(s) => s.values.mode}>
        {(mode) =>
          mode === "pick" && (
            <form.Field
              name="events"
              validators={{
                onSubmit: ({ value, fieldApi }) =>
                  fieldApi.form.getFieldValue("mode") === "pick" && value.length === 0
                    ? "Choose at least one event."
                    : undefined,
              }}
            >
              {(field) => (
                <div className="flex flex-col gap-1">
                  <div
                    className="grid gap-x-4 gap-y-2 rounded-sm border p-3 sm:grid-cols-2"
                    data-testid="hook-events"
                  >
                    {HOOK_EVENTS.map((ev) => (
                      <label key={ev.name} className="flex items-start gap-2 text-sm">
                        <Checkbox
                          className="mt-0.5"
                          checked={field.state.value.includes(ev.name)}
                          data-event={ev.name}
                          onCheckedChange={(on) =>
                            field.handleChange(
                              on
                                ? [...field.state.value, ev.name]
                                : field.state.value.filter((x) => x !== ev.name),
                            )
                          }
                        />
                        <span>
                          <span className="font-mono text-xs font-semibold">{ev.name}</span>
                          <span className="text-muted-foreground block text-xs">
                            {ev.description}
                          </span>
                        </span>
                      </label>
                    ))}
                  </div>
                  <FieldMessage field={field} />
                </div>
              )}
            </form.Field>
          )
        }
      </form.Subscribe>
      <form.Field name="active">
        {(field) => (
          <label className="flex items-start gap-2 text-sm">
            <Checkbox
              className="mt-0.5"
              checked={field.state.value}
              onCheckedChange={(v) => field.handleChange(Boolean(v))}
            />
            <span>
              <span className="font-medium">Active</span>
              <span className="text-muted-foreground block text-xs">
                Deliver event details when this hook is triggered.
              </span>
            </span>
          </label>
        )}
      </form.Field>
      {failure && (
        <p className="text-destructive text-xs" role="alert">
          {failure}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting}>
              {submitting ? "Saving…" : hook ? "Update webhook" : "Add webhook"}
            </Button>
          )}
        </form.Subscribe>
        {hook && (
          <Button
            type="button"
            variant="destructive"
            className="ml-auto"
            onClick={() => setConfirmDelete(true)}
          >
            Delete webhook
          </Button>
        )}
      </div>
      {hook && (
        <TypedConfirm
          open={confirmDelete}
          onOpenChange={setConfirmDelete}
          title="Delete this webhook"
          description="Future events are no longer delivered, and its delivery history is removed."
          phrase="delete"
          action="Delete webhook"
          pending={del.isPending}
          onConfirm={() => del.mutate()}
        />
      )}
    </form>
  );
}

// ---- Deliveries ----

export function Deliveries({ params, hookId }: { params: RepoParams; hookId: number }) {
  // A ping, test, or redelivery lands a new row a moment later, so poll briefly after one.
  const [pollUntil, setPollUntil] = useState(0);
  const { data, isFetching } = useSuspenseQuery({
    ...deliveriesQuery({ ...params, id: hookId }),
    refetchInterval: () => (Date.now() < pollUntil ? 1000 : false),
  });
  const [open, setOpen] = useState<number>();
  const keys = [
    deliveriesQuery({ ...params, id: hookId }).queryKey,
    hookQuery({ ...params, id: hookId }).queryKey,
    hooksQuery(params).queryKey,
  ];
  const after = (what: string) => () => {
    toast.success(what);
    setPollUntil(Date.now() + 4000);
    void refetch(...keys);
  };
  const ping = useMutation({
    mutationFn: () =>
      call(
        api.POST("/repos/{owner}/{repo}/hooks/{hook_id}/pings", {
          params: { path: { ...params, hook_id: hookId } },
        }),
      ),
    onError: failed("Ping not sent"),
    onSuccess: after("Ping sent"),
  });
  const test = useMutation({
    mutationFn: () =>
      call(
        api.POST("/repos/{owner}/{repo}/hooks/{hook_id}/tests", {
          params: { path: { ...params, hook_id: hookId } },
        }),
      ),
    onError: failed("Test push not sent"),
    onSuccess: after("Test push sent if the hook subscribes to push"),
  });
  const redeliver = useMutation({
    mutationFn: (id: number) =>
      call(
        api.POST("/repos/{owner}/{repo}/hooks/{hook_id}/deliveries/{delivery_id}/attempts", {
          params: { path: { ...params, hook_id: hookId, delivery_id: id } },
        }),
      ),
    onError: failed("Not redelivered"),
    onSuccess: after("Redelivery queued"),
  });
  return (
    <div className="flex flex-col" data-testid="deliveries">
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <Button size="sm" variant="outline" onClick={() => ping.mutate()} disabled={ping.isPending}>
          Ping
        </Button>
        <Button size="sm" variant="outline" onClick={() => test.mutate()} disabled={test.isPending}>
          Test push
        </Button>
        {isFetching && <span className="text-muted-foreground text-xs">Refreshing…</span>}
      </div>
      {data.length === 0 ? (
        <EmptyRow>No deliveries yet.</EmptyRow>
      ) : (
        <ul className="divide-y">
          {data.map((d) => {
            const ok = d.status_code >= 200 && d.status_code < 300;
            return (
              <li key={d.id} data-delivery={d.id}>
                <div className="hover:bg-muted/40 flex w-full items-center gap-2 px-4 py-2 text-left text-sm">
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-center gap-2 text-left"
                    onClick={() => setOpen(open === d.id ? undefined : d.id)}
                    aria-expanded={open === d.id}
                  >
                    <HugeiconsIcon
                      icon={ok ? CheckmarkCircle02Icon : AlertCircleIcon}
                      strokeWidth={2}
                      className={ok ? "text-success size-4" : "text-destructive size-4"}
                    />
                    <code className="truncate font-mono text-xs">{d.guid}</code>
                    <Badge variant="outline" className="font-mono">
                      {d.event}
                      {d.action ? `.${d.action}` : ""}
                    </Badge>
                    {d.redelivery && <Badge variant="info-light">redelivery</Badge>}
                    <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                      {d.status_code || "timeout"} · {d.duration}s · <Ago at={d.delivered_at} />
                    </span>
                  </button>
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Redeliver ${d.guid}`}
                    onClick={() => redeliver.mutate(d.id)}
                    disabled={redeliver.isPending}
                  >
                    <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                    Redeliver
                  </Button>
                </div>
                {open === d.id && <DeliveryDetail params={params} hookId={hookId} id={d.id} />}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function DeliveryDetail({
  params,
  hookId,
  id,
}: {
  params: RepoParams;
  hookId: number;
  id: number;
}) {
  const { data, isPending, error } = useQuery(deliveryQuery({ ...params, hook: hookId, id }));
  if (isPending)
    return <p className="text-muted-foreground px-4 py-3 text-xs">Loading the delivery…</p>;
  if (error)
    return (
      <p className="text-destructive px-4 py-3 text-xs" role="alert">
        Could not load the delivery: {error.message}
      </p>
    );
  return (
    <Tabs defaultValue="request" className="bg-muted/30 px-4 py-3" data-testid="delivery-detail">
      <TabsList>
        <TabsTrigger value="request">Request</TabsTrigger>
        <TabsTrigger value="response">Response {data.status_code || ""}</TabsTrigger>
      </TabsList>
      <TabsContent value="request" className="flex flex-col gap-2">
        <Headers headers={data.request.headers} />
        <Pre text={JSON.stringify(data.request.payload, null, 2)} />
      </TabsContent>
      <TabsContent value="response" className="flex flex-col gap-2">
        {data.response.headers ? (
          <Headers headers={data.response.headers} />
        ) : (
          <p className="text-muted-foreground text-xs">
            No response: the receiver did not answer in time.
          </p>
        )}
        {data.response.payload !== null && <Pre text={data.response.payload} />}
      </TabsContent>
    </Tabs>
  );
}

function Headers({ headers }: { headers: Record<string, unknown> | null }) {
  return (
    <dl className="grid grid-cols-[minmax(0,18rem)_minmax(0,1fr)] gap-x-3 gap-y-0.5 font-mono text-xs">
      {Object.entries(headers ?? {}).map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground truncate" title={k}>
            {k}
          </dt>
          <dd className="truncate" title={String(v)}>
            {String(v)}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function Pre({ text }: { text: string }) {
  return (
    <pre className="bg-background max-h-72 overflow-auto rounded-sm border p-2 font-mono text-xs">
      {text}
    </pre>
  );
}
