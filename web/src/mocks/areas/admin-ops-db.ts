import type { Schemas } from "@/api/client";
import type { components, paths } from "@/api/ghes";
import type { AgentAppRow, ReleaseEntry } from "@/areas/admin/graftyard-api";
import type { InstallApp, StaffRepo } from "@/areas/admin/stafftools-api";
import { encodeEntry, type DecodedEntry } from "@/areas/admin/release-codec";
import { API, HOST, id, sha, users } from "../world";
import { db as core } from "./core-db";
import {
  elapsed,
  iso,
  pause,
  phaseAt,
  resume,
  startedClock,
  total,
  type Clock,
  type Phase,
} from "./admin-time";

// Migrations, live migrations, the management console, and the Graftyard rows. Each
// long-running process is a clock plus the phases it walks; a read computes the state
// at that moment, so nothing drifts and a reload starts over.

type Repository = Schemas["repository"];
type Json<P extends keyof paths, M extends keyof paths[P]> = paths[P][M] extends {
  responses: { 200: { content: { "application/json": infer T } } };
}
  ? T
  : never;

// ---- Org and user migrations ----

export type MigrationState = "pending" | "exporting" | "exported" | "failed";

// About 15 s from pending to exported: queue, then the export itself.
export const EXPORT_PHASES: readonly Phase<MigrationState>[] = [
  ["pending", 3000],
  ["exporting", 12000],
  ["exported", 0],
];
// An export that includes a repository with git data over the archive limit fails late.
const FAILING_PHASES: readonly Phase<MigrationState>[] = [
  ["pending", 3000],
  ["exporting", 8000],
  ["failed", 0],
];

type Migration = {
  owner: "org" | "user";
  ownerLogin: string;
  record: Schemas["migration"];
  clock: Clock;
  phases: readonly Phase<MigrationState>[];
  archiveDeleted: boolean;
  unlocked: Set<string>;
};

const migrations: Migration[] = [];

function migration(
  owner: Migration["owner"],
  ownerLogin: string,
  repos: Repository[],
  options: Omit<
    Schemas["migration"],
    | "id"
    | "owner"
    | "guid"
    | "state"
    | "repositories"
    | "url"
    | "created_at"
    | "updated_at"
    | "node_id"
  >,
  startedAt: number,
): Migration {
  const n = id();
  const base =
    owner === "org" ? `${API}/orgs/${ownerLogin}/migrations/${n}` : `${API}/user/migrations/${n}`;
  const failing = repos.some((r) => r.name === "raft") && !options.exclude_git_data;
  const m: Migration = {
    owner,
    ownerLogin,
    record: {
      ...options,
      id: n,
      owner: owner === "org" ? users.org : users.mira,
      guid: `${sha(`mig:${n}`).slice(0, 8)}-${sha(`mig2:${n}`).slice(0, 4)}-11ef-8e2a-${sha(`mig3:${n}`).slice(0, 12)}`,
      state: "pending",
      repositories: repos,
      url: base,
      created_at: iso(startedAt),
      updated_at: iso(startedAt),
      node_id: `MI_${sha(`mig:${n}`).slice(0, 12)}`,
      archive_url: `${base}/archive`,
    },
    clock: startedClock(startedAt),
    phases: failing ? FAILING_PHASES : EXPORT_PHASES,
    archiveDeleted: false,
    unlocked: new Set(),
  };
  migrations.push(m);
  return m;
}

const NO_EXCLUDES = {
  exclude_metadata: false,
  exclude_git_data: false,
  exclude_attachments: false,
  exclude_releases: false,
  exclude_owner_projects: false,
  org_metadata_only: false,
};

// Seeds: one finished export from last week, one that failed, both already terminal.
const weekAgo = Date.parse("2026-11-01T10:00:00Z");
migration(
  "org",
  "graftyard",
  core.repos().filter((r) => r.name === "web"),
  { ...NO_EXCLUDES, lock_repositories: false },
  weekAgo,
);
migration(
  "org",
  "graftyard",
  core.repos().filter((r) => r.name === "raft"),
  { ...NO_EXCLUDES, lock_repositories: true },
  weekAgo + 86_400_000,
);

export function migrationView(m: Migration, now = Date.now()): Schemas["migration"] {
  const at = phaseAt(m.phases, elapsed(m.clock, now));
  const created = Date.parse(m.record.created_at);
  const done = at.index === m.phases.length - 1;
  return {
    ...m.record,
    state: at.state,
    updated_at: iso(done ? created + total(m.phases) : now),
    archive_url: m.archiveDeleted ? undefined : m.record.archive_url,
  };
}

export const migrationState = (m: Migration) => phaseAt(m.phases, elapsed(m.clock)).state;

export function startMigration(
  owner: Migration["owner"],
  ownerLogin: string,
  repos: Repository[],
  options: Omit<
    Schemas["migration"],
    | "id"
    | "owner"
    | "guid"
    | "state"
    | "repositories"
    | "url"
    | "created_at"
    | "updated_at"
    | "node_id"
  >,
) {
  return migration(owner, ownerLogin, repos, options, Date.now());
}

