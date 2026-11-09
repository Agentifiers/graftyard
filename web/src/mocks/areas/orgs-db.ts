import type { Schemas } from "@/api/client";
import type {
  MemberPrivileges,
  OrganizationInvitation,
  PendingRename,
  ProfileExtras,
  TeamSettings,
} from "@/areas/orgs/api";
import {
  API,
  HOST,
  edge,
  me,
  org,
  raft,
  repository,
  sha,
  teams as worldTeams,
  teamMembers as worldTeamMembers,
  orgOwners,
  orgMembers,
  user,
  users,
  web,
  type Repository,
  type SimpleUser,
} from "../world";
import {
  activity as feed,
  people as activityPeople,
  profiles as searchProfiles,
} from "./activity-db";
import { db } from "./core-db";
import { people as directory } from "./repo-admin-db";

// The organization's people, teams, hooks, rulesets, and audit log, and every user's
// profile, follows, pins, and contributions. Records are kept normalized (a team holds
// logins and repository names, not copies of users and repositories) and rendered to their
// GHES schema on the way out, so a rename or a removal shows everywhere at once.

type OrgRole = "admin" | "member";
type TeamRole = "member" | "maintainer";
export type RepoPermission = "pull" | "triage" | "push" | "maintain" | "admin";
type Privacy = "closed" | "secret";
type Notify = "notifications_enabled" | "notifications_disabled";

type Member = { role: OrgRole; twoFactor: boolean; joined: string };

export type TeamRec = {
  id: number;
  slug: string;
  name: string;
  description: string | null;
  privacy: Privacy;
  notification_setting: Notify;
  permission: string;
  parentId: number | null;
  created_at: string;
  updated_at: string;
  members: Map<string, TeamRole>;
  // Lower-cased full name to permission.
  repos: Map<string, RepoPermission>;
};

type Profile = {
  name: string | null;
  bio: string | null;
  company: string | null;
  location: string | null;
  blog: string | null;
  email: string | null;
  twitter: string | null;
  created_at: string;
};

const now = () => new Date().toISOString();
const DAY = 86_400_000;
const ago = (days: number, hours = 0) => new Date(Date.now() - days * DAY - hours * 3_600_000);

// Seeded so every reload draws the same history.
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// ---- People ----

// The instance's user directory, which GET /users serves. It reuses the activity area's
// people (same objects, so ids agree) and adds the org's own members and collaborators.
const extra = {
  kenji: user("kenji", "Kenji Watanabe"),
  olu: user("olu", "Olu Adeyemi"),
  rhea: user("rhea", "Rhea Kapoor"),
  sam: user("sam", "Sam Achterberg"),
  ines: user("ines", "Inês Duarte"),
};

const people = new Map<string, SimpleUser>(
  [...activityPeople, ...Object.values(extra)].map((u) => [u.login, u]),
);

// Repository admin resolves invitees through its own list, so every login GET /users
// suggests must be one it can invite.
for (const u of people.values()) {
  if (!directory.some((d) => d.login === u.login)) directory.push(u);
}

const profiles = new Map<string, Profile>([
  [
    "mira",
    {
      name: me.name,
      bio: me.bio,
      company: me.company,
      location: me.location,
      blog: "https://mira.dev",
      email: me.email,
      twitter: "miraokafor",
      created_at: me.created_at,
    },
  ],
  [
    "tomas",
    {
      name: "Tomás Ferreira",
      bio: "Wire formats and the GraphQL edge.",
      company: "@graftyard",
      location: "Porto",
      blog: null,
      email: null,
      twitter: null,
      created_at: "2025-03-05T10:00:00Z",
    },
  ],
  [
    "lin",
    {
      name: "Lin Zhao",
      bio: "Consensus, learners, and the web app's dark mode.",
      company: "@graftyard",
      location: "Singapore",
      blog: "https://lin.zhao.dev",
      email: "lin@graftyard.test",
      twitter: null,
      created_at: "2025-03-08T02:00:00Z",
    },
  ],
  [
    "devi",
    {
      name: "Devi Raman",
      bio: "Org owner. Rate limits and on-call.",
      company: "@graftyard",
      location: "Bengaluru",
      blog: null,
      email: "devi@graftyard.test",
      twitter: "devi_r",
      created_at: "2025-01-10T00:00:00Z",
    },
  ],
]);

function profileOf(login: string): Profile {
  // The viewer's profile follows the account area's PATCH /user, which edits `me`.
  if (login === me.login) {
    const { name, bio, company, location, blog, email, twitter_username, created_at } = me;
    return {
      name,
      bio,
      company,
      location,
      blog,
      email,
      twitter: twitter_username ?? null,
      created_at,
    };
  }
  return (
    profiles.get(login) ?? {
      name: people.get(login)?.name ?? null,
      bio: searchProfiles[login]?.bio ?? null,
      company: null,
      location: searchProfiles[login]?.location ?? null,
      blog: null,
      email: null,
      twitter: null,
      created_at: "2025-06-01T09:00:00Z",
    }
  );
}

