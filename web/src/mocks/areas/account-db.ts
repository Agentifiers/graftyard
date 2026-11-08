import type { Schemas } from "@/api/client";
import { canSign, parseGpgKey, parseSshKey } from "@/areas/account/keys";
import type {
  AppKey,
  AppManifest,
  AuthorizedApp,
  ClientSecret,
  DeveloperApp,
  EmailVisibility,
  NotificationSettings,
  OAuthApp,
  Security,
  SecurityEvent,
  Session,
  Token,
} from "@/areas/account/ui-api";
import { actionsApp, API, avatar, edge, HOST, id, me, raft, sha, user, users, web } from "../world";
import { db as core } from "./core-db";

// The viewer's account: emails, keys, tokens, two-factor, sessions, the security log,
// authorized apps, the apps mira develops, installations, and the sign-in flows' codes.
// Handlers live in account.ts (GHES REST) and account-ui.ts (/_t/ui). Every write appends
// to the security log, so the log page shows what the other pages did.

type Email = Schemas["email"];
type Key = Schemas["key"];
type SigningKey = Schemas["ssh-signing-key"];
type GpgKey = Schemas["gpg-key"];
type Grant = Schemas["application-grant"];
type Installation = Schemas["installation"];
type Integration = NonNullable<Schemas["integration"]>;
type SimpleUser = Schemas["simple-user"];
type Delivery = Schemas["hook-delivery"];

const now = () => new Date().toISOString();
const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000).toISOString();

// ---- Random values in the formats the design gives ----

const BASE62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

function random(alphabet: string, n: number) {
  const bytes = crypto.getRandomValues(new Uint8Array(n));
  return [...bytes].map((b) => alphabet[b % alphabet.length]).join("");
}

function crc32(text: string) {
  let crc = ~0;
  for (const ch of text) {
    crc ^= ch.charCodeAt(0);
    for (let k = 0; k < 8; k++) crc = (crc >>> 1) ^ (0xedb88320 & -(crc & 1));
  }
  return ~crc >>> 0;
}

// A stored token (`design/SHARED.md`, "Identity and tokens"): the prefix, 30 base62
// characters, and their CRC32 in 6 base62 characters, zero padded, as
// crates/identity/src/tokens.rs draws it. ghp_ tokens are 40 characters, github_pat_ 47.
export function storedToken(prefix: "ghp_" | "github_pat_" | "gho_" | "ghu_") {
  const secret = random(BASE62, 30);
  let v = crc32(secret);
  let crc = "";
  for (let k = 0; k < 6; k++) {
    crc = BASE62[v % 62] + crc;
    v = Math.floor(v / 62);
  }
  return `${prefix}${secret}${crc}`;
}

export const hexSecret = (n: number) => random("0123456789abcdef", n);
export const lastEight = (s: string) => s.slice(-8);

// Device-flow user codes: 8 characters from a vowel-free alphabet, shown as XXXX-XXXX.
const USER_CODE = "BCDFGHJKLMNPQRSTVWXZ23456789";
export const userCode = () => `${random(USER_CODE, 4)}-${random(USER_CODE, 4)}`;

// ---- Security log ----

const securityLog: SecurityEvent[] = [];

export function audit(action: string, detail: string) {
  securityLog.unshift({
    id: id(),
    action,
    actor: me.login,
    created_at: now(),
    ip: "198.51.100.23",
    location: "Lisbon, Portugal",
    detail,
  });
}

