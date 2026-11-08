import { createOpenApiHttp } from "openapi-msw";
import { STAFFTOOLS_BASE, type StafftoolsPaths } from "@/areas/admin/stafftools-api";
import { gate, http, notFound, page, unprocessable } from "../http";
import { API } from "../world";
import { installApps, staffRepos } from "./admin-ops-db";
import {
  announcement,
  createEnv,
  createOrg,
  createUser,
  deleteEnv,
  deleteHook,
  deleteKeys,
  deletePreHook,
  deleteToken,
  deleteUser,
  deliver,
  findEnv,
  findHook,
  findOrg,
  findPreHook,
  findUser,
  hookDeliveries,
  impersonate,
  isLogin,
  ldapUser,
  license,
  listEnvs,
  listHooks,
  listKeys,
  listOrgs,
  listPreHooks,
  listTokens,
  listUsers,
  makeHook,
  makePreHook,
  ping,
  queueLdapSync,
  refuse,
  renameOrg,
  renameUser,
  repoForRef,
  revokeImpersonation,
  simple,
  startDownload,
  stats,
  viewer,
} from "./admin-db";

// The site admin console's GHES operations: stats, license, announcement, users, orgs,
// keys, tokens, global webhooks, and pre-receive hooks, plus the private stafftools reads.
// Each write has its own fault key so the Mock panel can fail it alone.

const staff = createOpenApiHttp<StafftoolsPaths>({ baseUrl: STAFFTOOLS_BASE });
const queued = (what: string, url: string) => ({
  message: `Job queued to rename ${what}. It may take a few minutes to complete.`,
  url,
});

