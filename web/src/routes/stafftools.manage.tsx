import { useEffect, useRef, useState } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  InformationCircleIcon,
  PlayIcon,
  Download04Icon,
  Wrench01Icon,
} from "@hugeicons/core-free-icons";
import { api } from "@/api/client";
import type { components } from "@/api/ghes";
import {
  applyStatusQuery,
  clusterQuery,
  maintenanceQuery,
  nodesQuery,
  replicationQuery,
  settingsQuery,
  upgradeDownloadQuery,
  upgradeStatusQuery,
  versionQuery,
} from "@/areas/admin/queries";
import { message, send, StateBadge, TextField, useWrite, when } from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge, type BadgeProps } from "@/components/reui/badge";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";

export const Route = createFileRoute("/stafftools/manage")({
  loader: ({ context: { queryClient } }) => {
    void queryClient.prefetchQuery(clusterQuery());
    void queryClient.prefetchQuery(replicationQuery());
    void queryClient.prefetchQuery(nodesQuery());
    void queryClient.prefetchQuery(versionQuery());
    void queryClient.prefetchQuery(maintenanceQuery());
    void queryClient.prefetchQuery(settingsQuery());
  },
  component: Page,
});

function Page() {
  return (
    <>
      <PageHeader
        title="Management console"
        description="The appliance operations of GHES's /manage/v1 API."
      />
      <Alert variant="info" className="mb-4" data-testid="mapping-note">
        <HugeiconsIcon icon={InformationCircleIcon} strokeWidth={2} />
        <AlertTitle>How Graftyard serves these</AlertTitle>
        <AlertDescription>
          Graftyard has no appliance. It maps each management console operation to the install's own
          admin API, as the rest:enterprise-admin surface says (design/SHARED.md). Cluster and
          replication status report the install's sites and groups, maintenance mode drains writes,
          a config apply is a signed settings change, and an upgrade is a release rolled through
          site days. In managed mode, vendor staff tooling does the same.
        </AlertDescription>
      </Alert>
      <div className="grid items-start gap-4 lg:grid-cols-2">
        <Region name="cluster status" rows={4}>
          <Cluster />
        </Region>
        <Region name="replication status" rows={4}>
          <Replication />
        </Region>
        <Region name="maintenance mode" rows={4}>
          <Maintenance />
        </Region>
        <Region name="version" rows={4}>
          <Version />
        </Region>
        <div className="lg:col-span-2">
          <Region name="settings" rows={6}>
            <Settings />
          </Region>
        </div>
      </div>
    </>
  );
}

const tone = (s?: string): BadgeProps["variant"] =>
  s === "OK"
    ? "success-light"
    : s === "WARNING"
      ? "warning-light"
      : s === "CRITICAL"
        ? "destructive-light"
        : "outline";

type ServiceNode = {
  hostname?: string;
  status?: string;
  services?: { name?: string; status?: string; details?: string }[];
};