const SEED_LOG: [action: string, detail: string, minutesAgo: number][] = [
  ["user.login", "Signed in with a password and an authenticator app", 40],
  ["oauth_authorization.create", "Authorized GitHub CLI (repo, read:org, gist)", 60 * 26],
  ["public_key.create", "Added SSH key mira-laptop", 60 * 24 * 3],
  ["personal_access_token.create", "Created fine-grained token edge-deploy", 60 * 24 * 4],
  ["user.login", "Signed in from Firefox on macOS", 60 * 24 * 4 + 30],
  ["integration_installation.create", "Installed Graftyard Review on graftyard", 60 * 24 * 7],
  ["user.two_factor_requested", "Two-factor code requested", 60 * 24 * 9],
  ["gpg_key.create", "Added GPG key 683B7E44DB633D30", 60 * 24 * 12],
  ["user.change_password", "Changed password", 60 * 24 * 20],
  ["oauth_application.create", "Registered OAuth app Release Notes", 60 * 24 * 30],
  ["integration.create", "Registered GitHub App Release Notes Bot", 60 * 24 * 31],
  ["user.two_factor_enable", "Enabled two-factor authentication with an app", 60 * 24 * 60],
];
for (const [action, detail, m] of SEED_LOG) {
  securityLog.push({
    id: id(),
    action,
    actor: "mira",
    created_at: ago(m),
    ip: m > 60 * 24 * 4 ? "203.0.113.51" : "198.51.100.23",
    location: m > 60 * 24 * 4 ? "Porto, Portugal" : "Lisbon, Portugal",
    detail,
  });
}
// Older sign-ins, so the log pages.
for (let day = 61; day < 120; day += 2) {
  securityLog.push({
    id: id(),
    action: day % 6 === 1 ? "user.failed_login" : "user.login",
    actor: "mira",
    created_at: ago(60 * 24 * day),
    ip: "198.51.100.23",
    location: "Lisbon, Portugal",
    detail:
      day % 6 === 1 ? "Incorrect password" : "Signed in with a password and an authenticator app",
  });
}

// ---- Emails ----

const emails: Email[] = [
  { email: "mira@graftyard.test", primary: true, verified: true, visibility: "public" },
  { email: "mira@okafor.dev", primary: false, verified: true, visibility: null },
  { email: "m.okafor@lisbon.example", primary: false, verified: false, visibility: null },
];
const emailVisibility: EmailVisibility = { visibility: "public", block_push: false };

// Addresses other accounts hold. Adding one is a 422.
const otherEmails = new Set(["tomas@graftyard.test", "lin@graftyard.test", "devi@graftyard.test"]);

// ---- SSH and GPG keys ----

// Keys other accounts hold. The design's rule: registering a key another account already
// holds returns 422 (`design/SHARED.md`, "Surfaces, tiers, and oracles", git-ssh).
const heldElsewhere = new Map<string, string>([
  ["AAAAC3NzaC1lZDI1NTE5AAAAIJe2W0br55VeXPqm8gPHmu1uFJ0RXq1PLv3ojflaGW4C", "tomas"],
  ["AAAAC3NzaC1lZDI1NTE5AAAAIKU5MOrJNTAXC2/IBhqXjjESdoF04y3FHpPQqbvrEBFE", "devi"],
]);
const gpgHeldElsewhere = new Map<string, string>([
  ["C4773C66FAEBC8013BA18156FC9982F375236A5F", "lin"],
]);

function key(title: string, text: string, created: string, lastUsed: string | null): Key {
  const n = id();
  return {
    id: n,
    key: text,
    url: `${API}/user/keys/${n}`,
    title,
    created_at: created,
    verified: true,
    read_only: false,
    last_used: lastUsed,
  };
}

const sshKeys: Key[] = [
  key(
    "mira-laptop",
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMnBg/bU+epJ0435THIhnLfbEWfUwTV/W3DxRTNRbhDS",
    ago(60 * 24 * 3),
    ago(55),
  ),
];
const signingKeys: SigningKey[] = [
  {
    id: id(),
    key: "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICDuiRlkiH9tWPjecv2gTCWFbv/m6KMwavA+5BRuQE1B",
    title: "mira-signing",
    created_at: ago(60 * 24 * 3),
  },
];

