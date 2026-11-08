import { HttpResponse } from "msw";
import type { Schemas } from "@/api/client";
import type { HookDelivery, StaffOrg, StaffUser } from "@/areas/admin/stafftools-api";
import { API, HOST, avatar, id, org, orgMembers, repositories, sha, user, users } from "../world";
import { db as core } from "./core-db";

// The site admin console's store: users and orgs as stafftools sees them, keys, tokens,
// global webhooks, pre-receive hooks and environments, and the announcement. Records are
// typed by the GHES 3.22 schema they are served as. Migrations, the management console,
// and the Graftyard rows live in admin-ops-db.ts.

type SimpleUser = Schemas["simple-user"];
const DOCS = "https://docs.github.com/enterprise-server@3.22/rest/enterprise-admin";

// GitHub's error body for the statuses http.ts has no helper for.
export const refuse = (status: 403 | 404 | 409 | 422, message: string) =>
  HttpResponse.json({ message, documentation_url: DOCS, status: String(status) }, { status });

export const isLogin = (s: string) => /^[a-z\d](?:[a-z\d]|-(?=[a-z\d])){0,38}$/i.test(s);

// ---- Users ----

function staff(
  u: SimpleUser,
  extra: Partial<StaffUser> & { email: string | null; created_at: string },
): StaffUser {
  return {
    ...u,
    name: u.name ?? null,
    suspended_at: null,
    suspension_reason: null,
    ldap_dn: null,
    ldap_sync: "never",
    ldap_synced_at: null,
    two_factor: true,
    public_repos: 0,
    ...extra,
  };
}

const people: StaffUser[] = [
  staff(users.mira, {
    site_admin: true,
    email: "mira@graftyard.test",
    created_at: "2025-03-02T09:00:00Z",
    public_repos: 3,
    ldap_dn: "uid=mira,ou=people,dc=graftyard,dc=test",
    ldap_sync: "synced",
    ldap_synced_at: "2026-11-08T06:00:00Z",
  }),
  staff(users.devi, {
    site_admin: true,
    email: "devi@graftyard.test",
    created_at: "2025-03-04T11:20:00Z",
    public_repos: 1,
  }),
  staff(users.tomas, {
    email: "tomas@graftyard.test",
    created_at: "2025-04-12T08:00:00Z",
    public_repos: 2,
  }),
  staff(users.lin, {
    email: "lin@graftyard.test",
    created_at: "2025-05-20T14:45:00Z",
    two_factor: false,
  }),
  staff(user("priya", "Priya Natarajan"), {
    email: "priya@graftyard.test",
    created_at: "2025-09-01T10:00:00Z",
  }),
  staff(user("jun", "Jun Park"), {
    email: "jun@graftyard.test",
    created_at: "2026-01-15T16:30:00Z",
    two_factor: false,
  }),
  staff(user("sam", "Sam Achterberg"), {
    email: "sam@contractor.test",
    created_at: "2025-06-02T09:10:00Z",
    suspended_at: "2026-09-30T17:00:00Z",
    suspension_reason: "Contract ended 2026-09-30.",
  }),
  staff(user("release-bot", "Release automation"), {
    email: null,
    created_at: "2025-03-10T00:00:00Z",
    two_factor: false,
  }),
];

export const viewer = "mira";

export function findUser(login: string) {
  return people.find((u) => u.login.toLowerCase() === login.toLowerCase());
}

export const listUsers = () => [...people].sort((a, b) => a.login.localeCompare(b.login));

export function createUser(login: string, email: string | null, suspended: boolean) {
  const u = staff(user(login, ""), {
    name: null,
    email,
    created_at: new Date().toISOString(),
    two_factor: false,
    suspended_at: suspended ? new Date().toISOString() : null,
    suspension_reason: suspended ? "Created suspended." : null,
  });
  people.push(u);
  deliver("user", "created", { user: simple(u) });
  return u;
}

export function renameUser(u: StaffUser, login: string) {
  const next = user(login, u.name ?? "");
  Object.assign(u, { ...next, id: u.id, node_id: u.node_id, site_admin: u.site_admin });
  deliver("user", "renamed", { user: simple(u) });
}

