import { useEffect, useState } from "react";
import { useForm } from "@tanstack/react-form";
import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  Delete02Icon,
  LockIcon,
  Refresh01Icon,
  Robot01Icon,
  SquareUnlock01Icon,
  UserIcon,
} from "@hugeicons/core-free-icons";
import { userQuery } from "@/areas/core/queries";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
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
import { Input } from "@/components/ui/input";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "cn";
import { call, GraftyardError, gy, type Lease } from "./api";
import { globProblem, literalFirstSegment, MAX_GLOBS, parseGlobs } from "./globs";
import { accountQuery, agentAppsQuery, leasesKey, leasesQuery, tasksQuery } from "./queries";
import { FINAL_STATES, shortId } from "./states";
import { TrustInline } from "./trust-view";

type Repo = { owner: string; repo: string };

// A clock that ticks once a second, for countdowns.
function useNow() {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, []);
  return now;
}

function countdown(ms: number) {
  if (ms <= 0) return "expired";
  const s = Math.ceil(ms / 1000);
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  if (h) return `${h}h ${String(m % 60).padStart(2, "0")}m`;
  return `${m}:${String(s % 60).padStart(2, "0")}`;
}

function useLeaseWrites(r: Repo) {
  const client = useQueryClient();
  const key = leasesKey(r);
  const snapshot = async () => {
    await client.cancelQueries({ queryKey: key });
    return client.getQueryData<Lease[]>(key);
  };
  const restore = (before: Lease[] | undefined) => before && client.setQueryData(key, before);
  const settle = () => client.invalidateQueries({ queryKey: key });

  const renew = useMutation({
    mutationFn: ({ id, ttl }: { id: string; ttl: number }) =>
      call(
        gy.POST("/repos/{owner}/{repo}/leases/{id}/renew", {
          params: { path: { ...r, id } },
          body: { ttl_seconds: ttl },
        }),
      ),
    onMutate: async ({ id, ttl }) => {
      const before = await snapshot();
      const expires_at = new Date(Date.now() + ttl * 1000).toISOString();
      client.setQueryData<Lease[]>(key, (old) =>
        old?.map((l) => (l.id === id ? { ...l, expires_at } : l)),
      );
      return { before };
    },
    onError: (e, _v, ctx) => {
      restore(ctx?.before);
      toast.error(
        e instanceof GraftyardError && e.status === 404 ? "The lease is gone" : "Lease not renewed",
        {
          description: e.message,
        },
      );
    },
    onSuccess: (l) =>
      toast.success("Lease renewed", {
        description: `Expires at ${new Date(l.expires_at).toLocaleTimeString()}.`,
      }),
    onSettled: settle,
  });

  const release = useMutation({
    mutationFn: (id: string) =>
      call(gy.DELETE("/repos/{owner}/{repo}/leases/{id}", { params: { path: { ...r, id } } })),
    onMutate: async (id) => {
      const before = await snapshot();
      client.setQueryData<Lease[]>(key, (old) => old?.filter((l) => l.id !== id));
      return { before };
    },
    onError: (e, _v, ctx) => {
      restore(ctx?.before);
      toast.error("Lease not released", { description: e.message });
    },
    onSuccess: () => toast.success("Lease released"),
    onSettled: settle,
  });
  return { renew, release };
}