const MIRA_GPG = `-----BEGIN PGP PUBLIC KEY BLOCK-----

mDMEavDDuRYJKwYBBAHaRw8BAQdApH1P0Ycqb3lDcJjiHMf+SPvKvl7GCy+9G7V0
CNpRwLG0IU1pcmEgT2thZm9yIDxtaXJhQGdyYWZ0eWFyZC50ZXN0PoiWBBMWCgA+
FiEEYkb+Y2xszaaidVEiaDt+RNtjPTAFAmrww7kCGwMFCQPCZwAFCwkIBwIGFQoJ
CAsCBBYCAwECHgECF4AACgkQaDt+RNtjPTCzpwD8Cp4p6C4XwCiEZmY7mGAX36Nk
uvDgJMnTjhx6sILqtawBALO2e+1lDTve9YciNTgcJ2h7/iB8W7dqC91yJcjdqckF
=hQj6
-----END PGP PUBLIC KEY BLOCK-----`;

const gpgKeys: GpgKey[] = [];
const gpgFingerprints = new Map<number, string>();

export async function addGpgKey(name: string | null, armored: string): Promise<GpgKey | string> {
  const info = await parseGpgKey(armored);
  if ("error" in info) return info.error;
  if (
    gpgHeldElsewhere.has(info.fingerprint) ||
    [...gpgFingerprints.values()].includes(info.fingerprint)
  ) {
    return "key is already in use";
  }
  const verified = new Set(emails.filter((e) => e.verified).map((e) => e.email));
  const n = id();
  const gpg: GpgKey = {
    id: n,
    name,
    primary_key_id: null,
    key_id: info.keyId,
    public_key: armored.trim(),
    emails: info.emails.map((email) => ({ email, verified: verified.has(email) })),
    subkeys: info.subkeys.map((s) => ({
      id: id(),
      primary_key_id: n,
      key_id: s.keyId,
      public_key: "",
      emails: [],
      subkeys: [],
      can_sign: canSign(s.algorithm),
      can_encrypt_comms: !canSign(s.algorithm),
      can_encrypt_storage: !canSign(s.algorithm),
      can_certify: false,
      created_at: s.createdAt,
      expires_at: null,
      raw_key: null,
      revoked: false,
    })),
    can_sign: canSign(info.algorithm),
    can_encrypt_comms: false,
    can_encrypt_storage: false,
    can_certify: true,
    created_at: info.createdAt,
    expires_at: null,
    revoked: false,
    raw_key: armored.trim(),
  };
  gpgKeys.push(gpg);
  gpgFingerprints.set(n, info.fingerprint);
  return gpg;
}
void addGpgKey("mira@graftyard.test", MIRA_GPG);

// An SSH key line, checked against both of mira's lists and every other account's.
export function sshKeyProblem(
  text: string,
  list: "authentication" | "signing",
): string | undefined {
  const parsed = parseSshKey(text);
  if ("error" in parsed) return parsed.error;
  if (heldElsewhere.has(parsed.base64)) return "key is already in use";
  const mine = list === "authentication" ? sshKeys : signingKeys;
  if (mine.some((k) => k.key.split(/\s+/)[1] === parsed.base64)) {
    return "key is already in use";
  }
  return undefined;
}

export const normalizeKey = (text: string) => text.trim().split(/\s+/).slice(0, 2).join(" ");

// ---- Personal access tokens ----

const tokens: Token[] = [
  {
    kind: "fine_grained",
    id: id(),
    name: "edge-deploy",
    description: "Pushes release tags from the deploy runner.",
    resource_owner: "graftyard",
    repository_selection: "selected",
    repositories: ["graftyard/edge"],
    permissions: { contents: "write", metadata: "read", deployments: "write" },
    token_last_eight: "k2Hq9ZtM",
    created_at: ago(60 * 24 * 4),
    expires_at: new Date(Date.now() + 86_400_000 * 26).toISOString(),
    last_used_at: ago(180),
  },
  {
    kind: "fine_grained",
    id: id(),
    name: "raft-bench",
    description: "",
    resource_owner: "graftyard",
    repository_selection: "selected",
    repositories: ["graftyard/raft"],
    permissions: { contents: "read", metadata: "read", issues: "write" },
    token_last_eight: "Wq0pX7aD",
    created_at: ago(60 * 24 * 40),
    expires_at: ago(60 * 24 * 10),
    last_used_at: ago(60 * 24 * 11),
  },
  {
    kind: "classic",
    id: id(),
    note: "laptop git",
    scopes: ["repo", "workflow", "read:org"],
    token_last_eight: "Rr81sLkU",
    created_at: ago(60 * 24 * 90),
    expires_at: null,
    last_used_at: ago(60 * 5),
  },
  {
    kind: "classic",
    id: id(),
    note: "renovate self-hosted",
    scopes: ["repo"],
    token_last_eight: "uN4cB0eF",
    created_at: ago(60 * 24 * 20),
    expires_at: new Date(Date.now() + 86_400_000 * 5).toISOString(),
    last_used_at: null,
  },
];

