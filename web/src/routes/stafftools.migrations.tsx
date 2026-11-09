import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { useForm } from "@tanstack/react-form";
import { toast } from "sonner";
import { HugeiconsIcon } from "@hugeicons/react";
import {
  Add01Icon,
  ArrowTurnBackwardIcon,
  CancelCircleIcon,
  CheckmarkCircle02Icon,
  Delete02Icon,
  Download04Icon,
  LockKeyIcon,
  PauseIcon,
  PlayIcon,
  SquareUnlock01Icon,
} from "@hugeicons/core-free-icons";
import { api, type Schemas } from "@/api/client";
import {
  liveMigrationQuery,
  liveMigrationsQuery,
  orgMigrationsQuery,
  staffOrgsQuery,
  staffReposQuery,
  userMigrationsQuery,
} from "@/areas/admin/queries";
import {
  message,
  send,
  showFieldErrors,
  StateBadge,
  TextField,
  useWrite,
  when,
} from "@/areas/admin/ui";
import { PageHeader, Panel } from "@/components/panel";
import { Region } from "@/components/region";
import { Alert, AlertDescription, AlertTitle } from "@/components/reui/alert";
import { Badge } from "@/components/reui/badge";
import {
  Stepper,
  StepperDescription,
  StepperIndicator,
  StepperItem,
  StepperNav,
  StepperSeparator,
  StepperTitle,
  StepperTrigger,
} from "@/components/reui/stepper";
import {
  Timeline,
  TimelineContent,
  TimelineDate,
  TimelineHeader,
  TimelineIndicator,
  TimelineItem,
  TimelineSeparator,
  TimelineTitle,
} from "@/components/reui/timeline";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";

const TABS = ["exports", "live"] as const;
type Tab = (typeof TABS)[number];

export const Route = createFileRoute("/stafftools/migrations")({
  validateSearch: (s: Record<string, unknown>): { tab?: Tab; org?: string; live?: string } => ({
    tab: s.tab === "live" ? "live" : undefined,
    org: typeof s.org === "string" && s.org ? s.org : undefined,
    live: typeof s.live === "string" && s.live ? s.live : undefined,
  }),
  loader: ({ context: { queryClient }, location }) => {
    const q = new URLSearchParams(location.searchStr);
    void queryClient.prefetchQuery(staffOrgsQuery());
    if (q.get("tab") === "live") void queryClient.prefetchQuery(liveMigrationsQuery());
    else void queryClient.prefetchQuery(exportsQuery(q.get("org") ?? "graftyard"));
  },
  component: Page,
});

function Page() {
  const { tab = "exports" } = Route.useSearch();
  return (
    <>
      <PageHeader
        title="Migrations"
        description="Export an organization's repositories to an archive, or move repositories live to another GitHub with a cutover."
      />
      <div className="mb-3 flex gap-1 border-b text-sm" role="tablist">
        {TABS.map((t) => (
          <Link
            key={t}
            from={Route.fullPath}
            search={{ tab: t === "exports" ? undefined : t }}
            role="tab"
            aria-selected={tab === t}
            className="text-muted-foreground aria-selected:border-primary aria-selected:text-foreground -mb-px border-b-2 border-transparent px-3 py-1.5 font-medium"
          >
            {t === "exports" ? "Organization exports" : "Live migrations"}
          </Link>
        ))}
      </div>
      {tab === "exports" ? (
        <Region name="organizations" rows={4}>
          <Exports />
        </Region>
      ) : (
        <Region name="live migrations" rows={4}>
          <LiveMigrations />
        </Region>
      )}
    </>
  );
}

// ---- Organization exports ----

type Migration = Schemas["migration"];
type ExportState = "pending" | "exporting" | "exported" | "failed";
const EXPORT_STEPS: ExportState[] = ["pending", "exporting", "exported"];
const isActive = (m: Migration) => m.state === "pending" || m.state === "exporting";

// An export's source is an organization or, as `?org=@me`, the viewer's own account
// (`/user/migrations`). GHES 3.22 types no archive delete or unlock for the latter.
const ME = "@me";
const exportsQuery = (source: string) =>
  source === ME ? userMigrationsQuery() : orgMigrationsQuery(source);