export function LeasesPanel(r: Repo) {
  const { data } = useSuspenseQuery(leasesQuery(r));
  const { data: viewer } = useSuspenseQuery(userQuery());
  const { data: apps = [] } = useQuery(agentAppsQuery());
  const now = useNow();
  const { renew, release } = useLeaseWrites(r);
  const client = useQueryClient();
  const live = data.filter((l) => Date.parse(l.expires_at) > now);

  // An expired lease drops off at once; the next read confirms it.
  const expired = data.length - live.length;
  useEffect(() => {
    if (expired) void client.invalidateQueries({ queryKey: leasesKey(r) });
  }, [expired]);

  if (!live.length) {
    return (
      <p className="text-muted-foreground px-4 py-8 text-center text-sm" data-leases-empty>
        No live leases. A lease says who is working on which paths.
      </p>
    );
  }
  return (
    <ul className="divide-y" data-testid="lease-list">
      {live.map((l) => {
        const left = Date.parse(l.expires_at) - now;
        const mine = l.holder.actor.type === "User" && l.holder.actor.id === viewer.id;
        const app = apps.find((a) => a.app_id === l.holder.app_id);
        return (
          <li key={l.id} className="flex flex-col gap-1.5 px-4 py-2.5" data-lease={l.id}>
            <div className="flex flex-wrap items-center gap-2">
              <Badge
                variant={l.mode === "exclusive" ? "destructive-light" : "info-light"}
                size="sm"
              >
                <HugeiconsIcon
                  icon={l.mode === "exclusive" ? LockIcon : SquareUnlock01Icon}
                  strokeWidth={2}
                />
                {l.mode === "exclusive" ? "Exclusive" : "Shared"}
              </Badge>
              {l.plan_block && (
                <Badge variant="outline" size="sm">
                  Plan block
                </Badge>
              )}
              <span className="flex items-center gap-1 text-xs font-medium">
                <HugeiconsIcon
                  icon={l.holder.actor.type === "App" ? Robot01Icon : UserIcon}
                  strokeWidth={2}
                  className="text-muted-foreground size-3.5"
                />
                <HolderName lease={l} viewer={viewer} appSlug={app?.slug} />
              </span>
              {l.holder.session && (
                <span className="text-muted-foreground font-mono text-xs" title={l.holder.session}>
                  task {shortId(l.holder.session)}
                </span>
              )}
              <span
                className={cn(
                  "ml-auto font-mono text-xs tabular-nums",
                  left < 120_000 ? "text-destructive font-semibold" : "text-muted-foreground",
                )}
                title={`Expires ${new Date(l.expires_at).toLocaleString()}`}
                data-countdown={Math.ceil(left / 1000)}
              >
                {countdown(left)}
              </span>
              {mine && (
                <span className="flex gap-1">
                  <Button
                    size="xs"
                    variant="outline"
                    disabled={renew.isPending}
                    onClick={() => renew.mutate({ id: l.id, ttl: 600 })}
                    data-renew={l.id}
                  >
                    <HugeiconsIcon icon={Refresh01Icon} strokeWidth={2} data-icon="inline-start" />
                    Renew
                  </Button>
                  <Button
                    size="xs"
                    variant="outline"
                    disabled={release.isPending}
                    onClick={() => release.mutate(l.id)}
                    data-release={l.id}
                  >
                    <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
                    Release
                  </Button>
                </span>
              )}
            </div>
            <div className="flex flex-wrap gap-1">
              {[...l.paths, ...l.refs].map((p) => (
                <code key={p} className="bg-muted rounded-sm px-1 py-0.5 font-mono text-xs">
                  {p}
                </code>
              ))}
            </div>
            {l.intent.visible || l.intent.hidden.html_comments ? (
              <p className="text-muted-foreground text-xs">
                <TrustInline view={l.intent} />
              </p>
            ) : null}
          </li>
        );
      })}
    </ul>
  );
}

// A lease names its holder by id. Apps resolve from the agent Apps list, users by
// GET /user/{account_id}.
function HolderName({
  lease,
  viewer,
  appSlug,
}: {
  lease: Lease;
  viewer: { id: number; login: string };
  appSlug?: string;
}) {
  const { actor } = lease.holder;
  const user = useQuery({
    ...accountQuery(actor.id),
    enabled: actor.type === "User" && actor.id !== viewer.id,
  });
  if (actor.type === "App") return <>{appSlug ?? `App ${lease.holder.app_id}`}</>;
  if (actor.id === viewer.id) return <>{viewer.login} (you)</>;
  return <>{user.data?.login ?? `user ${actor.id}`}</>;
}

type AcquireValues = {
  paths: string;
  mode: "shared" | "exclusive";
  ttl: number;
  intent: string;
  session: string;
};