// ---- Two-factor, sessions ----

export const security: Security = {
  two_factor: {
    enabled: me.two_factor_authentication,
    method: me.two_factor_authentication ? "app" : null,
    enabled_at: ago(60 * 24 * 60),
    recovery_codes_generated_at: ago(60 * 24 * 60),
  },
  password_changed_at: ago(60 * 24 * 20),
};
export let recoveryCodes: string[] = makeRecoveryCodes();
export function makeRecoveryCodes() {
  return Array.from({ length: 16 }, () => `${hexSecret(5)}-${hexSecret(5)}`);
}
export function setRecoveryCodes(codes: string[]) {
  recoveryCodes = codes;
}
// The secret a setup handed out, until the person confirms it with a code.
export const pendingTwoFactor = { secret: "" };

const sessions: Session[] = [
  {
    id: id(),
    kind: "web",
    device: "Chrome on Linux",
    location: "Lisbon, Portugal",
    ip: "198.51.100.23",
    signed_in_at: ago(40),
    last_active_at: now(),
    current: true,
  },
  {
    id: id(),
    kind: "web",
    device: "Firefox on macOS",
    location: "Porto, Portugal",
    ip: "203.0.113.51",
    signed_in_at: ago(60 * 24 * 4 + 30),
    last_active_at: ago(60 * 26),
    current: false,
  },
  {
    id: id(),
    kind: "mobile",
    device: "GitHub Mobile on iPhone",
    location: "Lisbon, Portugal",
    ip: "198.51.100.77",
    signed_in_at: ago(60 * 24 * 15),
    last_active_at: ago(60 * 3),
    current: false,
  },
];

// ---- Apps: GitHub Apps on the instance, mira's own, and OAuth apps ----

const renovateOrg = user("renovatebot", "Renovate", "Organization");
const owners: Record<string, SimpleUser> = {
  mira: users.mira,
  graftyard: users.org,
  renovatebot: renovateOrg,
  tomas: users.tomas,
};
const ownerOf = (login: string) => owners[login] ?? users.mira;
const appOwner = (u: SimpleUser) => ({ login: u.login, avatar_url: u.avatar_url });

type AppRecord = DeveloperApp & {
  webhook_secret: string | null;
  client_secrets: ClientSecret[];
  deliveries: Delivery[];
};

function appRecord(
  slug: string,
  name: string,
  owner: SimpleUser,
  m: Omit<AppManifest, "name" | "hook_attributes"> & { hook: { active: boolean; url: string } },
  created: string,
): AppRecord {
  const appId = parseInt(sha(`app:${slug}`).slice(0, 5), 16);
  return {
    id: appId,
    slug,
    name,
    client_id: `Iv23li${sha(`client:${slug}`).slice(0, 14)}`,
    owner: appOwner(owner),
    description: m.description,
    homepage_url: m.homepage_url,
    callback_urls: m.callback_urls,
    request_oauth_on_install: m.request_oauth_on_install,
    setup_url: m.setup_url,
    setup_on_update: m.setup_on_update,
    permissions: m.permissions,
    events: m.events,
    public: m.public,
    webhook: { active: m.hook.active, url: m.hook.url, secret_set: true },
    keys: [
      {
        id: id(),
        fingerprint: `SHA256:${btoa(sha(`key:${slug}`)).slice(0, 43)}`,
        created_at: created,
      },
    ],
    installations_count: 0,
    created_at: created,
    updated_at: created,
    webhook_secret: hexSecret(20),
    client_secrets: [],
    deliveries: [],
  };
}