export function findUser(login: string): SimpleUser | undefined {
  return people.get(login.toLowerCase());
}

export function allUsers(): SimpleUser[] {
  return [...people.values()];
}

// Who follows whom: follower login to the logins they follow.
const following = new Map<string, Set<string>>([
  ["mira", new Set(["lin", "devi", "tomas", "kenji"])],
  ["tomas", new Set(["mira", "lin"])],
  ["lin", new Set(["mira", "devi", "priya"])],
  ["devi", new Set(["mira", "lin", "tomas", "olu"])],
  ["kenji", new Set(["mira", "lin"])],
  ["ana", new Set(["mira", "tomas"])],
  ["priya", new Set(["mira", "lin", "devi"])],
  ["olu", new Set(["mira", "devi"])],
  ["rhea", new Set(["mira"])],
  ["sam", new Set(["mira", "devi"])],
  ["ines", new Set(["mira", "tomas"])],
  ["jonas", new Set(["mira"])],
  ["yuki", new Set(["mira", "lin"])],
]);

export const follows = {
  following: (login: string) => [...(following.get(login) ?? [])].map((l) => people.get(l)!),
  followers: (login: string) =>
    [...following].filter(([, set]) => set.has(login)).map(([l]) => people.get(l)!),
  isFollowing: (who: string, target: string) => following.get(who)?.has(target) ?? false,
  follow(who: string, target: string) {
    if (!following.has(who)) following.set(who, new Set());
    following.get(who)!.add(target);
  },
  unfollow(who: string, target: string) {
    following.get(who)?.delete(target);
  },
};

// ---- Personal repositories (the viewer's), in the shared store so repo pages serve them ----

const dotfiles = repository("dotfiles", "Shell, editor, and tmux config.", "public", {
  language: "Shell",
  stars: 7,
  topics: ["dotfiles"],
  pushed: "2026-10-30T21:10:00Z",
  owner: users.mira,
});
const notes = repository(
  "raft-notes",
  "Reading notes on Raft, joint consensus, and leases.",
  "private",
  {
    language: null,
    stars: 0,
    topics: [],
    pushed: "2026-11-02T07:45:00Z",
    owner: users.mira,
  },
);
db.addRepo(dotfiles);
db.addRepo(notes);

// ---- The organization ----

const members = new Map<string, Member>(
  orgMembers.map((u, i) => [
    u.login,
    {
      role: orgOwners.includes(u) ? "admin" : "member",
      twoFactor: u.login !== "tomas",
      joined: ago(400 - i * 30).toISOString(),
    },
  ]),
);
for (const [login, joined, twoFactor] of [
  ["kenji", 120, true],
  ["priya", 90, true],
  ["olu", 60, false],
  ["rhea", 20, true],
] as const) {
  members.set(login, { role: "member", twoFactor, joined: ago(joined).toISOString() });
}

// Outside collaborators: login to the repositories they were added to.
const outside = new Map<string, string[]>([
  ["sam", ["graftyard/edge"]],
  ["ines", ["graftyard/web", "graftyard/edge"]],
]);

let invitationId = 4100;
const invitations: OrganizationInvitation[] = [];
export function invitation(
  target: { login: string | null; email: string | null },
  role: OrganizationInvitation["role"],
  teamIds: number[],
  inviter: SimpleUser,
  created: Date,
): OrganizationInvitation {
  const id = invitationId++;
  return {
    id,
    node_id: `OI_${sha(`inv:${id}`).slice(0, 12)}`,
    login: target.login,
    email: target.email,
    role,
    created_at: created.toISOString(),
    failed_at: null,
    failed_reason: null,
    inviter,
    team_count: teamIds.length,
    invitation_teams_url: `${API}/organizations/${org.id}/invitations/${id}/teams`,
    invitation_source: "member",
  };
}
const invitationTeams = new Map<number, number[]>();

// ---- Teams ----

const teams: TeamRec[] = [];
export function addTeam(
  t: Omit<TeamRec, "id" | "members" | "repos" | "created_at" | "updated_at"> & {
    members: [string, TeamRole][];
    repos: [string, RepoPermission][];
    created?: string;
  },
) {
  const rec: TeamRec = {
    ...t,
    id: parseInt(sha(`team:${t.slug}`).slice(0, 6), 16),
    created_at: t.created ?? "2025-02-01T09:00:00Z",
    updated_at: t.created ?? "2025-06-01T09:00:00Z",
    members: new Map(t.members),
    repos: new Map(t.repos),
  };
  teams.push(rec);
  return rec;
}

