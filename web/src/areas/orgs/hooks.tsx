import { useState } from "react";
import { Link, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  AlertCircleIcon,
  CheckmarkCircle02Icon,
  Clock01Icon,
  Refresh01Icon,
  WebhookIcon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import { Ago } from "@/components/github";
import { Badge } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { send } from "./api";
import {
  applyServerErrors,
  Confirm,
  EmptyRow,
  failed,
  FieldMessage,
  FormFailure,
  optimistic,
  refetch,
} from "./kit";
import { deliveriesQuery, deliveryQuery, hookQuery, hooksQuery } from "./queries";

type Hook = Schemas["org-hook"];

// The events an organization hook may subscribe to, including the org-only ones.
const EVENTS: { name: string; description: string }[] = [
  { name: "push", description: "Git push to any repository." },
  { name: "pull_request", description: "Pull request opened, closed, or synchronized." },
  { name: "pull_request_review", description: "Review submitted, edited, or dismissed." },
  { name: "issues", description: "Issue opened, edited, closed, or labeled." },
  { name: "issue_comment", description: "Comment created, edited, or deleted." },
  { name: "create", description: "Branch or tag created." },
  { name: "delete", description: "Branch or tag deleted." },
  { name: "release", description: "Release published, edited, or deleted." },
  { name: "check_suite", description: "Check suite requested or completed." },
  { name: "workflow_run", description: "Workflow run requested or completed." },
  { name: "repository", description: "Repository created, renamed, or deleted." },
  { name: "member", description: "Collaborator added or removed." },
  { name: "membership", description: "Team membership changed." },
  { name: "organization", description: "Member invited, added, or removed." },
  { name: "team", description: "Team created, edited, or deleted." },
];

// The latest delivery's outcome, from the hook's own delivery list.
function LastDelivery({ org, hook }: { org: string; hook: Hook }) {
  const { data } = useQuery(deliveriesQuery(org, hook.id));
  if (!hook.active) return <Badge variant="outline">Inactive</Badge>;
  const last = data?.[0];
  if (!data) return null;
  if (!last) return <Badge variant="outline">Not delivered yet</Badge>;
  if (last.status === "Pending") return <Badge variant="info-light">Delivering</Badge>;
  return last.status_code >= 200 && last.status_code < 300 ? (
    <Badge variant="success-light" data-last={last.status_code}>
      Last delivery {last.status_code}
    </Badge>
  ) : (
    <Badge variant="destructive-light" data-last={last.status_code}>
      Last delivery {last.status_code || "failed"}
    </Badge>
  );
}

export function HookList({ org }: { org: string }) {
  const { data } = useSuspenseQuery(hooksQuery(org));
  if (!data.length) {
    return (
      <EmptyRow>
        No webhooks. A webhook POSTs events from every repository in {org} to a URL you choose.
      </EmptyRow>
    );
  }
  return (
    <ul className="divide-y" data-testid="hooks">
      {data.map((h) => (
        <li key={h.id} className="flex items-center gap-3 px-4 py-2.5" data-hook={h.config.url}>
          <HugeiconsIcon
            icon={WebhookIcon}
            strokeWidth={2}
            className="text-muted-foreground size-4"
          />
          <div className="min-w-0 flex-1">
            <Link
              to="/orgs/$org/settings/hooks/$hookId"
              params={{ org, hookId: String(h.id) }}
              className="truncate font-mono text-sm font-semibold hover:underline"
            >
              {h.config.url}
            </Link>
            <p className="text-muted-foreground text-xs">
              {h.events.includes("*") ? "Every event" : h.events.join(", ")} ·{" "}
              {h.config.content_type}
            </p>
          </div>
          <LastDelivery org={org} hook={h} />
          <Button
            variant="outline"
            size="sm"
            render={
              <Link to="/orgs/$org/settings/hooks/$hookId" params={{ org, hookId: String(h.id) }} />
            }
          >
            Edit
          </Button>
        </li>
      ))}
    </ul>
  );
}

type Values = {
  url: string;
  content_type: "json" | "form";
  secret: string;
  ssl: "verify" | "skip";
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
    ssl: h && String(h.config.insecure_ssl) === "1" ? "skip" : "verify",
    mode: events.includes("*")
      ? "all"
      : events.length === 1 && events[0] === "push"
        ? "push"
        : "pick",
    events: events.filter((e) => e !== "*"),
    active: h?.active ?? true,
  };
}