export function deleteUser(u: StaffUser) {
  people.splice(people.indexOf(u), 1);
  for (let i = keys.length - 1; i >= 0; i--) if (keys[i]!.user_id === u.id) keys.splice(i, 1);
  for (let i = tokens.length - 1; i >= 0; i--) {
    if (tokens[i]!.user?.login === u.login) tokens.splice(i, 1);
  }
  deliver("user", "deleted", { user: simple(u) });
}

export function simple(u: StaffUser): SimpleUser {
  const { suspended_at, suspension_reason, ldap_dn, ldap_sync, ldap_synced_at, two_factor, ...s } =
    u;
  void [suspended_at, suspension_reason, ldap_dn, ldap_sync, ldap_synced_at, two_factor];
  const { created_at, public_repos, ...rest } = s;
  void [created_at, public_repos];
  return rest;
}

// The full user GHES answers an LDAP mapping change with.
export function ldapUser(u: StaffUser): Schemas["ldap-mapping-user"] {
  return {
    ...simple(u),
    ldap_dn: u.ldap_dn ?? undefined,
    gravatar_id: u.gravatar_id,
    name: u.name,
    company: null,
    blog: null,
    location: null,
    email: u.email,
    hireable: null,
    bio: null,
    public_repos: u.public_repos,
    public_gists: 0,
    followers: 0,
    following: 0,
    created_at: u.created_at,
    updated_at: new Date().toISOString(),
    private_gists: 0,
    total_private_repos: 0,
    owned_private_repos: 0,
    disk_usage: 0,
    collaborators: 0,
    two_factor_authentication: u.two_factor,
    suspended_at: u.suspended_at,
  };
}

// An LDAP sync is queued, then lands a few seconds later.
const LDAP_SYNC_MS = 4000;
export function queueLdapSync(u: StaffUser) {
  u.ldap_sync = "queued";
  setTimeout(() => {
    if (!people.includes(u) || u.ldap_sync !== "queued") return;
    u.ldap_sync = "synced";
    u.ldap_synced_at = new Date().toISOString();
  }, LDAP_SYNC_MS);
}

// ---- SSH keys (user keys and deploy keys, as GET /admin/keys lists them) ----

function publicKey(
  title: string,
  owner: { user?: StaffUser; repo?: Schemas["repository"] },
  created: string,
  lastUsed: string | null,
  readOnly = false,
): Schemas["public-key-full"] {
  const n = id();
  const material = sha(`key:${title}`);
  return {
    id: n,
    key: `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI${btoa(material).slice(0, 43)}`,
    user_id: owner.user?.id ?? null,
    repository_id: owner.repo?.id ?? null,
    url: `${API}/user/keys/${n}`,
    title,
    read_only: readOnly,
    verified: true,
    created_at: created,
    added_by: owner.user?.login ?? "mira",
    last_used: lastUsed,
    enabled: true,
  };
}

const [mira, devi, tomas, lin] = [
  findUser("mira")!,
  findUser("devi")!,
  findUser("tomas")!,
  findUser("lin")!,
];
const edge = repositories[0]!;

const keys: Schemas["public-key-full"][] = [
  publicKey("mira@laptop", { user: mira }, "2025-03-02T09:30:00Z", "2026-11-08T08:12:00Z"),
  publicKey("mira@yubikey", { user: mira }, "2025-11-14T10:00:00Z", "2026-11-01T19:00:00Z"),
  publicKey("devi@workstation", { user: devi }, "2025-03-05T12:00:00Z", "2026-11-07T15:40:00Z"),
  publicKey("tomas@ci-runner", { user: tomas }, "2025-04-12T08:30:00Z", null),
  publicKey("lin@mbp", { user: lin }, "2025-05-21T09:00:00Z", "2026-10-30T11:00:00Z"),
  publicKey(
    "deploy: edge staging",
    { repo: edge },
    "2026-02-01T10:00:00Z",
    "2026-11-08T09:00:00Z",
    true,
  ),
];

export const listKeys = () => keys;
export function deleteKeys(ids: number[]) {
  const before = keys.length;
  for (let i = keys.length - 1; i >= 0; i--) if (ids.includes(keys[i]!.id)) keys.splice(i, 1);
  return before - keys.length;
}

// ---- Personal access tokens and impersonation tokens ----

const app = (name: string, client: string) => ({
  client_id: client,
  name,
  url: "https://docs.github.com/enterprise-server@3.22/rest/enterprise-admin/users",
});
const IMPERSONATION_APP = app("GitHub Site Administrator", "Iv1.site-admin-impersonation");

