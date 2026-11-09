import type { Schemas } from "@/api/client";
import { id, orgMembers, sha, users, type SimpleUser } from "../world";
import {
  addComment,
  db,
  issue,
  label as coreLabel,
  LABELS,
  type Issue,
  type Label as PullLabel,
  type RepoState,
} from "./core-db";

// The issues area's own state on top of core-db's issues and comments: repository labels,
// milestones, the event log behind the timeline, and reactions. Issues and comments stay
// in core-db, and every write here mutates those records in place so the lists, the PR
// page, and other areas read the same objects.

type Label = Schemas["label"];
type Milestone = Schemas["milestone"];
type Reaction = Schemas["reaction"];
type ReactionContent = Reaction["content"];

// One stored event per kind of change. Each kind keeps the GHES schema it is served as, so
// the timeline serves `ev` as is and the events endpoint serves the kinds its union allows.
export type StoredEvent =
  | { kind: "label"; ev: Schemas["labeled-issue-event"] } // labeled, unlabeled
  | { kind: "milestone"; ev: Schemas["milestoned-issue-event"] } // milestoned, demilestoned
  | { kind: "rename"; ev: Schemas["renamed-issue-event"] }
  | { kind: "lock"; ev: Schemas["locked-issue-event"] } // locked, unlocked
  | { kind: "assign"; ev: Schemas["timeline-assigned-issue-event"] } // assigned, unassigned
  | { kind: "state"; ev: Schemas["state-change-issue-event"] } // closed, reopened, referenced
  | { kind: "xref"; ev: Schemas["timeline-cross-referenced-event"] };

type Extras = {
  labels: Label[];
  milestones: Milestone[];
  events: Map<number, StoredEvent[]>;
  reactions: Map<string, Reaction[]>;
};

const extras = new Map<string, Extras>();

export const REACTIONS = [
  "+1",
  "-1",
  "laugh",
  "hooray",
  "confused",
  "heart",
  "rocket",
  "eyes",
] as const satisfies readonly ReactionContent[];

const EXTRA_LABELS: Record<string, [string, string]> = {
  enhancement: ["a2eeef", "New feature or request"],
  question: ["d876e3", "Further information is requested"],
  wontfix: ["ffffff", "This will not be worked on"],
};

function repoLabel(state: RepoState, name: string, color: string, description: string): Label {
  return {
    id: parseInt(sha(`${state.repo.full_name}:label:${name}`).slice(0, 7), 16),
    node_id: `LA_${sha(`${state.repo.full_name}:${name}`).slice(0, 12)}`,
    url: `${state.repo.url}/labels/${encodeURIComponent(name)}`,
    name,
    description,
    color,
    default: false,
    archived_at: null,
    archived_by: null,
  };
}

export function extrasFor(state: RepoState): Extras {
  const key = state.repo.full_name.toLowerCase();
  let x = extras.get(key);
  if (!x) {
    const labels: Label[] = [
      ...Object.keys(LABELS).map((n) => ({ ...coreLabel(state.repo, n), archived_at: null })),
      ...Object.entries(EXTRA_LABELS).map(([n, [c, d]]) => repoLabel(state, n, c, d)),
    ];
    x = { labels, milestones: [], events: new Map(), reactions: new Map() };
    extras.set(key, x);
  }
  return x;
}

// ---- Labels ----

// The label as it appears on an issue and on a pull request record.
function onIssue(l: Label) {
  return {
    id: l.id,
    node_id: l.node_id,
    url: l.url,
    name: l.name,
    description: l.description ?? "",
    color: l.color,
    default: l.default,
    archived_by: null,
  };
}

export function labelNames(i: Issue): string[] {
  return i.labels.map((l) => (typeof l === "string" ? l : (l.name ?? "")));
}

export function createLabel(
  state: RepoState,
  input: { name: string; color: string; description: string },
) {
  const l = repoLabel(state, input.name, input.color, input.description);
  l.id = id();
  extrasFor(state).labels.push(l);
  return l;
}