export const listMigrations = (owner: Migration["owner"], ownerLogin: string) =>
  migrations
    .filter((m) => m.owner === owner && m.ownerLogin.toLowerCase() === ownerLogin.toLowerCase())
    .sort((a, b) => b.record.id - a.record.id);

export const findMigration = (owner: Migration["owner"], ownerLogin: string, migrationId: number) =>
  listMigrations(owner, ownerLogin).find((m) => m.record.id === migrationId);

// A locked repository stays locked until unlocked, whatever the export did.
export const isLocked = (m: Migration, repoName: string) =>
  m.record.lock_repositories && !m.unlocked.has(repoName);

export const deleteArchive = (m: Migration) => void (m.archiveDeleted = true);
export const archiveGone = (m: Migration) => m.archiveDeleted;
export const unlock = (m: Migration, repoName: string) => void m.unlocked.add(repoName);

// What a download returns: a small tar-like manifest, so the browser saves a real file.
export function archiveBytes(m: Migration) {
  const manifest = {
    version: "1.0.1",
    github_sha: sha("ghes-3.22.0"),
    guid: m.record.guid,
    repositories: m.record.repositories.map((r) => r.full_name),
    exported_at: migrationView(m).updated_at,
  };
  return new TextEncoder().encode(`schema.json\n${JSON.stringify(manifest, null, 2)}\n`);
}

// Every repository, with the migration that holds a lock on it.
export function staffRepos(): StaffRepo[] {
  return core
    .repos()
    .sort((a, b) => a.full_name.localeCompare(b.full_name))
    .map((r) => ({
      ...r,
      locked_by_migration:
        migrations.find(
          (m) =>
            m.owner === "org" &&
            m.record.repositories.some((x) => x.id === r.id) &&
            isLocked(m, r.name),
        )?.record.id ?? null,
    }));
}

// ---- GHES live migrations ----

type LiveList = Json<"/enterprise/live-migrations", "get">;
type LiveMigration = LiveList["migrations"][number];
type LiveGet = Json<"/enterprise/live-migrations/{migration_id}", "get">;

// created, then start: queued, in_progress (backfill, then live updates), ready for
// cutover; cutover: cutting_over, completed. Pause stops the clock. Revert from
// completed goes back to in_progress with the source unarchived.
export type LiveState =
  | "created"
  | "queued"
  | "in_progress"
  | "paused"
  | "cutting_over"
  | "completed"
  | "failed"
  | "terminated";

const RUN_PHASES: readonly Phase<"queued" | "backfill" | "live_update">[] = [
  ["queued", 3000],
  ["backfill", 9000],
  ["live_update", 0],
];
const CUTOVER_MS = 5000;

type Live = {
  row: LiveMigration;
  stage: "created" | "running" | "paused" | "cutover" | "completed" | "terminated";
  clock: Clock;
  cutoverAt: number | null;
  messages: LiveGet["messages"];
};

const lives: Live[] = [];

function live(source: string, target: string, created: string): Live {
  const migrationId = `lm_${sha(`live:${source}:${created}`).slice(0, 20)}`;
  const l: Live = {
    row: {
      migration_id: migrationId,
      status: "created",
      source_organization_login: "graftyard",
      target_organization_login: "graftyard-cloud",
      source_repository_name: source,
      target_repository_name: target,
      target_visibility: "private",
      target_api_endpoint: "https://api.github.com",
      target_migration_id: parseInt(sha(migrationId).slice(0, 6), 16),
      created_at: created,
      started_at: null,
      completed_at: null,
      expires_at: iso(Date.parse(created) + 7 * 86_400_000),
      repositories: [
        {
          source_repository_url: `${HOST}/graftyard/${source}`,
          target_repository_url: `https://github.com/graftyard-cloud/${target}`,
          status: null,
        },
      ],
    },
    stage: "created",
    clock: { banked: 0, runningSince: null },
    cutoverAt: null,
    messages: [{ message_type: "info", message: "Migration created.", created_at: created }],
  };
  lives.push(l);
  return l;
}

live("web", "web", "2026-11-08T07:30:00Z");

const say = (l: Live, message: string, type = "info") =>
  l.messages.push({ message_type: type, message, created_at: new Date().toISOString() });

function liveStatus(l: Live, now = Date.now()): LiveState {
  if (l.stage === "created") return "created";
  if (l.stage === "paused") return "paused";
  if (l.stage === "terminated") return "terminated";
  if (l.stage === "completed") return "completed";
  if (l.stage === "cutover") return now - l.cutoverAt! >= CUTOVER_MS ? "completed" : "cutting_over";
  return phaseAt(RUN_PHASES, elapsed(l.clock, now)).state === "queued" ? "queued" : "in_progress";
}

// A cutover finishes on read once its time is up, so later reads see completed.
function settle(l: Live, now = Date.now()) {
  if (l.stage === "cutover" && liveStatus(l, now) === "completed") {
    l.stage = "completed";
    l.row.completed_at = iso(l.cutoverAt! + CUTOVER_MS);
    say(l, "Cutover complete. The source repository is archived.");
  }
}