function token(
  u: StaffUser,
  note: string,
  scopes: string[],
  created: string,
  expires: string | null,
  appRef = app("Personal access token", "00000000000000000000"),
): Schemas["authorization"] {
  const n = id();
  const digest = sha(`token:${u.login}:${note}`);
  return {
    id: n,
    url: `${API}/authorizations/${n}`,
    scopes,
    token: "",
    token_last_eight: digest.slice(-8),
    hashed_token: digest,
    app: appRef,
    note,
    note_url: null,
    updated_at: created,
    created_at: created,
    fingerprint: null,
    user: simple(u),
    installation: null,
    expires_at: expires,
  };
}

const tokens: Schemas["authorization"][] = [
  token(mira, "gh cli", ["repo", "read:org", "workflow"], "2025-11-14T10:00:00Z", null),
  token(mira, "terraform", ["admin:org", "repo"], "2026-06-01T09:00:00Z", "2027-06-01T09:00:00Z"),
  token(devi, "release scripts", ["repo", "write:packages"], "2026-03-02T12:00:00Z", null),
  token(tomas, "local dev", ["repo"], "2026-08-20T08:00:00Z", "2026-11-20T08:00:00Z"),
  token(findUser("release-bot")!, "tag and publish", ["repo"], "2025-03-10T00:00:00Z", null),
  token(
    lin,
    "Site admin impersonation token",
    ["repo"],
    "2026-11-07T18:00:00Z",
    null,
    IMPERSONATION_APP,
  ),
];

export const listTokens = () => tokens;
export function deleteToken(tokenId: number) {
  const i = tokens.findIndex((t) => t.id === tokenId);
  if (i < 0) return false;
  tokens.splice(i, 1);
  return true;
}

export const impersonationFor = (u: StaffUser) =>
  tokens.find((t) => t.user?.login === u.login && t.app.client_id === IMPERSONATION_APP.client_id);

// GHES answers 200 with the existing token when the scopes match, 201 with a new one.
export function impersonate(u: StaffUser, scopes: string[]) {
  const existing = impersonationFor(u);
  if (existing && [...(existing.scopes ?? [])].sort().join() === [...scopes].sort().join()) {
    return { status: 200 as const, auth: existing };
  }
  if (existing) tokens.splice(tokens.indexOf(existing), 1);
  const created = token(
    u,
    "Site admin impersonation token",
    scopes,
    new Date().toISOString(),
    null,
    IMPERSONATION_APP,
  );
  tokens.push(created);
  // The token is shown once, on creation.
  return {
    status: 201 as const,
    auth: { ...created, token: `gho_${sha(`${created.id}`).slice(0, 36)}` },
  };
}

export function revokeImpersonation(u: StaffUser) {
  const t = impersonationFor(u);
  if (t) tokens.splice(tokens.indexOf(t), 1);
}

// ---- Organizations ----

function orgSimple(login: string, description: string | null): Schemas["organization-simple"] {
  const n = login === org.login ? org.id : id();
  const url = `${API}/orgs/${login}`;
  return {
    login,
    id: n,
    node_id: `O_${sha(`org:${login}`).slice(0, 12)}`,
    url,
    repos_url: `${url}/repos`,
    events_url: `${url}/events`,
    hooks_url: `${url}/hooks`,
    issues_url: `${url}/issues`,
    members_url: `${url}/members{/member}`,
    public_members_url: `${url}/public_members{/member}`,
    avatar_url: login === org.login ? org.avatar_url : avatar(login),
    description,
  };
}

const orgs: StaffOrg[] = [
  {
    ...orgSimple(org.login, org.description),
    name: org.name ?? null,
    created_at: org.created_at,
    admins: ["mira", "devi"],
    members: orgMembers.length,
    repositories: repositories.length,
  },
  {
    ...orgSimple("graftyard-agents", "Workspace forks for Graftyard-run agents."),
    name: "Graftyard agents",
    created_at: "2026-04-02T10:00:00Z",
    admins: ["mira"],
    members: 1,
    repositories: 0,
  },
  {
    ...orgSimple("field-ops", null),
    name: "Field operations",
    created_at: "2026-07-19T13:00:00Z",
    admins: ["devi"],
    members: 2,
    repositories: 0,
  },
];