function Exports() {
  const { data: orgs } = useSuspenseQuery(staffOrgsQuery());
  const { org = "graftyard" } = Route.useSearch();
  const navigate = Route.useNavigate();
  const [starting, setStarting] = useState(false);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2 text-sm">
          Source
          <NativeSelect
            size="sm"
            value={org}
            onChange={(e) => navigate({ search: (p) => ({ ...p, org: e.target.value }) })}
          >
            {orgs.map((o) => (
              <NativeSelectOption key={o.login} value={o.login}>
                {o.login}
              </NativeSelectOption>
            ))}
            <NativeSelectOption value={ME}>Your account</NativeSelectOption>
          </NativeSelect>
        </label>
        <Button size="sm" className="ml-auto" onClick={() => setStarting(true)}>
          <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
          Start an export
        </Button>
      </div>
      <Region name="migrations" rows={4}>
        <MigrationList key={org} org={org} />
      </Region>
      {starting && (
        <Region name="repositories" fallback={null}>
          <StartExport org={org} onClose={() => setStarting(false)} />
        </Region>
      )}
    </div>
  );
}

function MigrationList({ org }: { org: string }) {
  // Poll every 1.5 s while an export runs, so its state moves on screen.
  const { data } = useSuspenseQuery({
    ...exportsQuery(org),
    refetchInterval: (q) => (q.state.data?.some(isActive) ? 1500 : false),
  });
  if (data.length === 0) {
    return (
      <Panel>
        <p className="text-muted-foreground p-6 text-center text-sm">
          No exports for {org === ME ? "your account" : org} yet.
        </p>
      </Panel>
    );
  }
  return (
    <div className="flex flex-col gap-3" data-testid="migrations">
      {data.map((m) => (
        <MigrationCard key={m.id} org={org} m={m} />
      ))}
    </div>
  );
}