const apps: AppRecord[] = [
  appRecord(
    "graftyard-review",
    "Graftyard Review",
    users.org,
    {
      description: "Reviews pull requests against the design's invariants and posts check runs.",
      homepage_url: "https://graftyard.dev/review",
      callback_urls: ["https://review.graftyard.dev/auth/callback"],
      request_oauth_on_install: false,
      setup_url: "",
      setup_on_update: false,
      permissions: { checks: "write", contents: "read", metadata: "read", pull_requests: "write" },
      events: ["check_suite", "pull_request", "pull_request_review"],
      public: true,
      hook: { active: true, url: "https://review.graftyard.dev/webhook" },
    },
    ago(60 * 24 * 200),
  ),
  appRecord(
    "renovate",
    "Renovate",
    renovateOrg,
    {
      description: "Automated dependency updates. Multi-platform and multi-language.",
      homepage_url: "https://docs.renovatebot.com",
      callback_urls: ["https://app.renovatebot.com/auth/callback"],
      request_oauth_on_install: false,
      setup_url: "https://app.renovatebot.com/setup",
      setup_on_update: true,
      permissions: {
        contents: "write",
        issues: "write",
        metadata: "read",
        pull_requests: "write",
        statuses: "read",
        workflows: "write",
      },
      events: ["issues", "pull_request", "push", "repository"],
      public: true,
      hook: { active: true, url: "https://app.renovatebot.com/webhook" },
    },
    ago(60 * 24 * 400),
  ),
  appRecord(
    "release-notes-bot",
    "Release Notes Bot",
    users.mira,
    {
      description: "Drafts release notes from merged pull requests.",
      homepage_url: "https://mira.okafor.dev/release-notes",
      callback_urls: ["https://mira.okafor.dev/release-notes/callback"],
      request_oauth_on_install: false,
      setup_url: "",
      setup_on_update: false,
      permissions: { contents: "write", metadata: "read", pull_requests: "read" },
      events: ["pull_request", "release"],
      public: false,
      hook: { active: true, url: "https://mira.okafor.dev/release-notes/hook" },
    },
    ago(60 * 24 * 31),
  ),
];

export function integrationOf(a: AppRecord): Integration {
  return {
    id: a.id,
    slug: a.slug,
    node_id: `A_${sha(`app:${a.slug}`).slice(0, 12)}`,
    client_id: a.client_id,
    owner: ownerOf(a.owner.login),
    name: a.name,
    description: a.description || null,
    external_url: a.homepage_url,
    html_url: `${HOST}/apps/${a.slug}`,
    created_at: a.created_at,
    updated_at: a.updated_at,
    permissions: a.permissions as Integration["permissions"],
    events: a.events,
    installations_count: a.installations_count,
  };
}

// world.ts types the Actions app as the nullable `integration` schema.
const actions = actionsApp!;
const actionsRecord: AppRecord = {
  ...appRecord(
    actions.slug ?? "github-actions",
    actions.name,
    users.org,
    {
      description: actions.description ?? "",
      homepage_url: actions.external_url,
      callback_urls: [],
      request_oauth_on_install: false,
      setup_url: "",
      setup_on_update: false,
      permissions: { checks: "write", contents: "read", metadata: "read" },
      events: actions.events,
      public: false,
      hook: { active: false, url: "" },
    },
    actions.created_at,
  ),
  id: actions.id,
};
apps.push(actionsRecord);