export const listOrgs = () => [...orgs].sort((a, b) => a.login.localeCompare(b.login));
export const findOrg = (login: string) =>
  orgs.find((o) => o.login.toLowerCase() === login.toLowerCase());

export function createOrg(login: string, admin: string, profileName: string | undefined) {
  const o: StaffOrg = {
    ...orgSimple(login, null),
    name: profileName ?? null,
    created_at: new Date().toISOString(),
    admins: [admin],
    members: 1,
    repositories: 0,
  };
  orgs.push(o);
  deliver("organization", "created", { organization: orgSimple(login, null) });
  const { name, created_at, admins, members, repositories: r, ...wire } = o;
  void [name, created_at, admins, members, r];
  return wire;
}

export function renameOrg(o: StaffOrg, login: string) {
  Object.assign(o, { ...orgSimple(login, o.description), id: o.id, node_id: o.node_id });
  deliver("organization", "renamed", { organization: orgSimple(login, o.description) });
}

// ---- Announcement ----

export const announcement: Schemas["announcement"] = {
  announcement:
    "Scheduled maintenance **Saturday 14 November, 22:00 to 23:00 UTC**. Git pushes queue for up to 60 s while the `graftyard/raft` groups move. See the [runbook](https://graftyard.dev/runbook).",
  expires_at: "2026-11-15T00:00:00Z",
  user_dismissible: true,
};

// ---- Global webhooks and their deliveries ----

export const HOOK_EVENTS = ["user", "organization", "repository", "team", "membership", "member"];

type Hook = Schemas["global-hook"];
const hooks: Hook[] = [];
const deliveries = new Map<number, HookDelivery[]>();

export function makeHook(input: {
  name: string;
  config: { url: string; content_type?: string; secret?: string; insecure_ssl?: string };
  events?: string[];
  active?: boolean;
}): Hook {
  const n = id();
  const now = new Date().toISOString();
  const hook: Hook = {
    type: "Global",
    id: n,
    name: input.name,
    active: input.active ?? true,
    events: input.events?.length ? input.events : ["user", "organization"],
    config: {
      url: input.config.url,
      content_type: input.config.content_type ?? "json",
      insecure_ssl: input.config.insecure_ssl ?? "0",
      ...(input.config.secret ? { secret: "********" } : {}),
    },
    updated_at: now,
    created_at: now,
    url: `${API}/admin/hooks/${n}`,
    ping_url: `${API}/admin/hooks/${n}/pings`,
  };
  hooks.push(hook);
  deliveries.set(n, []);
  return hook;
}

makeHook({
  name: "web",
  config: { url: "https://audit.graftyard.test/ghes/hooks", secret: "s" },
  events: ["user", "organization", "repository", "team"],
});
makeHook({
  name: "web",
  config: { url: "https://chat-relay.graftyard.test/admin", insecure_ssl: "1" },
  events: ["user"],
  active: false,
});

export const listHooks = () => hooks;
export const findHook = (hookId: number) => hooks.find((h) => h.id === hookId);
export function deleteHook(h: Hook) {
  hooks.splice(hooks.indexOf(h), 1);
  deliveries.delete(h.id!);
}
export const hookDeliveries = (hookId: number) =>
  [...(deliveries.get(hookId) ?? [])].sort((a, b) => b.delivered_at.localeCompare(a.delivered_at));

// The chat relay host does not resolve, so its deliveries fail, as a real one would.
function respond(h: Hook): { code: number; status: string; body: string } {
  return h.config?.url?.includes("chat-relay")
    ? { code: 502, status: "Bad Gateway", body: "upstream connect error" }
    : { code: 200, status: "OK", body: '{"ok":true}' };
}

function record(h: Hook, event: string, action: string | null, payload: unknown) {
  const r = respond(h);
  const n = id();
  const guid = `${sha(`delivery:${n}`).slice(0, 8)}-${sha(`d2:${n}`).slice(0, 4)}-11ef-${sha(`d3:${n}`).slice(0, 4)}-${sha(`d4:${n}`).slice(0, 12)}`;
  deliveries.get(h.id!)?.push({
    id: n,
    guid,
    event,
    action,
    status_code: r.code,
    status: r.status,
    delivered_at: new Date().toISOString(),
    duration: Math.round(40 + Math.random() * 260) / 1000,
    request: {
      headers: {
        "Content-Type": `application/${h.config?.content_type ?? "json"}`,
        "User-Agent": "GitHub-Hookshot/3.22",
        "X-GitHub-Delivery": guid,
        "X-GitHub-Event": event,
        "X-GitHub-Hook-ID": String(h.id),
        "X-GitHub-Hook-Installation-Target-Type": "business",
        ...(h.config?.secret ? { "X-Hub-Signature-256": `sha256=${sha(`sig:${n}`)}` } : {}),
      },
      payload: action ? { action, ...(payload as object) } : payload,
    },
    response: {
      headers: { "Content-Type": "application/json", Server: "nginx" },
      body: r.body,
    },
  });
}