const engineering = addTeam({
  slug: "engineering",
  name: "Engineering",
  description: "Everyone who ships code. Parent of the product teams.",
  privacy: "closed",
  notification_setting: "notifications_enabled",
  permission: "pull",
  parentId: null,
  members: [
    ["devi", "maintainer"],
    ["kenji", "member"],
  ],
  repos: [
    ["graftyard/edge", "pull"],
    ["graftyard/raft", "pull"],
    ["graftyard/web", "pull"],
  ],
});
const world = Object.entries(worldTeams) as [keyof typeof worldTeams, Schemas["team"]][];
const byWorld = new Map<string, TeamRec>();
for (const [key, t] of world) {
  const rec = addTeam({
    slug: t.slug,
    name: t.name,
    description: t.description,
    privacy: "closed",
    notification_setting: "notifications_enabled",
    permission: t.permission,
    parentId: key === "maintainers" ? null : engineering.id,
    members: worldTeamMembers[key].map((u, i) => [u.login, i === 0 ? "maintainer" : "member"]),
    repos:
      key === "maintainers"
        ? [
            ["graftyard/edge", "maintain"],
            ["graftyard/raft", "maintain"],
            ["graftyard/web", "maintain"],
          ]
        : key === "edge"
          ? [
              ["graftyard/edge", "push"],
              ["graftyard/web", "triage"],
            ]
          : [["graftyard/raft", "push"]],
  });
  byWorld.set(key, rec);
}
addTeam({
  slug: "wire",
  name: "Wire",
  description: "REST and GraphQL byte compatibility with GHES 3.22.",
  privacy: "closed",
  notification_setting: "notifications_enabled",
  permission: "push",
  parentId: byWorld.get("edge")!.id,
  members: [
    ["tomas", "maintainer"],
    ["priya", "member"],
  ],
  repos: [["graftyard/edge", "push"]],
});
addTeam({
  slug: "blobs",
  name: "Blobs",
  description: "Pack files, the blob store, and GC.",
  privacy: "closed",
  notification_setting: "notifications_disabled",
  permission: "push",
  parentId: byWorld.get("storage")!.id,
  members: [["olu", "maintainer"]],
  repos: [["graftyard/raft", "push"]],
});
addTeam({
  slug: "security-response",
  name: "Security response",
  description: "Embargoed advisories. Visible only to its members and owners.",
  privacy: "secret",
  notification_setting: "notifications_enabled",
  permission: "pull",
  parentId: null,
  members: [
    ["devi", "maintainer"],
    ["mira", "member"],
  ],
  repos: [["graftyard/edge", "admin"]],
});

invitations.push(
  invitation(
    { login: "ana", email: null },
    "direct_member",
    [byWorld.get("edge")!.id],
    users.mira,
    ago(3),
  ),
  invitation({ login: null, email: "jo@contractor.test" }, "direct_member", [], users.devi, ago(9)),
);
invitationTeams.set(invitations[0].id, [byWorld.get("edge")!.id]);

export function slugify(name: string) {
  return name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
}

// ---- Settings the PATCH body does not cover ----

// Seeded onto the org record, which GET /orgs/{org} serves, so every write edits `org`.
const privileges: MemberPrivileges & { members_can_create_teams: boolean } = {
  members_can_delete_repositories: false,
  members_can_change_repo_visibility: false,
  members_can_invite_outside_collaborators: true,
  members_can_delete_issues: false,
  members_can_create_teams: true,
  display_commenter_full_name_setting_enabled: false,
  readers_can_create_discussions: true,
};
Object.assign(org, privileges, {
  billing_email: "billing@graftyard.test",
  members_can_create_pages: true,
  members_allowed_repository_creation_type: "private",
});

// ---- Webhooks ----

type Hook = Schemas["org-hook"];
type Delivery = Schemas["hook-delivery"];

let hookId = 510;
let deliveryId = 90_000;
const hooks: Hook[] = [];
const deliveries = new Map<number, Delivery[]>();

export function makeHook(input: {
  url: string;
  content_type: string;
  secret?: string;
  insecure_ssl: string;
  events: string[];
  active: boolean;
  created?: string;
}): Hook {
  const id = hookId++;
  const base = `${API}/orgs/${org.login}/hooks/${id}`;
  return {
    id,
    url: base,
    ping_url: `${base}/pings`,
    deliveries_url: `${base}/deliveries`,
    name: "web",
    events: input.events,
    active: input.active,
    config: {
      url: input.url,
      content_type: input.content_type,
      insecure_ssl: input.insecure_ssl,
      ...(input.secret ? { secret: "********" } : {}),
    },
    created_at: input.created ?? now(),
    updated_at: input.created ?? now(),
    type: "Organization",
  };
}

// A receiver whose URL says it fails answers 500; everything else answers 200.
function outcome(url: string | undefined) {
  if (url && /fail|5\d\d|down/.test(url)) {
    return { status: "Invalid HTTP Response: 500", status_code: 500, body: '{"error":"boom"}' };
  }
  return { status: "OK", status_code: 200, body: '{"ok":true}' };
}