// Renaming or recolouring a label changes it on every issue and pull request that has it.
export function updateLabel(
  state: RepoState,
  l: Label,
  patch: { name?: string; color?: string; description?: string },
) {
  const old = l.name;
  if (patch.name !== undefined) {
    l.name = patch.name;
    l.url = `${state.repo.url}/labels/${encodeURIComponent(patch.name)}`;
  }
  if (patch.color !== undefined) l.color = patch.color;
  if (patch.description !== undefined) l.description = patch.description;
  const fresh = onIssue(l);
  for (const i of state.issues) {
    i.labels = i.labels.map((x) => (typeof x !== "string" && x.name === old ? fresh : x));
  }
  for (const p of state.pulls)
    p.labels = p.labels.map(
      (x): PullLabel => (x.name === old ? fresh : { ...x, description: x.description ?? "" }),
    );
}

export function deleteLabel(state: RepoState, l: Label) {
  const x = extrasFor(state);
  x.labels = x.labels.filter((y) => y !== l);
  for (const i of state.issues) {
    i.labels = i.labels.filter((y) => (typeof y === "string" ? y : y.name) !== l.name);
  }
  for (const p of state.pulls) {
    const kept = new Set(p.labels.map((y) => y.name).filter((n) => n !== l.name));
    p.labels = x.labels.filter((y) => kept.has(y.name)).map(onIssue);
  }
}

// Sets an issue's labels to exactly `names` (unknown names are dropped) and records one
// labeled or unlabeled event per change.
export function setIssueLabels(state: RepoState, i: Issue, names: string[], actor: SimpleUser) {
  const repoLabels = extrasFor(state).labels;
  const wanted = repoLabels.filter((l) => names.includes(l.name));
  const before = new Set(labelNames(i));
  const after = new Set(wanted.map((l) => l.name));
  const now = new Date().toISOString();
  for (const l of wanted)
    if (!before.has(l.name)) labelEvent(state, i.number, actor, "labeled", l, now);
  for (const n of before) {
    if (after.has(n)) continue;
    const l = repoLabels.find((x) => x.name === n);
    labelEvent(state, i.number, actor, "unlabeled", l ?? { name: n, color: "ededed" }, now);
  }
  i.labels = wanted.map(onIssue);
  const pr = state.pulls.find((p) => p.number === i.number);
  if (pr) pr.labels = wanted.map(onIssue);
  touch(i, now);
  return wanted;
}

// ---- Milestones ----

export function milestone(
  state: RepoState,
  s: {
    number: number;
    title: string;
    description: string | null;
    due: string | null;
    closed?: string;
    created: string;
  },
): Milestone {
  const url = `${state.repo.url}/milestones/${s.number}`;
  return {
    url,
    html_url: `${state.repo.html_url}/milestone/${s.number}`,
    labels_url: `${url}/labels`,
    id: id(),
    node_id: `MI_${sha(`${state.repo.full_name}:milestone:${s.number}`).slice(0, 12)}`,
    number: s.number,
    state: s.closed ? "closed" : "open",
    title: s.title,
    description: s.description,
    creator: users.mira,
    open_issues: 0,
    closed_issues: 0,
    created_at: s.created,
    updated_at: s.closed ?? s.created,
    closed_at: s.closed ?? null,
    due_on: s.due,
  };
}

// Open and closed counts follow the issues, so recount before any read.
export function recountMilestones(state: RepoState) {
  for (const m of extrasFor(state).milestones) {
    const mine = state.issues.filter((i) => i.milestone?.number === m.number);
    m.open_issues = mine.filter((i) => i.state === "open").length;
    m.closed_issues = mine.length - m.open_issues;
  }
}