function MigrationCard({ org, m }: { org: string; m: Migration }) {
  const key = exportsQuery(org).queryKey;
  const own = org === ME;
  const state = m.state as ExportState;
  const download = useWrite({
    run: async () => {
      const { data, response } = own
        ? await api.GET("/user/migrations/{migration_id}/archive", {
            params: { path: { migration_id: m.id } },
            parseAs: "blob",
          })
        : await api.GET("/orgs/{org}/migrations/{migration_id}/archive", {
            params: { path: { org, migration_id: m.id } },
            parseAs: "blob",
          });
      if (!response.ok || !data)
        throw new Error(
          response.status === 404 ? "The archive is gone." : `HTTP ${response.status}`,
        );
      const url = URL.createObjectURL(data);
      const a = Object.assign(document.createElement("a"), {
        href: url,
        download: `migration-${m.guid}.tar.gz`,
      });
      a.click();
      URL.revokeObjectURL(url);
      return data.size;
    },
    invalidate: [],
    success: (size) => `Downloaded the archive (${size} bytes)`,
  });
  const remove = useWrite({
    run: () =>
      send(
        api.DELETE("/orgs/{org}/migrations/{migration_id}/archive", {
          params: { path: { org, migration_id: m.id } },
        }),
      ),
    invalidate: [key],
    success: "Archive deleted. The migration record stays.",
  });
  const unlock = useWrite({
    run: (repo: string) =>
      send(
        api.DELETE("/orgs/{org}/migrations/{migration_id}/repos/{repo_name}/lock", {
          params: { path: { org, migration_id: m.id, repo_name: repo } },
        }),
      ),
    invalidate: [key, staffReposQuery().queryKey],
    success: (_, repo) => `Unlocked ${org}/${repo}`,
  });
  const failed = state === "failed";
  const step = failed ? 2 : EXPORT_STEPS.indexOf(state);
  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          Export #{m.id}
          <StateBadge
            tone={
              failed
                ? "destructive-light"
                : state === "exported"
                  ? "success-light"
                  : "warning-light"
            }
          >
            {isActive(m) && <Spinner className="size-3" />}
            <span data-migration-state={m.id}>{state}</span>
          </StateBadge>
        </span>
      }
      actions={<span className="text-muted-foreground text-xs">Started {when(m.created_at)}</span>}
    >
      <div className="grid gap-4 md:grid-cols-[14rem_minmax(0,1fr)]">
        <Timeline value={step + 1} className="self-start" data-testid={`timeline-${m.id}`}>
          {(failed ? (["pending", "exporting", "failed"] as const) : EXPORT_STEPS).map((s, i) => (
            <TimelineItem
              key={s}
              step={i + 1}
              className="group-data-[orientation=vertical]/timeline:ms-8"
            >
              <TimelineHeader>
                <TimelineSeparator />
                <TimelineIndicator
                  className={
                    s === "failed"
                      ? "border-destructive bg-destructive"
                      : i < step || (i === step && !isActive(m))
                        ? "border-primary bg-primary"
                        : i === step
                          ? "border-warning"
                          : undefined
                  }
                />
                <TimelineTitle className="text-sm capitalize">{s}</TimelineTitle>
              </TimelineHeader>
              <TimelineContent className="text-xs">
                <TimelineDate className="mb-0">
                  {i === step ? (isActive(m) ? "now" : when(m.updated_at)) : i < step ? "done" : ""}
                </TimelineDate>
              </TimelineContent>
            </TimelineItem>
          ))}
        </Timeline>
        <div className="flex min-w-0 flex-col gap-3 text-sm">
          {failed && (
            <Alert variant="destructive">
              <HugeiconsIcon icon={CancelCircleIcon} strokeWidth={2} />
              <AlertTitle>The export failed</AlertTitle>
              <AlertDescription>
                A repository's git data is over the archive size limit. Start again with git data
                excluded, or export it alone.
              </AlertDescription>
            </Alert>
          )}
          <div className="flex flex-wrap gap-1">
            {[
              ["lock_repositories", "locks repositories"],
              ["exclude_git_data", "no git data"],
              ["exclude_metadata", "no metadata"],
              ["exclude_attachments", "no attachments"],
              ["exclude_releases", "no releases"],
              ["exclude_owner_projects", "no projects"],
              ["org_metadata_only", "org metadata only"],
            ]
              .filter(([k]) => m[k as keyof Migration])
              .map(([, label]) => (
                <Badge key={label} variant="outline" size="sm">
                  {label}
                </Badge>
              ))}
          </div>
          <ul className="divide-y rounded-sm border">
            {m.repositories.map((r) => {
              return (
                <li key={r.id} className="flex items-center gap-2 px-3 py-1.5">
                  <span className="flex-1 font-medium">{r.full_name}</span>
                  {m.lock_repositories && !own && (
                    <Lock
                      m={m}
                      repo={r.name}
                      onUnlock={() => unlock.mutate(r.name)}
                      busy={unlock.isPending}
                    />
                  )}
                </li>
              );
            })}
          </ul>
          <div className="flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="outline"
              disabled={state !== "exported" || !m.archive_url || download.isPending}
              onClick={() => download.mutate(undefined)}
            >
              <HugeiconsIcon icon={Download04Icon} strokeWidth={2} data-icon="inline-start" />
              Download archive
            </Button>
            {!own && (
              <Button
                size="sm"
                variant="destructive"
                disabled={state !== "exported" || !m.archive_url || remove.isPending}
                onClick={() => remove.mutate(undefined)}
              >
                <HugeiconsIcon icon={Delete02Icon} strokeWidth={2} data-icon="inline-start" />
                Delete archive
              </Button>
            )}
            {state === "exported" && !m.archive_url && (
              <span className="text-muted-foreground self-center text-xs">Archive deleted.</span>
            )}
          </div>
        </div>
      </div>
    </Panel>
  );
}

// Whether a repository is still locked reads from the stafftools repository index.
function Lock({
  m,
  repo,
  onUnlock,
  busy,
}: {
  m: Migration;
  repo: string;
  onUnlock: () => void;
  busy: boolean;
}) {
  const { data } = useQuery(staffReposQuery());
  const r = data?.find((x) => x.name === repo);
  if (!r || r.locked_by_migration !== m.id)
    return <span className="text-muted-foreground text-xs">unlocked</span>;
  return (
    <>
      <Badge variant="warning-light" size="sm">
        <HugeiconsIcon icon={LockKeyIcon} strokeWidth={2} />
        locked
      </Badge>
      <Button size="xs" variant="outline" disabled={busy} onClick={onUnlock}>
        <HugeiconsIcon icon={SquareUnlock01Icon} strokeWidth={2} />
        Unlock
      </Button>
    </>
  );
}