export function deliver(
  hook: Hook,
  event: string,
  action: string | null,
  at: Date,
  opts: { redelivery?: boolean; pending?: boolean } = {},
): Delivery {
  const id = deliveryId++;
  const guid = `${sha(`guid:${id}`).slice(0, 8)}-${sha(`g2:${id}`).slice(0, 4)}-11f0-${sha(`g3:${id}`).slice(0, 4)}-${sha(`g4:${id}`).slice(0, 12)}`;
  const result = outcome(hook.config.url);
  const payload: Record<string, unknown> =
    event === "ping"
      ? {
          zen: "Design for failure.",
          hook_id: hook.id,
          hook: { type: "Organization", id: hook.id, events: hook.events },
        }
      : {
          action,
          organization: { login: org.login, id: org.id },
          sender: { login: "mira", id: users.mira.id },
          ...(event === "push" ? { ref: "refs/heads/main", after: sha(`push:${id}`) } : {}),
        };
  const d: Delivery = {
    id,
    guid,
    delivered_at: at.toISOString(),
    redelivery: opts.redelivery ?? false,
    duration: opts.pending ? 0 : 0.08 + ((id * 37) % 90) / 100,
    status: opts.pending ? "Pending" : result.status,
    status_code: opts.pending ? 0 : result.status_code,
    event,
    action,
    installation_id: null,
    repository_id: event === "push" ? edge.id : null,
    url: hook.config.url,
    request: {
      headers: {
        Accept: "*/*",
        "Content-Type": `application/${hook.config.content_type === "form" ? "x-www-form-urlencoded" : "json"}`,
        "User-Agent": "GitHub-Hookshot/3.22",
        "X-GitHub-Delivery": guid,
        "X-GitHub-Event": event,
        "X-GitHub-Hook-ID": String(hook.id),
        "X-GitHub-Hook-Installation-Target-Type": "organization",
        "X-GitHub-Enterprise-Version": "3.22.0",
        "X-GitHub-Enterprise-Host": "ghes.graftyard.test",
      },
      payload,
    },
    response: opts.pending
      ? { headers: null, payload: null }
      : {
          headers: { "Content-Type": "application/json", Server: "receiver/1.4" },
          payload: result.body,
        },
  };
  deliveries.set(hook.id, [d, ...(deliveries.get(hook.id) ?? [])]);
  if (opts.pending) {
    // The receiver answers a moment later, as a real delivery does.
    setTimeout(() => {
      d.status = result.status;
      d.status_code = result.status_code;
      d.duration = 0.42;
      d.response = {
        headers: { "Content-Type": "application/json", Server: "receiver/1.4" },
        payload: result.body,
      };
    }, 2500);
  }
  return d;
}

{
  const ci = makeHook({
    url: "https://ci.graftyard.test/hooks/github",
    content_type: "json",
    secret: "s3cret",
    insecure_ssl: "0",
    events: ["push", "pull_request", "check_suite"],
    active: true,
    created: "2025-04-01T10:00:00Z",
  });
  const chat = makeHook({
    url: "https://chat.example.test/fail/incoming/graftyard",
    content_type: "form",
    insecure_ssl: "0",
    events: ["*"],
    active: true,
    created: "2025-09-12T14:00:00Z",
  });
  const audit = makeHook({
    url: "https://siem.graftyard.test/collect",
    content_type: "json",
    insecure_ssl: "1",
    events: ["member", "membership", "team", "organization"],
    active: false,
    created: "2026-02-20T08:30:00Z",
  });
  hooks.push(ci, chat, audit);
  const r = rng(7);
  for (let i = 30; i > 0; i--) {
    const at = ago(i / 3, r() * 5);
    deliver(ci, r() < 0.6 ? "push" : "pull_request", r() < 0.6 ? null : "synchronize", at);
    if (i % 3 === 0) deliver(chat, "push", null, at);
  }
  deliver(audit, "member", "added", ago(40));
}

// ---- Rulesets ----

type Ruleset = Schemas["repository-ruleset"];
type RulesetVersion = Schemas["ruleset-version"] & { state: Ruleset };
let rulesetId = 3100;
const rulesets: Ruleset[] = [];
const rulesetHistory = new Map<number, RulesetVersion[]>();

export type RulesetInput = Pick<
  Ruleset,
  "name" | "target" | "enforcement" | "bypass_actors" | "conditions" | "rules"
>;

export function makeRuleset(input: RulesetInput, at = now()): Ruleset {
  const id = rulesetId++;
  return {
    id,
    node_id: `RRS_${sha(`ruleset:${id}`).slice(0, 12)}`,
    source_type: "Organization",
    source: org.login,
    current_user_can_bypass: "always",
    _links: {
      self: { href: `${API}/orgs/${org.login}/rulesets/${id}` },
      html: { href: `${HOST}/organizations/${org.login}/settings/rules/${id}` },
    },
    created_at: at,
    updated_at: at,
    ...input,
  };
}

export function recordRulesetVersion(r: Ruleset, actorId: number) {
  const list = rulesetHistory.get(r.id) ?? [];
  list.unshift({
    version_id: (list[0]?.version_id ?? 0) + 1,
    actor: { id: actorId, type: "User" },
    updated_at: r.updated_at ?? now(),
    state: structuredClone(r),
  });
  rulesetHistory.set(r.id, list);
}