function liveRow(l: Live): LiveMigration {
  settle(l);
  const status = liveStatus(l);
  return {
    ...l.row,
    status,
    repositories: l.row.repositories.map((r) => ({ ...r, status })),
  };
}

export const listLive = (filter: string | null) =>
  lives
    .map(liveRow)
    .filter((r) => !filter || filter === "all" || r.status === filter)
    .sort((a, b) => (b.created_at ?? "").localeCompare(a.created_at ?? ""));

export const findLive = (migrationId: string) =>
  lives.find((l) => l.row.migration_id === migrationId);

export function createLive(input: {
  source_repository_name: string;
  target_repository_name: string;
  target_organization_login: string;
  target_api_endpoint: string;
  target_visibility?: "private" | "internal";
}) {
  const l = live(
    input.source_repository_name,
    input.target_repository_name,
    new Date().toISOString(),
  );
  l.row.target_organization_login = input.target_organization_login;
  l.row.target_api_endpoint = input.target_api_endpoint;
  l.row.target_visibility = input.target_visibility ?? "private";
  l.row.repositories[0]!.target_repository_url = `https://github.com/${input.target_organization_login}/${input.target_repository_name}`;
  return l;
}

// Each action names the state it needs; otherwise GHES answers 409.
export type LiveAction = "start" | "pause" | "resume" | "cutover" | "revert-cutover" | "cancel";

export function liveAct(l: Live, action: LiveAction, force = false): string | undefined {
  settle(l);
  const status = liveStatus(l);
  const now = Date.now();
  switch (action) {
    case "start":
      if (l.stage !== "created") return `Cannot start a migration that is ${status}.`;
      l.stage = "running";
      l.clock = startedClock(now);
      l.row.started_at = iso(now);
      say(l, "Migration started. Initial git push queued.");
      return;
    case "pause":
      if (l.stage !== "running") return `Cannot pause a migration that is ${status}.`;
      pause(l.clock, now);
      l.stage = "paused";
      say(l, "Paused by mira. Live updates are held at the source.");
      return;
    case "resume":
      if (l.stage !== "paused") return `Cannot resume a migration that is ${status}.`;
      resume(l.clock, now);
      l.stage = "running";
      say(l, "Resumed by mira.");
      return;
    case "cutover": {
      if (l.stage !== "running") return `Cannot cut over a migration that is ${status}.`;
      const ready = readyForCutover(l);
      if (!ready && !force) return "Not ready for cutover: backfill has not finished.";
      l.stage = "cutover";
      l.cutoverAt = now;
      say(
        l,
        force && !ready
          ? "Forced cutover started before backfill finished."
          : "Cutover started. The source is read only.",
        force && !ready ? "warning" : "info",
      );
      return;
    }
    case "revert-cutover":
      if (l.stage !== "completed" && l.stage !== "cutover") {
        return `Cannot revert the cutover of a migration that is ${status}.`;
      }
      l.stage = "running";
      l.cutoverAt = null;
      l.row.completed_at = null;
      say(l, "Cutover reverted. The source repository is unarchived and writable.", "warning");
      return;
    case "cancel":
      if (l.stage === "completed" || l.stage === "terminated") {
        return `Cannot cancel a migration that is ${status}.`;
      }
      l.stage = "terminated";
      if (l.clock.runningSince !== null) pause(l.clock, now);
      say(l, "Cancelled by mira.", "warning");
      return;
  }
}

function readyForCutover(l: Live) {
  return phaseAt(RUN_PHASES, elapsed(l.clock)).state === "live_update";
}

