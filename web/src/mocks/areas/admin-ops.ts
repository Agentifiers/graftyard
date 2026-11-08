import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import type { Schemas } from "@/api/client";
import { GRAFTYARD_BASE, type GraftyardAdminPaths } from "@/areas/admin/graftyard-api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { db as core } from "./core-db";
import { findOrg } from "./admin-db";
import {
  agentApps,
  applyEvents,
  applySettings,
  applyStatus,
  archiveBytes,
  archiveGone,
  clusterStatus,
  configNodes,
  createLive,
  deleteArchive,
  downloadReady,
  downloadStatus,
  findLive,
  findMigration,
  getMaintenance,
  installApps,
  installedVersion,
  isLocked,
  liveAct,
  liveDetail,
  listLive,
  listMigrations,
  migrationState,
  migrationView,
  releaseEntries,
  replicationStatus,
  setMaintenance,
  settings,
  startApply,
  startDownload,
  startMigration,
  startUpgrade,
  unlock,
  upgradeStatus,
  UPGRADE_TO,
  versions,
  type LiveAction,
} from "./admin-ops-db";

// Migrations, live migrations, the management console, and the two Graftyard site-admin
// paths. The management console's operations are served on /api/v3 like every other GHES
// path here; Graftyard maps them to the install's own admin API (design/SHARED.md, the
// `rest:enterprise-admin` row).

const graftyard = createOpenApiHttp<GraftyardAdminPaths>({ baseUrl: GRAFTYARD_BASE });
const DOCS = "https://docs.github.com/enterprise-server@3.22/rest";
const conflict = (message: string) =>
  HttpResponse.json({ message, documentation_url: DOCS, status: "409" }, { status: 409 });

const MIGRATION_OPTIONS = [
  "lock_repositories",
  "exclude_metadata",
  "exclude_git_data",
  "exclude_attachments",
  "exclude_releases",
  "exclude_owner_projects",
  "org_metadata_only",
] as const;

type MigrationBody = { repositories: string[] } & Partial<
  Record<(typeof MIGRATION_OPTIONS)[number], boolean>
> & {
    exclude?: "repositories"[];
  };

function migrationInput(owner: string, body: MigrationBody) {
  const names = body.repositories ?? [];
  if (!names.length && !body.org_metadata_only) {
    return { error: unprocessable("Migration", "repositories", "missing_field") };
  }
  const repos = names.map((n) =>
    core
      .repos()
      .find(
        (r) => r.full_name.toLowerCase() === (n.includes("/") ? n : `${owner}/${n}`).toLowerCase(),
      ),
  );
  if (repos.some((r) => !r))
    return { error: unprocessable("Migration", "repositories", "invalid") };
  const options = Object.fromEntries(MIGRATION_OPTIONS.map((k) => [k, !!body[k]])) as Record<
    (typeof MIGRATION_OPTIONS)[number],
    boolean
  >;
  return { repos: repos.map((r) => r!), options: { ...options, exclude: body.exclude } };
}

const strip = (m: ReturnType<typeof migrationView>, exclude: string | null) =>
  exclude === "repositories" ? { ...m, repositories: [] } : m;

const archive = (bytes: Uint8Array, guid: string) =>
  new HttpResponse(bytes, {
    status: 200,
    headers: {
      "Content-Type": "application/gzip",
      "Content-Disposition": `attachment; filename="migration-${guid}.tar.gz"`,
    },
  });