export const handlers = [
  // ---- Overview ----
  http.get("/enterprise/stats/all", async ({ response }) => {
    const fault = await gate("admin-stats");
    if (fault) return response.untyped(fault);
    return response(200).json(stats());
  }),

  http.get("/enterprise/settings/license", async ({ response }) => {
    const fault = await gate("admin-license");
    if (fault) return response.untyped(fault);
    return response(200).json(license());
  }),

  // ---- Announcement ----
  http.get("/enterprise/announcement", async ({ response }) => {
    const fault = await gate("admin-announcement");
    if (fault) return response.untyped(fault);
    return response(200).json(announcement);
  }),

  http.patch("/enterprise/announcement", async ({ request, response }) => {
    const fault = await gate("admin-announcement-set");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    if (!body.announcement?.trim()) {
      return response.untyped(unprocessable("Announcement", "announcement", "missing_field"));
    }
    if (body.expires_at && Date.parse(body.expires_at) <= Date.now()) {
      return response.untyped(unprocessable("Announcement", "expires_at", "invalid"));
    }
    announcement.announcement = body.announcement;
    announcement.expires_at = body.expires_at ?? null;
    announcement.user_dismissible = body.user_dismissible ?? false;
    return response(200).json(announcement);
  }),

  http.delete("/enterprise/announcement", async ({ response }) => {
    const fault = await gate("admin-announcement-clear");
    if (fault) return response.untyped(fault);
    announcement.announcement = null;
    announcement.expires_at = null;
    announcement.user_dismissible = null;
    return response(204).empty();
  }),

  // ---- Users ----
  staff.get("/users", async ({ response }) => {
    const fault = await gate("admin-users");
    if (fault) return response.untyped(fault);
    return response(200).json(listUsers());
  }),

  staff.get("/users/{username}", async ({ params, response }) => {
    const fault = await gate("admin-user");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    return u ? response(200).json(u) : response.untyped(notFound());
  }),

  http.post("/admin/users", async ({ request, response }) => {
    const fault = await gate("admin-user-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const login = body.login?.trim() ?? "";
    if (!login) return response.untyped(unprocessable("User", "login", "missing_field"));
    if (!isLogin(login)) return response.untyped(unprocessable("User", "login", "invalid"));
    if (findUser(login) || findOrg(login)) {
      return response.untyped(unprocessable("User", "login", "already_exists"));
    }
    if (body.email && !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(body.email)) {
      return response.untyped(unprocessable("User", "email", "invalid"));
    }
    if (body.email && listUsers().some((u) => u.email === body.email)) {
      return response.untyped(unprocessable("User", "email", "already_exists"));
    }
    return response(201).json(simple(createUser(login, body.email ?? null, !!body.suspended)));
  }),

  http.patch("/admin/users/{username}", async ({ params, request, response }) => {
    const fault = await gate("admin-user-rename");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    const { login } = await request.json();
    if (!isLogin(login ?? "")) return response.untyped(unprocessable("User", "login", "invalid"));
    if ((findUser(login) && findUser(login) !== u) || findOrg(login)) {
      return response.untyped(unprocessable("User", "login", "already_exists"));
    }
    renameUser(u, login);
    return response(202).json(queued("user", `${API}/users/${login}`));
  }),

  http.delete("/admin/users/{username}", async ({ params, response }) => {
    const fault = await gate("admin-user-delete");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    if (u.login === viewer)
      return response.untyped(refuse(403, "You cannot delete your own account."));
    deleteUser(u);
    return response(204).empty();
  }),

  http.put("/users/{username}/site_admin", async ({ params, response }) => {
    const fault = await gate("admin-site-admin");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    if (u.suspended_at) {
      return response.untyped(refuse(403, "A suspended user cannot be a site administrator."));
    }
    u.site_admin = true;
    deliver("user", "promoted_to_site_admin", { user: simple(u) });
    return response(204).empty();
  }),

  http.delete("/users/{username}/site_admin", async ({ params, response }) => {
    const fault = await gate("admin-site-admin");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    if (u.login === viewer) {
      return response.untyped(
        refuse(403, "You cannot demote yourself. Ask another site administrator."),
      );
    }
    u.site_admin = false;
    deliver("user", "demoted_from_site_admin", { user: simple(u) });
    return response(204).empty();
  }),

  http.put("/users/{username}/suspended", async ({ params, request, response }) => {
    const fault = await gate("admin-suspend");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    // GHES: any account except a site administrator may be suspended.
    if (u.site_admin) {
      return response.untyped(
        refuse(403, "You cannot suspend a site administrator. Demote them first."),
      );
    }
    const body = await request.json().catch(() => null);
    u.suspended_at = new Date().toISOString();
    u.suspension_reason = body?.reason?.trim() || null;
    deliver("user", "suspend", { user: simple(u), reason: u.suspension_reason });
    return response(204).empty();
  }),

  http.delete("/users/{username}/suspended", async ({ params, response }) => {
    const fault = await gate("admin-suspend");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    u.suspended_at = null;
    u.suspension_reason = null;
    deliver("user", "unsuspend", { user: simple(u) });
    return response(204).empty();
  }),

  http.post("/admin/users/{username}/authorizations", async ({ params, request, response }) => {
    const fault = await gate("admin-impersonate");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    const { scopes } = await request.json();
    if (!scopes?.length) {
      return response.untyped(unprocessable("OauthAccess", "scopes", "missing_field"));
    }
    const { status, auth } = impersonate(u, scopes);
    return status === 200 ? response(200).json(auth) : response(201).json(auth);
  }),

  http.delete("/admin/users/{username}/authorizations", async ({ params, response }) => {
    const fault = await gate("admin-impersonate");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    revokeImpersonation(u);
    return response(204).empty();
  }),

  http.patch("/admin/ldap/users/{username}/mapping", async ({ params, request, response }) => {
    const fault = await gate("admin-ldap");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    const { ldap_dn } = await request.json();
    if (!/^(\w+=[^,]+)(,\w+=[^,]+)+$/.test(ldap_dn?.trim() ?? "")) {
      return response.untyped(unprocessable("LdapMapping", "ldap_dn", "invalid"));
    }
    u.ldap_dn = ldap_dn.trim();
    u.ldap_sync = "never";
    u.ldap_synced_at = null;
    return response(200).json(ldapUser(u));
  }),

  http.post("/admin/ldap/users/{username}/sync", async ({ params, response }) => {
    const fault = await gate("admin-ldap-sync");
    if (fault) return response.untyped(fault);
    const u = findUser(params.username);
    if (!u) return response.untyped(notFound());
    if (!u.ldap_dn) return response.untyped(refuse(422, "This user has no LDAP mapping."));
    queueLdapSync(u);
    return response(201).json({ status: "queued" });
  }),

  // ---- SSH keys and tokens ----
  http.get("/admin/keys", async ({ query, response }) => {
    const fault = await gate("admin-keys");
    if (fault) return response.untyped(fault);
    const sort = query.get("sort") ?? "created";
    const field = sort === "accessed" ? "last_used" : "created_at";
    const dir = query.get("direction") === "asc" ? 1 : -1;
    const keys = [...listKeys()].sort(
      (a, b) => dir * (a[field] ?? "").localeCompare(b[field] ?? ""),
    );
    return response(200).json(page(keys, query));
  }),

  http.delete("/admin/keys/{key_ids}", async ({ params, response }) => {
    const fault = await gate("admin-key-delete");
    if (fault) return response.untyped(fault);
    const ids = params.key_ids.split(",").map(Number);
    return deleteKeys(ids) ? response(204).empty() : response.untyped(notFound());
  }),

  http.get("/admin/tokens", async ({ query, response }) => {
    const fault = await gate("admin-tokens");
    if (fault) return response.untyped(fault);
    return response(200).json(page(listTokens(), query));
  }),

  http.delete("/admin/tokens/{token_id}", async ({ params, response }) => {
    const fault = await gate("admin-token-delete");
    if (fault) return response.untyped(fault);
    return deleteToken(Number(params.token_id))
      ? response(204).empty()
      : response.untyped(notFound());
  }),

  // ---- Organizations ----
  staff.get("/orgs", async ({ response }) => {
    const fault = await gate("admin-orgs");
    if (fault) return response.untyped(fault);
    return response(200).json(listOrgs());
  }),

  http.post("/admin/organizations", async ({ request, response }) => {
    const fault = await gate("admin-org-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const login = body.login?.trim() ?? "";
    if (!login) return response.untyped(unprocessable("Organization", "login", "missing_field"));
    if (!isLogin(login)) return response.untyped(unprocessable("Organization", "login", "invalid"));
    if (findOrg(login) || findUser(login)) {
      return response.untyped(unprocessable("Organization", "login", "already_exists"));
    }
    const admin = findUser(body.admin ?? "");
    if (!admin) return response.untyped(unprocessable("Organization", "admin", "invalid"));
    if (admin.suspended_at)
      return response.untyped(unprocessable("Organization", "admin", "invalid"));
    return response(201).json(
      createOrg(login, admin.login, body.profile_name?.trim() || undefined),
    );
  }),

  http.patch("/admin/organizations/{org}", async ({ params, request, response }) => {
    const fault = await gate("admin-org-rename");
    if (fault) return response.untyped(fault);
    const o = findOrg(params.org);
    if (!o) return response.untyped(notFound());
    const { login } = await request.json();
    if (!isLogin(login ?? ""))
      return response.untyped(unprocessable("Organization", "login", "invalid"));
    if ((findOrg(login) && findOrg(login) !== o) || findUser(login)) {
      return response.untyped(unprocessable("Organization", "login", "already_exists"));
    }
    renameOrg(o, login);
    return response(202).json(queued("organization", `${API}/orgs/${login}`));
  }),

  // ---- Repositories (every repository on the instance, as a site admin sees them) ----
  staff.get("/repos", async ({ response }) => {
    const fault = await gate("admin-repos");
    if (fault) return response.untyped(fault);
    return response(200).json(staffRepos());
  }),

  // ---- Global webhooks ----
  http.get("/admin/hooks", async ({ query, response }) => {
    const fault = await gate("admin-hooks");
    if (fault) return response.untyped(fault);
    return response(200).json(page(listHooks(), query));
  }),

  http.get("/admin/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-hook");
    if (fault) return response.untyped(fault);
    const h = findHook(Number(params.hook_id));
    return h ? response(200).json(h) : response.untyped(notFound());
  }),

  http.post("/admin/hooks", async ({ request, response }) => {
    const fault = await gate("admin-hook-write");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const invalid = hookInvalid(body.config?.url, body.events);
    if (invalid) return response.untyped(invalid);
    return response(201).json(makeHook(body));
  }),

  http.patch("/admin/hooks/{hook_id}", async ({ params, request, response }) => {
    const fault = await gate("admin-hook-write");
    if (fault) return response.untyped(fault);
    const h = findHook(Number(params.hook_id));
    if (!h) return response.untyped(notFound());
    const body = await request.json();
    const invalid = hookInvalid(body.config?.url ?? h.config?.url, body.events ?? h.events);
    if (invalid) return response.untyped(invalid);
    if (body.config) {
      h.config = {
        url: body.config.url,
        content_type: body.config.content_type ?? h.config?.content_type,
        insecure_ssl: body.config.insecure_ssl ?? h.config?.insecure_ssl,
        ...(body.config.secret || h.config?.secret ? { secret: "********" } : {}),
      };
    }
    if (body.events) h.events = body.events;
    if (body.active !== undefined) h.active = body.active;
    h.updated_at = new Date().toISOString();
    const { secret, ...config } = h.config ?? {};
    void secret;
    return response(200).json({ ...h, config });
  }),

  http.delete("/admin/hooks/{hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-hook-delete");
    if (fault) return response.untyped(fault);
    const h = findHook(Number(params.hook_id));
    if (!h) return response.untyped(notFound());
    deleteHook(h);
    return response(204).empty();
  }),

  http.post("/admin/hooks/{hook_id}/pings", async ({ params, response }) => {
    const fault = await gate("admin-hook-ping");
    if (fault) return response.untyped(fault);
    const h = findHook(Number(params.hook_id));
    if (!h) return response.untyped(notFound());
    ping(h);
    return response(204).empty();
  }),

  staff.get("/hooks/{hook_id}/deliveries", async ({ params, response }) => {
    const fault = await gate("admin-hook-deliveries");
    if (fault) return response.untyped(fault);
    if (!findHook(Number(params.hook_id))) return response.untyped(notFound());
    return response(200).json(hookDeliveries(Number(params.hook_id)));
  }),

  // ---- Pre-receive environments ----
  http.get("/admin/pre-receive-environments", async ({ query, response }) => {
    const fault = await gate("admin-pre-receive-envs");
    if (fault) return response.untyped(fault);
    return response(200).json(page(listEnvs(), query));
  }),

  http.get(
    "/admin/pre-receive-environments/{pre_receive_environment_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-pre-receive-env");
      if (fault) return response.untyped(fault);
      const e = findEnv(Number(params.pre_receive_environment_id));
      return e ? response(200).json(e) : response.untyped(notFound());
    },
  ),

  http.post("/admin/pre-receive-environments", async ({ request, response }) => {
    const fault = await gate("admin-pre-receive-env-write");
    if (fault) return response.untyped(fault);
    const { name, image_url } = await request.json();
    const invalid = envInvalid(name, image_url);
    if (invalid) return response.untyped(invalid);
    return response(201).json(createEnv(name.trim(), image_url.trim()));
  }),

  http.patch(
    "/admin/pre-receive-environments/{pre_receive_environment_id}",
    async ({ params, request, response }) => {
      const fault = await gate("admin-pre-receive-env-write");
      if (fault) return response.untyped(fault);
      const e = findEnv(Number(params.pre_receive_environment_id));
      if (!e) return response.untyped(notFound());
      if (e.default_environment) {
        return response(422).json({
          message: "Validation Failed",
          errors: [
            {
              resource: "PreReceiveEnvironment",
              code: "custom",
              message: "Cannot modify or delete the default environment",
            },
          ],
        });
      }
      const body = (await request.json()) ?? {};
      const invalid = envInvalid(
        body.name ?? e.name ?? "",
        body.image_url ?? e.image_url ?? "",
        e.id,
      );
      if (invalid) return response.untyped(invalid);
      if (body.name) e.name = body.name.trim();
      if (body.image_url && body.image_url !== e.image_url) {
        e.image_url = body.image_url.trim();
        e.download = { ...e.download, state: "not_started", downloaded_at: null, message: null };
      }
      return response(200).json(e);
    },
  ),

  http.delete(
    "/admin/pre-receive-environments/{pre_receive_environment_id}",
    async ({ params, response }) => {
      const fault = await gate("admin-pre-receive-env-write");
      if (fault) return response.untyped(fault);
      const e = findEnv(Number(params.pre_receive_environment_id));
      if (!e) return response.untyped(notFound());
      const reason = e.default_environment
        ? "Cannot modify or delete the default environment"
        : listPreHooks().some((h) => h.environment?.id === e.id)
          ? "Cannot delete environment that has hooks"
          : e.download?.state === "in_progress"
            ? "Cannot delete environment when download is in progress"
            : undefined;
      if (reason) {
        return response(422).json({
          message: "Validation Failed",
          errors: [{ resource: "PreReceiveEnvironment", code: "custom", message: reason }],
        });
      }
      deleteEnv(e);
      return response(204).empty();
    },
  ),

  http.post(
    "/admin/pre-receive-environments/{pre_receive_environment_id}/downloads",
    async ({ params, response }) => {
      const fault = await gate("admin-pre-receive-download");
      if (fault) return response.untyped(fault);
      const e = findEnv(Number(params.pre_receive_environment_id));
      if (!e) return response.untyped(notFound());
      const reason = e.default_environment
        ? "Cannot modify or delete the default environment"
        : e.download?.state === "in_progress"
          ? "Can not start a new download when a download is in progress"
          : undefined;
      if (reason) {
        return response(422).json({
          message: "Validation Failed",
          errors: [{ resource: "PreReceiveEnvironment", code: "custom", message: reason }],
        });
      }
      return response(202).json(startDownload(e));
    },
  ),

  http.get(
    "/admin/pre-receive-environments/{pre_receive_environment_id}/downloads/latest",
    async ({ params, response }) => {
      const fault = await gate("admin-pre-receive-download-status");
      if (fault) return response.untyped(fault);
      const e = findEnv(Number(params.pre_receive_environment_id));
      return e?.download ? response(200).json(e.download) : response.untyped(notFound());
    },
  ),

  // ---- Pre-receive hooks ----
  http.get("/admin/pre-receive-hooks", async ({ query, response }) => {
    const fault = await gate("admin-pre-receive-hooks");
    if (fault) return response.untyped(fault);
    const sort = query.get("sort") ?? "created";
    const dir = query.get("direction") === "asc" ? 1 : -1;
    const hooks = [...listPreHooks()].sort((a, b) =>
      sort === "name"
        ? dir * (a.name ?? "").localeCompare(b.name ?? "")
        : dir * ((a.id ?? 0) - (b.id ?? 0)),
    );
    return response(200).json(page(hooks, query));
  }),

  http.get("/admin/pre-receive-hooks/{pre_receive_hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-pre-receive-hook");
    if (fault) return response.untyped(fault);
    const h = findPreHook(Number(params.pre_receive_hook_id));
    return h ? response(200).json(h) : response.untyped(notFound());
  }),

  http.post("/admin/pre-receive-hooks", async ({ request, response }) => {
    const fault = await gate("admin-pre-receive-hook-write");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const name = body.name?.trim() ?? "";
    if (!name) return response.untyped(unprocessable("PreReceiveHook", "name", "missing_field"));
    if (listPreHooks().some((h) => h.name === name)) {
      return response.untyped(unprocessable("PreReceiveHook", "name", "already_exists"));
    }
    if (!body.script?.trim())
      return response.untyped(unprocessable("PreReceiveHook", "script", "missing_field"));
    const repo = repoForRef(body.script_repository);
    if (!repo)
      return response.untyped(unprocessable("PreReceiveHook", "script_repository", "invalid"));
    const env = findEnv(Number(body.environment?.id));
    if (!env) return response.untyped(unprocessable("PreReceiveHook", "environment", "invalid"));
    if (!isEnforcement(body.enforcement)) {
      return response.untyped(unprocessable("PreReceiveHook", "enforcement", "invalid"));
    }
    return response(201).json(
      makePreHook({
        name,
        script: body.script.trim(),
        repo,
        env,
        enforcement: body.enforcement,
        allow_downstream_configuration: body.allow_downstream_configuration,
      }),
    );
  }),

  http.patch(
    "/admin/pre-receive-hooks/{pre_receive_hook_id}",
    async ({ params, request, response }) => {
      const fault = await gate("admin-pre-receive-hook-write");
      if (fault) return response.untyped(fault);
      const h = findPreHook(Number(params.pre_receive_hook_id));
      if (!h) return response.untyped(notFound());
      const body = (await request.json()) ?? {};
      if (body.name !== undefined) {
        const name = body.name.trim();
        if (!name)
          return response.untyped(unprocessable("PreReceiveHook", "name", "missing_field"));
        if (listPreHooks().some((o) => o !== h && o.name === name)) {
          return response.untyped(unprocessable("PreReceiveHook", "name", "already_exists"));
        }
        h.name = name;
      }
      if (body.enforcement !== undefined) {
        if (!isEnforcement(body.enforcement)) {
          return response.untyped(unprocessable("PreReceiveHook", "enforcement", "invalid"));
        }
        h.enforcement = body.enforcement;
      }
      if (body.script !== undefined) {
        if (!body.script.trim())
          return response.untyped(unprocessable("PreReceiveHook", "script", "missing_field"));
        h.script = body.script.trim();
      }
      if (body.script_repository) {
        const repo = repoForRef(body.script_repository);
        if (!repo)
          return response.untyped(unprocessable("PreReceiveHook", "script_repository", "invalid"));
        h.script_repository = {
          id: repo.id,
          full_name: repo.full_name,
          url: repo.url,
          html_url: repo.html_url,
        };
      }
      if (body.environment) {
        const env = findEnv(Number(body.environment.id));
        if (!env)
          return response.untyped(unprocessable("PreReceiveHook", "environment", "invalid"));
        h.environment = env;
      }
      if (body.allow_downstream_configuration !== undefined) {
        h.allow_downstream_configuration = body.allow_downstream_configuration;
      }
      return response(200).json(h);
    },
  ),

  http.delete("/admin/pre-receive-hooks/{pre_receive_hook_id}", async ({ params, response }) => {
    const fault = await gate("admin-pre-receive-hook-delete");
    if (fault) return response.untyped(fault);
    const h = findPreHook(Number(params.pre_receive_hook_id));
    if (!h) return response.untyped(notFound());
    deletePreHook(h);
    return response(204).empty();
  }),

  // The Apps on this install, for the agent apps page to pick from.
  staff.get("/apps", async ({ response }) => {
    const fault = await gate("admin-install-apps");
    if (fault) return response.untyped(fault);
    return response(200).json(installApps);
  }),
];

function hookInvalid(url: string | undefined, events: string[] | undefined) {
  if (!url?.trim()) return unprocessable("Hook", "config.url", "missing_field");
  try {
    const u = new URL(url);
    if (u.protocol !== "https:" && u.protocol !== "http:") throw new Error();
  } catch {
    return unprocessable("Hook", "config.url", "invalid");
  }
  if (events && events.length === 0) return unprocessable("Hook", "events", "missing_field");
  return undefined;
}

function envInvalid(name: string, imageUrl: string, self?: number) {
  if (!name?.trim()) return unprocessable("PreReceiveEnvironment", "name", "missing_field");
  if (listEnvs().some((e) => e.id !== self && e.name === name.trim())) {
    return unprocessable("PreReceiveEnvironment", "name", "already_exists");
  }
  if (!/^https?:\/\/\S+\.(tar\.gz|tgz)$/.test(imageUrl?.trim() ?? "")) {
    return unprocessable("PreReceiveEnvironment", "image_url", "invalid");
  }
  return undefined;
}

const isEnforcement = (s: string | undefined) =>
  s === undefined || s === "enabled" || s === "disabled" || s === "testing";