export function liveDetail(l: Live): LiveGet {
  const row = liveRow(l);
  const run = phaseAt(RUN_PHASES, elapsed(l.clock));
  const totalResources = 1840;
  const processed =
    l.stage === "created"
      ? 0
      : run.state === "queued"
        ? 0
        : run.state === "backfill"
          ? Math.round(totalResources * run.fraction)
          : totalResources;
  const ready = l.stage === "running" && run.state === "live_update";
  const nwo = `${row.source_organization_login}/${row.source_repository_name}`;
  const blockers =
    ready || l.stage === "completed" || l.stage === "cutover" || l.stage === "terminated"
      ? []
      : [
          l.stage === "created"
            ? "The migration has not started."
            : l.stage === "paused"
              ? "The migration is paused."
              : "Backfill is still running.",
        ];
  const breakdown = (state: string, count: number) => [
    { state, kind: "issue", type: "backfill", count: Math.round(count * 0.4) },
    { state, kind: "pull_request", type: "backfill", count: Math.round(count * 0.35) },
    {
      state,
      kind: "comment",
      type: "backfill",
      count: count - Math.round(count * 0.4) - Math.round(count * 0.35),
    },
  ];
  return {
    migration: row,
    target_state: {
      status: row.status,
      last_updated: new Date().toISOString(),
      target_unavailable: false,
      repository_summaries: [
        {
          namespace: row.target_organization_login,
          repository: row.target_repository_name,
          total: totalResources,
          total_backfill: totalResources,
          total_live_update: ready ? 12 : 0,
          total_processed: processed,
          total_failed: 0,
          total_pending: totalResources - processed,
          total_eligible: totalResources,
          total_acknowledged: processed,
          initial_git_push_complete: l.stage !== "created" && run.state !== "queued",
          backfill: {
            total: totalResources,
            total_pending: totalResources - processed,
            total_processed: processed,
            total_failed: 0,
            total_eligible: totalResources,
            total_acknowledged: processed,
            breakdown: breakdown("processed", processed),
          },
          live_update: ready
            ? {
                total: 12,
                total_pending: 0,
                total_processed: 12,
                total_failed: 0,
                total_eligible: 12,
                total_acknowledged: 12,
                breakdown: [],
              }
            : null,
        },
      ],
      repository_progress: [
        {
          repository_nwo: nwo,
          backfill_resources_added: totalResources,
          backfill_resources_processed: processed,
          backfill_resources_failed: 0,
          backfill_resources_eligible: totalResources,
          backfill_resources_acknowledged: processed,
          live_update_resources_added: ready ? 12 : 0,
          live_update_resources_processed: ready ? 12 : 0,
          live_update_resources_eligible: ready ? 12 : 0,
          live_update_resources_acknowledged: ready ? 12 : 0,
          live_update_resources_failed: 0,
          all_resources_sent: processed === totalResources,
          initial_git_push_complete: l.stage !== "created" && run.state !== "queued",
          repository_locked: l.stage === "cutover",
        },
      ],
      terminal_failure: null,
    },
    combined_state: {
      status: row.status,
      display_message: {
        created: "Created. Start the migration to push git data.",
        queued: "Queued for the initial git push.",
        in_progress: ready
          ? "Live updates are flowing. Ready for cutover."
          : "Backfilling issues, pull requests, and comments.",
        paused: "Paused. Live updates are held.",
        cutting_over: "Cutting over. The source is read only.",
        completed: "Completed. The target is authoritative.",
        failed: "Failed.",
        terminated: "Cancelled.",
      }[liveStatus(l)],
      ready_for_cutover: ready,
      cutover_blockers: blockers,
      repositories: [{ repository_nwo: nwo, phase: run.state, display_status: row.status ?? "" }],
      terminal_failure: null,
    },
    messages: [...l.messages].reverse(),
  };
}

// ---- Management console (/manage/v1) ----

const NODES = [
  { hostname: "ghes-primary", uuid: "1b6cf518-f97c-11ed-8544-061d81f7eedb", replica: false },
  { hostname: "ghes-replica-fra", uuid: "4f2a21aa-f97c-11ed-8544-061d81f7eedc", replica: true },
] as const;

const ROLES: components["schemas"]["ghes-cluster-roles"][] = [
  "ConsulServer",
  "ElasticsearchServer",
  "GitServer",
  "JobServer",
  "MysqlServer",
  "PagesServer",
  "RedisServer",
  "StorageServer",
  "WebServer",
];

export const configNodes = (): components["schemas"]["ghes-config-nodes"] => ({
  topology: "Ha",
  nodes: NODES.map((n) => ({ ...n, cluster_roles: ROLES })),
});

const SERVICES = ["consul", "elasticsearch", "git-daemon", "memcache", "mysql", "redis", "storage"];

export function clusterStatus(): components["schemas"]["ghes-cluster-status"] {
  const busy = configRun !== null && applyRunning();
  return {
    status: busy ? "WARNING" : "OK",
    nodes: NODES.map((n) => ({
      hostname: n.hostname,
      status: busy && !n.replica ? "WARNING" : "OK",
      services: SERVICES.map((name) => ({
        name,
        status: busy && !n.replica && name === "mysql" ? "WARNING" : "OK",
        details:
          busy && !n.replica && name === "mysql" ? "restarting after config apply" : "running",
      })),
    })),
  };
}

export function replicationStatus(): components["schemas"]["ghes-replication-status"] {
  return {
    status: "OK",
    nodes: [
      {
        hostname: NODES[1].hostname,
        status: "OK",
        services: [
          { name: "mysql", status: "OK", details: "replication is running, 0 s behind" },
          { name: "redis", status: "OK", details: "replication is in sync" },
          { name: "elasticsearch", status: "OK", details: "cluster is green" },
          { name: "git", status: "OK", details: "all repositories in sync" },
          { name: "pages", status: "OK", details: "all pages in sync" },
          { name: "alambic", status: "WARNING", details: "14 objects pending, catching up" },
        ],
      },
    ],
  };
}

export const versions = (): components["schemas"]["ghes-version"] =>
  NODES.map((n) => ({
    hostname: n.hostname,
    version: {
      version: "3.22.0",
      platform: "kvm",
      build_id: "fc5d70c2a9",
      build_date: "2026-10-14",
    },
  }));