function StartExport({ org, onClose }: { org: string; onClose: () => void }) {
  const { data: repos } = useSuspenseQuery(staffReposQuery());
  const own = org === ME;
  // An account export names repositories in full; an organization's, by name.
  const mine = own ? repos : repos.filter((r) => r.owner.login.toLowerCase() === org.toLowerCase());
  const repoName = (r: (typeof repos)[number]) => (own ? r.full_name : r.name);
  const start = useWrite({
    run: (body: {
      repositories: string[];
      lock_repositories: boolean;
      exclude_git_data: boolean;
      exclude_metadata: boolean;
      exclude_attachments: boolean;
      exclude_releases: boolean;
    }) =>
      send(
        own
          ? api.POST("/user/migrations", { body })
          : api.POST("/orgs/{org}/migrations", { params: { path: { org } }, body }),
      ),
    invalidate: [exportsQuery(org).queryKey, staffReposQuery().queryKey],
    success: (m) => `Export #${m.id} queued`,
    onSuccess: onClose,
    onError: (e) => showFieldErrors(e, form, { repositories: "repositories" }),
  });
  const form = useForm({
    defaultValues: {
      repositories: [] as string[],
      lock_repositories: false,
      exclude_git_data: false,
      exclude_metadata: false,
      exclude_attachments: false,
      exclude_releases: false,
    },
    onSubmit: ({ value }) => start.mutateAsync(value).catch(() => {}),
  });
  const options = [
    [
      "lock_repositories",
      "Lock the repositories",
      "Nobody can push or change them until you unlock them. Use it for a final export.",
    ],
    ["exclude_git_data", "Exclude git data", "Export metadata only, without commits."],
    [
      "exclude_metadata",
      "Exclude metadata",
      "Export git data only, without issues and pull requests.",
    ],
    ["exclude_attachments", "Exclude attachments", "Smaller archives, without uploaded files."],
    ["exclude_releases", "Exclude releases", "Without release assets."],
  ] as const;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Export from {own ? "your account" : org}</DialogTitle>
          <DialogDescription>
            The archive is ready to download once the export finishes, and is kept for 7 days.
          </DialogDescription>
        </DialogHeader>
        <form
          id="start-export"
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field
            name="repositories"
            validators={{
              onSubmit: ({ value }) =>
                value.length === 0 ? "Pick at least one repository." : undefined,
            }}
          >
            {(field) => (
              <FieldSet>
                <FieldLegend variant="label">Repositories</FieldLegend>
                {mine.length === 0 && (
                  <p className="text-muted-foreground text-sm">{org} has no repositories.</p>
                )}
                {mine.map((r) => (
                  <Field key={r.id} orientation="horizontal">
                    <Checkbox
                      id={`repo-${r.id}`}
                      checked={field.state.value.includes(repoName(r))}
                      onCheckedChange={(v) => {
                        field.setMeta((m) => ({
                          ...m,
                          errorMap: { ...m.errorMap, onServer: undefined },
                        }));
                        field.handleChange(
                          v
                            ? [...field.state.value, repoName(r)]
                            : field.state.value.filter((x) => x !== repoName(r)),
                        );
                      }}
                    />
                    <FieldLabel htmlFor={`repo-${r.id}`}>
                      {r.full_name}
                      {r.locked_by_migration && (
                        <span className="text-warning-foreground dark:text-warning text-xs">
                          {" "}
                          (locked)
                        </span>
                      )}
                    </FieldLabel>
                  </Field>
                ))}
                {field.state.meta.errors.length > 0 && (
                  <FieldError>{field.state.meta.errors.join(". ")}</FieldError>
                )}
              </FieldSet>
            )}
          </form.Field>
          <FieldSet>
            <FieldLegend variant="label">Options</FieldLegend>
            {options.map(([name, label, help]) => (
              <form.Field key={name} name={name}>
                {(field) => (
                  <Field orientation="horizontal" className="items-start">
                    <Checkbox
                      id={name}
                      checked={field.state.value}
                      onCheckedChange={(v) => field.handleChange(!!v)}
                    />
                    <div>
                      <FieldLabel htmlFor={name}>{label}</FieldLabel>
                      <FieldDescription className="text-xs">{help}</FieldDescription>
                    </div>
                  </Field>
                )}
              </form.Field>
            ))}
          </FieldSet>
        </form>
        <DialogFooter>
          <Button size="sm" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" form="start-export" disabled={start.isPending}>
            Start export
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ---- Live migrations ----

// The state machine the stepper draws: each step is a status, and each status allows
// the actions in its row. Paused sits on the running step.
const STEPS = [
  { title: "Created", statuses: ["created"] },
  { title: "Queued", statuses: ["queued"] },
  { title: "Migrating", statuses: ["in_progress", "paused"] },
  { title: "Cutting over", statuses: ["cutting_over"] },
  { title: "Completed", statuses: ["completed"] },
] as const;

type Action = "start" | "pause" | "resume" | "cutover" | "revert-cutover" | "cancel";
const ACTIONS: Record<string, Action[]> = {
  created: ["start", "cancel"],
  queued: ["pause", "cancel"],
  in_progress: ["pause", "cutover", "cancel"],
  paused: ["resume", "cancel"],
  cutting_over: ["revert-cutover"],
  completed: ["revert-cutover"],
  failed: [],
  terminated: [],
};

function LiveMigrations() {
  const { data } = useSuspenseQuery({
    ...liveMigrationsQuery(),
    refetchInterval: (q) =>
      q.state.data?.migrations.some((m) =>
        ["queued", "in_progress", "cutting_over"].includes(m.status ?? ""),
      )
        ? 2000
        : false,
  });
  const { live } = Route.useSearch();
  const [creating, setCreating] = useState(false);
  const selected = live ?? data.migrations[0]?.migration_id;
  return (
    <div className="flex flex-col gap-4">
      <Panel
        title={`${data.total_count} live ${data.total_count === 1 ? "migration" : "migrations"}`}
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <HugeiconsIcon icon={Add01Icon} strokeWidth={2} data-icon="inline-start" />
            New live migration
          </Button>
        }
        flush
      >
        {data.migrations.length === 0 ? (
          <p className="text-muted-foreground p-6 text-center text-sm">No live migrations.</p>
        ) : (
          <ul className="divide-y" data-testid="live-migrations">
            {data.migrations.map((m) => (
              <li key={m.migration_id}>
                <Link
                  from={Route.fullPath}
                  search={(p) => ({ ...p, live: m.migration_id })}
                  data-active={m.migration_id === selected || undefined}
                  className="hover:bg-muted/40 data-active:bg-muted flex items-center gap-3 px-4 py-2 text-sm"
                >
                  <span className="font-medium">
                    {m.source_organization_login}/{m.source_repository_name}
                  </span>
                  <span className="text-muted-foreground text-xs">
                    to {m.target_organization_login}/{m.target_repository_name}
                  </span>
                  <LiveBadge status={m.status} />
                </Link>
              </li>
            ))}
          </ul>
        )}
      </Panel>
      {selected && (
        <Region name="live migration" rows={6}>
          <LiveDetail key={selected} id={selected} />
        </Region>
      )}
      {creating && <NewLive onClose={() => setCreating(false)} />}
    </div>
  );
}