{
  const main = makeRuleset(
    {
      name: "Protect default branches",
      target: "branch",
      enforcement: "active",
      bypass_actors: [{ actor_id: 1, actor_type: "OrganizationAdmin", bypass_mode: "always" }],
      conditions: {
        ref_name: { include: ["~DEFAULT_BRANCH"], exclude: [] },
        repository_name: { include: ["~ALL"], exclude: [], protected: true },
      },
      rules: [
        { type: "deletion" },
        { type: "non_fast_forward" },
        {
          type: "pull_request",
          parameters: {
            required_approving_review_count: 1,
            dismiss_stale_reviews_on_push: true,
            require_code_owner_review: false,
            require_last_push_approval: false,
            required_review_thread_resolution: true,
          },
        },
        {
          type: "required_status_checks",
          parameters: {
            strict_required_status_checks_policy: true,
            required_status_checks: [{ context: "ci / test" }, { context: "ci / wire-diff" }],
          },
        },
      ],
    },
    "2025-05-10T09:00:00Z",
  );
  const tags = makeRuleset(
    {
      name: "Release tags",
      target: "tag",
      enforcement: "evaluate",
      bypass_actors: [
        { actor_id: byWorld.get("maintainers")!.id, actor_type: "Team", bypass_mode: "always" },
      ],
      conditions: {
        ref_name: { include: ["refs/tags/v*"], exclude: [] },
        repository_name: { include: ["edge", "raft"], exclude: [] },
      },
      rules: [
        { type: "creation" },
        { type: "update" },
        { type: "deletion" },
        { type: "required_signatures" },
      ],
    },
    "2026-01-22T12:00:00Z",
  );
  rulesets.push(main, tags);
  recordRulesetVersion(main, users.devi.id);
  recordRulesetVersion(tags, users.mira.id);
}

// ---- Pre-receive hooks the site admin made available to the org ----

const preReceive: Schemas["org-pre-receive-hook"][] = [
  {
    id: 11,
    name: "reject-large-files",
    enforcement: "enabled",
    configuration_url: `${API}/admin/pre-receive-hooks/11`,
    allow_downstream_configuration: true,
  },
  {
    id: 12,
    name: "require-signed-off-by",
    enforcement: "testing",
    configuration_url: `${API}/orgs/${org.login}/pre-receive-hooks/12`,
    allow_downstream_configuration: true,
  },
  {
    id: 13,
    name: "block-secrets",
    enforcement: "enabled",
    configuration_url: `${API}/admin/pre-receive-hooks/13`,
    allow_downstream_configuration: false,
  },
  {
    id: 14,
    name: "conventional-commits",
    enforcement: "disabled",
    configuration_url: `${API}/admin/pre-receive-hooks/14`,
    allow_downstream_configuration: true,
  },
];
// What each hook reverts to when the org removes its own enforcement.
const preReceiveDefaults = new Map(preReceive.map((h) => [h.id!, h.enforcement!]));
preReceive[1].enforcement = "enabled";

// ---- Audit log ----

type AuditEvent = Schemas["audit-log-event"];
const audit: AuditEvent[] = [];
let auditSeq = 0;

export function log(
  action: string,
  actor: string,
  extra: Partial<AuditEvent> = {},
  at = new Date(),
) {
  const actorUser = people.get(actor);
  const event: AuditEvent = {
    "@timestamp": at.getTime(),
    created_at: at.getTime(),
    _document_id: sha(`audit:${auditSeq++}`).slice(0, 22),
    action,
    actor,
    actor_id: actorUser?.id,
    actor_location: { country_name: ACTOR_COUNTRY[actor] ?? "Portugal" },
    org: org.login,
    org_id: org.id,
    operation_type: OPERATION[action.split(".")[1] ?? ""] ?? "modify",
    transport_protocol_name: action.startsWith("git.") ? "ssh" : "http",
    ...extra,
  };
  audit.push(event);
  return event;
}

const ACTOR_COUNTRY: Record<string, string> = {
  mira: "Portugal",
  tomas: "Portugal",
  lin: "Singapore",
  devi: "India",
  kenji: "Japan",
  priya: "India",
  olu: "Nigeria",
  rhea: "India",
};
const OPERATION: Record<string, string> = {
  create: "create",
  add_member: "create",
  invite_member: "create",
  destroy: "remove",
  remove_member: "remove",
  delete: "remove",
  clone: "access",
  fetch: "access",
  login: "authentication",
};