function delivery(
  app: AppRecord,
  event: string,
  action: string | null,
  status: number,
  minutes: number,
): Delivery {
  const n = id();
  // A version-1 GUID with the RFC 4122 variant, as Hookshot sends in X-GitHub-Delivery.
  const guid = `${sha(`delivery:${n}`).slice(0, 8)}-${sha(`d2:${n}`).slice(0, 4)}-11f0-8${sha(`d3:${n}`).slice(0, 3)}-${sha(`d4:${n}`).slice(0, 12)}`;
  return {
    id: n,
    guid,
    delivered_at: ago(minutes),
    redelivery: false,
    duration: 0.12 + (n % 7) / 10,
    status:
      status < 300
        ? "OK"
        : status === 0
          ? "Invalid HTTP Response: 0"
          : `Invalid HTTP Response: ${status}`,
    status_code: status,
    event,
    action,
    installation_id: null,
    repository_id: edge.id,
    url: app.webhook.url,
    request: {
      headers: {
        Accept: "*/*",
        "Content-Type": "application/json",
        "User-Agent": "GitHub-Hookshot/graftyard",
        "X-GitHub-Event": event,
        "X-GitHub-Delivery": guid,
        "X-Hub-Signature-256": `sha256=${sha(`sig:${n}`)}${sha(`sig2:${n}`).slice(0, 24)}`,
      },
      payload: { action, repository: { full_name: edge.full_name }, sender: { login: "mira" } },
    },
    response: {
      headers: status ? { "Content-Type": "text/plain" } : null,
      payload: status < 300 ? "ok" : status ? "Bad Gateway" : null,
    },
  };
}
const releaseBot = apps.find((a) => a.slug === "release-notes-bot")!;
releaseBot.deliveries.push(
  delivery(releaseBot, "pull_request", "closed", 200, 90),
  delivery(releaseBot, "release", "published", 502, 60 * 24),
  delivery(releaseBot, "pull_request", "opened", 200, 60 * 26),
  delivery(releaseBot, "installation", "created", 200, 60 * 24 * 30),
);
releaseBot.client_secrets.push({
  id: id(),
  last_eight: "a41f09c2",
  created_at: ago(60 * 24 * 31),
  last_used_at: ago(60 * 24),
});

// ---- Installations ----

const installations: (Installation & { repository_ids: number[] })[] = [];

export function addInstallation(
  app: AppRecord,
  account: SimpleUser,
  selection: "all" | "selected",
  repositoryIds: number[],
  created = now(),
) {
  const n = id();
  const inst = {
    id: n,
    account,
    repository_selection: selection,
    access_tokens_url: `${API}/app/installations/${n}/access_tokens`,
    repositories_url: `${API}/installation/repositories`,
    html_url:
      account.type === "Organization"
        ? `${HOST}/organizations/${account.login}/settings/installations/${n}`
        : `${HOST}/settings/installations/${n}`,
    app_id: app.id,
    client_id: app.client_id,
    target_id: account.id,
    target_type: account.type,
    permissions: app.permissions,
    events: app.events,
    created_at: created,
    updated_at: created,
    single_file_name: null,
    app_slug: app.slug,
    suspended_by: null,
    suspended_at: null,
    repository_ids: selection === "all" ? [] : repositoryIds,
  };
  installations.push(inst);
  app.installations_count++;
  return inst;
}

addInstallation(actionsRecord, users.org, "all", [], ago(60 * 24 * 300));
addInstallation(apps[0], users.org, "selected", [edge.id, raft.id], ago(60 * 24 * 7));
addInstallation(apps[1], users.org, "selected", [web.id], ago(60 * 24 * 100));
addInstallation(releaseBot, users.mira, "all", [], ago(60 * 24 * 30));

// Strips the store's own field before an installation goes on the wire.
export const wireInstallation = ({
  repository_ids,
  ...rest
}: (typeof installations)[number]): Installation => rest;

// The repositories an account owns, for the installation pages.
export const reposOf = (login: string) =>
  core
    .repos()
    .filter((r) => r.owner.login.toLowerCase() === login.toLowerCase())
    .sort((a, b) => a.full_name.localeCompare(b.full_name));

export function installationRepos(inst: (typeof installations)[number]) {
  const owned = reposOf(inst.account && "login" in inst.account ? inst.account.login : "");
  return inst.repository_selection === "all"
    ? owned
    : owned.filter((r) => inst.repository_ids.includes(r.id));
}