export function HookEditor({ org, id }: { org: string; id: number | "new" }) {
  if (id === "new") return <HookForm org={org} />;
  return <Existing org={org} id={id} />;
}

function Existing({ org, id }: { org: string; id: number }) {
  const { data } = useSuspenseQuery(hookQuery(org, id));
  return <HookForm org={org} hook={data} />;
}

function HookForm({ org, hook }: { org: string; hook?: Hook }) {
  const navigate = useNavigate();
  const [failure, setFailure] = useState<string>();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const listKey = hooksQuery(org).queryKey;
  const form = useForm({
    defaultValues: valuesOf(hook),
    onSubmit: async ({ value, formApi }) => {
      setFailure(undefined);
      const events = value.mode === "all" ? ["*"] : value.mode === "push" ? ["push"] : value.events;
      const config = {
        url: value.url.trim(),
        content_type: value.content_type,
        insecure_ssl: value.ssl === "skip" ? "1" : "0",
        ...(value.secret ? { secret: value.secret } : {}),
      };
      try {
        const saved = hook
          ? await send(
              api.PATCH("/orgs/{org}/hooks/{hook_id}", {
                params: { path: { org, hook_id: hook.id } },
                body: { config, events, active: value.active },
              }),
            )
          : await send(
              api.POST("/orgs/{org}/hooks", {
                params: { path: { org } },
                body: { name: "web", config, events, active: value.active },
              }),
            );
        toast.success(hook ? "Webhook updated" : "Webhook created. GHES sent it a ping.");
        await refetch(listKey, hookQuery(org, saved.id).queryKey, ["orgs", org, "hooks", saved.id]);
        if (!hook) {
          void navigate({
            to: "/orgs/$org/settings/hooks/$hookId",
            params: { org, hookId: String(saved.id) },
          });
        } else formApi.reset({ ...value, secret: "" });
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
      send(
        api.DELETE("/orgs/{org}/hooks/{hook_id}", { params: { path: { org, hook_id: hook!.id } } }),
      ),
    onMutate: () => optimistic<Hook[]>(listKey, (old) => old.filter((h) => h.id !== hook!.id)),
    onError: (e, _v, rollback) => {
      rollback?.();
      failed("Webhook not deleted")(e);
    },
    onSuccess: () => {
      setConfirmDelete(false);
      toast.success("Webhook deleted");
      void navigate({ to: "/orgs/$org/settings/hooks", params: { org } });
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
          <div className="flex max-w-xl flex-col gap-1.5">
            <Label htmlFor="hook-url">Payload URL</Label>
            <Input
              id="hook-url"
              className="font-mono"
              placeholder="https://example.com/postreceive"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              aria-invalid={field.state.meta.errors.length > 0 || undefined}
            />
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Field name="content_type">
        {(field) => (
          <div className="flex max-w-xs flex-col gap-1.5">
            <Label htmlFor="hook-content-type">Content type</Label>
            <NativeSelect
              id="hook-content-type"
              className="w-full"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value as Values["content_type"])}
            >
              <NativeSelectOption value="json">application/json</NativeSelectOption>
              <NativeSelectOption value="form">
                application/x-www-form-urlencoded
              </NativeSelectOption>
            </NativeSelect>
            <FieldMessage field={field} />
          </div>
        )}
      </form.Field>
      <form.Field name="secret">
        {(field) => (
          <div className="flex max-w-xl flex-col gap-1.5">
            <Label htmlFor="hook-secret">Secret</Label>
            <Input
              id="hook-secret"
              type="password"
              autoComplete="new-password"
              placeholder={hook?.config.secret ? "Unchanged (a secret is set)" : ""}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <p className="text-muted-foreground text-xs">
              Signs each payload in the X-Hub-Signature-256 header.
            </p>
          </div>
        )}
      </form.Field>
      <form.Field name="ssl">
        {(field) => (
          <fieldset className="flex flex-col gap-1.5 text-sm">
            <legend className="mb-1 font-medium">SSL verification</legend>
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as Values["ssl"])}
              className="flex gap-4"
            >
              <label className="flex items-center gap-2">
                <RadioGroupItem value="verify" /> Enable SSL verification
              </label>
              <label className="flex items-center gap-2">
                <RadioGroupItem value="skip" /> Disable (not recommended)
              </label>
            </RadioGroup>
          </fieldset>
        )}
      </form.Field>
      <form.Field name="mode">
        {(field) => (
          <fieldset className="flex flex-col gap-1.5 text-sm">
            <legend className="mb-1 font-medium">Which events should trigger this webhook?</legend>
            <RadioGroup
              value={field.state.value}
              onValueChange={(v) => field.handleChange(v as Values["mode"])}
            >
              {(
                [
                  ["push", "Just the push event"],
                  ["all", "Send me everything"],
                  ["pick", "Let me select individual events"],
                ] as const
              ).map(([v, label]) => (
                <label key={v} className="flex items-center gap-2" data-mode={v}>
                  <RadioGroupItem value={v} /> {label}
                </label>
              ))}
            </RadioGroup>
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
                    {EVENTS.map((ev) => (
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
      <FormFailure>{failure}</FormFailure>
      <div className="flex flex-wrap gap-2">
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-testid="save-hook">
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
        <Confirm
          open={confirmDelete}
          onOpenChange={setConfirmDelete}
          title="Delete this webhook?"
          description="Future events stop, and its delivery history is removed."
          action="Delete webhook"
          pending={del.isPending}
          onConfirm={() => del.mutate()}
        />
      )}
    </form>
  );
}

// ---- Deliveries ----

export function Deliveries({ org, hookId }: { org: string; hookId: number }) {
  const [status, setStatus] = useState<"success" | "failure" | undefined>();
  // A ping or redelivery answers a moment later, so poll while any row is pending.
  const { data, isFetching } = useSuspenseQuery({
    ...deliveriesQuery(org, hookId, status),
    refetchInterval: (q) => (q.state.data?.some((d) => d.status === "Pending") ? 1000 : false),
  });
  const [open, setOpen] = useState<number>();
  const after = (what: string) => () => {
    toast.success(what);
    void refetch(["orgs", org, "hooks", hookId, "deliveries"]);
  };
  const ping = useMutation({
    mutationFn: () =>
      send(
        api.POST("/orgs/{org}/hooks/{hook_id}/pings", {
          params: { path: { org, hook_id: hookId } },
        }),
      ),
    onError: failed("Ping not sent"),
    onSuccess: after("Ping sent"),
  });
  const redeliver = useMutation({
    mutationFn: (id: number) =>
      send(
        api.POST("/orgs/{org}/hooks/{hook_id}/deliveries/{delivery_id}/attempts", {
          params: { path: { org, hook_id: hookId, delivery_id: id } },
        }),
      ),
    onError: failed("Not redelivered"),
    onSuccess: after("Redelivery queued"),
  });
  return (
    <div className="flex flex-col" data-testid="deliveries">
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <Button
          size="sm"
          variant="outline"
          onClick={() => ping.mutate()}
          disabled={ping.isPending}
          data-testid="ping"
        >
          Ping
        </Button>
        <NativeSelect
          size="sm"
          aria-label="Delivery status"
          value={status ?? ""}
          onChange={(e) => setStatus((e.target.value || undefined) as typeof status)}
        >
          <NativeSelectOption value="">All deliveries</NativeSelectOption>
          <NativeSelectOption value="success">Succeeded</NativeSelectOption>
          <NativeSelectOption value="failure">Failed</NativeSelectOption>
        </NativeSelect>
        {isFetching && <span className="text-muted-foreground text-xs">Refreshing…</span>}
      </div>
      {data.length === 0 ? (
        <EmptyRow>No deliveries{status ? " with this outcome" : " yet"}.</EmptyRow>
      ) : (
        <ul className="divide-y">
          {data.map((d) => {
            const pending = d.status === "Pending";
            const ok = d.status_code >= 200 && d.status_code < 300;
            return (
              <li key={d.id} data-delivery={d.id} data-status={pending ? "pending" : d.status_code}>
                <div className="hover:bg-muted/40 flex w-full items-center gap-2 px-4 py-2 text-left text-sm">
                  <button
                    type="button"
                    className="flex min-w-0 flex-1 items-center gap-2 text-left"
                    onClick={() => setOpen(open === d.id ? undefined : d.id)}
                    aria-expanded={open === d.id}
                  >
                    <HugeiconsIcon
                      icon={pending ? Clock01Icon : ok ? CheckmarkCircle02Icon : AlertCircleIcon}
                      strokeWidth={2}
                      className={
                        pending
                          ? "text-muted-foreground size-4"
                          : ok
                            ? "text-success size-4"
                            : "text-destructive size-4"
                      }
                    />
                    <code className="truncate font-mono text-xs">{d.guid}</code>
                    <Badge variant="outline" className="font-mono">
                      {d.event}
                      {d.action ? `.${d.action}` : ""}
                    </Badge>
                    {d.redelivery && <Badge variant="info-light">redelivery</Badge>}
                    <span className="text-muted-foreground ml-auto shrink-0 text-xs">
                      {pending ? "pending" : d.status_code} · {d.duration.toFixed(2)}s ·{" "}
                      <Ago at={d.delivered_at} />
                    </span>
                  </button>
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={`Redeliver ${d.guid}`}
                    onClick={() => redeliver.mutate(d.id)}
                    disabled={redeliver.isPending || pending}
                  >
                    <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                    Redeliver
                  </Button>
                </div>
                {open === d.id && <DeliveryDetail org={org} hookId={hookId} id={d.id} />}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function DeliveryDetail({ org, hookId, id }: { org: string; hookId: number; id: number }) {
  const { data, isPending, error } = useQuery(deliveryQuery(org, hookId, id));
  if (isPending)
    return <p className="text-muted-foreground px-4 py-3 text-xs">Loading the delivery…</p>;
  if (error) {
    return (
      <p className="text-destructive px-4 py-3 text-xs" role="alert">
        Could not load the delivery: {error.message}
      </p>
    );
  }
  return (
    <Tabs defaultValue="request" className="bg-muted/30 px-4 py-3" data-testid="delivery-detail">
      <TabsList>
        <TabsTrigger value="request">Request</TabsTrigger>
        <TabsTrigger value="response">Response {data.status_code || ""}</TabsTrigger>
      </TabsList>
      <TabsContent value="request" className="flex flex-col gap-2">
        <HeaderList headers={data.request.headers} />
        <Pre text={JSON.stringify(data.request.payload, null, 2)} />
      </TabsContent>
      <TabsContent value="response" className="flex flex-col gap-2">
        {data.response.headers ? (
          <HeaderList headers={data.response.headers} />
        ) : (
          <p className="text-muted-foreground text-xs">No response yet.</p>
        )}
        {data.response.payload !== null && <Pre text={data.response.payload} />}
      </TabsContent>
    </Tabs>
  );
}

function HeaderList({ headers }: { headers: Record<string, unknown> | null }) {
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