{
  const r = rng(42);
  const actors = ["mira", "devi", "tomas", "lin", "kenji", "priya", "olu", "rhea"];
  const repoNames = ["graftyard/edge", "graftyard/raft", "graftyard/web"];
  const kinds: [string, (who: string) => Partial<AuditEvent>, number][] = [
    ["git.clone", () => ({ repo: repoNames[Math.floor(r() * 3)] }), 30],
    ["git.push", () => ({ repo: repoNames[Math.floor(r() * 3)] }), 22],
    ["git.fetch", () => ({ repo: repoNames[Math.floor(r() * 3)] }), 18],
    ["repo.update", () => ({ repo: repoNames[Math.floor(r() * 3)] }), 4],
    ["protected_branch.update", () => ({ repo: repoNames[Math.floor(r() * 3)], name: "main" }), 2],
    [
      "team.add_member",
      () => ({
        team: `graftyard/${teams[Math.floor(r() * teams.length)].slug}`,
        user: actors[Math.floor(r() * actors.length)],
      }),
      3,
    ],
    [
      "team.remove_member",
      () => ({
        team: `graftyard/${teams[Math.floor(r() * teams.length)].slug}`,
        user: actors[Math.floor(r() * actors.length)],
      }),
      1,
    ],
    [
      "team.add_repository",
      () => ({
        team: `graftyard/${teams[Math.floor(r() * teams.length)].slug}`,
        repo: repoNames[Math.floor(r() * 3)],
      }),
      2,
    ],
    ["hook.create", () => ({ hook_id: hooks[Math.floor(r() * hooks.length)].id }), 1],
    ["hook.config_changed", () => ({ hook_id: hooks[Math.floor(r() * hooks.length)].id }), 1],
    ["org.update_member", () => ({ user: actors[Math.floor(r() * actors.length)] }), 1],
    ["org.invite_member", () => ({ user: actors[Math.floor(r() * actors.length)] }), 1],
    ["repository_ruleset.update", () => ({ name: "Protect default branches" }), 1],
    ["pre_receive_hook.enforcement", () => ({ name: "require-signed-off-by" }), 1],
  ];
  const total = kinds.reduce((n, [, , w]) => n + w, 0);
  // About 1,800 events over 120 days, enough that the table has to virtualize.
  for (let i = 0; i < 1800; i++) {
    let pick = r() * total;
    const [action, extra] = kinds.find(([, , w]) => (pick -= w) < 0) ?? kinds[0];
    const who = actors[Math.floor(Math.pow(r(), 1.6) * actors.length)];
    log(action, who, extra(who), ago(r() * 120));
  }
  audit.sort((a, b) => a["@timestamp"]! - b["@timestamp"]!);
}

// GHES audit-log search phrase: `actor:`, `action:` (a trailing `.*` or bare prefix
// matches the category), `created:>=YYYY-MM-DD`, `created:<=`, `repo:`, `team:`, and
// free words matched against the action and the actor.
export function searchAudit(phrase: string, order: "asc" | "desc", include: string) {
  const terms = phrase.split(/\s+/).filter(Boolean);
  const checks = terms.map((t): ((e: AuditEvent) => boolean) => {
    const m = t.match(/^(-?)(\w+):(.*)$/);
    if (!m) return (e) => `${e.action} ${e.actor}`.includes(t);
    const [, neg, key, raw] = m;
    const test = ((): ((e: AuditEvent) => boolean) => {
      switch (key) {
        case "actor":
          return (e) => e.actor === raw;
        case "action":
          return (e) => {
            const p = raw.replace(/\.\*$/, "");
            return e.action === p || (e.action ?? "").startsWith(`${p}.`);
          };
        case "repo":
          return (e) => e.repo === raw || e.repo === `${org.login}/${raw}`;
        case "team":
          return (e) => e.team === raw || e.team === `${org.login}/${raw}`;
        case "created": {
          const c = raw.match(/^(>=|<=|>|<)?(\d{4}-\d{2}-\d{2})$/);
          if (!c) return () => true;
          const day = Date.parse(`${c[2]}T00:00:00Z`);
          const op = c[1] ?? "=";
          return (e) => {
            const t = e["@timestamp"]!;
            if (op === ">=") return t >= day;
            if (op === ">") return t >= day + DAY;
            if (op === "<=") return t < day + DAY;
            if (op === "<") return t < day;
            return t >= day && t < day + DAY;
          };
        }
        default:
          return () => true;
      }
    })();
    return neg ? (e) => !test(e) : test;
  });
  const source =
    include === "git"
      ? (e: AuditEvent) => e.action!.startsWith("git.")
      : include === "all"
        ? () => true
        : (e: AuditEvent) => !e.action!.startsWith("git.");
  const hits = audit.filter((e) => source(e) && checks.every((c) => c(e)));
  return order === "asc" ? hits : [...hits].reverse();
}

// ---- Profiles' README and pins (the private UI API) ----

const readmes = new Map<string, string>([
  [
    "mira",
    `### Hi, I'm Mira 👋

I work on the **edge Worker** and the **storage** groups at Graftyard.

- 🔭 Now: idempotent retries on the write path
- 🌱 Learning: Iceberg tables for audit retention
- 📫 Reach me in \`#edge\` or at mira@graftyard.test`,
  ],
  [
    "graftyard",
    `## Graftyard

A GHES-compatible forge that runs on Cloudflare. Same REST and GraphQL wire format as
GitHub Enterprise Server 3.22, so \`gh\`, Actions runners, and your existing tooling just work.

| Repository | What it is |
| --- | --- |
| [edge](${HOST}/graftyard/edge) | The Rust edge Worker |
| [raft](${HOST}/graftyard/raft) | Log, lease, transfer, and learners |
| [web](${HOST}/graftyard/web) | This web app |

**Contributing.** Read the design in \`design/DESIGN.md\` first, then pick a work package.`,
  ],
  ["lin", "Distributed systems, mostly. Currently teaching learners to read."],
]);
const pins = new Map<string, string[]>([
  ["mira", ["graftyard/edge", "graftyard/raft", "mira/dotfiles"]],
  ["graftyard", ["graftyard/edge", "graftyard/raft", "graftyard/web"]],
  ["lin", ["graftyard/raft", "graftyard/web"]],
  ["devi", ["graftyard/edge"]],
]);