// ---- Authorizations: OAuth grants and authorized GitHub Apps ----

type OAuthClient = OAuthApp & { first_party?: boolean };

function oauthApp(
  name: string,
  owner: SimpleUser,
  clientId: string,
  m: Pick<OAuthApp, "description" | "homepage_url" | "callback_url" | "device_flow">,
  created: string,
  users = 0,
): OAuthClient {
  return {
    client_id: clientId,
    name,
    owner: appOwner(owner),
    users,
    secrets: [],
    created_at: created,
    updated_at: created,
    ...m,
  };
}

// First-party clients are seeded once with GHES-era callbacks (`design/SHARED.md`, OAuth).
const oauthApps: OAuthClient[] = [
  {
    ...oauthApp(
      "GitHub CLI",
      users.org,
      "178c6fc778ccc68e1d6a",
      {
        description: "GitHub on the command line.",
        homepage_url: "https://cli.github.com",
        callback_url: "http://127.0.0.1/callback",
        device_flow: true,
      },
      ago(60 * 24 * 500),
    ),
    first_party: true,
  },
  {
    ...oauthApp(
      "Git Credential Manager",
      users.org,
      "0120e057bd645470c1ed",
      {
        description: "Secure, cross-platform Git credential storage.",
        homepage_url: "https://github.com/git-ecosystem/git-credential-manager",
        callback_url: "http://127.0.0.1/",
        device_flow: true,
      },
      ago(60 * 24 * 500),
    ),
    first_party: true,
  },
  oauthApp(
    "Sprint Board",
    users.tomas,
    "Ov23liT0m4sSpr1ntBd",
    {
      description: "A kanban board over Graftyard issues.",
      homepage_url: "https://sprint.graftyard.test",
      callback_url: "https://sprint.graftyard.test/auth/callback",
      device_flow: false,
    },
    ago(60 * 24 * 80),
    9,
  ),
  {
    ...oauthApp(
      "Release Notes",
      users.mira,
      "Ov23liM1raRe1Notes0",
      {
        description: "The web half of Release Notes Bot.",
        homepage_url: "https://mira.okafor.dev/release-notes",
        callback_url: "https://mira.okafor.dev/release-notes/oauth",
        device_flow: false,
      },
      ago(60 * 24 * 30),
      3,
    ),
    secrets: [
      {
        id: id(),
        last_eight: "9c0d55e1",
        created_at: ago(60 * 24 * 30),
        last_used_at: ago(60 * 2),
      },
    ],
  },
];

function grant(app: OAuthClient, scopes: string[], created: string): Grant {
  const n = id();
  return {
    id: n,
    url: `${API}/applications/grants/${n}`,
    app: { client_id: app.client_id, name: app.name, url: app.homepage_url },
    created_at: created,
    updated_at: created,
    scopes,
  };
}

const grants: Grant[] = [
  grant(oauthApps[0], ["gist", "read:org", "repo"], ago(60 * 26)),
  grant(oauthApps[1], ["gist", "repo", "workflow"], ago(60 * 24 * 45)),
  grant(oauthApps[2], ["read:user", "repo"], ago(60 * 24 * 12)),
];

const authorizedApps: AuthorizedApp[] = [
  {
    slug: "graftyard-review",
    name: "Graftyard Review",
    owner: appOwner(users.org),
    authorized_at: ago(60 * 24 * 7),
    last_used_at: ago(60 * 4),
    permissions: { pull_requests: "write", metadata: "read" },
  },
  {
    slug: "renovate",
    name: "Renovate",
    owner: appOwner(renovateOrg),
    authorized_at: ago(60 * 24 * 100),
    last_used_at: null,
    permissions: { email_addresses: "read", metadata: "read" },
  },
];

