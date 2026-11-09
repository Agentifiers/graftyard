import type { Schemas } from "@/api/client";
import type { WatchEvent } from "@/areas/activity/ui-api";
import {
  API,
  HOST,
  edge,
  findRepo,
  me,
  raft,
  sha,
  tags,
  user,
  users,
  web,
  type Repository,
  type SimpleUser,
} from "../world";
import { db as core, type Issue, type PullRequest } from "./core-db";

// The activity area's store: notification threads, stars, watches, the event feed, and
// the rate-limit counters. Every record is derived from world.ts and core-db.ts, so a
// notification names an issue the issues list has, and a star count matches the header.

type Thread = Schemas["thread"];
type MinimalRepository = Schemas["minimal-repository"];
type Event = Schemas["event"];
type Actor = Schemas["actor"];

const minutesAgo = (m: number) => new Date(Date.now() - m * 60_000).toISOString();

export function minimal(r: Repository): MinimalRepository {
  const { temp_clone_token, anonymous_access_enabled, license, ...rest } = r;
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

// ---- People who star and watch but are not in world.users ----

const outsiders = [
  user("ana", "Ana Lima"),
  user("kofi", "Kofi Mensah"),
  user("sven", "Sven Aalto"),
  user("priya", "Priya Natarajan"),
  user("jonas", "Jonas Weber"),
  user("yuki", "Yuki Tanaka"),
  user("omar", "Omar Haddad"),
  user("elena", "Elena Petrova"),
];

export const people: SimpleUser[] = [users.mira, users.tomas, users.lin, users.devi, ...outsiders];

// What each person looks like on a user search result.
export const profiles: Record<string, { bio: string; location: string; followers: number }> = {
  mira: { bio: "Edge and storage.", location: "Lisbon", followers: 12 },
  tomas: { bio: "Wire compatibility and label objects.", location: "Porto", followers: 31 },
  lin: { bio: "Raft, learners, and dark mode.", location: "Shanghai", followers: 48 },
  devi: { bio: "Rate limits and on-call.", location: "Bengaluru", followers: 22 },
  ana: { bio: "Rust at the edge.", location: "São Paulo", followers: 9 },
  kofi: { bio: "Consensus and chaos runs.", location: "Accra", followers: 17 },
  sven: { bio: "Git internals.", location: "Helsinki", followers: 5 },
  priya: { bio: "Search relevance.", location: "Chennai", followers: 64 },
  jonas: { bio: "TanStack and React.", location: "Berlin", followers: 14 },
  yuki: { bio: "Dark mode contrast checker.", location: "Osaka", followers: 3 },
  omar: { bio: "Durable Objects.", location: "Amman", followers: 27 },
  elena: { bio: "Release engineering.", location: "Sofia", followers: 11 },
};

// ---- Stars ----

// The people who starred each repository, with when. The repository's stargazers_count
// stays the big seeded number; these are the most recent stargazers the list pages show.
const stargazers = new Map<string, { user: SimpleUser; at: string }[]>([
  [
    "graftyard/edge",
    [users.tomas, users.lin, users.devi, ...outsiders].map((u, i) => ({
      user: u,
      at: minutesAgo(60 * (i * 9 + 3)),
    })),
  ],
  [
    "graftyard/raft",
    [users.lin, users.devi, outsiders[1], outsiders[6], users.mira].map((u, i) => ({
      user: u,
      at: minutesAgo(60 * (i * 30 + 12)),
    })),
  ],
  [
    "graftyard/web",
    [users.devi, outsiders[4], outsiders[5]].map((u, i) => ({
      user: u,
      at: minutesAgo(60 * (i * 20 + 5)),
    })),
  ],
]);

const key = (owner: string, repo: string) => `${owner}/${repo}`.toLowerCase();

function starredBy(login: string) {
  const out: { repo: Repository; at: string }[] = [];
  for (const [full, list] of stargazers) {
    const hit = list.find((s) => s.user.login === login);
    const [o, r] = full.split("/");
    const repo = findRepo(o, r);
    if (hit && repo) out.push({ repo, at: hit.at });
  }
  return out;
}

// ---- Watching ----

type Watch = {
  subscribed: boolean;
  ignored: boolean;
  created_at: string;
  // "Custom" in the web UI: only these events notify. Null when not custom.
  events: WatchEvent[] | null;
};

const watches = new Map<string, Watch>([
  [
    "graftyard/edge",
    { subscribed: true, ignored: false, created_at: "2025-03-02T09:00:00Z", events: null },
  ],
  [
    "graftyard/raft",
    {
      subscribed: true,
      ignored: false,
      created_at: "2025-04-11T10:00:00Z",
      events: ["releases", "security_alerts"],
    },
  ],
]);

// The other people watching each repository. The viewer is added when they watch.
const otherWatchers: Record<string, SimpleUser[]> = {
  "graftyard/edge": [users.tomas, users.devi, outsiders[0], outsiders[3]],
  "graftyard/raft": [users.lin, users.devi, outsiders[1]],
  "graftyard/web": [users.lin, outsiders[4]],
};

function subscription(owner: string, repo: string, w: Watch): Schemas["repository-subscription"] {
  const base = `${API}/repos/${owner}/${repo}`;
  return {
    subscribed: w.subscribed,
    ignored: w.ignored,
    reason: null,
    created_at: w.created_at,
    url: `${base}/subscription`,
    repository_url: base,
  };
}

// ---- Notifications ----

// "none" is a thread with no notification yet, made when an issue page subscribes.
type Box = "inbox" | "done" | "none";
type ThreadState = { thread: Thread; box: Box; saved: boolean; subscribed: boolean };

let threadSeq = 4100;

function threadFor(
  repo: Repository,
  subject: { title: string; type: string; path: string; comment?: string },
  reason: string,
  updated: string,
  unread: boolean,
): Thread {
  const id = String(threadSeq++);
  const url = `${API}/notifications/threads/${id}`;
  return {
    id,
    repository: minimal(repo),
    subject: {
      title: subject.title,
      url: `${repo.url}/${subject.path}`,
      latest_comment_url: subject.comment ?? `${repo.url}/${subject.path}`,
      type: subject.type,
    },
    reason,
    unread,
    updated_at: updated,
    last_read_at: unread ? null : updated,
    url,
    subscription_url: `${url}/subscription`,
  };
}

function issueSubject(_repo: Repository, i: Issue | PullRequest, isPull: boolean) {
  return {
    title: i.title,
    type: isPull ? "PullRequest" : "Issue",
    path: `${isPull ? "pulls" : "issues"}/${i.number}`,
  };
}

const threads: ThreadState[] = [];

function threadForIssueId(id: string): ThreadState | undefined {
  for (const r of core.repos()) {
    const state = core.state(r.owner.login, r.name);
    const i = state?.issues.find((x) => String(x.id) === id);
    if (!state || !i) continue;
    const t = threadFor(
      state.repo,
      issueSubject(state.repo, i, Boolean(i.pull_request)),
      "subscribed",
      i.updated_at,
      false,
    );
    t.id = id;
    t.url = `${API}/notifications/threads/${id}`;
    t.subscription_url = `${t.url}/subscription`;
    const rec: ThreadState = { thread: t, box: "none", saved: false, subscribed: false };
    threads.push(rec);
    return rec;
  }
  return undefined;
}

function push(t: Thread, extra: Partial<Omit<ThreadState, "thread">> = {}) {
  threads.push({ thread: t, box: "inbox", saved: false, subscribed: true, ...extra });
}

function seedNotifications() {
  const edgeState = core.state("graftyard", "edge")!;
  const webState = core.state("graftyard", "web")!;
  const raftState = core.state("graftyard", "raft")!;
  const issueOf = (s: typeof edgeState, n: number) => s.issues.find((i) => i.number === n)!;
  const pullOf = (s: typeof edgeState, n: number) => s.pulls.find((p) => p.number === n)!;

  const rows: [Repository, Issue | PullRequest, boolean, string, number, boolean][] = [
    [edge, pullOf(edgeState, 1), true, "ci_activity", 4, true],
    [edge, pullOf(edgeState, 1), true, "comment", 6, true],
    [edge, issueOf(edgeState, 6), false, "mention", 38, true],
    [edge, pullOf(edgeState, 9), true, "review_requested", 95, true],
    [edge, issueOf(edgeState, 16), false, "assign", 160, true],
    [edge, issueOf(edgeState, 3), false, "author", 300, false],
    [edge, issueOf(edgeState, 2), false, "subscribed", 1500, false],
    [web, issueOf(webState, 1), false, "team_mention", 1440 + 50, true],
    [web, issueOf(webState, 2), false, "subscribed", 1440 + 200, false],
    [raft, issueOf(raftState, 1), false, "state_change", 4 * 1440, false],
    [edge, pullOf(edgeState, 14), true, "author", 2 * 1440, false],
    [edge, issueOf(edgeState, 15), false, "manual", 3 * 1440, false],
  ];
  // Two rows on the same subject (PR #1's CI and its comment) collapse into one thread.
  const seen = new Set<string>();
  for (const [repo, item, isPull, reason, mins, unread] of rows) {
    const k = `${repo.full_name}#${item.number}`;
    if (seen.has(k)) continue;
    seen.add(k);
    push(threadFor(repo, issueSubject(repo, item, isPull), reason, minutesAgo(mins), unread));
  }
  push(
    threadFor(
      edge,
      { title: "v0.4.0-rc.1", type: "Release", path: "releases/4004" },
      "subscribed",
      minutesAgo(2 * 1440 + 120),
      true,
    ),
  );
  push(
    threadFor(
      edge,
      {
        title: "Security advisory: token hash timing",
        type: "RepositoryVulnerabilityAlert",
        path: "security/advisories/1",
      },
      "security_alert",
      minutesAgo(5 * 1440),
      false,
    ),
    { saved: true },
  );
  // Saved and done ones, so every sidebar box has something.
  push(
    threadFor(
      edge,
      issueSubject(edge, issueOf(edgeState, 10), false),
      "comment",
      minutesAgo(4 * 1440),
      false,
    ),
    { saved: true },
  );
  push(
    threadFor(
      edge,
      issueSubject(edge, pullOf(edgeState, 5), true),
      "review_requested",
      minutesAgo(3 * 1440 + 30),
      false,
    ),
    { box: "done" },
  );
  push(
    threadFor(
      edge,
      issueSubject(edge, issueOf(edgeState, 11), false),
      "author",
      minutesAgo(3 * 1440 + 90),
      false,
    ),
    { box: "done" },
  );
}
seedNotifications();

// The mock adds a notification every 30 s, cycling through events a busy repository has.
const arrivals: (() => Thread)[] = [
  () => {
    const pr = core.state("graftyard", "edge")!.pulls.find((p) => p.number === 1)!;
    return threadFor(
      edge,
      issueSubject(edge, pr, true),
      "review_requested",
      new Date().toISOString(),
      true,
    );
  },
  () => {
    const i = core.state("graftyard", "edge")!.issues.find((x) => x.number === 8)!;
    return threadFor(edge, issueSubject(edge, i, false), "mention", new Date().toISOString(), true);
  },
  () => {
    const i = core.state("graftyard", "raft")!.issues.find((x) => x.number === 1)!;
    return threadFor(raft, issueSubject(raft, i, false), "comment", new Date().toISOString(), true);
  },
  () =>
    threadFor(
      web,
      { title: "CI: build failed on main", type: "CheckSuite", path: "check-suites/88" },
      "ci_activity",
      new Date().toISOString(),
      true,
    ),
  () => {
    const i = core.state("graftyard", "edge")!.issues.find((x) => x.number === 12)!;
    return threadFor(edge, issueSubject(edge, i, false), "assign", new Date().toISOString(), true);
  },
];
let arrival = 0;

export const ARRIVAL_MS = 30_000;
// The poll interval the mock advertises, as GHES does on /notifications.
export const POLL_INTERVAL_S = 15;

function arrive() {
  const fresh = arrivals[arrival++ % arrivals.length]();
  // A new event on a subject the inbox already has bumps that thread instead of adding one.
  const existing = threads.find(
    (t) => t.thread.subject.url === fresh.subject.url && t.box === "inbox",
  );
  if (existing) {
    existing.thread.reason = fresh.reason;
    existing.thread.unread = true;
    existing.thread.last_read_at = null;
    existing.thread.updated_at = fresh.updated_at;
    if (fresh.subject.type === "CheckSuite") existing.thread.subject.title = fresh.subject.title;
  } else {
    push(fresh);
  }
}
if (typeof window !== "undefined") setInterval(arrive, ARRIVAL_MS);

// ---- Events (the home feed) ----

const actor = (u: SimpleUser): Actor => ({
  id: u.id,
  login: u.login,
  display_login: u.login,
  gravatar_id: u.gravatar_id,
  url: u.url,
  avatar_url: u.avatar_url,
});

const eventRepo = (r: Repository) => ({ id: r.id, name: r.full_name, url: r.url });

let eventSeq = 52_000_000_000;

function event(
  type: string,
  who: SimpleUser,
  repo: Repository,
  payload: Event["payload"],
  at: string,
): Event {
  return {
    id: String(eventSeq++),
    type,
    actor: actor(who),
    repo: eventRepo(repo),
    org: actor(users.org),
    payload,
    public: repo.visibility === "public",
    created_at: at,
  };
}

function release(
  repo: Repository,
  tag: string,
  author: SimpleUser,
  at: string,
): Schemas["release"] {
  const n = parseInt(sha(`release:${repo.full_name}:${tag}`).slice(0, 6), 16);
  return {
    url: `${repo.url}/releases/${n}`,
    html_url: `${repo.html_url}/releases/tag/${tag}`,
    assets_url: `${repo.url}/releases/${n}/assets`,
    upload_url: `${HOST}/api/uploads/repos/${repo.full_name}/releases/${n}/assets{?name,label}`,
    tarball_url: `${repo.url}/tarball/${tag}`,
    zipball_url: `${repo.url}/zipball/${tag}`,
    id: n,
    node_id: `RE_${sha(`${n}`).slice(0, 12)}`,
    tag_name: tag,
    target_commitish: "main",
    name: tag,
    body: `Release ${tag}.`,
    draft: false,
    prerelease: tag.includes("-rc"),
    created_at: at,
    published_at: at,
    author,
    assets: [],
  };
}

function pushEvent(
  repo: Repository,
  who: SimpleUser,
  branch: string,
  messages: string[],
  at: string,
): Event {
  const head = sha(`${repo.full_name}:${branch}`);
  return event(
    "PushEvent",
    who,
    repo,
    {
      repository_id: repo.id,
      push_id: eventSeq,
      ref: `refs/heads/${branch}`,
      head,
      before: sha(`${repo.full_name}:${branch}:before`),
      size: messages.length,
      distinct_size: messages.length,
      commits: messages.map((message, i) => ({
        sha: i === messages.length - 1 ? head : sha(`${repo.full_name}:${branch}:${i}`),
        message,
        author: { name: who.name ?? who.login, email: `${who.login}@graftyard.test` },
        url: `${repo.url}/commits/${head}`,
        distinct: true,
      })),
    },
    at,
  );
}

const events: Event[] = [];

function seedEvents() {
  const edgeState = core.state("graftyard", "edge")!;
  const pr = (n: number) => edgeState.pulls.find((p) => p.number === n)!;
  const iss = (s: ReturnType<typeof core.state>, n: number) =>
    s!.issues.find((i) => i.number === n)!;
  const comments = edgeState.comments.get(1) ?? [];
  const lastComment = comments.at(-1);
  const list: Event[] = [
    pushEvent(
      edge,
      users.mira,
      "mira/idempotent-retry",
      ["Cap retries at 2", "Full jitter on backoff"],
      minutesAgo(9),
    ),
    ...(lastComment
      ? [
          event(
            "IssueCommentEvent",
            users.tomas,
            edge,
            { action: "created", issue: iss(edgeState, 1), comment: lastComment },
            minutesAgo(14),
          ),
        ]
      : []),
    event("WatchEvent", outsiders[0], edge, { action: "started" }, minutesAgo(41)),
    event(
      "PullRequestEvent",
      users.lin,
      edge,
      { action: "opened", number: 9, pull_request: pr(9) },
      minutesAgo(80),
    ),
    event(
      "IssuesEvent",
      users.tomas,
      edge,
      { action: "opened", issue: iss(edgeState, 16) },
      minutesAgo(190),
    ),
    event(
      "CreateEvent",
      users.devi,
      edge,
      {
        ref: "release/0.4",
        ref_type: "branch",
        master_branch: "main",
        description: edge.description,
        pusher_type: "user",
      },
      minutesAgo(400),
    ),
    event(
      "ReleaseEvent",
      users.devi,
      edge,
      {
        action: "published",
        release: release(edge, tags["graftyard/edge"].at(-1)!, users.devi, minutesAgo(2 * 1440)),
      },
      minutesAgo(2 * 1440),
    ),
    event(
      "PullRequestEvent",
      users.devi,
      edge,
      { action: "closed", number: 5, pull_request: pr(5) },
      minutesAgo(2 * 1440 + 60),
    ),
    event(
      "IssuesEvent",
      users.lin,
      web,
      { action: "opened", issue: iss(core.state("graftyard", "web"), 1) },
      minutesAgo(1440 + 300),
    ),
    pushEvent(
      web,
      users.lin,
      "lin/dark-diff",
      ["Raise deletion contrast to 4.6:1"],
      minutesAgo(1440 + 320),
    ),
    event("WatchEvent", outsiders[1], raft, { action: "started" }, minutesAgo(3 * 1440)),
    event(
      "ForkEvent",
      outsiders[2],
      edge,
      {
        forkee: {
          id: 99_001,
          name: "edge",
          full_name: "sven/edge",
          owner: outsiders[2],
          html_url: `${HOST}/sven/edge`,
          fork: true,
          private: false,
        },
      },
      minutesAgo(3 * 1440 + 200),
    ),
    event(
      "DeleteEvent",
      users.tomas,
      edge,
      { ref: "tomas/old-labels", ref_type: "branch", pusher_type: "user" },
      minutesAgo(4 * 1440),
    ),
    event(
      "IssuesEvent",
      users.tomas,
      raft,
      { action: "opened", issue: iss(core.state("graftyard", "raft"), 1) },
      minutesAgo(6 * 1440),
    ),
  ];
  events.push(...list);
}
seedEvents();

// ---- Rate limit ----

const HOUR = 3600;
type Bucket =
  | "core"
  | "search"
  | "code_search"
  | "graphql"
  | "integration_manifest"
  | "source_import";
const limits: Record<Bucket, { limit: number; window: number }> = {
  core: { limit: 5000, window: HOUR },
  search: { limit: 30, window: 60 },
  code_search: { limit: 10, window: 60 },
  graphql: { limit: 5000, window: HOUR },
  integration_manifest: { limit: 5000, window: HOUR },
  source_import: { limit: 100, window: 60 },
};
const now = () => Math.floor(Date.now() / 1000);
const used: Record<Bucket, { used: number; reset: number }> = {
  core: { used: 812, reset: now() + 41 * 60 },
  search: { used: 4, reset: now() + 38 },
  code_search: { used: 1, reset: now() + 52 },
  graphql: { used: 1290, reset: now() + 23 * 60 },
  integration_manifest: { used: 0, reset: now() + HOUR },
  source_import: { used: 0, reset: now() + 60 },
};

function bucket(b: Bucket): Schemas["rate-limit"] {
  const u = used[b];
  if (u.reset <= now()) {
    u.used = 0;
    u.reset = now() + limits[b].window;
  }
  return {
    limit: limits[b].limit,
    used: u.used,
    remaining: limits[b].limit - u.used,
    reset: u.reset,
  };
}

// ---- The store API the handlers use ----

export const activity = {
  // Notifications
  threads: (box: Box) => threads.filter((t) => t.box === box),
  saved: () => threads.filter((t) => t.saved),
  thread: (id: string) => threads.find((t) => t.thread.id === id),
  markRead(id: string) {
    const t = threads.find((x) => x.thread.id === id);
    if (!t) return false;
    t.thread.unread = false;
    t.thread.last_read_at = new Date().toISOString();
    return true;
  },
  markDone(id: string) {
    const t = threads.find((x) => x.thread.id === id);
    if (!t) return false;
    t.box = "done";
    t.thread.unread = false;
    return true;
  },
  markUnread(id: string) {
    const t = threads.find((x) => x.thread.id === id);
    if (!t) return false;
    t.thread.unread = true;
    t.thread.last_read_at = null;
    return true;
  },
  moveToInbox(id: string) {
    const t = threads.find((x) => x.thread.id === id);
    if (!t || t.box !== "done") return false;
    t.box = "inbox";
    return true;
  },
  markAllRead(before: string, repo?: string) {
    let n = 0;
    for (const t of threads) {
      if (t.box !== "inbox" || !t.thread.unread || t.thread.updated_at > before) continue;
      if (repo && t.thread.repository.full_name.toLowerCase() !== repo) continue;
      t.thread.unread = false;
      t.thread.last_read_at = before;
      n++;
    }
    return n;
  },
  setSaved(id: string, saved: boolean) {
    const t = threads.find((x) => x.thread.id === id);
    if (!t) return false;
    t.saved = saved;
    return true;
  },
  threadSubscription(t: ThreadState, ignored = false): Schemas["thread-subscription"] {
    return {
      subscribed: t.subscribed,
      ignored,
      reason: t.subscribed ? t.thread.reason : null,
      created_at: t.thread.updated_at,
      url: t.thread.subscription_url,
      thread_url: t.thread.url,
      repository_url: t.thread.repository.url,
    };
  },
  setThreadSubscribed(id: string, subscribed: boolean) {
    const t = threads.find((x) => x.thread.id === id) ?? threadForIssueId(id);
    if (!t) return undefined;
    t.subscribed = subscribed;
    return t;
  },
  // The issue page names its thread by the issue's id. A thread for an issue nobody was
  // notified about yet starts unsubscribed and out of the inbox, as GHES does.
  threadOrIssue: (id: string) => threads.find((t) => t.thread.id === id) ?? threadForIssueId(id),

  // Stars
  stargazers: (owner: string, repo: string) =>
    [...(stargazers.get(key(owner, repo)) ?? [])].sort((a, b) => b.at.localeCompare(a.at)),
  isStarred: (owner: string, repo: string) =>
    (stargazers.get(key(owner, repo)) ?? []).some((s) => s.user.login === me.login),
  star(owner: string, repo: string) {
    const r = findRepo(owner, repo);
    if (!r) return false;
    const list = stargazers.get(key(owner, repo)) ?? [];
    if (!list.some((s) => s.user.login === me.login)) {
      list.push({ user: users.mira, at: new Date().toISOString() });
      stargazers.set(key(owner, repo), list);
      r.stargazers_count += 1;
      r.watchers_count = r.watchers = r.stargazers_count;
      events.unshift(
        event("WatchEvent", users.mira, r, { action: "started" }, new Date().toISOString()),
      );
    }
    return true;
  },
  unstar(owner: string, repo: string) {
    const r = findRepo(owner, repo);
    if (!r) return false;
    const list = stargazers.get(key(owner, repo)) ?? [];
    const at = list.findIndex((s) => s.user.login === me.login);
    if (at >= 0) {
      list.splice(at, 1);
      r.stargazers_count -= 1;
      r.watchers_count = r.watchers = r.stargazers_count;
    }
    return true;
  },
  starredBy,

  // Watching
  watch: (owner: string, repo: string) => watches.get(key(owner, repo)),
  subscription,
  setWatch(owner: string, repo: string, body: { subscribed?: boolean; ignored?: boolean }) {
    const prev = watches.get(key(owner, repo));
    const w: Watch = {
      subscribed: Boolean(body.subscribed) && !body.ignored,
      ignored: Boolean(body.ignored),
      created_at: prev?.created_at ?? new Date().toISOString(),
      // "All activity" and "Ignore" both replace a custom choice.
      events: null,
    };
    watches.set(key(owner, repo), w);
    return w;
  },
  setCustom(owner: string, repo: string, events: WatchEvent[]) {
    const prev = watches.get(key(owner, repo));
    const w: Watch = {
      subscribed: false,
      ignored: false,
      created_at: prev?.created_at ?? new Date().toISOString(),
      events,
    };
    watches.set(key(owner, repo), w);
    return w;
  },
  clearCustom(owner: string, repo: string) {
    const w = watches.get(key(owner, repo));
    if (w) w.events = null;
  },
  unwatch(owner: string, repo: string) {
    watches.delete(key(owner, repo));
  },
  watchers(owner: string, repo: string): SimpleUser[] {
    const w = watches.get(key(owner, repo));
    const mine = w && (w.subscribed || w.events) ? [users.mira] : [];
    return [...mine, ...(otherWatchers[key(owner, repo)] ?? [])];
  },
  watched: () =>
    core.repos().filter((r) => {
      const w = watches.get(r.full_name.toLowerCase());
      return w && (w.subscribed || w.events);
    }),

  // Events
  events: () => [...events].sort((a, b) => (b.created_at ?? "").localeCompare(a.created_at ?? "")),

  // Rate limit
  bucket,
  spend(b: Bucket) {
    bucket(b);
    used[b].used = Math.min(limits[b].limit, used[b].used + 1);
    return bucket(b);
  },
};

export type { Bucket };
