import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "@/api/client";
import { gy } from "@/areas/agents/api";
import { stafftools } from "./stafftools-api";

// One factory per page region of the site admin console. Keys start with the REST path
// segments; private stafftools reads start with "_t", Graftyard's with "graftyard".

export const metaQuery = () =>
  queryOptions({
    queryKey: ["meta"],
    queryFn: ({ signal }) => unwrap(api.GET("/meta", { signal })),
  });

export const statsQuery = () =>
  queryOptions({
    queryKey: ["enterprise", "stats", "all"],
    queryFn: ({ signal }) => unwrap(api.GET("/enterprise/stats/all", { signal })),
  });

export const licenseQuery = () =>
  queryOptions({
    queryKey: ["enterprise", "settings", "license"],
    queryFn: ({ signal }) => unwrap(api.GET("/enterprise/settings/license", { signal })),
  });

export const announcementQuery = () =>
  queryOptions({
    queryKey: ["enterprise", "announcement"],
    queryFn: ({ signal }) => unwrap(api.GET("/enterprise/announcement", { signal })),
  });

// ---- Users ----

export const staffUsersQuery = () =>
  queryOptions({
    queryKey: ["_t", "stafftools", "users"],
    queryFn: ({ signal }) => unwrap(stafftools.GET("/users", { signal })),
  });

export const staffUserQuery = (username: string) =>
  queryOptions({
    queryKey: ["_t", "stafftools", "users", username],
    queryFn: ({ signal }) =>
      unwrap(stafftools.GET("/users/{username}", { params: { path: { username } }, signal })),
  });

export const keysQuery = () =>
  queryOptions({
    queryKey: ["admin", "keys"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/admin/keys", { params: { query: { per_page: 100, sort: "created" } }, signal }),
      ),
  });

export const tokensQuery = () =>
  queryOptions({
    queryKey: ["admin", "tokens"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/admin/tokens", { params: { query: { per_page: 100 } }, signal })),
  });

// ---- Organizations and repositories ----

export const staffOrgsQuery = () =>
  queryOptions({
    queryKey: ["_t", "stafftools", "orgs"],
    queryFn: ({ signal }) => unwrap(stafftools.GET("/orgs", { signal })),
  });

export const staffReposQuery = () =>
  queryOptions({
    queryKey: ["_t", "stafftools", "repos"],
    queryFn: ({ signal }) => unwrap(stafftools.GET("/repos", { signal })),
  });

export const repoPreReceiveQuery = (owner: string, repo: string) =>
  queryOptions({
    queryKey: ["repos", owner, repo, "pre-receive-hooks"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/repos/{owner}/{repo}/pre-receive-hooks", {
          params: { path: { owner, repo } },
          signal,
        }),
      ),
  });

// ---- Pre-receive ----

export const preReceiveHooksQuery = () =>
  queryOptions({
    queryKey: ["admin", "pre-receive-hooks"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/admin/pre-receive-hooks", {
          params: { query: { per_page: 100, sort: "name", direction: "asc" } },
          signal,
        }),
      ),
  });

export const preReceiveEnvsQuery = () =>
  queryOptions({
    queryKey: ["admin", "pre-receive-environments"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/admin/pre-receive-environments", {
          params: { query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const envDownloadQuery = (envId: number) =>
  queryOptions({
    queryKey: ["admin", "pre-receive-environments", envId, "downloads", "latest"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/admin/pre-receive-environments/{pre_receive_environment_id}/downloads/latest", {
          params: { path: { pre_receive_environment_id: envId } },
          signal,
        }),
      ),
  });

// ---- Global webhooks ----

export const hooksQuery = () =>
  queryOptions({
    queryKey: ["admin", "hooks"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/admin/hooks", { params: { query: { per_page: 100 } }, signal })),
  });

// A deleted hook reads as null, so its page says so instead of failing.
export const hookQuery = (hookId: number) =>
  queryOptions({
    queryKey: ["admin", "hooks", hookId],
    queryFn: async ({ signal }) => {
      const call = api.GET("/admin/hooks/{hook_id}", {
        params: { path: { hook_id: hookId } },
        signal,
      });
      return (await call).response.status === 404 ? null : unwrap(call);
    },
  });

export const hookDeliveriesQuery = (hookId: number) =>
  queryOptions({
    queryKey: ["_t", "stafftools", "hooks", hookId, "deliveries"],
    queryFn: ({ signal }) =>
      unwrap(
        stafftools.GET("/hooks/{hook_id}/deliveries", {
          params: { path: { hook_id: hookId } },
          signal,
        }),
      ),
  });

// ---- Migrations ----

export const orgMigrationsQuery = (org: string) =>
  queryOptions({
    queryKey: ["orgs", org, "migrations"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/orgs/{org}/migrations", {
          params: { path: { org }, query: { per_page: 100 } },
          signal,
        }),
      ),
  });

export const userMigrationsQuery = () =>
  queryOptions({
    queryKey: ["user", "migrations"],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/user/migrations", { params: { query: { per_page: 100 } }, signal })),
  });

export const liveMigrationsQuery = () =>
  queryOptions({
    queryKey: ["enterprise", "live-migrations"],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/enterprise/live-migrations", { params: { query: { status: "all" } }, signal }),
      ),
  });

export const liveMigrationQuery = (migrationId: string) =>
  queryOptions({
    queryKey: ["enterprise", "live-migrations", migrationId],
    queryFn: ({ signal }) =>
      unwrap(
        api.GET("/enterprise/live-migrations/{migration_id}", {
          params: { path: { migration_id: migrationId } },
          signal,
        }),
      ),
  });

// ---- Management console ----

export const clusterQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "cluster", "status"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/cluster/status", { signal })),
  });

export const replicationQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "replication", "status"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/replication/status", { signal })),
  });

export const nodesQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "config", "nodes"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/config/nodes", { signal })),
  });

export const versionQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "version"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/version", { signal })),
  });

export const maintenanceQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "maintenance"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/maintenance", { signal })),
  });

export const settingsQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "config", "settings"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/config/settings", { signal })),
  });

export const applyStatusQuery = (runId: string) =>
  queryOptions({
    queryKey: ["manage", "v1", "config", "apply", runId],
    queryFn: ({ signal }) =>
      unwrap(api.GET("/manage/v1/config/apply", { params: { query: { run_id: runId } }, signal })),
  });

export const upgradeDownloadQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "upgrade", "download", "status"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/upgrade/download/status", { signal })),
  });

export const upgradeStatusQuery = () =>
  queryOptions({
    queryKey: ["manage", "v1", "upgrade", "status"],
    queryFn: ({ signal }) => unwrap(api.GET("/manage/v1/upgrade/status", { signal })),
  });

// ---- Graftyard ----

export const installAppsQuery = () =>
  queryOptions({
    queryKey: ["_t", "stafftools", "apps"],
    queryFn: ({ signal }) => unwrap(stafftools.GET("/apps", { signal })),
  });

export const releaseEntriesQuery = () =>
  queryOptions({
    queryKey: ["graftyard", "release", "entries"],
    queryFn: ({ signal }) =>
      unwrap(gy.GET("/release/entries", { params: { query: { per_page: 100 } }, signal })),
  });