export const profilesDb = {
  extras(login: string, visible: (full: string) => Repository | undefined): ProfileExtras {
    return {
      login,
      readme: readmes.get(login) ?? null,
      pinned: (pins.get(login) ?? []).flatMap((f) => {
        const r = visible(f);
        return r ? [minimalRepo(r)] : [];
      }),
    };
  },
  setPins(login: string, fullNames: string[]) {
    pins.set(login, fullNames);
  },
};

// ---- Contributions: one year of daily counts from the user's events ----

const activity = new Map<string, number[]>();
function seededYear(login: string) {
  let days = activity.get(login);
  if (days) return days;
  const r = rng(parseInt(sha(`contrib:${login}`).slice(0, 8), 16));
  const busy = login === "mira" ? 1 : login === "graftyard" ? 0 : 0.6;
  days = Array.from({ length: 371 }, (_, i) => {
    const weekday = new Date(Date.now() - i * DAY).getUTCDay();
    const weekend = weekday === 0 || weekday === 6;
    const roll = r();
    if (roll < (weekend ? 0.75 : 0.25) / busy) return 0;
    const base = Math.floor(Math.pow(r(), 2.2) * 14 * busy) + 1;
    return i < 60 ? base + Math.floor(r() * 4) : base;
  });
  activity.set(login, days);
  return days;
}

export function contributions(login: string) {
  const days = [...seededYear(login)];
  // Issues, pull requests, and comments this user wrote in the shared store count too, so
  // opening an issue lights today's square.
  for (const repo of db.repos()) {
    const state = db.state(repo.owner.login, repo.name);
    if (!state) continue;
    const stamps = [
      ...state.issues.filter((i) => i.user?.login === login).map((i) => i.created_at),
      ...[...state.comments.values()]
        .flat()
        .filter((c) => c.user?.login === login)
        .map((c) => c.created_at),
    ];
    for (const at of stamps) bump(at);
  }
  // So do the events the activity area's feed shows for them.
  for (const e of feed.events()) if (e.actor.login === login && e.created_at) bump(e.created_at);
  function bump(at: string) {
    const i = Math.floor((Date.now() - Date.parse(at)) / DAY);
    if (i >= 0 && i < days.length) days[i]++;
  }
  return days
    .map((count, i) => ({ date: new Date(Date.now() - i * DAY).toISOString().slice(0, 10), count }))
    .reverse();
}

// ---- Renderers to GHES schemas ----

// Members who show their membership on their profile and in GET /orgs/{org}/public_members.
const publicMembers = new Set(["mira", "devi", "lin"]);

// members_can_create_teams lives on the org record; these are the rest of the page.
const teamDefaults: Omit<TeamSettings, "members_can_create_teams"> = {
  default_privacy: "closed",
  default_notification_setting: "notifications_enabled",
  team_discussions_enabled: false,
};

const rename: PendingRename = { login: null, queued_at: null, message: "No rename is queued." };

export const orgDb = {
  org,
  teamDefaults,
  rename,
  // DELETE /orgs/{org} sets this; GET /orgs/{org} then answers 404 until a reload.
  deleted: false,
  members,
  publicMembers,
  outside,
  invitations,
  invitationTeams,
  teams,
  hooks,
  deliveries,
  rulesets,
  rulesetHistory,
  preReceive,
  preReceiveDefaults,
  audit,
};

export function findOrg(name: string) {
  return !orgDb.deleted && name.toLowerCase() === org.login.toLowerCase() ? orgDb : undefined;
}

export function orgSimple(): Schemas["organization-simple"] {
  const { login, id, node_id, url, repos_url, events_url, hooks_url, issues_url } = org;
  const { members_url, public_members_url, avatar_url, description } = org;
  return {
    login,
    id,
    node_id,
    url,
    repos_url,
    events_url,
    hooks_url,
    issues_url,
    members_url,
    public_members_url,
    avatar_url,
    description,
  };
}

export function teamById(id: number) {
  return teams.find((t) => t.id === id);
}
export function teamBySlug(slug: string) {
  return teams.find((t) => t.slug === slug.toLowerCase());
}

function teamUrls(slug: string) {
  const url = `${API}/orgs/${org.login}/teams/${slug}`;
  return {
    url,
    html_url: `${HOST}/orgs/${org.login}/teams/${slug}`,
    members_url: `${url}/members{/member}`,
    repositories_url: `${url}/repos`,
  };
}