export function setIssueMilestone(
  state: RepoState,
  i: Issue,
  m: Milestone | null,
  actor: SimpleUser,
) {
  const prev = i.milestone;
  if (prev?.number === m?.number) return;
  const now = new Date().toISOString();
  if (prev) milestoneEvent(state, i.number, actor, "demilestoned", prev.title, now);
  if (m) milestoneEvent(state, i.number, actor, "milestoned", m.title, now);
  i.milestone = m;
  const pr = state.pulls.find((p) => p.number === i.number);
  if (pr) pr.milestone = m;
  touch(i, now);
}

export function deleteMilestone(state: RepoState, m: Milestone) {
  const x = extrasFor(state);
  x.milestones = x.milestones.filter((y) => y !== m);
  for (const i of state.issues) if (i.milestone?.number === m.number) i.milestone = null;
  for (const p of state.pulls) if (p.milestone?.number === m.number) p.milestone = null;
}

// ---- Assignees ----

export const assignable = orgMembers;

export function setIssueAssignees(state: RepoState, i: Issue, logins: string[], actor: SimpleUser) {
  const wanted = assignable.filter((u) => logins.includes(u.login));
  const before = new Set((i.assignees ?? []).map((u) => u.login));
  const after = new Set(wanted.map((u) => u.login));
  const now = new Date().toISOString();
  for (const u of wanted)
    if (!before.has(u.login)) assignEvent(state, i.number, actor, "assigned", u, now);
  for (const u of i.assignees ?? []) {
    if (!after.has(u.login)) assignEvent(state, i.number, actor, "unassigned", u, now);
  }
  i.assignees = wanted;
  i.assignee = wanted[0] ?? null;
  const pr = state.pulls.find((p) => p.number === i.number);
  if (pr) {
    pr.assignees = wanted;
    pr.assignee = wanted[0] ?? null;
  }
  touch(i, now);
}

// ---- State, title, lock ----

export function setIssueState(
  state: RepoState,
  i: Issue,
  next: "open" | "closed",
  reason: Issue["state_reason"],
  actor: SimpleUser,
) {
  if (i.state === next && (next === "open" || i.state_reason === reason)) return;
  const now = new Date().toISOString();
  const wasOpen = i.state === "open";
  i.state = next;
  i.state_reason = next === "open" ? (wasOpen ? null : "reopened") : (reason ?? "completed");
  i.closed_at = next === "closed" ? now : null;
  i.closed_by = next === "closed" ? actor : null;
  stateEvent(state, i.number, actor, next === "closed" ? "closed" : "reopened", now, {
    state_reason: i.state_reason,
  });
  const delta = wasOpen === (next === "open") ? 0 : next === "open" ? 1 : -1;
  state.repo.open_issues_count = state.repo.open_issues = state.repo.open_issues + delta;
  touch(i, now);
}

export function renameIssue(state: RepoState, i: Issue, title: string, actor: SimpleUser) {
  if (i.title === title) return;
  const now = new Date().toISOString();
  push(state, i.number, {
    kind: "rename",
    ev: { ...base(state, actor, "renamed", now), rename: { from: i.title, to: title } },
  });
  i.title = title;
  const pr = state.pulls.find((p) => p.number === i.number);
  if (pr) pr.title = title;
  touch(i, now);
}

export function lockIssue(state: RepoState, i: Issue, reason: string | null, actor: SimpleUser) {
  const now = new Date().toISOString();
  push(state, i.number, {
    kind: "lock",
    ev: { ...base(state, actor, "locked", now), lock_reason: reason },
  });
  i.locked = true;
  i.active_lock_reason = reason;
  touch(i, now);
}

export function unlockIssue(state: RepoState, i: Issue, actor: SimpleUser) {
  const now = new Date().toISOString();
  push(state, i.number, {
    kind: "lock",
    ev: { ...base(state, actor, "unlocked", now), lock_reason: null },
  });
  i.locked = false;
  i.active_lock_reason = null;
  touch(i, now);
}