type Maintenance = components["schemas"]["ghes-get-maintenance"][number];
const maintenance: Maintenance = {
  status: "off",
  scheduled_time: undefined,
  connection_services: [
    { name: "git operations", number: 3 },
    { name: "mysql queries", number: 12 },
    { name: "aqueduct jobs", number: 0 },
    { name: "resque jobs", number: 1 },
  ],
  can_unset_maintenance: true,
  ip_exception_list: [],
  maintenance_mode_message: "",
};

export function getMaintenance(): components["schemas"]["ghes-get-maintenance"] {
  // A scheduled window turns on once its time arrives.
  if (
    maintenance.status === "scheduled" &&
    maintenance.scheduled_time &&
    Date.parse(maintenance.scheduled_time) <= Date.now()
  ) {
    maintenance.status = "on";
  }
  return NODES.map((n) => ({ ...maintenance, hostname: n.hostname, uuid: n.uuid }));
}

export function setMaintenance(input: components["schemas"]["ghes-set-maintenance-request"]) {
  const when = input.when && input.when !== "now" ? input.when : undefined;
  if (!input.enabled) {
    maintenance.status = "off";
    maintenance.scheduled_time = undefined;
  } else if (when && Date.parse(when) > Date.now()) {
    maintenance.status = "scheduled";
    maintenance.scheduled_time = new Date(when).toISOString();
  } else {
    maintenance.status = "on";
    maintenance.scheduled_time = new Date().toISOString();
  }
  maintenance.ip_exception_list = input.ip_exception_list ?? [];
  maintenance.maintenance_mode_message = input.maintenance_mode_message ?? "";
  const message = {
    off: "maintenance mode disabled",
    on: "maintenance mode enabled",
    scheduled: `maintenance mode scheduled for ${maintenance.scheduled_time}`,
  }[maintenance.status!];
  return NODES.map((n) => ({ hostname: n.hostname, uuid: n.uuid, message }));
}

export const settings: components["schemas"]["ghes-get-settings"] = {
  private_mode: true,
  public_pages: false,
  subdomain_isolation: true,
  signup_enabled: false,
  github_hostname: "ghes.graftyard.test",
  identicons_host: "dotcom",
  http_proxy: null,
  auth_mode: "ldap",
  expire_sessions: false,
  configuration_id: 1_418_221,
  configuration_run_count: 27,
  avatar: { enabled: false, uri: "" },
  customer: {
    name: "Graftyard",
    email: "ops@graftyard.test",
    uuid: "d3b7f2a0-1c11-11ef-9c7a-0242ac120002",
  },
  license: {
    seats: 25,
    evaluation: false,
    perpetual: false,
    unlimited_seating: false,
    support_key: "ssh-rsa AAAA…",
    ssh_allowed: true,
    cluster_support: false,
    expire_at: "2027-03-31T23:59:59Z",
  },
  github_ssl: { enabled: true, cert: null, key: null },
  ldap: {
    host: "ldap.graftyard.test",
    port: 636,
    base: ["ou=people,dc=graftyard,dc=test"],
    uid: "uid",
    bind_dn: "cn=ghes,ou=services,dc=graftyard,dc=test",
    method: "SSL",
    search_strategy: "detect",
    user_groups: [],
    admin_group: "cn=site-admins,ou=groups,dc=graftyard,dc=test",
    virtual_attribute_enabled: false,
    recursive_group_search: false,
    posix_support: true,
    user_sync_emails: true,
    user_sync_keys: true,
    user_sync_interval: 4,
    team_sync_interval: 4,
    sync_enabled: true,
    reconciliation: { user: null, org: null },
    profile: { uid: "uid", name: "cn", mail: "mail", key: null },
  },
  smtp: {
    enabled: true,
    address: "smtp.graftyard.test",
    authentication: "plain",
    port: "587",
    domain: "graftyard.test",
    username: "ghes",
    enable_starttls_auto: true,
    support_address: "ops@graftyard.test",
    support_address_type: "email",
    noreply_address: "noreply@ghes.graftyard.test",
  },
  ntp: { primary_server: "time.cloudflare.com", secondary_server: "pool.ntp.org" },
  timezone: "UTC",
  snmp: { enabled: false, community: "" },
  syslog: { enabled: true, server: "syslog.graftyard.test:6514", protocol_name: "TCP" },
  pages: { enabled: true },
  collectd: {
    enabled: false,
    server: null,
    port: 25826,
    encryption: null,
    username: null,
    password: null,
  },
  mapping: { enabled: false, tileserver: null, basemap: "streets", token: null },
  load_balancer: null,
  prometheus: { enabled: true, trusted_ips: "10.0.0.0/8" },
};

// Settings are saved immediately but take effect only through a config apply, as on GHES.
export let pendingSettings = false;
export function applySettings(patch: Partial<components["schemas"]["ghes-get-settings"]>) {
  Object.assign(settings, patch);
  pendingSettings = true;
}