function NodeTable({ nodes }: { nodes: ServiceNode[] }) {
  return (
    <div className="flex flex-col gap-3">
      {nodes.map((n) => (
        <div key={n.hostname}>
          <div className="mb-1 flex items-center gap-2 text-sm font-medium">
            {n.hostname}
            <StateBadge tone={tone(n.status)}>{n.status}</StateBadge>
          </div>
          <ul className="divide-y rounded-sm border text-xs">
            {n.services?.map((s) => (
              <li key={s.name} className="flex items-center gap-2 px-2 py-1">
                <span
                  className={`size-2 shrink-0 rounded-sm ${s.status === "OK" ? "bg-success" : s.status === "WARNING" ? "bg-warning" : "bg-destructive"}`}
                />
                <span className="w-28 font-mono">{s.name}</span>
                <span className="text-muted-foreground truncate">{s.details}</span>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function Cluster() {
  const { data } = useSuspenseQuery({ ...clusterQuery(), refetchInterval: 5000 });
  const { data: topo } = useQuery(nodesQuery());
  return (
    <Panel
      title="Cluster status"
      actions={
        <span className="flex items-center gap-2">
          {topo?.topology && (
            <Badge variant="outline" size="sm">
              {
                { SingleNode: "single node", Ha: "high availability", Cluster: "cluster" }[
                  topo.topology
                ]
              }
            </Badge>
          )}
          <StateBadge tone={tone(data.status)}>
            <span data-testid="cluster-status">{data.status}</span>
          </StateBadge>
        </span>
      }
    >
      <NodeTable nodes={data.nodes ?? []} />
    </Panel>
  );
}

function Replication() {
  const { data } = useSuspenseQuery({ ...replicationQuery(), refetchInterval: 5000 });
  return (
    <Panel
      title="Replication status"
      actions={<StateBadge tone={tone(data.status)}>{data.status}</StateBadge>}
    >
      <NodeTable nodes={data.nodes ?? []} />
    </Panel>
  );
}

const toLocal = (iso?: string) => {
  if (!iso) return "";
  const d = new Date(iso);
  return new Date(d.getTime() - d.getTimezoneOffset() * 60000).toISOString().slice(0, 16);
};

function Maintenance() {
  const { data } = useSuspenseQuery({
    ...maintenanceQuery(),
    // A scheduled window turns on by itself; re-read while one is pending.
    refetchInterval: (q) => (q.state.data?.some((n) => n.status === "scheduled") ? 5000 : false),
  });
  const primary = data[0];
  const status = primary?.status ?? "off";
  const save = useWrite({
    run: (body: components["schemas"]["ghes-set-maintenance-request"]) =>
      send(api.POST("/manage/v1/maintenance", { body })),
    invalidate: [maintenanceQuery().queryKey],
    success: (r) => r[0]?.message ?? "Saved",
  });
  const form = useForm({
    defaultValues: {
      enabled: status !== "off",
      schedule: status === "scheduled",
      when: toLocal(primary?.scheduled_time),
      message: primary?.maintenance_mode_message ?? "",
      ips: (primary?.ip_exception_list ?? []).join("\n"),
    },
    onSubmit: ({ value }) =>
      save
        .mutateAsync({
          enabled: value.enabled,
          when:
            value.enabled && value.schedule && value.when
              ? new Date(value.when).toISOString()
              : "now",
          maintenance_mode_message: value.message.trim(),
          ip_exception_list: value.ips.split(/\s+/).filter(Boolean),
        })
        .catch(() => {}),
  });
  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          <HugeiconsIcon icon={Wrench01Icon} strokeWidth={2} className="size-4" />
          Maintenance mode
        </span>
      }
      actions={
        <StateBadge
          tone={
            status === "on"
              ? "destructive-light"
              : status === "scheduled"
                ? "warning-light"
                : "success-light"
          }
        >
          <span data-testid="maintenance-status">{status}</span>
        </StateBadge>
      }
    >
      {status === "scheduled" && (
        <p className="mb-3 text-sm" data-testid="maintenance-scheduled">
          Starts {when(primary?.scheduled_time)}.
        </p>
      )}
      <p className="text-muted-foreground mb-3 text-xs">
        Open now: {primary?.connection_services?.map((c) => `${c.number} ${c.name}`).join(", ")}
      </p>
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void form.handleSubmit();
        }}
      >
        <form.Field name="enabled">
          {(field) => (
            <Field orientation="horizontal">
              <Switch
                id="mm-enabled"
                checked={field.state.value}
                onCheckedChange={(v) => field.handleChange(v)}
              />
              <FieldLabel htmlFor="mm-enabled">Maintenance mode on</FieldLabel>
            </Field>
          )}
        </form.Field>
        <form.Subscribe selector={(s) => s.values.enabled}>
          {(enabled) =>
            enabled && (
              <>
                <form.Field name="schedule">
                  {(field) => (
                    <Field orientation="horizontal">
                      <Switch
                        id="mm-schedule"
                        checked={field.state.value}
                        onCheckedChange={(v) => field.handleChange(v)}
                      />
                      <FieldLabel htmlFor="mm-schedule">Schedule for later</FieldLabel>
                    </Field>
                  )}
                </form.Field>
                <form.Subscribe selector={(s) => s.values.schedule}>
                  {(schedule) =>
                    schedule && (
                      <form.Field
                        name="when"
                        validators={{
                          onSubmit: ({ value }) =>
                            !value
                              ? "Pick a time."
                              : new Date(value).getTime() <= Date.now()
                                ? "Pick a time in the future."
                                : undefined,
                        }}
                      >
                        {(field) => (
                          <TextField field={field} type="datetime-local" label="Start at" />
                        )}
                      </form.Field>
                    )
                  }
                </form.Subscribe>
              </>
            )
          }
        </form.Subscribe>
        <form.Field name="message">
          {(field) => (
            <TextField field={field} label="Message to users" placeholder="Back by 23:00 UTC." />
          )}
        </form.Field>
        <form.Field
          name="ips"
          validators={{
            onSubmit: ({ value }) =>
              value
                .split(/\s+/)
                .filter(Boolean)
                .some((ip) => !/^[\d.:a-f/]+$/i.test(ip))
                ? "One IP or CIDR per line."
                : undefined,
          }}
        >
          {(field) => (
            <Field>
              <FieldLabel htmlFor="mm-ips">IP exception list</FieldLabel>
              <Textarea
                id="mm-ips"
                className="min-h-16 rounded-sm font-mono text-xs"
                value={field.state.value}
                onChange={(e) => field.handleChange(e.target.value)}
                placeholder={"10.0.0.0/8\n192.0.2.4"}
              />
              <FieldDescription>
                These addresses keep full access during maintenance.
              </FieldDescription>
              {field.state.meta.errors.length > 0 && (
                <FieldError>{field.state.meta.errors.join(". ")}</FieldError>
              )}
            </Field>
          )}
        </form.Field>
        <div>
          <Button size="sm" type="submit" disabled={save.isPending}>
            Save maintenance settings
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function Version() {
  const { data } = useSuspenseQuery(versionQuery());
  const queryClient = useQueryClient();
  const download = useQuery({
    ...upgradeDownloadQuery(),
    refetchInterval: (q) => (q.state.data?.status === "downloading" ? 1000 : false),
  });
  const upgrade = useQuery({
    ...upgradeStatusQuery(),
    refetchInterval: (q) => (q.state.data?.some((n) => n.is_running) ? 1000 : false),
  });
  const running = upgrade.data?.some((n) => n.is_running) ?? false;
  const finished = upgrade.data?.[0]?.status === "completed";
  // Once the upgrade completes, the installed version changed.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && finished)
      void queryClient.invalidateQueries({ queryKey: versionQuery().queryKey });
    wasRunning.current = running;
  }, [running, finished, queryClient]);
  const start = useWrite({
    run: () =>
      send(
        api.POST("/manage/v1/upgrade/download", {
          body: {
            version: "3.22.1",
            force: false,
            max_parallel: 2,
            node_timeout: "30m",
            max_attempts: 3,
          },
        }),
      ),
    invalidate: [upgradeDownloadQuery().queryKey],
    success: "Downloading 3.22.1 to every node",
  });
  const apply = useWrite({
    run: () =>
      send(
        api.POST("/manage/v1/upgrade/apply", { body: { skip_reboot: false, phase: "upgrade" } }),
      ),
    invalidate: [upgradeStatusQuery().queryKey],
    success: "Upgrade started",
  });
  const d = download.data;
  const installed = data[0]?.version?.version;
  return (
    <Panel title="Version and upgrade">
      <ul className="divide-y rounded-sm border text-sm">
        {data.map((n) => (
          <li key={n.hostname} className="flex items-center gap-2 px-3 py-1.5">
            <span className="flex-1 font-medium">{n.hostname}</span>
            <code className="text-xs" data-version>
              {n.version?.version}
            </code>
            <span className="text-muted-foreground text-xs">
              {n.version?.platform} · build {n.version?.build_id} · {n.version?.build_date}
            </span>
          </li>
        ))}
      </ul>
      <div className="mt-3 flex flex-col gap-2 text-sm">
        {installed === "3.22.1" ? (
          <p className="text-success-foreground dark:text-success" data-testid="upgrade-done">
            Up to date on 3.22.1.
          </p>
        ) : (
          <>
            <p>
              <Badge variant="info-light" size="sm">
                3.22.1
              </Badge>{" "}
              is available: security fixes for the management console and Git.
            </p>
            {d && d.status !== "not_started" && (
              <div data-testid="upgrade-download">
                <div className="flex justify-between text-xs">
                  <span>Download {d.package_name}</span>
                  <span>
                    {d.orchestration?.completed} of {d.orchestration?.total} nodes · {d.status}
                  </span>
                </div>
                <div className="mt-1 flex flex-col gap-1">
                  {d.nodes?.map((n) => (
                    <div key={n.hostname} className="flex items-center gap-2 text-xs">
                      <span className="text-muted-foreground w-32 truncate">{n.hostname}</span>
                      <Progress value={parseInt(n.progress ?? "0")} className="flex-1" />
                      <span className="w-10 text-right tabular-nums">{n.progress}</span>
                    </div>
                  ))}
                </div>
              </div>
            )}
            {upgrade.data?.[0]?.steps?.length ? (
              <ol className="flex flex-col gap-1 text-xs" data-testid="upgrade-steps">
                {upgrade.data[0].steps.map((s) => (
                  <li key={s.phase} className="flex items-center gap-2">
                    {s.status === "in_progress" ? (
                      <Spinner className="size-3" />
                    ) : (
                      <span
                        className={`size-2 rounded-sm ${s.status === "completed" ? "bg-success" : "bg-muted-foreground/40"}`}
                      />
                    )}
                    <span>{s.step}</span>
                    <span className="text-muted-foreground ml-auto">{s.status}</span>
                  </li>
                ))}
              </ol>
            ) : null}
            <div className="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                disabled={start.isPending || d?.status === "downloading" || running}
                onClick={() => start.mutate(undefined)}
              >
                <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
                Download 3.22.1
              </Button>
              <Button
                size="sm"
                disabled={apply.isPending || d?.status !== "completed" || running}
                onClick={() => apply.mutate(undefined)}
              >
                <HugeiconsIcon icon={PlayIcon} strokeWidth={2} data-icon="inline-start" />
                Apply upgrade
              </Button>
            </div>
          </>
        )}
      </div>
    </Panel>
  );
}

type ApplyEvent = {
  timestamp?: string;
  severity_text?: string;
  body?: string;
  event_name?: string;
  hostname?: string;
};

// Saves settings, then optionally runs a config apply and streams its events: each
// poll sends the last request id it saw per node and appends what came back.
function Settings() {
  const { data } = useSuspenseQuery(settingsQuery());
  const [runId, setRunId] = useState<string>();
  const [events, setEvents] = useState<ApplyEvent[]>([]);
  const [dirty, setDirty] = useState(false);
  const last = useRef<string | undefined>(undefined);
  const status = useQuery({
    ...applyStatusQuery(runId ?? ""),
    enabled: !!runId,
    refetchInterval: (q) => (q.state.data?.running === false ? false : 1000),
  });
  const running = !!runId && status.data?.running !== false;

  useEffect(() => {
    if (!runId) return;
    let stop = false;
    const poll = async () => {
      while (!stop) {
        const { data: ev } = await api.GET("/manage/v1/config/apply/events", {
          params: { query: { last_request_id: last.current } },
        });
        const fresh = (ev?.nodes ?? []).flatMap((n) => n.events ?? []);
        const ids = (ev?.nodes ?? []).map((n) => n.last_request_id).filter((x): x is string => !!x);
        // Request ids are "<run>:<index>", so the highest index is the newest.
        if (ids.length)
          last.current = ids
            .sort((a, b) => Number(a.split(":")[1]) - Number(b.split(":")[1]))
            .at(-1);
        if (fresh.length)
          setEvents((p) =>
            [...p, ...fresh].sort((a, b) => (a.timestamp ?? "").localeCompare(b.timestamp ?? "")),
          );
        await new Promise((r) => setTimeout(r, 700));
      }
    };
    void poll();
    return () => {
      stop = true;
    };
  }, [runId]);

  const save = useWrite({
    run: (body: Record<string, unknown>) =>
      send(api.PUT("/manage/v1/config/settings", { body: body as Record<string, never> })),
    invalidate: [settingsQuery().queryKey],
    success: "Settings saved. Run a config apply to put them into effect.",
    onSuccess: () => setDirty(true),
  });
  const apply = useWrite({
    run: () => send(api.POST("/manage/v1/config/apply", { body: {} })),
    invalidate: [clusterQuery().queryKey],
    success: (r) => `Config apply ${r.run_id} started`,
    onSuccess: (r) => {
      last.current = undefined;
      setEvents([]);
      setDirty(false);
      setRunId(r.run_id);
    },
  });
  const form = useForm({
    defaultValues: {
      github_hostname: data.github_hostname ?? "",
      timezone: data.timezone ?? "UTC",
      private_mode: !!data.private_mode,
      public_pages: !!data.public_pages,
      subdomain_isolation: !!data.subdomain_isolation,
      signup_enabled: !!data.signup_enabled,
    },
    onSubmit: ({ value }) =>
      save.mutateAsync({ ...value, github_hostname: value.github_hostname.trim() }).catch(() => {}),
  });
  const toggles = [
    ["private_mode", "Private mode", "Require sign-in to see anything."],
    ["public_pages", "Public Pages", "Pages sites are visible without sign-in."],
    ["subdomain_isolation", "Subdomain isolation", "Serve user content from separate subdomains."],
    ["signup_enabled", "Sign-up", "Anyone may create an account."],
  ] as const;
  return (
    <Panel
      title="Settings and config apply"
      actions={
        <span className="text-muted-foreground text-xs">
          Configuration {data.configuration_id}, {data.configuration_run_count} runs · auth{" "}
          {data.auth_mode}
        </span>
      }
    >
      <div className="grid gap-4 lg:grid-cols-2">
        <form
          className="flex flex-col gap-3"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="github_hostname"
            validators={{
              onSubmit: ({ value }) =>
                !/^[a-z\d.-]+\.[a-z]{2,}$/i.test(value.trim()) ? "Enter a hostname." : undefined,
            }}
          >
            {(field) => <TextField field={field} label="Hostname" />}
          </form.Field>
          <form.Field name="timezone">
            {(field) => <TextField field={field} label="Time zone" />}
          </form.Field>
          {toggles.map(([name, label, help]) => (
            <form.Field key={name} name={name}>
              {(field) => (
                <Field orientation="horizontal" className="items-start">
                  <Switch
                    id={name}
                    checked={field.state.value}
                    onCheckedChange={(v) => field.handleChange(v)}
                  />
                  <div>
                    <FieldLabel htmlFor={name}>{label}</FieldLabel>
                    <FieldDescription className="text-xs">{help}</FieldDescription>
                  </div>
                </Field>
              )}
            </form.Field>
          ))}
          {save.error && <p className="text-destructive text-xs">{message(save.error)}</p>}
          <div className="flex gap-2">
            <Button size="sm" variant="outline" type="submit" disabled={save.isPending || running}>
              Save settings
            </Button>
            <Button
              size="sm"
              disabled={apply.isPending || running}
              onClick={() => apply.mutate(undefined)}
              data-testid="config-apply"
            >
              {running ? (
                <Spinner className="size-3" />
              ) : (
                <HugeiconsIcon icon={PlayIcon} strokeWidth={2} data-icon="inline-start" />
              )}
              {running ? "Applying" : "Run config apply"}
            </Button>
          </div>
          {dirty && !running && (
            <p className="text-warning-foreground dark:text-warning text-xs">
              Saved changes wait for a config apply.
            </p>
          )}
        </form>
        <div>
          <div className="mb-1 flex items-center gap-2 text-xs font-semibold">
            Config apply events
            {runId && (
              <StateBadge
                tone={
                  running
                    ? "warning-light"
                    : status.data?.successful
                      ? "success-light"
                      : "destructive-light"
                }
              >
                <span data-testid="apply-state">
                  {running ? "running" : status.data?.successful ? "successful" : "failed"}
                </span>
              </StateBadge>
            )}
          </div>
          <ol
            className="bg-muted/30 h-64 overflow-auto rounded-sm border p-2 font-mono text-xs"
            data-testid="apply-events"
            aria-live="polite"
          >
            {events.length === 0 && (
              <li className="text-muted-foreground">
                {runId
                  ? "Waiting for the first event"
                  : "No config apply has run since this page opened."}
              </li>
            )}
            {events.map((e, i) => (
              <li key={i} className="flex gap-2">
                <span className="text-muted-foreground">
                  {e.timestamp
                    ? new Date(e.timestamp).toLocaleTimeString("en-GB", { timeZone: "UTC" })
                    : ""}
                </span>
                <span className="w-28 truncate">{e.hostname}</span>
                <span
                  className={
                    e.severity_text === "WARN" ? "text-warning-foreground dark:text-warning" : ""
                  }
                >
                  [{e.event_name}] {e.body}
                </span>
              </li>
            ))}
          </ol>
        </div>
      </div>
    </Panel>
  );
}