function LiveBadge({ status }: { status: string | null }) {
  const tone =
    status === "completed"
      ? "success-light"
      : status === "terminated" || status === "failed"
        ? "destructive-light"
        : status === "paused"
          ? "outline"
          : "warning-light";
  return (
    <StateBadge tone={tone}>
      <span data-live-status>{(status ?? "unknown").replace("_", " ")}</span>
    </StateBadge>
  );
}

function LiveDetail({ id }: { id: string }) {
  const { data } = useSuspenseQuery({
    ...liveMigrationQuery(id),
    refetchInterval: (q) =>
      ["queued", "in_progress", "cutting_over"].includes(q.state.data?.migration?.status ?? "")
        ? 1000
        : false,
  });
  const m = data.migration!;
  const status = m.status ?? "created";
  const combined = data.combined_state;
  const summary = data.target_state?.repository_summaries[0];
  const step =
    Math.max(
      0,
      STEPS.findIndex((s) => (s.statuses as readonly string[]).includes(status)),
    ) + 1;
  const done = status === "completed";
  const [forcing, setForcing] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const act = useWrite({
    run: async ({ action, force }: { action: Action; force?: boolean }) => {
      const params = { params: { path: { migration_id: id } } };
      if (action === "cutover")
        return send(
          api.POST("/enterprise/live-migrations/{migration_id}/cutover", {
            ...params,
            body: { force },
          }),
        );
      if (action === "revert-cutover")
        return send(api.POST("/enterprise/live-migrations/{migration_id}/revert-cutover", params));
      const path = `/enterprise/live-migrations/{migration_id}/${action}` as const;
      return send(api.POST(path, params));
    },
    invalidate: [liveMigrationQuery(id).queryKey, liveMigrationsQuery().queryKey],
    success: (_, v) =>
      ({
        start: "Started",
        pause: "Paused",
        resume: "Resumed",
        cutover: "Cutover started",
        "revert-cutover": "Cutover reverted",
        cancel: "Cancelled",
      })[v.action],
    onSuccess: () => {
      setForcing(false);
      setCancelling(false);
    },
  });
  const allowed = ACTIONS[status] ?? [];
  const buttons: {
    action: Action;
    label: string;
    icon: typeof PlayIcon;
    tone?: "destructive" | "outline";
  }[] = [
    { action: "start", label: "Start", icon: PlayIcon },
    { action: "pause", label: "Pause", icon: PauseIcon, tone: "outline" },
    { action: "resume", label: "Resume", icon: PlayIcon },
    { action: "cutover", label: "Cut over", icon: CheckmarkCircle02Icon },
    {
      action: "revert-cutover",
      label: "Revert cutover",
      icon: ArrowTurnBackwardIcon,
      tone: "outline",
    },
    { action: "cancel", label: "Cancel migration", icon: CancelCircleIcon, tone: "destructive" },
  ];
  const processed = summary
    ? Math.round((summary.total_processed / Math.max(1, summary.total)) * 100)
    : 0;
  return (
    <Panel
      title={
        <span className="flex items-center gap-2">
          {m.source_organization_login}/{m.source_repository_name} to {m.target_organization_login}/
          {m.target_repository_name}
          <LiveBadge status={status} />
        </span>
      }
      actions={<code className="text-muted-foreground text-xs">{m.migration_id}</code>}
    >
      <div className="flex flex-col gap-4">
        <Stepper value={step} data-testid="live-stepper">
          <StepperNav>
            {STEPS.map((s, i) => (
              <StepperItem
                key={s.title}
                step={i + 1}
                completed={i + 1 < step || (done && i + 1 === step)}
                loading={
                  ["queued", "in_progress", "cutting_over"].includes(status) && i + 1 === step
                }
              >
                <StepperTrigger className="flex-col gap-1.5" disabled>
                  <StepperIndicator>{i + 1}</StepperIndicator>
                  <StepperTitle className="text-xs">{s.title}</StepperTitle>
                  <StepperDescription className="text-xs">
                    {
                      i + 1 === step && status === "paused"
                        ? "paused"
                        : i + 1 === step
                          ? "current"
                          : "\u00a0" /* keeps every step the same height */
                    }
                  </StepperDescription>
                </StepperTrigger>
                {i < STEPS.length - 1 && (
                  <StepperSeparator className="group-data-[state=completed]/step:bg-primary" />
                )}
              </StepperItem>
            ))}
          </StepperNav>
        </Stepper>
        {status === "terminated" && (
          <Alert variant="destructive">
            <HugeiconsIcon icon={CancelCircleIcon} strokeWidth={2} />
            <AlertTitle>Cancelled</AlertTitle>
            <AlertDescription>
              The source repository was not changed. Start a new live migration to try again.
            </AlertDescription>
          </Alert>
        )}
        <div className="grid gap-4 md:grid-cols-2">
          <div className="text-sm">
            <p data-testid="live-message">{combined?.display_message}</p>
            {summary && (
              <div className="mt-2">
                <div className="flex justify-between text-xs">
                  <span>Backfill</span>
                  <span className="tabular-nums" data-testid="live-progress">
                    {summary.total_processed} of {summary.total} resources
                  </span>
                </div>
                <Progress value={processed} className="mt-1" />
                <p className="text-muted-foreground mt-1 text-xs">
                  Initial git push {summary.initial_git_push_complete ? "done" : "pending"} ·{" "}
                  {summary.total_live_update} live updates · {summary.total_failed} failed
                </p>
              </div>
            )}
            {combined?.cutover_blockers.length ? (
              <ul
                className="text-muted-foreground mt-2 list-disc pl-4 text-xs"
                data-testid="cutover-blockers"
              >
                {combined.cutover_blockers.map((b) => (
                  <li key={b}>{b}</li>
                ))}
              </ul>
            ) : null}
            <div className="mt-3 flex flex-wrap gap-2">
              {buttons
                .filter((b) => allowed.includes(b.action))
                .map((b) => (
                  <Button
                    key={b.action}
                    size="sm"
                    variant={b.tone ?? "default"}
                    disabled={act.isPending}
                    onClick={() => {
                      if (b.action === "cutover" && !combined?.ready_for_cutover) setForcing(true);
                      else if (b.action === "cancel") setCancelling(true);
                      else act.mutate({ action: b.action });
                    }}
                  >
                    <HugeiconsIcon icon={b.icon} strokeWidth={2} data-icon="inline-start" />
                    {b.label}
                  </Button>
                ))}
            </div>
            {forcing && (
              <Alert variant="warning" className="mt-3">
                <AlertTitle>Backfill has not finished</AlertTitle>
                <AlertDescription>
                  A forced cutover makes the target authoritative now. Data not yet copied stays
                  only on the source.
                  <span className="mt-2 flex gap-2">
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={() => act.mutate({ action: "cutover", force: true })}
                    >
                      Force cutover
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => setForcing(false)}>
                      Wait
                    </Button>
                  </span>
                </AlertDescription>
              </Alert>
            )}
            {act.error && <p className="text-destructive mt-2 text-xs">{message(act.error)}</p>}
            <AlertDialog open={cancelling} onOpenChange={setCancelling}>
              <AlertDialogContent className="rounded-sm">
                <AlertDialogHeader>
                  <AlertDialogTitle>Cancel this live migration?</AlertDialogTitle>
                  <AlertDialogDescription>
                    A cancelled migration cannot be resumed. The source repository stays as it is.
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel size="sm">Keep it</AlertDialogCancel>
                  <Button
                    variant="destructive"
                    size="sm"
                    disabled={act.isPending}
                    onClick={() => act.mutate({ action: "cancel" })}
                  >
                    Cancel migration
                  </Button>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </div>
          <div>
            <h3 className="mb-1 text-xs font-semibold">Messages</h3>
            <ul
              className="max-h-48 divide-y overflow-auto rounded-sm border text-xs"
              data-testid="live-messages"
            >
              {data.messages.map((x, i) => (
                <li key={i} className="flex gap-2 px-2 py-1">
                  <span
                    className={
                      x.message_type === "warning"
                        ? "text-warning-foreground dark:text-warning"
                        : "text-muted-foreground"
                    }
                  >
                    {x.created_at
                      ? new Date(x.created_at).toLocaleTimeString("en-GB", { timeZone: "UTC" })
                      : ""}
                  </span>
                  <span>{x.message}</span>
                </li>
              ))}
            </ul>
            <dl className="text-muted-foreground mt-2 grid grid-cols-[auto_1fr] gap-x-3 text-xs">
              <dt>Target</dt>
              <dd className="truncate">{m.target_api_endpoint}</dd>
              <dt>Visibility</dt>
              <dd>{m.target_visibility}</dd>
              <dt>Started</dt>
              <dd>{when(m.started_at)}</dd>
              <dt>Completed</dt>
              <dd>{when(m.completed_at)}</dd>
              <dt>Expires</dt>
              <dd>{when(m.expires_at)}</dd>
            </dl>
          </div>
        </div>
      </div>
    </Panel>
  );
}