// A config apply: each event has an offset from the start; reads return what has happened.
type ApplyEvent = { at: number; node: number; name: string; body: string; severity?: string };
const APPLY_SCRIPT: ApplyEvent[] = [
  { at: 0, node: 0, name: "ConfigApply", body: "Starting configuration run" },
  { at: 600, node: 0, name: "ValidateSettings", body: "Validating settings" },
  { at: 1500, node: 1, name: "ValidateSettings", body: "Validating settings" },
  { at: 2500, node: 0, name: "RenderTemplates", body: "Rendering 214 templates" },
  { at: 4000, node: 0, name: "RestartService", body: "Restarting mysql" },
  { at: 4200, node: 1, name: "RenderTemplates", body: "Rendering 214 templates" },
  { at: 6000, node: 0, name: "RestartService", body: "Restarting github-unicorn" },
  {
    at: 7400,
    node: 0,
    name: "RestartService",
    body: "Waiting for github-unicorn to report healthy",
    severity: "WARN",
  },
  { at: 9000, node: 1, name: "ReplicationCheck", body: "Replica in sync" },
  { at: 10_500, node: 0, name: "Migrations", body: "No pending database migrations" },
  { at: 12_000, node: 0, name: "ConfigApply", body: "Configuration run complete" },
  { at: 12_200, node: 1, name: "ConfigApply", body: "Configuration run complete" },
];
const APPLY_MS = 12_400;

let configRun: { id: string; startedAt: number } | null = null;
const applyRunning = () => configRun !== null && Date.now() - configRun.startedAt < APPLY_MS;

export function startApply(runId: string | undefined): { conflict: string } | { runId: string } {
  if (applyRunning()) return { conflict: configRun!.id };
  configRun = { id: runId ?? sha(`run:${Date.now()}`).slice(0, 12), startedAt: Date.now() };
  pendingSettings = false;
  settings.configuration_run_count = (settings.configuration_run_count ?? 0) + 1;
  return { runId: configRun.id };
}

type ApplyStatus = Json<"/manage/v1/config/apply", "get">;
type ApplyEvents = Json<"/manage/v1/config/apply/events", "get">;

export function applyStatus(runId: string | null): ApplyStatus {
  if (!configRun || (runId && runId !== configRun.id))
    return { running: false, successful: true, nodes: [] };
  const running = applyRunning();
  return {
    running,
    successful: !running,
    nodes: NODES.map((n) => ({
      run_id: configRun!.id,
      hostname: n.hostname,
      running,
      successful: !running,
    })),
  };
}

// Events after last_request_id; each event's request id is "<run>:<index>".
export function applyEvents(lastRequestId: string | null): ApplyEvents {
  if (!configRun) return { nodes: [] };
  const run = configRun;
  const now = Date.now() - run.startedAt;
  const after = lastRequestId?.startsWith(`${run.id}:`) ? Number(lastRequestId.split(":")[1]) : -1;
  const happened = APPLY_SCRIPT.map((e, i) => ({ e, i })).filter(
    ({ e, i }) => e.at <= now && i > after,
  );
  return {
    nodes: NODES.map((n, ni) => {
      const mine = happened.filter(({ e }) => e.node === ni);
      return {
        node: n.hostname,
        last_request_id: mine.length ? `${run.id}:${mine.at(-1)!.i}` : (lastRequestId ?? undefined),
        events: mine.map(({ e, i }) => ({
          timestamp: iso(run.startedAt + e.at),
          severity_text: e.severity ?? "INFO",
          body: e.body,
          event_name: e.name,
          topology: "Ha",
          hostname: n.hostname,
          config_run_id: run.id,
          trace_id: sha(`${run.id}:trace`).slice(0, 32),
          span_id: sha(`${run.id}:${i}`).slice(0, 16),
          span_parent_id: sha(`${run.id}:root`).slice(0, 16),
          span_depth: e.name === "ConfigApply" ? 0 : 1,
        })),
      };
    }),
  };
}

// Upgrades: download the package, then apply it. Both walk a timeline.
export const UPGRADE_TO = "3.22.1";
const DOWNLOAD_PHASES: readonly Phase<"downloading" | "completed">[] = [
  ["downloading", 8000],
  ["completed", 0],
];
const UPGRADE_PHASES: readonly Phase<"pre-upgrade" | "upgrade" | "reboot" | "completed">[] = [
  ["pre-upgrade", 4000],
  ["upgrade", 8000],
  ["reboot", 4000],
  ["completed", 0],
];

let download: { id: string; startedAt: number; version: string } | null = null;
let upgrade: { startedAt: number; version: string } | null = null;

export function startDownload(version: string): { conflict: string } | { downloadId: string } {
  if (
    download &&
    phaseAt(DOWNLOAD_PHASES, Date.now() - download.startedAt).state === "downloading"
  ) {
    return { conflict: download.id };
  }
  download = { id: `dl_${sha(`dl:${Date.now()}`).slice(0, 10)}`, startedAt: Date.now(), version };
  return { downloadId: download.id };
}