export function AcquireLeaseButton(r: Repo) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="xs" variant="outline" onClick={() => setOpen(true)} data-acquire>
        <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
        Acquire
      </Button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="sm:max-w-lg">
          {open && <AcquireForm {...r} onDone={() => setOpen(false)} />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function AcquireForm({ onDone, ...r }: Repo & { onDone: () => void }) {
  const client = useQueryClient();
  const { data: tasks } = useSuspenseQuery(tasksQuery(r));
  const live = tasks.filter((t) => !FINAL_STATES.has(t.state));
  const [conflicts, setConflicts] = useState<Lease[]>();
  const [overlaps, setOverlaps] = useState<Lease[]>();
  const [refusal, setRefusal] = useState<string>();

  const acquire = useMutation({
    mutationFn: (v: AcquireValues) =>
      call(
        gy.POST("/repos/{owner}/{repo}/leases", {
          params: {
            path: r,
            header: v.session ? { "X-Graftyard-Agent-Session": v.session } : undefined,
          },
          body: {
            paths: parseGlobs(v.paths),
            mode: v.mode,
            ttl_seconds: v.ttl,
            intent: v.intent || undefined,
            session: v.session || undefined,
          },
        }),
      ),
    onSettled: () => client.invalidateQueries({ queryKey: leasesKey(r) }),
  });

  const form = useForm({
    defaultValues: {
      paths: "",
      mode: "shared",
      ttl: 600,
      intent: "",
      session: "",
    } as AcquireValues,
    onSubmit: async ({ value, formApi }) => {
      setConflicts(undefined);
      setRefusal(undefined);
      try {
        const res = await acquire.mutateAsync(value);
        if (res.overlaps.length) {
          setOverlaps(res.overlaps);
          toast.success("Lease acquired", {
            description: `It overlaps ${res.overlaps.length} other lease(s).`,
          });
        } else {
          toast.success("Lease acquired");
          onDone();
        }
      } catch (e) {
        if (!(e instanceof GraftyardError)) throw e;
        if (e.status === 409) return setConflicts(e.body.conflicts ?? []);
        const fields = e.fieldErrors();
        if (e.status === 422 && Object.keys(fields).length) {
          for (const [f, msg] of Object.entries(fields)) {
            const name = (f === "ttl_seconds" ? "ttl" : f) as keyof AcquireValues;
            if (name in value)
              formApi.setFieldMeta(name, (m) => ({
                ...m,
                errorMap: { onServer: `Server: ${msg}` },
              }));
          }
          return;
        }
        if (e.status === 422) return setRefusal(e.message);
        toast.error("Lease not acquired", { description: e.message });
      }
    },
  });

  if (overlaps) {
    return (
      <div className="flex flex-col gap-4">
        <DialogHeader>
          <DialogTitle>Lease acquired</DialogTitle>
          <DialogDescription>
            Shared leases never block. These live leases overlap yours:
          </DialogDescription>
        </DialogHeader>
        <LeaseRows leases={overlaps} />
        <DialogFooter>
          <Button onClick={onDone}>Done</Button>
        </DialogFooter>
      </div>
    );
  }

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        void form.handleSubmit();
      }}
    >
      <DialogHeader>
        <DialogTitle>Acquire a lease</DialogTitle>
        <DialogDescription>
          Tell agents and people which paths you are working on. Exclusive leases need a live task
          and a literal first path segment.
        </DialogDescription>
      </DialogHeader>
      <form.Field
        name="paths"
        validators={{
          onChange: ({ value, fieldApi }) => {
            const globs = parseGlobs(value);
            if (!globs.length) return undefined;
            if (globs.length > MAX_GLOBS) return `At most ${MAX_GLOBS} globs.`;
            const bad = globs.map(globProblem).find(Boolean);
            if (bad) return bad;
            if (fieldApi.form.getFieldValue("mode") === "exclusive") {
              const loose = globs.find((g) => !literalFirstSegment(g));
              if (loose) return `${loose}: an Exclusive lease needs a literal first segment.`;
            }
            return undefined;
          },
          onChangeListenTo: ["mode"],
          onSubmit: ({ value }) =>
            parseGlobs(value).length ? undefined : "Name at least one path glob.",
        }}
      >
        {(field) => (
          <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
            <FieldLabel htmlFor="lease-paths">Paths</FieldLabel>
            <Textarea
              id="lease-paths"
              className="min-h-20 font-mono text-xs"
              placeholder={"crates/api/src/rest/**\ndocs/api.md"}
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
              onBlur={field.handleBlur}
            />
            <FieldDescription>
              One glob per line or comma separated, 1 to {MAX_GLOBS}.{" "}
              {parseGlobs(field.state.value).length} given.
            </FieldDescription>
            <FieldError
              errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
            />
          </Field>
        )}
      </form.Field>
      <div className="grid gap-4 sm:grid-cols-2">
        <form.Field name="mode">
          {(field) => (
            <Field>
              <FieldLabel htmlFor="lease-mode">Mode</FieldLabel>
              <NativeSelect
                id="lease-mode"
                className="w-full"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value as AcquireValues["mode"])}
              >
                <NativeSelectOption value="shared">Shared</NativeSelectOption>
                <NativeSelectOption value="exclusive">Exclusive</NativeSelectOption>
              </NativeSelect>
            </Field>
          )}
        </form.Field>
        <form.Field
          name="ttl"
          validators={{
            onChange: ({ value }) =>
              Number.isInteger(value) && value >= 60 && value <= 3600
                ? undefined
                : "60 to 3,600 seconds.",
          }}
        >
          {(field) => (
            <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
              <FieldLabel htmlFor="lease-ttl">Time to live (seconds)</FieldLabel>
              <Input
                id="lease-ttl"
                type="number"
                min={60}
                max={3600}
                step={60}
                value={Number.isNaN(field.state.value) ? "" : field.state.value}
                onChange={(e) => field.handleChange(e.target.valueAsNumber)}
              />
              <FieldError
                errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
              />
            </Field>
          )}
        </form.Field>
      </div>
      <form.Subscribe selector={(s) => s.values.mode}>
        {(mode) => (
          <form.Field
            name="session"
            validators={{
              onChangeListenTo: ["mode"],
              onChange: ({ value, fieldApi }) =>
                fieldApi.form.getFieldValue("mode") === "exclusive" && !value
                  ? "An Exclusive lease names the live task it works for."
                  : undefined,
              onSubmit: ({ value }) =>
                mode === "exclusive" && !value
                  ? "An Exclusive lease names the live task it works for."
                  : undefined,
            }}
          >
            {(field) => (
              <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
                <FieldLabel htmlFor="lease-session">
                  Task {mode === "shared" && "(optional)"}
                </FieldLabel>
                <NativeSelect
                  id="lease-session"
                  className="w-full"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                >
                  <NativeSelectOption value="">No task</NativeSelectOption>
                  {live.map((t) => (
                    <NativeSelectOption key={t.id} value={t.id}>
                      {t.id.slice(0, 18)} {t.issue_number ? `#${t.issue_number}` : ""} {t.state}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
                <FieldDescription>Sent as X-Graftyard-Agent-Session.</FieldDescription>
                <FieldError
                  errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
                />
              </Field>
            )}
          </form.Field>
        )}
      </form.Subscribe>
      <form.Field
        name="intent"
        validators={{
          onChange: ({ value }) =>
            new TextEncoder().encode(value).length > 280 ? "At most 280 bytes." : undefined,
        }}
      >
        {(field) => (
          <Field data-invalid={field.state.meta.errors.length > 0 || undefined}>
            <FieldLabel htmlFor="lease-intent">Intent</FieldLabel>
            <Input
              id="lease-intent"
              placeholder="What you are doing there"
              value={field.state.value}
              onChange={(e) => field.handleChange(e.target.value)}
            />
            <FieldDescription className="tabular-nums">
              {new TextEncoder().encode(field.state.value).length} / 280 bytes. Others see it
              through the trust view.
            </FieldDescription>
            <FieldError
              errors={field.state.meta.errors.map((message) => ({ message: String(message) }))}
            />
          </Field>
        )}
      </form.Field>

      {conflicts && (
        <Alert variant="destructive" data-lease-conflicts={conflicts.length}>
          <AlertTitle className="line-clamp-none">
            409: Exclusive leases already cover these paths
          </AlertTitle>
          <AlertDescription>
            <p>Nothing was written. Wait for these to expire, or take a Shared lease instead.</p>
            <LeaseRows leases={conflicts} />
          </AlertDescription>
        </Alert>
      )}
      {refusal && (
        <Alert variant="destructive">
          <AlertTitle>Lease refused</AlertTitle>
          <AlertDescription>{refusal}</AlertDescription>
        </Alert>
      )}

      <DialogFooter>
        <Button type="button" variant="outline" onClick={onDone}>
          Close
        </Button>
        <form.Subscribe selector={(s) => s.isSubmitting}>
          {(submitting) => (
            <Button type="submit" disabled={submitting} data-submit-lease>
              {submitting ? "Acquiring…" : "Acquire lease"}
            </Button>
          )}
        </form.Subscribe>
      </DialogFooter>
    </form>
  );
}

function LeaseRows({ leases }: { leases: Lease[] }) {
  return (
    <ul className="mt-1 flex flex-col gap-1.5">
      {leases.map((l) => (
        <li
          key={l.id}
          className="text-foreground flex flex-wrap items-center gap-1 text-xs"
          data-conflict={l.id}
        >
          <Badge variant={l.mode === "exclusive" ? "destructive-light" : "info-light"} size="xs">
            {l.mode}
          </Badge>
          {l.paths.map((p) => (
            <code key={p} className="bg-muted rounded-sm px-1 font-mono">
              {p}
            </code>
          ))}
          <span className="text-muted-foreground">
            {l.holder.actor.type === "App" ? `App ${l.holder.app_id}` : `user ${l.holder.actor.id}`}
            , until {new Date(l.expires_at).toLocaleTimeString()}
          </span>
        </li>
      ))}
    </ul>
  );
}