function NewLive({ onClose }: { onClose: () => void }) {
  const { data: repos } = useQuery(staffReposQuery());
  const navigate = Route.useNavigate();
  const create = useWrite({
    run: (body: {
      source_organization_login: string;
      source_repository_name: string;
      target_organization_login: string;
      target_repository_name: string;
      target_api_endpoint: string;
      pat_name: string;
      target_visibility: "private" | "internal";
    }) => send(api.POST("/enterprise/live-migrations", { body })),
    invalidate: [liveMigrationsQuery().queryKey],
    success: "Live migration created. Start it when ready.",
    onSuccess: (r) => {
      onClose();
      void navigate({ search: (p) => ({ ...p, live: r.migration_id }) });
    },
    onError: (e) => {
      if (
        showFieldErrors(e, form, {
          source_repository_name: "source",
          target_repository_name: "target_repo",
          target_api_endpoint: "endpoint",
          pat_name: "pat",
        })
      )
        return true;
      if (e instanceof Error && "status" in e && e.status === 409) {
        toast.error(e.message);
        return true;
      }
    },
  });
  const form = useForm({
    defaultValues: {
      source: "graftyard/edge",
      target_org: "graftyard-cloud",
      target_repo: "edge",
      endpoint: "https://api.github.com",
      pat: "LIVE_MIGRATION_PAT",
      visibility: "private" as "private" | "internal",
    },
    onSubmit: ({ value }) => {
      const [org, name] = value.source.split("/") as [string, string];
      return create
        .mutateAsync({
          source_organization_login: org,
          source_repository_name: name,
          target_organization_login: value.target_org.trim(),
          target_repository_name: value.target_repo.trim(),
          target_api_endpoint: value.endpoint.trim(),
          pat_name: value.pat.trim(),
          target_visibility: value.visibility,
        })
        .catch(() => {});
    },
  });
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="rounded-sm sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>New live migration</DialogTitle>
          <DialogDescription>
            The source stays writable while data copies. At cutover it goes read only, then
            archived.
          </DialogDescription>
        </DialogHeader>
        <form
          id="new-live"
          className="grid gap-3 sm:grid-cols-2"
          onSubmit={(e) => {
            e.preventDefault();
            void form.handleSubmit();
          }}
        >
          <form.Field name="source">
            {(field) => (
              <Field
                className="sm:col-span-2"
                data-invalid={field.state.meta.errors.length > 0 || undefined}
              >
                <FieldLabel htmlFor="live-source">Source repository</FieldLabel>
                <NativeSelect
                  id="live-source"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value)}
                >
                  {(repos ?? []).map((r) => (
                    <NativeSelectOption key={r.id} value={r.full_name}>
                      {r.full_name}
                    </NativeSelectOption>
                  ))}
                </NativeSelect>
                {field.state.meta.errors.length > 0 && (
                  <FieldError>{field.state.meta.errors.join(". ")}</FieldError>
                )}
              </Field>
            )}
          </form.Field>
          <form.Field
            name="target_org"
            validators={{ onSubmit: ({ value }) => (!value.trim() ? "Required." : undefined) }}
          >
            {(field) => <TextField field={field} label="Target organization" />}
          </form.Field>
          <form.Field
            name="target_repo"
            validators={{ onSubmit: ({ value }) => (!value.trim() ? "Required." : undefined) }}
          >
            {(field) => <TextField field={field} label="Target repository" />}
          </form.Field>
          <form.Field name="endpoint">
            {(field) => <TextField field={field} label="Target API" className="sm:col-span-2" />}
          </form.Field>
          <form.Field
            name="pat"
            validators={{
              onSubmit: ({ value }) => (!value.trim() ? "Name the stored token." : undefined),
            }}
          >
            {(field) => (
              <TextField
                field={field}
                label="Stored token name"
                description="A target token saved in the management console."
              />
            )}
          </form.Field>
          <form.Field name="visibility">
            {(field) => (
              <Field>
                <FieldLabel htmlFor="live-vis">Target visibility</FieldLabel>
                <NativeSelect
                  id="live-vis"
                  value={field.state.value}
                  onChange={(e) => field.handleChange(e.target.value as "private" | "internal")}
                >
                  <NativeSelectOption value="private">private</NativeSelectOption>
                  <NativeSelectOption value="internal">internal</NativeSelectOption>
                </NativeSelect>
              </Field>
            )}
          </form.Field>
        </form>
        <DialogFooter>
          <Button size="sm" variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" form="new-live" disabled={create.isPending}>
            Create
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