// Every active hook subscribed to the event gets a delivery, as GHES fans out global events.
export function deliver(event: string, action: string, payload: Record<string, unknown>) {
  for (const h of hooks) {
    if (h.active && h.events?.includes(event)) {
      record(h, event, action, {
        ...payload,
        sender: simple(mira),
        enterprise: { slug: "graftyard" },
      });
    }
  }
}

export function ping(h: Hook) {
  record(h, "ping", null, {
    zen: "Design for failure.",
    hook_id: h.id,
    hook: {
      type: "Global",
      id: h.id,
      name: h.name,
      active: h.active,
      events: h.events,
      config: h.config,
    },
  });
}

// ---- Pre-receive environments and hooks ----

type Env = Schemas["pre-receive-environment"];
type PreHook = Schemas["pre-receive-hook"];

function env(name: string, image: string | null, state: string, isDefault = false): Env {
  const n = id();
  return {
    id: n,
    name,
    image_url: image ?? "githubenterprise://internal",
    url: `${API}/admin/pre-receive-environments/${n}`,
    html_url: `${HOST}/admin/pre-receive-environments/${n}`,
    default_environment: isDefault,
    created_at: isDefault ? "2025-01-10T00:00:00Z" : "2026-03-12T10:00:00Z",
    hooks_count: 0,
    download: {
      url: `${API}/admin/pre-receive-environments/${n}/downloads/latest`,
      state,
      downloaded_at: state === "success" ? "2026-03-12T10:04:00Z" : null,
      message: null,
    },
  };
}

const envs: Env[] = [
  env("default", null, "success", true),
  env(
    "node-22-alpine",
    "https://artifacts.graftyard.test/pre-receive/node-22-alpine.tar.gz",
    "success",
  ),
  env(
    "python-3.13",
    "https://artifacts.graftyard.test/pre-receive/python-3.13.tar.gz",
    "not_started",
  ),
];

export const findEnv = (envId: number) => envs.find((e) => e.id === envId);
export function listEnvs() {
  for (const e of envs) e.hooks_count = preHooks.filter((h) => h.environment?.id === e.id).length;
  return envs;
}
export function createEnv(name: string, image: string) {
  const e = env(name, image, "not_started");
  e.created_at = new Date().toISOString();
  envs.push(e);
  return e;
}
export const deleteEnv = (e: Env) => void envs.splice(envs.indexOf(e), 1);

// not_started, then in_progress, then success; an image URL on a host that does not
// resolve ends in failed, with the message GHES shows.
const DOWNLOAD_MS = 6000;
export function startDownload(e: Env) {
  const d = (e.download ??= {});
  d.state = "in_progress";
  d.message = null;
  setTimeout(() => {
    if (!envs.includes(e)) return;
    if (e.image_url?.includes("unreachable")) {
      d.state = "failed";
      d.message = `Failed to download ${e.image_url}: getaddrinfo ENOTFOUND`;
    } else {
      d.state = "success";
      d.downloaded_at = new Date().toISOString();
    }
  }, DOWNLOAD_MS);
  return { ...d, state: "not_started" };
}

const repoRef = (r: Schemas["repository"]) => ({
  id: r.id,
  full_name: r.full_name,
  url: r.url,
  html_url: r.html_url,
});

const preHooks: PreHook[] = [];

export function makePreHook(input: {
  name: string;
  script: string;
  repo: Schemas["repository"];
  env: Env;
  enforcement?: string;
  allow_downstream_configuration?: boolean;
}) {
  const h: PreHook = {
    id: id(),
    name: input.name,
    enforcement: input.enforcement ?? "disabled",
    script: input.script,
    script_repository: repoRef(input.repo),
    environment: input.env,
    allow_downstream_configuration: input.allow_downstream_configuration ?? false,
  };
  preHooks.push(h);
  return h;
}