function touch(i: Issue, at: string) {
  i.updated_at = at;
}

// ---- Events ----

function base(state: RepoState, actor: SimpleUser, event: string, at: string) {
  const n = id();
  return {
    id: n,
    node_id: `IE_${sha(`event:${n}`).slice(0, 12)}`,
    url: `${state.repo.url}/issues/events/${n}`,
    actor,
    event,
    commit_id: null,
    commit_url: null,
    created_at: at,
    performed_via_github_app: null,
  };
}

function push(state: RepoState, number: number, e: StoredEvent) {
  const map = extrasFor(state).events;
  map.set(number, [...(map.get(number) ?? []), e]);
}

function labelEvent(
  state: RepoState,
  number: number,
  actor: SimpleUser,
  event: "labeled" | "unlabeled",
  l: { name: string; color: string },
  at: string,
) {
  push(state, number, {
    kind: "label",
    ev: { ...base(state, actor, event, at), label: { name: l.name, color: l.color } },
  });
}

function milestoneEvent(
  state: RepoState,
  number: number,
  actor: SimpleUser,
  event: "milestoned" | "demilestoned",
  title: string,
  at: string,
) {
  push(state, number, {
    kind: "milestone",
    ev: { ...base(state, actor, event, at), milestone: { title } },
  });
}

function assignEvent(
  state: RepoState,
  number: number,
  actor: SimpleUser,
  event: "assigned" | "unassigned",
  assignee: SimpleUser,
  at: string,
) {
  push(state, number, { kind: "assign", ev: { ...base(state, actor, event, at), assignee } });
}

function stateEvent(
  state: RepoState,
  number: number,
  actor: SimpleUser,
  event: "closed" | "reopened" | "referenced",
  at: string,
  extra: { state_reason?: string | null; commit?: string } = {},
) {
  const ev: Schemas["state-change-issue-event"] = {
    ...base(state, actor, event, at),
    state_reason: extra.state_reason ?? null,
  };
  if (extra.commit) {
    ev.commit_id = extra.commit;
    ev.commit_url = `${state.repo.url}/commits/${extra.commit}`;
  }
  push(state, number, { kind: "state", ev });
}