// Creates or widens the grant for an OAuth app, as an authorization does.
export function grantFor(app: OAuthClient, scopes: string[]) {
  const existing = grants.find((g) => g.app.client_id === app.client_id);
  if (existing) {
    existing.scopes = [...new Set([...existing.scopes, ...scopes])].sort();
    existing.updated_at = now();
    return existing;
  }
  const g = grant(app, [...scopes].sort(), now());
  grants.unshift(g);
  return g;
}

export function authorizeApp(app: AppRecord) {
  const existing = authorizedApps.find((a) => a.slug === app.slug);
  if (existing) return existing;
  const a: AuthorizedApp = {
    slug: app.slug,
    name: app.name,
    owner: app.owner,
    authorized_at: now(),
    last_used_at: null,
    permissions: app.permissions,
  };
  authorizedApps.unshift(a);
  return a;
}

// ---- Codes for the device flow, the authorization-code flow, and App manifests ----

// A device code lives 900 s and is spent once (`design/SHARED.md`, OAuth; `design/DESIGN.md`,
// journeys).
export const DEVICE_TTL_MS = 900_000;
export type DeviceCodeRow = {
  user_code: string;
  client_id: string;
  scopes: string[];
  created_ms: number;
  state: "pending" | "authorized" | "denied";
};
export const deviceCodes: DeviceCodeRow[] = [
  {
    user_code: "WDJB-MJHT",
    client_id: "178c6fc778ccc68e1d6a",
    scopes: ["repo", "read:org", "gist"],
    created_ms: Date.now(),
    state: "pending",
  },
  {
    user_code: "KQRT-8XPL",
    client_id: "0120e057bd645470c1ed",
    scopes: ["repo", "gist", "workflow"],
    created_ms: Date.now() - DEVICE_TTL_MS - 60_000,
    state: "pending",
  },
  {
    user_code: "HVNM-4ZCB",
    client_id: "178c6fc778ccc68e1d6a",
    scopes: ["repo"],
    created_ms: Date.now() - 120_000,
    state: "authorized",
  },
];

export const AUTH_CODE_TTL_S = 600;
export const authCodes = new Map<string, { client_id: string; expires_ms: number }>();

// A manifest code is spent once by POST /app-manifests/{code}/conversions, within an hour.
export const manifestCodes = new Map<string, { slug: string; expires_ms: number; pem: string }>();

// ---- Notifications ----

const notifications: NotificationSettings = {
  default_email: "mira@graftyard.test",
  automatically_watch_repositories: true,
  automatically_watch_teams: true,
  participating: { email: true, web: true },
  watching: { email: false, web: true },
  actions: { email: true, web: true, failed_only: true },
  dependabot: { email: false, web: true },
  own_updates: false,
  routes: [{ org: "graftyard", email: "mira@graftyard.test" }],
};

// ---- Sign-in ----

export const loginAttempts = new Map<string, string>(); // attempt id -> login
export const account = { deleted_at: null as string | null };

// Logins nobody may take: routes of this app and GHES reserved names.
export const RESERVED_LOGINS = new Set([
  "admin",
  "api",
  "apps",
  "login",
  "logout",
  "new",
  "notifications",
  "orgs",
  "organizations",
  "search",
  "settings",
  "stafftools",
  "explore",
  "enterprise",
  "ghost",
  "graftyard",
  "site",
]);
// The viewer's own former login stays theirs to take back (`design/SHARED.md`, owner renames).
export const takenLogins = () =>
  new Set(
    Object.values(users)
      .filter((u) => u !== users.mira)
      .map((u) => u.login.toLowerCase()),
  );

export const myAvatar = { custom: false };
export const defaultAvatar = () => avatar(me.login);

export const db = {
  emails,
  emailVisibility,
  otherEmails,
  sshKeys,
  signingKeys,
  gpgKeys,
  gpgFingerprints,
  tokens,
  sessions,
  securityLog,
  apps,
  oauthApps,
  grants,
  authorizedApps,
  installations,
  notifications,
  owners,
  ownerOf,
  appOwner,
  key,
  appRecord,
  delivery,
};

export type { AppKey, AppRecord, OAuthClient };