makePreHook({
  name: "block-large-files",
  script: "hooks/block_large_files.sh",
  repo: edge,
  env: envs[0]!,
  enforcement: "enabled",
  allow_downstream_configuration: true,
});
makePreHook({
  name: "require-signed-tags",
  script: "hooks/require_signed_tags.js",
  repo: edge,
  env: envs[1]!,
  enforcement: "testing",
  allow_downstream_configuration: true,
});
makePreHook({
  name: "no-plaintext-secrets",
  script: "hooks/no_secrets.py",
  repo: repositories[1]!,
  env: envs[2]!,
  enforcement: "disabled",
});

export const listPreHooks = () => preHooks;
export const findPreHook = (hookId: number) => preHooks.find((h) => h.id === hookId);
export function deletePreHook(h: PreHook) {
  preHooks.splice(preHooks.indexOf(h), 1);
}
export const repoForRef = (ref: { [key: string]: unknown } | undefined) => {
  const name = typeof ref?.full_name === "string" ? ref.full_name : undefined;
  return core.repos().find((r) => r.full_name === name);
};

// A repository's view of the instance hooks. Per-repository overrides are repo-admin's
// writes, so this serves the instance setting.
export function repoHooks(): Schemas["repository-pre-receive-hook"][] {
  return preHooks.map((h) => ({
    id: h.id,
    name: h.name,
    enforcement: h.enforcement,
    configuration_url: `${API}/admin/pre-receive-hooks/${h.id}`,
  }));
}

// ---- Stats and license, computed from the stores so every write shows up ----

export function stats(): Schemas["enterprise-overview"] {
  const states = core.repos().map((r) => core.state(r.owner.login, r.name)!);
  const issues = states.flatMap((s) => s.issues.filter((i) => !i.pull_request));
  const pulls = states.flatMap((s) => s.pulls);
  const comments = states.flatMap((s) => [...s.comments.values()].flat());
  const repos = core.repos();
  return {
    repos: {
      total_repos: repos.length,
      root_repos: repos.filter((r) => !r.fork).length,
      fork_repos: repos.filter((r) => r.fork).length,
      org_repos: repos.filter((r) => r.owner.type === "Organization").length,
      total_pushes: 18_412,
      total_wikis: repos.filter((r) => r.has_wiki).length,
    },
    hooks: {
      total_hooks: hooks.length,
      active_hooks: hooks.filter((h) => h.active).length,
      inactive_hooks: hooks.filter((h) => !h.active).length,
    },
    pages: { total_pages: repos.filter((r) => r.has_pages).length },
    orgs: {
      total_orgs: orgs.length,
      disabled_orgs: 0,
      total_teams: 3,
      total_team_members: 7,
    },
    users: {
      total_users: people.length,
      admin_users: people.filter((u) => u.site_admin).length,
      suspended_users: people.filter((u) => u.suspended_at).length,
    },
    pulls: {
      total_pulls: pulls.length,
      merged_pulls: pulls.filter((p) => p.merged_at).length,
      mergeable_pulls: pulls.filter((p) => p.state === "open" && p.mergeable !== false).length,
      unmergeable_pulls: pulls.filter((p) => p.mergeable === false).length,
    },
    issues: {
      total_issues: issues.length,
      open_issues: issues.filter((i) => i.state === "open").length,
      closed_issues: issues.filter((i) => i.state === "closed").length,
    },
    milestones: { total_milestones: 2, open_milestones: 1, closed_milestones: 1 },
    gists: { total_gists: 4, private_gists: 3, public_gists: 1 },
    comments: {
      total_commit_comments: 6,
      total_gist_comments: 1,
      total_issue_comments: comments.length,
      total_pull_request_comments: 9,
    },
  };
}

export const SEATS = 25;
export const LICENSE_EXPIRES = "2027-03-31T23:59:59Z";

export function license(): Schemas["license-info"] {
  const used = people.filter((u) => !u.suspended_at).length;
  return {
    seats: SEATS,
    seats_used: used,
    seats_available: SEATS - used,
    kind: "standard",
    days_until_expiration: Math.ceil((Date.parse(LICENSE_EXPIRES) - Date.now()) / 86_400_000),
    expire_at: LICENSE_EXPIRES,
  };
}