// `#N` in a body posted on issue `from` cross-references issue N, as GHES records it.
export function crossReference(state: RepoState, from: Issue, body: string, actor: SimpleUser) {
  const at = new Date().toISOString();
  const seen = new Set<number>();
  for (const m of body.matchAll(/(?:^|[\s(])#(\d+)\b/g)) {
    const n = Number(m[1]);
    if (n === from.number || seen.has(n)) continue;
    seen.add(n);
    if (!state.issues.some((i) => i.number === n)) continue;
    push(state, n, {
      kind: "xref",
      ev: {
        event: "cross-referenced",
        actor,
        created_at: at,
        updated_at: at,
        source: { type: "issue", issue: { ...from, repository: state.repo } },
      },
    });
  }
}

export function events(state: RepoState, number: number): StoredEvent[] {
  return extrasFor(state).events.get(number) ?? [];
}

// ---- Reactions ----

type Subject = { kind: "issue"; number: number } | { kind: "comment"; id: number };

const subjectKey = (s: Subject) => (s.kind === "issue" ? `issue:${s.number}` : `comment:${s.id}`);

export function reactions(state: RepoState, s: Subject): Reaction[] {
  return extrasFor(state).reactions.get(subjectKey(s)) ?? [];
}

function rollup(url: string, list: Reaction[]): Schemas["reaction-rollup"] {
  const count = (c: ReactionContent) => list.filter((r) => r.content === c).length;
  return {
    url: `${url}/reactions`,
    total_count: list.length,
    "+1": count("+1"),
    "-1": count("-1"),
    laugh: count("laugh"),
    confused: count("confused"),
    heart: count("heart"),
    hooray: count("hooray"),
    eyes: count("eyes"),
    rocket: count("rocket"),
  };
}

export function findComment(state: RepoState, commentId: number) {
  for (const [number, list] of state.comments) {
    const c = list.find((x) => x.id === commentId);
    if (c) return { number, list, comment: c };
  }
  return undefined;
}

function syncRollup(state: RepoState, s: Subject) {
  const list = reactions(state, s);
  if (s.kind === "issue") {
    const i = state.issues.find((x) => x.number === s.number);
    if (i) i.reactions = rollup(i.url, list);
  } else {
    const c = findComment(state, s.id)?.comment;
    if (c) c.reactions = rollup(c.url, list);
  }
}

// GHES answers 200 with the existing reaction when the user already reacted with it.
export function react(
  state: RepoState,
  s: Subject,
  user: SimpleUser,
  content: ReactionContent,
  at?: string,
) {
  const map = extrasFor(state).reactions;
  const list = map.get(subjectKey(s)) ?? [];
  const existing = list.find((r) => r.user?.login === user.login && r.content === content);
  if (existing) return { reaction: existing, created: false };
  const n = id();
  const reaction: Reaction = {
    id: n,
    node_id: `REA_${sha(`reaction:${n}`).slice(0, 12)}`,
    user,
    content,
    created_at: at ?? new Date().toISOString(),
  };
  map.set(subjectKey(s), [...list, reaction]);
  syncRollup(state, s);
  return { reaction, created: true };
}

export function unreact(state: RepoState, s: Subject, reactionId: number) {
  const map = extrasFor(state).reactions;
  const list = map.get(subjectKey(s)) ?? [];
  if (!list.some((r) => r.id === reactionId)) return false;
  map.set(
    subjectKey(s),
    list.filter((r) => r.id !== reactionId),
  );
  syncRollup(state, s);
  return true;
}

export function syncAllRollups(state: RepoState) {
  for (const i of state.issues) syncRollup(state, { kind: "issue", number: i.number });
  for (const list of state.comments.values()) {
    for (const c of list) syncRollup(state, { kind: "comment", id: c.id });
  }
}

// ---- Seed ----

const hour = (at: string, h: number) =>
  new Date(new Date(at).getTime() + h * 3_600_000).toISOString();

function seedExtraIssues(state: RepoState) {
  const seeds: (Parameters<typeof issue>[1] & { notPlanned?: boolean })[] = [
    {
      number: 17,
      title: "Honour If-Modified-Since on issue lists",
      body: "Octokit sends `If-Modified-Since` on polled lists. Answer 304 when nothing changed since that time.",
      author: users.devi,
      labels: ["perf", "wire"],
      created: "2026-10-26T08:00:00Z",
      updated: "2026-10-27T10:00:00Z",
    },
    {
      number: 18,
      title: "Webhook deliveries retry with no jitter",
      body: "Every failed delivery retries at exactly 10 s, 20 s, 40 s. A receiver that went down gets a thundering herd when it returns.",
      author: users.tomas,
      labels: ["bug", "edge"],
      created: "2026-10-27T09:30:00Z",
      updated: "2026-11-02T12:00:00Z",
    },
    {
      number: 19,
      title: "Document the fault keys the mocked web app gates on",
      body: "List every `gate()` key so testers know what `?fail=` accepts.",
      author: users.lin,
      labels: ["docs"],
      closed: "2026-11-05T09:00:00Z",
      created: "2026-10-27T14:00:00Z",
      updated: "2026-11-05T09:00:00Z",
    },
    {
      number: 20,
      title: "Support sort=comments on the issues list",
      body: "`GET /repos/{owner}/{repo}/issues?sort=comments` falls back to `created`. GHES sorts by comment count.",
      author: users.mira,
      labels: ["wire"],
      created: "2026-10-28T07:10:00Z",
      updated: "2026-10-30T15:00:00Z",
    },
    {
      number: 21,
      title: "Timeline API misses cross-referenced events from PR bodies",
      body: "A PR body with `Closes #4` should add a `cross-referenced` event to #4's timeline.",
      author: users.tomas,
      labels: ["wire", "bug"],
      created: "2026-10-29T16:00:00Z",
      updated: "2026-11-03T11:00:00Z",
    },
    {
      number: 22,
      title: "Reactions rollup is missing from issue comments",
      body: "GHES includes `reactions` with `total_count` and a count per content on every comment.",
      author: users.devi,
      labels: ["wire"],
      created: "2026-10-30T09:00:00Z",
      updated: "2026-11-01T09:00:00Z",
    },
    {
      number: 23,
      title: "Lock reasons should match GHES's four values",
      body: "`off-topic`, `too heated`, `resolved`, and `spam`. Anything else is a 422.",
      author: users.lin,
      labels: ["wire", "good first issue"],
      closed: "2026-11-02T17:00:00Z",
      created: "2026-10-31T10:00:00Z",
      updated: "2026-11-02T17:00:00Z",
    },
    {
      number: 24,
      title: "Renaming a label drops it from closed issues",
      body: "Rename `perf` to `performance` and the closed issues lose the label.",
      author: users.mira,
      labels: ["bug"],
      created: "2026-11-01T08:00:00Z",
      updated: "2026-11-06T13:00:00Z",
    },
    {
      number: 25,
      title: "Milestone due dates drift by a day west of UTC",
      body: "A milestone due on the 20th shows the 19th in `America/Los_Angeles`. `due_on` is a date at 07:00 UTC, not midnight.",
      author: users.lin,
      labels: ["bug", "ui"],
      created: "2026-11-02T11:00:00Z",
      updated: "2026-11-07T09:30:00Z",
    },
    {
      number: 26,
      title: "Expose active_lock_reason on locked issues",
      body: "We only send `locked: true`.",
      author: users.tomas,
      labels: ["wire"],
      closed: "2026-11-04T08:00:00Z",
      notPlanned: true,
      created: "2026-11-02T15:00:00Z",
      updated: "2026-11-04T08:00:00Z",
    },
    {
      number: 27,
      title: "Rate limit the markdown render endpoint",
      body: "`POST /markdown` is the most expensive anonymous call. Give it its own bucket.",
      author: users.devi,
      labels: ["perf"],
      created: "2026-11-03T10:00:00Z",
      updated: "2026-11-05T10:00:00Z",
    },
    {
      number: 28,
      title: "Assignee check returns 204 for non-collaborators",
      body: "`GET /repos/{owner}/{repo}/assignees/{assignee}` should 404 for a user who cannot be assigned.",
      author: users.tomas,
      labels: ["bug", "wire"],
      created: "2026-11-04T13:00:00Z",
      updated: "2026-11-06T08:00:00Z",
    },
    {
      number: 29,
      title: "Add a dark mode screenshot to the README",
      body: "Nice to have.",
      author: users.lin,
      labels: ["docs", "ui"],
      closed: "2026-11-06T18:00:00Z",
      notPlanned: true,
      created: "2026-11-05T12:00:00Z",
      updated: "2026-11-06T18:00:00Z",
    },
    {
      number: 30,
      title: 'Pagination Link header is missing rel="prev"',
      body: 'Page 2 of 3 sends `rel="next"` and `rel="last"` but no `rel="prev"` or `rel="first"`.',
      author: users.mira,
      labels: ["wire"],
      created: "2026-11-06T09:00:00Z",
      updated: "2026-11-08T08:15:00Z",
    },
  ];
  for (const s of seeds) {
    const i = issue(state.repo, s);
    if (s.notPlanned) i.state_reason = "not_planned";
    state.issues.push(i);
  }
  addComment(
    state.repo.full_name,
    18,
    users.devi,
    "Full jitter like the client retry in #1 would fix it.",
    "2026-11-02T12:00:00Z",
  );
  addComment(
    state.repo.full_name,
    24,
    users.lin,
    "Reproduced. The rename rewrites open issues only.",
    "2026-11-06T13:00:00Z",
  );
  addComment(
    state.repo.full_name,
    26,
    users.mira,
    "Not planned. `active_lock_reason` is already sent; the bug was in the client.",
    "2026-11-04T08:00:00Z",
  );
}

function seedRepo(state: RepoState, full: string) {
  const x = extrasFor(state);
  if (full === "graftyard/edge") {
    seedExtraIssues(state);
    // core-db's builder knows only its own labels; these use the area's extra ones.
    for (const [n, name] of [
      [27, "enhancement"],
      [20, "question"],
      [29, "wontfix"],
    ] as const) {
      const i = state.issues.find((y) => y.number === n);
      const l = x.labels.find((y) => y.name === name);
      if (i && l) i.labels.push(onIssue(l));
    }
    x.milestones = [
      milestone(state, {
        number: 1,
        title: "v0.3.1",
        description: "Error responses carry the rate limit headers.",
        due: "2026-11-05T07:00:00Z",
        closed: "2026-11-05T19:00:00Z",
        created: "2026-10-20T09:00:00Z",
      }),
      milestone(state, {
        number: 2,
        title: "v0.4.0",
        description: "Wire compatibility for the issues and checks APIs.",
        due: "2026-11-20T07:00:00Z",
        created: "2026-10-24T09:00:00Z",
      }),
      milestone(state, {
        number: 3,
        title: "v0.5.0",
        description: "Learner reads and conditional requests.",
        due: "2026-12-18T07:00:00Z",
        created: "2026-10-30T09:00:00Z",
      }),
      milestone(state, {
        number: 4,
        title: "Web app preview",
        description: "The mocked web app, end to end.",
        due: null,
        created: "2026-11-01T09:00:00Z",
      }),
      milestone(state, {
        number: 5,
        title: "Lifeboat drill",
        description: "Run the workerd lifeboat install against the hosted install's backup.",
        due: "2026-11-01T07:00:00Z",
        created: "2026-10-12T09:00:00Z",
      }),
    ];
  } else if (full === "graftyard/web") {
    x.milestones = [
      milestone(state, {
        number: 1,
        title: "Preview",
        description: "The first usable build.",
        due: "2026-11-27T07:00:00Z",
        created: "2026-11-01T09:00:00Z",
      }),
    ];
  }

  const byNumber = (n: number) => state.issues.find((i) => i.number === n);
  const ms = (n: number) => x.milestones.find((m) => m.number === n) ?? null;

  // Every issue's labels were added by its author when it was opened.
  for (const i of state.issues) {
    for (const l of i.labels) {
      if (typeof l === "string" || !l.name) continue;
      labelEvent(
        state,
        i.number,
        i.user ?? users.mira,
        "labeled",
        { name: l.name, color: l.color ?? "ededed" },
        hour(i.created_at, 0.05),
      );
    }
  }

  if (full === "graftyard/edge") {
    const assign: Record<number, SimpleUser[]> = {
      2: [users.tomas],
      3: [users.devi],
      6: [users.lin, users.devi],
      8: [users.mira],
      12: [users.lin],
      16: [users.tomas],
      18: [users.devi],
      21: [users.tomas],
      24: [users.mira],
      25: [users.lin],
      30: [users.mira],
    };
    for (const [n, who] of Object.entries(assign)) {
      const i = byNumber(Number(n));
      if (!i) continue;
      for (const u of who)
        assignEvent(state, i.number, users.mira, "assigned", u, hour(i.created_at, 1));
      i.assignees = who;
      i.assignee = who[0];
    }
    const plan: Record<number, number> = {
      11: 1,
      2: 2,
      3: 2,
      8: 2,
      10: 2,
      13: 2,
      16: 2,
      20: 2,
      21: 2,
      23: 2,
      26: 2,
      30: 2,
      6: 3,
      17: 3,
      27: 3,
      7: 4,
      12: 4,
      15: 4,
      19: 4,
      25: 4,
      29: 4,
    };
    for (const [n, m] of Object.entries(plan)) {
      const i = byNumber(Number(n));
      const target = ms(m);
      if (!i || !target) continue;
      milestoneEvent(
        state,
        i.number,
        users.mira,
        "milestoned",
        target.title,
        hour(i.created_at, 2),
      );
      i.milestone = target;
    }
    const i15 = byNumber(15);
    if (i15) {
      push(state, 15, {
        kind: "rename",
        ev: {
          ...base(state, users.devi, "renamed", hour(i15.created_at, 3)),
          rename: { from: "Search should match numbers", to: i15.title },
        },
      });
    }
    const i16 = byNumber(16);
    if (i16) {
      stateEvent(state, 16, users.tomas, "referenced", "2026-11-08T07:30:00Z", {
        commit: sha("graftyard/edge:fix-empty-per-page"),
      });
    }
    const i26 = byNumber(26);
    if (i26) {
      push(state, 26, {
        kind: "lock",
        ev: {
          ...base(state, users.mira, "locked", hour(i26.updated_at, 0.1)),
          lock_reason: "resolved",
        },
      });
      i26.locked = true;
      i26.active_lock_reason = "resolved";
    }
    // Pull request bodies that mention an issue.
    for (const [issueNo, prNo] of [
      [4, 1],
      [11, 5],
      [6, 9],
      [8, 14],
    ] as const) {
      const pr = byNumber(prNo);
      if (!pr) continue;
      push(state, issueNo, {
        kind: "xref",
        ev: {
          event: "cross-referenced",
          actor: pr.user ?? users.mira,
          created_at: pr.created_at,
          updated_at: pr.created_at,
          source: { type: "issue", issue: { ...pr, repository: state.repo } },
        },
      });
    }
    const two = byNumber(2);
    if (two) {
      for (const u of [users.mira, users.lin, users.devi])
        react(state, { kind: "issue", number: 2 }, u, "+1", "2026-10-28T12:30:00Z");
      react(state, { kind: "issue", number: 2 }, users.tomas, "eyes", "2026-10-28T13:00:00Z");
      const first = state.comments.get(2)?.[0];
      if (first) {
        react(state, { kind: "comment", id: first.id }, users.tomas, "+1", "2026-10-28T12:10:00Z");
        react(state, { kind: "comment", id: first.id }, users.lin, "heart", "2026-10-28T12:20:00Z");
      }
    }
    react(state, { kind: "issue", number: 6 }, users.mira, "rocket", "2026-10-31T09:00:00Z");
    react(state, { kind: "issue", number: 6 }, users.tomas, "rocket", "2026-10-31T10:00:00Z");
    react(state, { kind: "issue", number: 30 }, users.devi, "hooray", "2026-11-08T08:30:00Z");
  }

  // Closed issues record who closed them and why.
  for (const i of state.issues) {
    if (i.state !== "closed" || !i.closed_at || i.pull_request) continue;
    const actor = i.number === 11 ? users.devi : (i.user ?? users.mira);
    i.closed_by = actor;
    stateEvent(state, i.number, actor, "closed", i.closed_at, { state_reason: i.state_reason });
  }

  syncAllRollups(state);
  recountMilestones(state);
  state.repo.open_issues_count = state.repo.open_issues = state.issues.filter(
    (i) => i.state === "open",
  ).length;
}

for (const full of ["graftyard/edge", "graftyard/web", "graftyard/raft"]) {
  const [owner, name] = full.split("/");
  const state = db.state(owner, name);
  if (state) seedRepo(state, full);
}

export type { Label, Milestone, Reaction, ReactionContent, Subject };