export const handlers = [
  // ---- Org migrations ----
  http.get("/orgs/{org}/migrations", async ({ params, query, response }) => {
    const fault = await gate("admin-migrations");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const list = listMigrations("org", params.org).map((m) =>
      strip(migrationView(m), query.get("exclude")),
    );
    return response(200).json(page(list, query));
  }),

  http.post("/orgs/{org}/migrations", async ({ params, request, response }) => {
    const fault = await gate("admin-migration-start");
    if (fault) return response.untyped(fault);
    if (!findOrg(params.org)) return response.untyped(notFound());
    const input = migrationInput(params.org, await request.json());
    if (input.error) return response.untyped(input.error);
    return response(201).json(
      migrationView(startMigration("org", params.org, input.repos, input.options)),
    );
  }),

  http.get("/orgs/{org}/migrations/{migration_id}", async ({ params, query, response }) => {
    const fault = await gate("admin-migration");
    if (fault) return response.untyped(fault);
    const m = findMigration("org", params.org, Number(params.migration_id));
    return m
      ? response(200).json(strip(migrationView(m), query.get("exclude")))
      : response.untyped(notFound());
  }),

  http.get("/orgs/{org}/migrations/{migration_id}/archive", async ({ params, response }) => {
    const fault = await gate("admin-migration-archive");
    if (fault) return response.untyped(fault);
    const m = findMigration("org", params.org, Number(params.migration_id));
    if (!m || archiveGone(m) || migrationState(m) !== "exported")
      return response.untyped(notFound());
    // GHES redirects to a signed storage URL; the mock serves the bytes directly.
    return response.untyped(archive(archiveBytes(m), m.record.guid));
  }),

  http.delete("/orgs/{org}/migrations/{migration_id}/archive", async ({ params, response }) => {
    const fault = await gate("admin-migration-archive-delete");
    if (fault) return response.untyped(fault);
    const m = findMigration("org", params.org, Number(params.migration_id));
    if (!m || archiveGone(m) || migrationState(m) !== "exported")
      return response.untyped(notFound());
    deleteArchive(m);
    return response(204).empty();
  }),

  http.get(
    "/orgs/{org}/migrations/{migration_id}/repositories",
    async ({ params, query, response }) => {
      const fault = await gate("admin-migration");
      if (fault) return response.untyped(fault);
      const m = findMigration("org", params.org, Number(params.migration_id));
      return m
        ? response(200).json(page(m.record.repositories.map(minimal), query))
        : response.untyped(notFound());
    },
  ),

  http.delete(
    "/orgs/{org}/migrations/{migration_id}/repos/{repo_name}/lock",
    async ({ params, response }) => {
      const fault = await gate("admin-migration-unlock");
      if (fault) return response.untyped(fault);
      const m = findMigration("org", params.org, Number(params.migration_id));
      if (
        !m ||
        !m.record.repositories.some((r) => r.name === params.repo_name) ||
        !isLocked(m, params.repo_name)
      ) {
        return response.untyped(notFound());
      }
      unlock(m, params.repo_name);
      return response(204).empty();
    },
  ),

  // ---- The viewer's own migrations ----
  http.get("/user/migrations", async ({ query, response }) => {
    const fault = await gate("admin-user-migrations");
    if (fault) return response.untyped(fault);
    return response(200).json(
      page(
        listMigrations("user", "mira").map((m) => migrationView(m)),
        query,
      ),
    );
  }),

  http.post("/user/migrations", async ({ request, response }) => {
    const fault = await gate("admin-user-migration-start");
    if (fault) return response.untyped(fault);
    const input = migrationInput("mira", await request.json());
    if (input.error) return response.untyped(input.error);
    return response(201).json(
      migrationView(startMigration("user", "mira", input.repos, input.options)),
    );
  }),

  http.get("/user/migrations/{migration_id}/archive", async ({ params, response }) => {
    const fault = await gate("admin-user-migration-archive");
    if (fault) return response.untyped(fault);
    const m = findMigration("user", "mira", Number(params.migration_id));
    if (!m || migrationState(m) !== "exported") return response.untyped(notFound());
    return response.untyped(archive(archiveBytes(m), m.record.guid));
  }),

  http.get("/user/migrations/{migration_id}/repositories", async ({ params, query, response }) => {
    const fault = await gate("admin-user-migrations");
    if (fault) return response.untyped(fault);
    const m = findMigration("user", "mira", Number(params.migration_id));
    return m
      ? response(200).json(page(m.record.repositories.map(minimal), query))
      : response.untyped(notFound());
  }),

  // ---- GHES live migrations ----
  http.get("/enterprise/live-migrations", async ({ query, response }) => {
    const fault = await gate("admin-live-migrations");
    if (fault) return response.untyped(fault);
    const list = listLive(query.get("status"));
    return response(200).json({ migrations: list, total_count: list.length });
  }),

  http.post("/enterprise/live-migrations", async ({ request, response }) => {
    const fault = await gate("admin-live-migration-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    if (
      !core
        .repos()
        .some(
          (r) =>
            r.owner.login === body.source_organization_login &&
            r.name === body.source_repository_name,
        )
    ) {
      return response.untyped(unprocessable("LiveMigration", "source_repository_name", "invalid"));
    }
    if (!/^[\w.-]+$/.test(body.target_repository_name ?? "")) {
      return response.untyped(unprocessable("LiveMigration", "target_repository_name", "invalid"));
    }
    if (!/^https:\/\/\S+$/.test(body.target_api_endpoint ?? "")) {
      return response.untyped(unprocessable("LiveMigration", "target_api_endpoint", "invalid"));
    }
    if (!body.pat_name?.trim())
      return response.untyped(unprocessable("LiveMigration", "pat_name", "missing_field"));
    const open = listLive("all").find(
      (l) =>
        l.source_repository_name === body.source_repository_name &&
        !["completed", "terminated", "failed"].includes(l.status ?? ""),
    );
    if (open)
      return response.untyped(
        conflict(`${body.source_repository_name} already has an open live migration.`),
      );
    const l = createLive(body);
    return response(201).json({ migration_id: l.row.migration_id, expires_at: l.row.expires_at });
  }),

  http.get("/enterprise/live-migrations/{migration_id}", async ({ params, response }) => {
    const fault = await gate("admin-live-migration");
    if (fault) return response.untyped(fault);
    const l = findLive(params.migration_id);
    return l ? response(200).json(liveDetail(l)) : response.untyped(notFound());
  }),

  ...(["start", "pause", "resume", "cancel"] as const).map((action) =>
    http.post(
      `/enterprise/live-migrations/{migration_id}/${action}`,
      async ({ params, response }) => {
        const fault = await gate(`admin-live-${action}`);
        if (fault) return response.untyped(fault);
        return response.untyped(act(params.migration_id, action));
      },
    ),
  ),

  http.post(
    "/enterprise/live-migrations/{migration_id}/cutover",
    async ({ params, request, response }) => {
      const fault = await gate("admin-live-cutover");
      if (fault) return response.untyped(fault);
      const body = await request.json().catch(() => undefined);
      return response.untyped(act(params.migration_id, "cutover", !!body?.force));
    },
  ),

  http.post(
    "/enterprise/live-migrations/{migration_id}/revert-cutover",
    async ({ params, response }) => {
      const fault = await gate("admin-live-revert");
      if (fault) return response.untyped(fault);
      const l = findLive(params.migration_id);
      if (!l) return response.untyped(notFound());
      const wasDone = liveDetail(l).migration?.status === "completed";
      const refusal = liveAct(l, "revert-cutover");
      if (refusal) return response.untyped(conflict(refusal));
      return response(200).json({
        success: true,
        unarchived_source_repository: wasDone,
        in_progress_cutover_terminated: !wasDone,
        in_progress_migration_terminated: false,
      });
    },
  ),

  // ---- Management console ----
  http.get("/manage/v1/cluster/status", async ({ response }) => {
    const fault = await gate("admin-manage-cluster");
    if (fault) return response.untyped(fault);
    return response(200).json(clusterStatus());
  }),

  http.get("/manage/v1/replication/status", async ({ response }) => {
    const fault = await gate("admin-manage-replication");
    if (fault) return response.untyped(fault);
    return response(200).json(replicationStatus());
  }),

  http.get("/manage/v1/config/nodes", async ({ response }) => {
    const fault = await gate("admin-manage-nodes");
    if (fault) return response.untyped(fault);
    return response(200).json(configNodes());
  }),

  http.get("/manage/v1/version", async ({ response }) => {
    const fault = await gate("admin-manage-version");
    if (fault) return response.untyped(fault);
    const v = installedVersion();
    return response(200).json(
      versions().map((n) => ({ ...n, version: { ...n.version, version: v } })),
    );
  }),

  http.get("/manage/v1/maintenance", async ({ response }) => {
    const fault = await gate("admin-manage-maintenance");
    if (fault) return response.untyped(fault);
    return response(200).json(getMaintenance());
  }),

  http.post("/manage/v1/maintenance", async ({ request, response }) => {
    const fault = await gate("admin-manage-maintenance-set");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    if (body.when && body.when !== "now" && Number.isNaN(Date.parse(body.when))) {
      return response.untyped(
        HttpResponse.json({ message: `invalid when: ${body.when}` }, { status: 400 }),
      );
    }
    if (body.ip_exception_list?.some((ip) => !/^[\d.:a-f/]+$/i.test(ip))) {
      return response.untyped(
        HttpResponse.json({ message: "invalid ip_exception_list" }, { status: 400 }),
      );
    }
    return response(200).json(setMaintenance(body));
  }),

  http.get("/manage/v1/config/settings", async ({ response }) => {
    const fault = await gate("admin-manage-settings");
    if (fault) return response.untyped(fault);
    return response(200).json(settings);
  }),

  // GHES types this body as {}; the mock reads the fields the page edits.
  http.put("/manage/v1/config/settings", async ({ request, response }) => {
    const fault = await gate("admin-manage-settings-set");
    if (fault) return response.untyped(fault);
    const raw: unknown = await request.clone().json();
    const patch = parseSettings(raw);
    if (typeof patch === "string")
      return response.untyped(HttpResponse.json({ message: patch }, { status: 400 }));
    applySettings(patch);
    return response(204).empty();
  }),

  http.post("/manage/v1/config/apply", async ({ request, response }) => {
    const fault = await gate("admin-manage-apply");
    if (fault) return response.untyped(fault);
    const body = await request.json().catch(() => undefined);
    const run = startApply(body?.run_id);
    if ("conflict" in run) {
      return response.untyped(
        HttpResponse.json(
          { message: `config apply ${run.conflict} is already running` },
          { status: 400 },
        ),
      );
    }
    return response(200).json({ run_id: run.runId });
  }),

  http.get("/manage/v1/config/apply", async ({ query, response }) => {
    const fault = await gate("admin-manage-apply-status");
    if (fault) return response.untyped(fault);
    return response(200).json(applyStatus(query.get("run_id")));
  }),

  http.get("/manage/v1/config/apply/events", async ({ query, response }) => {
    const fault = await gate("admin-manage-apply-events");
    if (fault) return response.untyped(fault);
    return response(200).json(applyEvents(query.get("last_request_id")));
  }),

  http.post("/manage/v1/upgrade/download", async ({ request, response }) => {
    const fault = await gate("admin-manage-upgrade-download");
    if (fault) return response.untyped(fault);
    const body = await request.json().catch(() => undefined);
    const version = body?.version ?? UPGRADE_TO;
    if (version === installedVersion()) {
      return response.untyped(
        HttpResponse.json({ message: `${version} is already installed` }, { status: 400 }),
      );
    }
    const started = startDownload(version);
    if ("conflict" in started) {
      return response(409).json({
        error: { message: "a download is already in progress", request_id: started.conflict },
      });
    }
    return response(202).json({
      message: "download started",
      download_id: started.downloadId,
      max_parallel: body?.max_parallel ?? 2,
      total_nodes: 2,
    });
  }),

  http.get("/manage/v1/upgrade/download/status", async ({ response }) => {
    const fault = await gate("admin-manage-upgrade-status");
    if (fault) return response.untyped(fault);
    return response(200).json(downloadStatus());
  }),

  http.post("/manage/v1/upgrade/apply", async ({ response }) => {
    const fault = await gate("admin-manage-upgrade-apply");
    if (fault) return response.untyped(fault);
    if (!downloadReady()) {
      return response.untyped(
        HttpResponse.json({ message: "no upgrade package has been downloaded" }, { status: 400 }),
      );
    }
    startUpgrade();
    return response(200).json(
      upgradeStatus().map((n) => ({
        hostname: n.hostname,
        uuid: n.uuid,
        message: "upgrade started",
      })),
    );
  }),

  http.get("/manage/v1/upgrade/status", async ({ response }) => {
    const fault = await gate("admin-manage-upgrade-status");
    if (fault) return response.untyped(fault);
    return response(200).json(upgradeStatus());
  }),

  // ---- Graftyard: agent apps ----
  graftyard.get("/agents/apps", async ({ response }) => {
    const fault = await gate("admin-agent-apps");
    if (fault) return response.untyped(fault);
    return response(200).json([...agentApps].sort((a, b) => a.app_id - b.app_id));
  }),

  graftyard.put("/agents/apps/{app_id}", async ({ params, request, response }) => {
    const fault = await gate("admin-agent-app-put");
    if (fault) return response.untyped(fault);
    const appId = Number(params.app_id);
    const app = installApps.find((a) => a.app_id === appId);
    if (!app) return response.untyped(notFound());
    const body = await request.json();
    if (!["graftyard_run", "third_party", "mcp"].includes(body.kind)) {
      return response(422).json(error("AgentApp", "kind", "invalid"));
    }
    if (body.kind !== "third_party" && !app.owned) {
      return response(422).json({
        ...error("AgentApp", "kind", "invalid"),
        message: `${body.kind} is only for an App this install owns. ${app.slug} is not one.`,
      });
    }
    const confine = body.confine?.trim() || null;
    if (confine && !/^refs\/heads\/[\w./*-]+$/.test(confine)) {
      return response(422).json(error("AgentApp", "confine", "invalid"));
    }
    const row = { app_id: appId, slug: app.slug, kind: body.kind, confine };
    const i = agentApps.findIndex((a) => a.app_id === appId);
    if (i >= 0) agentApps[i] = row;
    else agentApps.push(row);
    return response(200).json(row);
  }),

  graftyard.delete("/agents/apps/{app_id}", async ({ params, response }) => {
    const fault = await gate("admin-agent-app-delete");
    if (fault) return response.untyped(fault);
    const i = agentApps.findIndex((a) => a.app_id === Number(params.app_id));
    if (i < 0) return response(404).json({ message: "Not Found", documentation_url: DOCS });
    agentApps.splice(i, 1);
    return response(204).empty();
  }),

  // ---- Graftyard: the release log ----
  graftyard.get("/release/entries", async ({ query, response }) => {
    const fault = await gate("admin-release");
    if (fault) return response.untyped(fault);
    const after = Number(query.get("after")) || 0;
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    const rest = releaseEntries.filter((e) => e.index > after);
    const items = rest.slice(0, perPage);
    const headers: Record<string, string> = {};
    if (rest.length > items.length) {
      headers.Link = `<${GRAFTYARD_BASE}/release/entries?after=${items.at(-1)!.index}&per_page=${perPage}>; rel="next"`;
    }
    return response(200).json(items, { headers });
  }),
];

// GHES lists a migration's repositories as minimal-repository, whose license has no nulls.
function minimal(r: Schemas["repository"]): Schemas["minimal-repository"] {
  const { license, ...rest } = r;
  return {
    ...rest,
    license: license && {
      key: license.key,
      name: license.name,
      url: license.url,
      node_id: license.node_id,
      spdx_id: license.spdx_id ?? undefined,
    },
  };
}

function act(migrationId: string, action: LiveAction, force = false) {
  const l = findLive(migrationId);
  if (!l) return notFound();
  const refusal = liveAct(l, action, force);
  return refusal ? conflict(refusal) : new HttpResponse(null, { status: 204 });
}

const error = (resource: string, field: string, code: "invalid" | "missing_field") => ({
  message: "Validation Failed",
  documentation_url: DOCS,
  errors: [{ resource, field, code }],
});

// The settings this console edits, parsed from the untyped body.
function parseSettings(raw: unknown): Partial<typeof settings> | string {
  if (!raw || typeof raw !== "object") return "body must be an object";
  const r = raw as Record<string, unknown>;
  const out: Partial<typeof settings> = {};
  for (const k of [
    "private_mode",
    "public_pages",
    "subdomain_isolation",
    "signup_enabled",
  ] as const) {
    if (k in r) {
      if (typeof r[k] !== "boolean") return `${k} must be a boolean`;
      out[k] = r[k];
    }
  }
  if ("github_hostname" in r) {
    if (
      typeof r.github_hostname !== "string" ||
      !/^[a-z\d.-]+\.[a-z]{2,}$/i.test(r.github_hostname)
    ) {
      return "github_hostname must be a hostname";
    }
    out.github_hostname = r.github_hostname;
  }
  if ("timezone" in r) {
    if (typeof r.timezone !== "string") return "timezone must be a string";
    out.timezone = r.timezone;
  }
  return out;
}