function teamSimple(t: TeamRec): NonNullable<Schemas["nullable-team-simple"]> {
  return {
    id: t.id,
    node_id: `T_${sha(`team:${t.slug}`).slice(0, 12)}`,
    name: t.name,
    slug: t.slug,
    description: t.description,
    privacy: t.privacy,
    notification_setting: t.notification_setting,
    permission: t.permission,
    type: "organization",
    organization_id: org.id,
    ...teamUrls(t.slug),
  };
}

export function teamView(t: TeamRec): Schemas["team"] {
  const parent = t.parentId === null ? null : teamById(t.parentId);
  return { ...teamSimple(t), parent: parent ? teamSimple(parent) : null };
}

export function teamFull(t: TeamRec): Schemas["team-full"] {
  const parent = t.parentId === null ? null : teamById(t.parentId);
  return {
    ...teamSimple(t),
    privacy: t.privacy,
    notification_setting: t.notification_setting,
    parent: parent ? teamSimple(parent) : null,
    members_count: t.members.size,
    repos_count: t.repos.size,
    created_at: t.created_at,
    updated_at: t.updated_at,
    organization: org,
  };
}

export function teamMember(
  login: string,
  role: TeamRole,
  inherited = false,
): Schemas["team-member"] {
  return { ...people.get(login)!, role, inherited };
}

// GHES's permission flags for a role, each implying the ones below it.
const LADDER: RepoPermission[] = ["pull", "triage", "push", "maintain", "admin"];
export function permissionFlags(p: RepoPermission) {
  const rank = LADDER.indexOf(p);
  return {
    pull: rank >= 0,
    triage: rank >= 1,
    push: rank >= 2,
    maintain: rank >= 3,
    admin: rank >= 4,
  };
}
const ROLE_NAME: Record<RepoPermission, string> = {
  pull: "read",
  triage: "triage",
  push: "write",
  maintain: "maintain",
  admin: "admin",
};
export function asPermission(raw: string | undefined): RepoPermission | undefined {
  const map: Record<string, RepoPermission> = {
    pull: "pull",
    read: "pull",
    triage: "triage",
    push: "push",
    write: "push",
    maintain: "maintain",
    admin: "admin",
  };
  return raw ? map[raw] : undefined;
}

export function minimalRepo(
  r: Repository,
  permission?: RepoPermission,
): Schemas["minimal-repository"] {
  const l = r.license;
  const license = l && {
    key: l.key,
    name: l.name,
    url: l.url,
    node_id: l.node_id,
    ...(l.spdx_id && { spdx_id: l.spdx_id }),
  };
  return permission
    ? {
        ...r,
        license,
        permissions: permissionFlags(permission),
        role_name: ROLE_NAME[permission],
      }
    : { ...r, license };
}

export function publicUser(u: SimpleUser): Schemas["public-user"] {
  const p = profileOf(u.login);
  const owned = db.repos().filter((r) => r.owner.login === u.login && !r.private).length;
  return {
    ...u,
    name: p.name,
    user_view_type: "public",
    company: p.company,
    blog: p.blog,
    location: p.location,
    email: p.email,
    hireable: null,
    bio: p.bio,
    twitter_username: p.twitter,
    public_repos: owned,
    public_gists: 0,
    followers: follows.followers(u.login).length,
    following: follows.following(u.login).length,
    created_at: p.created_at,
    updated_at: "2026-11-01T09:00:00Z",
  };
}

// The org as GET /users/{username} answers for it.
export function orgAsUser(): Schemas["public-user"] {
  return {
    ...users.org,
    login: org.login,
    name: org.name ?? null,
    user_view_type: "public",
    company: null,
    blog: org.blog ?? null,
    location: org.location ?? null,
    email: org.email ?? null,
    hireable: null,
    bio: org.description,
    public_repos: org.public_repos,
    public_gists: 0,
    followers: org.followers,
    following: 0,
    created_at: org.created_at,
    updated_at: org.updated_at,
  };
}

// Repositories a user reaches through the org: owners see all, members through teams.
export function reachable(login: string): Map<string, RepoPermission> {
  const out = new Map<string, RepoPermission>();
  const m = members.get(login);
  if (!m) {
    for (const full of outside.get(login) ?? []) out.set(full, "push");
    return out;
  }
  const orgRepos = db.repos().filter((r) => r.owner.login === org.login);
  const base = asPermission(org.default_repository_permission ?? "read");
  for (const r of orgRepos) {
    const key = r.full_name.toLowerCase();
    if (m.role === "admin") out.set(key, "admin");
    else if (base && (!r.private || r.visibility === "internal")) out.set(key, base);
  }
  for (const t of teams) {
    if (!t.members.has(login)) continue;
    for (const [full, p] of t.repos) {
      const prev = out.get(full);
      if (!prev || LADDER.indexOf(p) > LADDER.indexOf(prev)) out.set(full, p);
    }
  }
  return out;
}

export const builtins = { edge, raft, web, dotfiles, notes };