export function downloadStatus(): components["schemas"]["ghes-get-manage-upgrade-download-status-response"] {
  if (!download) return { status: "not_started", nodes: [] };
  const at = phaseAt(DOWNLOAD_PHASES, Date.now() - download.startedAt);
  const pkg = `github-enterprise-${download.version}.hpkg`;
  // Each node runs at its own speed; a node is done at 100%, and all are once the phase is.
  const pct = (i: number) =>
    at.state === "completed" ? 100 : Math.min(100, Math.round(at.fraction * 100 * (1.2 - i * 0.3)));
  const done = NODES.filter((_, i) => pct(i) === 100).length;
  return {
    download_id: download.id,
    package_name: pkg,
    status: at.state,
    nodes: NODES.map((n, i) => ({
      hostname: n.hostname,
      uuid: n.uuid,
      download_id: download!.id,
      package_name: pkg,
      status: pct(i) === 100 ? "completed" : "downloading",
      progress: `${pct(i)}%`,
    })),
    orchestration: {
      phase: at.state === "completed" ? "completed" : "running",
      max_parallel: 2,
      queued: 0,
      in_flight: NODES.length - done,
      completed: done,
      failed: 0,
      total: NODES.length,
    },
  };
}

export const downloadReady = () =>
  download !== null &&
  phaseAt(DOWNLOAD_PHASES, Date.now() - download.startedAt).state === "completed";

export function startUpgrade() {
  upgrade = { startedAt: Date.now(), version: download!.version };
}

export function upgradeStatus(): components["schemas"]["ghes-get-manage-upgrade-status-response"] {
  return NODES.map((n) => {
    if (!upgrade)
      return {
        hostname: n.hostname,
        uuid: n.uuid,
        status: "not_started",
        pre_upgrade_status: "not_started",
        is_running: false,
        failed: false,
        steps: [],
      };
    const at = phaseAt(UPGRADE_PHASES, Date.now() - upgrade.startedAt);
    const doneSteps = UPGRADE_PHASES.slice(0, at.index).map(([phase]) => phase);
    let t = upgrade.startedAt;
    const steps = UPGRADE_PHASES.slice(0, -1).map(([phase, ms], i) => {
      const step = {
        phase,
        step: {
          "pre-upgrade": "Run pre-upgrade checks",
          upgrade: "Install the package",
          reboot: "Reboot into the new partition",
          completed: "",
        }[phase],
        status: i < at.index ? "completed" : i === at.index ? "in_progress" : "pending",
        started_at: i <= at.index ? iso(t) : undefined,
        ended_at: i < at.index ? iso(t + ms) : undefined,
      };
      t += ms;
      return step;
    });
    const complete = at.state === "completed";
    return {
      hostname: n.hostname,
      uuid: n.uuid,
      release_version: upgrade.version,
      release_sha: sha(`release:${upgrade.version}`),
      status: complete ? "completed" : "in_progress",
      pre_upgrade_status: doneSteps.includes("pre-upgrade") ? "completed" : "in_progress",
      upgrade_last_completed_step: doneSteps.at(-1) ?? null,
      is_running: !complete,
      failed: false,
      steps,
      messages: complete ? [`Upgraded to ${upgrade.version}`] : [],
    };
  });
}

export const installedVersion = () =>
  upgrade && phaseAt(UPGRADE_PHASES, Date.now() - upgrade.startedAt).state === "completed"
    ? upgrade.version
    : "3.22.0";

// ---- Graftyard: agent apps ----

// The Apps on this install. The install owns the first-party ones, so only those may be
// graftyard_run or mcp (design/AGENT-LAYER.md, "API reference").
export const installApps: InstallApp[] = [
  {
    app_id: 101,
    slug: "graftyard-agent",
    name: "graftyard-agent",
    owned: true,
    description: "Runs Graftyard agent tasks.",
  },
  {
    app_id: 102,
    slug: "graftyard-workspace",
    name: "graftyard-workspace",
    owned: true,
    description: "Pushes agent work to workspace forks.",
  },
  {
    app_id: 103,
    slug: "graftyard-mcp",
    name: "Graftyard MCP",
    owned: true,
    description: "The install's MCP server.",
  },
  {
    app_id: 104,
    slug: "graftyard-insights",
    name: "graftyard-insights",
    owned: true,
    description: "Overlap and provenance checks.",
  },
  {
    app_id: 210,
    slug: "claude",
    name: "Claude",
    owned: false,
    description: "Anthropic's coding agent.",
  },
  {
    app_id: 211,
    slug: "cursor",
    name: "Cursor",
    owned: false,
    description: "Cursor background agents.",
  },
  {
    app_id: 212,
    slug: "devin",
    name: "Devin",
    owned: false,
    description: "Cognition's coding agent.",
  },
  {
    app_id: 213,
    slug: "renovate",
    name: "Renovate",
    owned: false,
    description: "Dependency updates.",
  },
];

export const agentApps: AgentAppRow[] = [
  { app_id: 101, slug: "graftyard-agent", kind: "graftyard_run", confine: null },
  { app_id: 102, slug: "graftyard-workspace", kind: "graftyard_run", confine: null },
  { app_id: 103, slug: "graftyard-mcp", kind: "mcp", confine: null },
  { app_id: 210, slug: "claude", kind: "third_party", confine: "refs/heads/claude/**" },
  { app_id: 211, slug: "cursor", kind: "third_party", confine: null },
];

// ---- Graftyard: the release log ----

const INSTALL = "01JD3Q8Z4T6W7N2K5M9V0R1S3X";
const key = (seed: string) => sha(`key:${seed}`) + sha(`key2:${seed}`).slice(0, 24);
const signature = (seed: string) => ({
  key: key(seed.split(":")[0]!),
  sig: sha(`s:${seed}`) + sha(`s2:${seed}`) + sha(`s3:${seed}`).slice(0, 48),
});

const releaseId = (v: string) => sha(`release.json:${v}`) + sha(`release2.json:${v}`).slice(0, 24);
const intentId = (n: number) => `01JD${sha(`intent:${n}`).slice(0, 22).toUpperCase()}`;

const ENTRIES: Omit<DecodedEntry, "bound">[] = [
  {
    kind: "Genesis",
    body: {
      install: INSTALL,
      account: "acct_graftyard",
      evaluation: false,
      sites: ["weur", "enam", "apac"],
      keys: {
        generation: 0,
        customer: key("customer"),
        vendor: null,
        release: key("release"),
        drain: key("drain"),
        executor: key("executor"),
      },
      restore: null,
    },
    signatures: [signature("customer:genesis")],
  },
  {
    kind: "Keys",
    body: {
      generation: 1,
      customer: key("customer"),
      vendor: null,
      release: key("release"),
      drain: key("drain"),
      executor: key("executor"),
      agents_executor: key("agents_executor"),
      ring: [],
    },
    signatures: [signature("customer:keys1")],
  },
  {
    kind: "ReleaseUploaded",
    body: {
      release: releaseId("0.4.0"),
      versions: [
        ["AgentsFront", "7c1e…"],
        ["Controller", "9a42…"],
      ],
      reads: [{ family: 1, version: 3 }],
    },
    signatures: [signature("agents_executor:up1")],
  },
  {
    kind: "Intent",
    body: {
      id: intentId(1),
      issued_at: "2026-11-02T09:00:00Z",
      not_after: "2026-11-09T09:00:00Z",
      intent: { ControllerRoll: { release: releaseId("0.4.0") } },
      undo: [{ ControllerPointer: { pin: releaseId("0.3.2") } }],
    },
    signatures: [signature("customer:intent1")],
  },
  {
    kind: "IntentStep",
    body: { intent: intentId(1), step: 0 },
    signatures: [signature("agents_executor:step0")],
  },
  {
    kind: "IntentStep",
    body: { intent: intentId(1), step: 1 },
    signatures: [signature("agents_executor:step1")],
  },
  {
    kind: "ControllerPointer",
    body: {
      pin: releaseId("0.3.2"),
      reason: "graft-controller 1% gate: error rate 2.1% over 0.5%",
    },
    signatures: [signature("customer:undo1")],
  },
  {
    kind: "IntentDone",
    body: { intent: intentId(1) },
    signatures: [signature("agents_executor:done1")],
  },
  {
    kind: "Intent",
    body: {
      id: intentId(2),
      issued_at: "2026-11-06T09:00:00Z",
      not_after: "2026-11-13T09:00:00Z",
      intent: { Rotate: { secret: "AgentsAppKey", slot: 1 } },
      undo: [],
    },
    signatures: [signature("customer:intent2")],
  },
  {
    kind: "IntentStep",
    body: { intent: intentId(2), step: 0 },
    signatures: [signature("agents_executor:r0")],
  },
  { kind: "Halt", body: { intent: intentId(2) }, signatures: [signature("customer:halt2")] },
  {
    kind: "Resume",
    body: { intent: intentId(2), undo: [] },
    signatures: [signature("customer:resume2")],
  },
  {
    kind: "IntentDone",
    body: { intent: intentId(2) },
    signatures: [signature("agents_executor:done2")],
  },
];

export const releaseEntries: ReleaseEntry[] = ENTRIES.map((e, i) => {
  const prev = i === 0 ? null : sha(`digest:${i - 1}`) + sha(`digest2:${i - 1}`).slice(0, 24);
  const undo = e.kind === "ControllerPointer";
  return {
    index: i + 1,
    entry: encodeEntry({
      ...e,
      bound: {
        install: INSTALL,
        group: "Release",
        prev: undo ? null : prev,
        parent: undo ? intentId(1) : null,
      },
    }),
  };
});
// One entry in a format this console cannot read, to show the opaque case.
releaseEntries.push({
  index: releaseEntries.length + 1,
  entry: btoa(
    String.fromCharCode(0x47, 0x59, 0x52, 0x45, 2, ...new TextEncoder().encode("SiteMode v2")),
  ),
});
