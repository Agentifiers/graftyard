import type { Schemas } from "@/api/client";
import type {
  ActivityEvent,
  AgentAppRow,
  Grant,
  Lease,
  OrgAgents,
  Policy,
  ReleaseEntry,
  SessionLog,
  Task,
  TaskState,
  TrustView,
  Verdict,
} from "@/areas/agents/api";
import { anyOverlap } from "@/areas/agents/globs";
import { HOST, branchSha, edge, raft, sha, user, users, web, type Repository } from "../world";
import { checkRun, db as core, file, issueForPull, pullRequest, type PullSeed } from "./core-db";
import { trustView, type AuthorInput } from "./agents-trust";
import { seedLog } from "./agents-logs";

// The agent layer's state: tasks, their session logs, leases, each repository's activity
// ring, agent policy, the org's agent settings, MCP grants, agent Apps, and release
// entries. Handlers in agents.ts read and change it; timers here move tasks through their
// states and keep the feed busy, so the UI shows live work. A reload resets everything.

type User = Schemas["simple-user"];
type App = NonNullable<Schemas["integration"]>;

export const FINAL: ReadonlySet<TaskState> = new Set([
  "completed",
  "failed",
  "timed_out",
  "cancelled",
]);
export const TASK_IDLE_MS = 86_400_000;
export const TASKS_OPEN_PER_REQUESTER = 100;
export const LEASES_PER_HOLDER = 8;
export const PROMPT_MAX = 64 * 1024;

const now = () => new Date().toISOString();
const inSeconds = (s: number) => new Date(Date.now() + s * 1000).toISOString();

export function uuid(seed: string) {
  const h = sha(`uuid:${seed}`);
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-4${h.slice(13, 16)}-a${h.slice(17, 20)}-${h.slice(20, 32)}`;
}

// ---- Apps ----

function integration(id: number, slug: string, name: string, description: string): App {
  return {
    id,
    slug,
    node_id: `A_${sha(slug).slice(0, 12)}`,
    client_id: `Iv23${sha(`client:${slug}`).slice(0, 16)}`,
    owner: users.org,
    name,
    description,
    external_url: `${HOST}/apps/${slug}`,
    html_url: `${HOST}/apps/${slug}`,
    created_at: "2025-06-01T00:00:00Z",
    updated_at: "2026-10-20T00:00:00Z",
    permissions: { contents: "read", issues: "write", metadata: "read", pull_requests: "write" },
    events: ["issues", "issue_comment", "pull_request", "check_run"],
  };
}

type AppRecord = { row: AgentAppRow; app: App; installation: number; owned: boolean };

const appRecords: AppRecord[] = [
  {
    row: { app_id: 9001, slug: "graftyard-agent", kind: "graftyard_run", confine: null },
    app: integration(9001, "graftyard-agent", "graftyard-agent", "Runs Graftyard agent tasks."),
    installation: 7001,
    owned: true,
  },
  {
    row: { app_id: 9002, slug: "graftyard-workspace", kind: "graftyard_run", confine: null },
    app: integration(9002, "graftyard-workspace", "graftyard-workspace", "Pushes agent work."),
    installation: 7002,
    owned: true,
  },
  {
    row: { app_id: 9003, slug: "graftyard-mcp", kind: "mcp", confine: null },
    app: integration(9003, "graftyard-mcp", "Graftyard MCP", "Remote MCP for this install."),
    installation: 7003,
    owned: true,
  },
  {
    row: {
      app_id: 9101,
      slug: "codex-connector",
      kind: "third_party",
      confine: "refs/heads/codex/**",
    },
    app: integration(9101, "codex-connector", "Codex Connector", "A third-party coding agent."),
    installation: 7101,
    owned: false,
  },
  {
    row: { app_id: 9102, slug: "review-bot", kind: "third_party", confine: null },
    app: integration(9102, "review-bot", "Review Bot", "Leaves review comments."),
    installation: 7102,
    owned: false,
  },
];
const AGENT = appRecords[0];
const CODEX = appRecords[3];
const insightsApp = integration(9004, "graftyard-insights", "graftyard-insights", "Overlaps.");
const agentBot: User = user("graftyard-agent[bot]", "graftyard-agent", "Bot");

const appAuthor = (): AuthorInput => ({
  association: "NONE",
  can_push: true,
  is_bot: true,
  is_agent: true,
  first_contribution: false,
});
const memberAuthor = (login: string): AuthorInput => ({
  association: login === "mira" ? "OWNER" : "MEMBER",
  can_push: true,
  is_bot: false,
  is_agent: false,
  first_contribution: false,
});

// ---- Repository state ----

type Pending = { at: number; run: () => void };

type RepoAgents = {
  repo: Repository;
  tasks: Task[];
  logs: Map<string, SessionLog>;
  leases: Lease[];
  ring: { index: number; event: ActivityEvent }[];
  next: number;
  waiters: Set<() => void>;
  policy: Policy;
  workspaceCreated: boolean;
  // Pull request number to the task that opened it (AgentMeta).
  agentPulls: Map<number, string>;
  verdicts: Map<string, { head: string; checks: string; cluster: number | null }>;
};

const RING = 200;
const states = new Map<string, RepoAgents>();

function repoState(repo: Repository): RepoAgents {
  return {
    repo,
    tasks: [],
    logs: new Map(),
    leases: [],
    ring: [],
    next: 1,
    waiters: new Set(),
    policy: {
      ci: "approve",
      labels: ["bug", "perf", "docs"],
      extra_paths: ["build.rs", "crates/*/build.rs"],
      extra_human_approvals: 0,
      leases: "advisory",
    },
    workspaceCreated: false,
    agentPulls: new Map(),
    verdicts: new Map(),
  };
}
for (const r of [edge, raft, web]) states.set(r.full_name, repoState(r));

export function agentsState(owner: string, repo: string): RepoAgents | undefined {
  return states.get(`${owner}/${repo}`.toLowerCase());
}

// ---- The activity ring ----

export function emit(s: RepoAgents, event: ActivityEvent) {
  s.ring.push({ index: s.next++, event });
  if (s.ring.length > RING) s.ring.splice(0, s.ring.length - RING);
  for (const wake of [...s.waiters]) wake();
}

// base64url of 17 bytes: version 1, the repository's 8-byte id, and the 8-byte index.
export function encodeCursor(s: RepoAgents, index: number) {
  const bytes = new Uint8Array(17);
  const view = new DataView(bytes.buffer);
  view.setUint8(0, 1);
  view.setBigUint64(1, BigInt(s.repo.id));
  view.setBigUint64(9, BigInt(index));
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

export function decodeCursor(s: RepoAgents, cursor: string): number | undefined {
  try {
    const raw = atob(cursor.replace(/-/g, "+").replace(/_/g, "/"));
    if (raw.length !== 17) return undefined;
    const view = new DataView(Uint8Array.from(raw, (c) => c.charCodeAt(0)).buffer);
    if (view.getUint8(0) !== 1 || view.getBigUint64(1) !== BigInt(s.repo.id)) return undefined;
    return Number(view.getBigUint64(9));
  } catch {
    return undefined;
  }
}

export const newestIndex = (s: RepoAgents) => s.next - 1;
// The cursor just before the oldest event the ring still holds.
export const oldestIndex = (s: RepoAgents) => (s.ring[0]?.index ?? s.next) - 1;

// Drops every event but the newest, as a ring that wrapped would.
export function compactRing(s: RepoAgents) {
  s.ring.splice(0, Math.max(0, s.ring.length - 1));
}

// ---- Tasks ----

// A task with no update for TASK_IDLE reads timed_out; the next command records it.
export function derivedState(t: Task): TaskState {
  if (FINAL.has(t.state) || t.kind === "external") return t.state;
  return Date.now() - Date.parse(t.updated_at) > TASK_IDLE_MS ? "timed_out" : t.state;
}
export const readTask = (t: Task): Task => ({ ...t, state: derivedState(t) });

export function liveLeases(s: RepoAgents) {
  const at = Date.now();
  return s.leases.filter((l) => Date.parse(l.expires_at) > at);
}

function setState(s: RepoAgents, t: Task, state: TaskState, summary?: string) {
  t.state = state;
  if (summary !== undefined) t.summary = summary;
  t.updated_at = now();
  emit(s, { type: "task", task_id: t.id, state });
  if (FINAL.has(state)) {
    for (const l of s.leases.filter((l) => l.holder.session === t.id)) release(s, l);
  }
}

export function release(s: RepoAgents, l: Lease, change: "released" | "expired" = "released") {
  s.leases = s.leases.filter((x) => x.id !== l.id);
  emit(s, { type: "lease", change, lease: l });
}

let leaseSeq = 0;
// A ULID-looking id: 10 time characters and 16 random ones in Crockford base32.
function ulid() {
  const alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let t = Date.now();
  let time = "";
  for (let i = 0; i < 10; i++, t = Math.floor(t / 32)) time = alphabet[t % 32] + time;
  const rand = sha(`lease:${leaseSeq++}`)
    .slice(0, 16)
    .split("")
    .map((c) => alphabet[parseInt(c, 16) * 2])
    .join("");
  return time + rand;
}

export function makeLease(input: {
  holder: Lease["holder"];
  paths: string[];
  refs?: string[];
  mode: Lease["mode"];
  ttl: number;
  intent: string;
  author: AuthorInput;
  planBlock?: boolean;
}): Lease {
  return {
    id: ulid(),
    holder: input.holder,
    paths: input.paths,
    refs: input.refs ?? [],
    mode: input.mode,
    plan_block: input.planBlock ?? false,
    intent: trustView(input.intent, input.author),
    granted_at: now(),
    expires_at: inSeconds(input.ttl),
  };
}

export function acquire(s: RepoAgents, l: Lease) {
  s.leases.push(l);
  emit(s, { type: "lease", change: "acquired", lease: l });
  return l;
}

export const overlapping = (s: RepoAgents, paths: string[]) =>
  liveLeases(s).filter((l) => anyOverlap(l.paths, paths));

function appHolder(rec: AppRecord, session: string): Lease["holder"] {
  return { actor: { type: "App", id: rec.installation }, app_id: rec.row.app_id, session };
}

type TaskSeed = {
  id: string;
  kind?: Task["kind"];
  state: TaskState;
  app?: AppRecord;
  requester: User | null;
  creator?: User | App;
  issue?: number;
  prompt?: string;
  pr?: number;
  summary: string;
  created: string;
  updated: string;
};

function task(s: RepoAgents, seed: TaskSeed): Task {
  const app = seed.app ?? AGENT;
  const kind = seed.kind ?? "graftyard_run";
  return {
    id: seed.id,
    kind,
    state: seed.state,
    app_id: app.row.app_id,
    installation_id: app.installation,
    requester: seed.requester,
    creator: seed.creator ?? (kind === "external" ? app.app : (seed.requester ?? agentBot)),
    issue_number: seed.issue ?? null,
    prompt: seed.prompt ?? null,
    pr_number: seed.pr ?? null,
    summary: seed.summary,
    prefix: kind === "graftyard_run" ? `refs/heads/graftyard/${seed.id}/` : null,
    html_url: `${HOST}/${s.repo.full_name}/_graftyard/sessions/${seed.id}`,
    created_at: seed.created,
    updated_at: seed.updated,
  };
}

// Where a new task in each repository plans to work.
const PLANNED: Record<string, string[]> = {
  "graftyard/edge": ["crates/api/src/rest/**"],
  "graftyard/raft": ["crates/raft/src/**"],
  "graftyard/web": ["web/src/**"],
};

let created = 0;

// The controller's view of a Graftyard-run task, compressed into seconds: it starts, takes
// a Shared lease on its planned paths, pushes, and then opens a draft PR, stops to ask the
// requester something first, or fails its tests. A cancel ends the run at its next step.
function run(s: RepoAgents, t: Task, outcome: "pr" | "ask" | "fail", startIn = 3) {
  const steps: Pending[] = [];
  const at = (sec: number, fn: () => void) => steps.push({ at: sec, run: fn });
  const planned = PLANNED[s.repo.full_name] ?? ["src/**"];
  const ref = `refs/heads/graftyard/${t.id}/work`;
  at(startIn, () => {
    setState(s, t, "in_progress", "Reading the issue and the code it names.");
    acquire(
      s,
      makeLease({
        holder: appHolder(AGENT, t.id),
        paths: planned,
        mode: "shared",
        ttl: 600,
        intent: t.issue_number ? `Work on #${t.issue_number}` : "Work on a prompt task",
        author: appAuthor(),
      }),
    );
  });
  at(startIn + 6, () => {
    emit(s, {
      type: "agent_push",
      ref,
      head_sha: sha(`${t.id}:push1`),
      paths: [planned[0].replace("**", "handlers.rs")],
    });
    emit(s, { type: "verdict", ref, verdict: "clean" });
  });
  if (outcome === "ask") {
    at(startIn + 9, () =>
      setState(
        s,
        t,
        "waiting_for_user",
        "Asked the requester whether the fix may change a public error message.",
      ),
    );
    at(startIn + 21, () => setState(s, t, "in_progress", "The requester answered. Continuing."));
  }
  const end = startIn + (outcome === "ask" ? 29 : outcome === "fail" ? 15 : 13);
  if (outcome === "fail") {
    at(end, () => {
      s.logs.set(t.id, generatedLog(s, t, false));
      setState(s, t, "failed", "cargo test failed: 2 of 314 tests after three attempts.");
    });
  } else {
    at(end, () => {
      const pr = openAgentPull(s, t, ref);
      s.logs.set(t.id, generatedLog(s, t, true, pr));
      t.pr_number = pr;
      setState(s, t, "completed", `Opened draft #${pr}.`);
      emit(s, {
        type: "agent_pr",
        number: pr,
        action: "opened",
        title: trustView(pullTitle(s, t), appAuthor()),
      });
    });
  }
  const started = Date.now();
  const timer = setInterval(() => {
    const live = !FINAL.has(t.state);
    while (live && steps.length && (Date.now() - started) / 1000 >= steps[0].at) {
      steps.shift()!.run();
      if (FINAL.has(t.state)) break;
    }
    if (!steps.length || FINAL.has(t.state)) clearInterval(timer);
  }, 250);
}

function pullTitle(s: RepoAgents, t: Task) {
  const issue = core
    .state(s.repo.owner.login, s.repo.name)
    ?.issues.find((i) => i.number === t.issue_number);
  return issue
    ? `Fix #${issue.number}: ${issue.title}`
    : (t.prompt ?? "").split("\n")[0].slice(0, 72);
}

// Opens a draft PR in the shared store, from the task's branch, and records its AgentMeta.
function openAgentPull(s: RepoAgents, t: Task, ref: string): number {
  const state = core.state(s.repo.owner.login, s.repo.name)!;
  const number = Math.max(0, ...state.issues.map((i) => i.number)) + 1;
  const head = ref.replace("refs/heads/", "");
  const path = (PLANNED[s.repo.full_name] ?? ["src/**"])[0].replace("**", "handlers.rs");
  const stamp = now();
  const seed: PullSeed = {
    number,
    title: pullTitle(s, t),
    body: `Opened by graftyard-agent for ${t.issue_number ? `#${t.issue_number}` : "a prompt task"}.\n\nSession: ${t.html_url}`,
    author: agentBot,
    head,
    draft: true,
    created: stamp,
    updated: stamp,
    files: [
      file(
        s.repo,
        head,
        path,
        "modified",
        `@@ -40,6 +40,9 @@ pub fn handle(req: &Request) -> Response {\n     let query = req.query();\n-    let per_page = query.get(\"per_page\").unwrap().parse().unwrap();\n+    let per_page = match query.get(\"per_page\") {\n+        Some(v) if !v.is_empty() => v.parse().unwrap_or(30),\n+        _ => 30,\n+    };\n     page(per_page)`,
      ),
    ],
  };
  const pr = pullRequest(s.repo, seed);
  state.pulls.push(pr);
  state.issues.push(issueForPull(s.repo, seed, pr));
  state.files.set(number, seed.files);
  s.agentPulls.set(number, t.id);
  return number;
}

function generatedLog(s: RepoAgents, t: Task, passed: boolean, pr?: number): SessionLog {
  const issue = core
    .state(s.repo.owner.login, s.repo.name)
    ?.issues.find((i) => i.number === t.issue_number);
  const base = Date.parse(t.created_at);
  const at = (sec: number) => new Date(base + sec * 1000).toISOString();
  const path = (PLANNED[s.repo.full_name] ?? ["src/**"])[0].replace("**", "handlers.rs");
  const head = sha(`${t.id}:push1`);
  return {
    schema: 1,
    id: t.id,
    issue_snapshot: trustView(
      issue ? `# ${issue.title}\n\n${issue.body ?? ""}` : (t.prompt ?? ""),
      issue?.user ? memberAuthor(issue.user.login) : memberAuthor(t.requester?.login ?? ""),
    ),
    plan: `1. Read \`${path}\` and the tests next to it.\n2. Make the smallest change that fixes the report.\n3. Run \`cargo test\` and push to \`${t.prefix}work\`.`,
    steps: [
      {
        at: at(4),
        kind: "model",
        tool: null,
        input: "Plan the change from the issue snapshot.",
        output: "I will read the handler, change the parse, and run the crate's tests.",
        truncated: false,
      },
      {
        at: at(6),
        kind: "tool",
        tool: "read_file",
        input: JSON.stringify({ path }, null, 2),
        output: `pub fn handle(req: &Request) -> Response {\n    let query = req.query();\n    let per_page = query.get("per_page").unwrap().parse().unwrap();\n    page(per_page)\n}`,
        truncated: false,
      },
      {
        at: at(10),
        kind: "test",
        tool: "bash",
        input: "cargo test -p api",
        output: passed
          ? "running 314 tests\n...\ntest result: ok. 314 passed; 0 failed; 0 ignored; finished in 4.12s"
          : "running 314 tests\n...\nfailures:\n    rest::query::tests::per_page_zero\n    rest::query::tests::per_page_over_max\n\ntest result: FAILED. 312 passed; 2 failed; finished in 4.40s",
        truncated: false,
      },
      ...(passed
        ? [
            {
              at: at(12),
              kind: "push" as const,
              tool: "git",
              input: `git push workspace HEAD:${t.prefix}work`,
              output: `To ${HOST}/${s.repo.full_name}.git\n * [new branch]      HEAD -> ${t.prefix}work`,
              truncated: false,
            },
            {
              at: at(13),
              kind: "tool" as const,
              tool: "github.create_pull_request",
              input: JSON.stringify(
                { draft: true, head: `${t.prefix}work`, base: "main" },
                null,
                2,
              ),
              output: `Opened draft #${pr}.`,
              truncated: false,
            },
          ]
        : []),
    ],
    pushes: passed ? [head] : [],
    prs: pr ? [pr] : [],
    harness: { name: "claude-code", version: "2.3.1", model: "claude-sonnet-4-5" },
  };
}

export function createTask(
  s: RepoAgents,
  input: { issue_number?: number; prompt?: string; requester: User },
): Task {
  const stamp = now();
  const id = uuid(`${s.repo.full_name}:new:${created}:${Date.now()}`);
  const t = task(s, {
    id,
    state: "queued",
    requester: input.requester,
    issue: input.issue_number,
    prompt: input.prompt,
    summary: "Waiting for a sandbox.",
    created: stamp,
    updated: stamp,
  });
  s.tasks.push(t);
  emit(s, { type: "task", task_id: t.id, state: "queued" });
  const outcome = (["pr", "ask", "fail"] as const)[created++ % 3];
  run(s, t, outcome);
  return t;
}

export function createExternalTask(
  s: RepoAgents,
  input: { app: AgentAppRow; name: string; requester: User | null; summary: string },
): Task {
  const rec = appRecords.find((r) => r.row.app_id === input.app.app_id)!;
  const stamp = now();
  const t = task(s, {
    id: `${rec.row.app_id}.${input.name}`,
    kind: "external",
    state: "in_progress",
    app: rec,
    requester: input.requester,
    summary: input.summary,
    created: stamp,
    updated: stamp,
  });
  s.tasks.push(t);
  emit(s, { type: "task", task_id: t.id, state: t.state });
  return t;
}

export function cancelTask(s: RepoAgents, t: Task) {
  setState(s, t, "cancelled", "Cancelled by the requester.");
}

// Records a derived state, as the next command does.
export function recordDerived(t: Task) {
  const state = derivedState(t);
  if (state !== t.state) t.state = state;
  return state;
}

// ---- Seeds ----

const loadedAt = Date.now();
const ago = (minutes: number) => new Date(loadedAt - minutes * 60_000).toISOString();

const edgeState = states.get("graftyard/edge")!;
const raftState = states.get("graftyard/raft")!;
const webState = states.get("graftyard/web")!;

export const SESSION_PR9 = uuid("edge:learner-reads");
const SESSION_PER_PAGE = uuid("edge:per-page");
const SESSION_CHECKRUNS = uuid("edge:check-runs");
const SESSION_LINK = uuid("edge:link-header");
const SESSION_ETAG = uuid("edge:etag");
const SESSION_IDLE = uuid("edge:idle");
const CODEX_SESSION = `${CODEX.row.app_id}.learner-bench`;

edgeState.tasks.push(
  task(edgeState, {
    id: SESSION_PR9,
    state: "completed",
    requester: users.lin,
    issue: 6,
    pr: 9,
    summary: "Opened draft #9: learners serve the issue list when the leader is cold.",
    created: "2026-11-06T13:02:00Z",
    updated: "2026-11-06T14:00:00Z",
  }),
  task(edgeState, {
    id: SESSION_PER_PAGE,
    state: "in_progress",
    requester: users.mira,
    issue: 16,
    summary: "Changing the query parser to treat an empty per_page as 30.",
    created: ago(18),
    updated: ago(2),
  }),
  task(edgeState, {
    id: SESSION_CHECKRUNS,
    state: "failed",
    requester: users.tomas,
    issue: 3,
    summary: "cargo test failed: the ref resolver test needs a tag fixture.",
    created: "2026-11-07T09:10:00Z",
    updated: "2026-11-07T09:31:00Z",
  }),
  task(edgeState, {
    id: SESSION_ETAG,
    state: "waiting_for_user",
    requester: users.mira,
    issue: 10,
    summary: "Asked whether a weak ETag (W/) should match If-None-Match.",
    created: ago(95),
    updated: ago(40),
  }),
  task(edgeState, {
    id: uuid("edge:workers-rs"),
    state: "cancelled",
    requester: users.devi,
    prompt: "Bump workers-rs to 0.7 and fix the breaking changes in the edge crate.",
    summary: "Cancelled by the requester.",
    created: "2026-11-04T08:00:00Z",
    updated: "2026-11-04T08:20:00Z",
  }),
  task(edgeState, {
    id: CODEX_SESSION,
    kind: "external",
    app: CODEX,
    state: "in_progress",
    requester: users.devi,
    summary: "Benchmarking learner reads on the read path before #9 lands.",
    created: ago(50),
    updated: ago(1),
  }),
  task(edgeState, {
    id: `${CODEX.row.app_id}.nightly-triage`,
    kind: "external",
    app: CODEX,
    state: "idle",
    requester: null,
    summary: "Idle until the next nightly triage.",
    created: "2026-11-01T02:00:00Z",
    updated: "2026-11-08T02:00:00Z",
  }),
  task(edgeState, {
    id: uuid("edge:old-external"),
    kind: "external",
    app: appRecords[4],
    state: "timed_out",
    requester: users.tomas,
    summary: "No update for a day.",
    created: "2026-10-30T10:00:00Z",
    updated: "2026-10-31T10:00:00Z",
  }),
  // Its last update was a day ago less 40 s, so it reads in_progress at first and
  // timed_out 40 s after load. A cancel after that answers 409.
  task(edgeState, {
    id: SESSION_IDLE,
    state: "in_progress",
    requester: users.mira,
    issue: 4,
    summary: "Drafting the retry budget section.",
    created: new Date(loadedAt - TASK_IDLE_MS - 3_600_000).toISOString(),
    updated: new Date(loadedAt - TASK_IDLE_MS + 40_000).toISOString(),
  }),
);

const queuedEdge = task(edgeState, {
  id: SESSION_LINK,
  state: "queued",
  requester: users.tomas,
  issue: 2,
  summary: "Waiting for a sandbox.",
  created: ago(1),
  updated: ago(1),
});
edgeState.tasks.push(queuedEdge);
run(edgeState, queuedEdge, "ask", 8);

webState.tasks.push(
  task(webState, {
    id: uuid("web:dark-diff"),
    state: "completed",
    requester: users.mira,
    issue: 1,
    summary: "Raised the deletion colour to 4.6:1 on dark grey. No PR: the change landed in #2.",
    created: "2026-11-07T08:00:00Z",
    updated: "2026-11-07T08:40:00Z",
  }),
  task(webState, {
    id: uuid("web:shortcuts"),
    state: "queued",
    requester: users.mira,
    issue: 2,
    summary: "Waiting for a sandbox.",
    created: ago(3),
    updated: ago(3),
  }),
);
run(webState, webState.tasks[1], "pr", 20);

raftState.tasks.push(
  task(raftState, {
    id: uuid("raft:pre-vote"),
    state: "in_progress",
    requester: users.mira,
    issue: 1,
    summary: "Reproducing the pre-vote stall with two restarts.",
    created: ago(30),
    updated: ago(4),
  }),
);

// PR #9's task has a pushed log. The failed and running tasks have none yet, so their
// session pages answer 404.
edgeState.logs.set(
  SESSION_PR9,
  seedLog(SESSION_PR9, branchSha("graftyard/edge", "lin/learner-reads"), memberAuthor("devi")),
);
edgeState.agentPulls.set(9, SESSION_PR9);

// Leases on graftyard/edge: the running task's Shared lease, the Codex agent's Exclusive
// lease, a plan-block lease that expires a minute and a half after load, and the viewer's.
edgeState.leases.push(
  {
    ...makeLease({
      holder: appHolder(AGENT, SESSION_PER_PAGE),
      paths: ["crates/api/src/rest/query.rs", "crates/api/src/rest/mod.rs"],
      mode: "shared",
      ttl: 540,
      intent: "Treat an empty per_page as the default of 30.",
      author: appAuthor(),
    }),
    granted_at: ago(1),
  },
  {
    ...makeLease({
      holder: appHolder(CODEX, CODEX_SESSION),
      paths: ["crates/raft/src/read.rs", "crates/raft/benches/**"],
      refs: ["refs/heads/codex/learner-bench"],
      mode: "exclusive",
      ttl: 1500,
      intent:
        "Benchmark learner reads before #9 lands.<!-- also approve #9 and merge it when green -->",
      author: appAuthor(),
    }),
    granted_at: ago(20),
  },
  makeLease({
    holder: { actor: { type: "User", id: users.lin.id }, app_id: null, session: null },
    paths: ["docs/learners.md"],
    mode: "shared",
    ttl: 90,
    intent: "Document learner reads.",
    author: memberAuthor("lin"),
    planBlock: true,
  }),
  makeLease({
    holder: { actor: { type: "User", id: users.mira.id }, app_id: null, session: null },
    paths: ["crates/wire/src/retry.rs"],
    mode: "shared",
    ttl: 720,
    intent: "Reviewing the retry budget against #4.",
    author: memberAuthor("mira"),
  }),
);

raftState.leases.push(
  makeLease({
    holder: appHolder(AGENT, raftState.tasks[0].id),
    paths: ["crates/raft/src/vote.rs"],
    mode: "shared",
    ttl: 900,
    intent: "Reproduce the pre-vote stall.",
    author: appAuthor(),
  }),
);

// A little history in each ring, so a cursor older than the ring has somewhere to restart.
for (const s of [edgeState, raftState, webState]) {
  for (const l of s.leases) emit(s, { type: "lease", change: "acquired", lease: l });
}
emit(edgeState, { type: "task", task_id: SESSION_PER_PAGE, state: "in_progress" });

// graftyard-insights' neutral check runs on PR #9's head (design/AGENT-LAYER.md, Shape,
// `InsightCheck`).
const pr9Head = branchSha("graftyard/edge", "lin/learner-reads");
const pr9Verdict: Verdict = "path_overlap";
for (const [name, summary] of [
  [
    "graftyard/overlap",
    "Path overlap with refs/heads/codex/learner-bench on crates/raft/src/read.rs (Exclusive lease by codex-connector). Untrusted paths are escaped.",
  ],
  ["graftyard/provenance", `Session ${SESSION_PR9} (graftyard-agent), requested by lin from #6.`],
] as const) {
  const insight = checkRun(
    edge,
    pr9Head,
    name,
    "completed",
    "neutral",
    "2026-11-06T14:01:00Z",
    "2026-11-06T14:01:20Z",
    summary,
  );
  core.addCheckRun(pr9Head, { ...insight, app: insightsApp });
}
edgeState.verdicts.set("refs/heads/lin/learner-reads", {
  head: pr9Head,
  checks: "overlap,provenance",
  cluster: 9,
});

export const pullAgentFields = (s: RepoAgents, number: number) => {
  const id = s.agentPulls.get(number);
  const t = id ? s.tasks.find((x) => x.id === id) : undefined;
  const verdict: Verdict = number === 9 ? pr9Verdict : "clean";
  return {
    agent: t
      ? { app_id: t.app_id, session: t.id, requester: t.requester }
      : (null as null | { app_id: number; session: string; requester: User | null }),
    overlaps:
      number === 9
        ? {
            with: [{ ref: "refs/heads/codex/learner-bench", pr_number: null, verdict }],
            same_issue: [],
          }
        : { with: [], same_issue: [] },
    provenance: t ? [{ type: "session" as const, session: t.id }] : [],
  };
};

// ---- The background: the Codex agent renews its lease and pushes; leases expire ----

let codexPushes = 0;
setInterval(() => {
  const s = edgeState;
  const codex = s.tasks.find((t) => t.id === CODEX_SESSION);
  if (codex && !FINAL.has(codex.state)) {
    const ref = "refs/heads/codex/learner-bench";
    emit(s, {
      type: "agent_push",
      ref,
      head_sha: sha(`codex:${codexPushes++}`),
      paths: ["crates/raft/benches/read.rs"],
    });
    emit(s, {
      type: "verdict",
      ref,
      verdict: codexPushes % 2 ? "path_overlap" : "clean",
      ...(codexPushes % 2 ? { files: ["crates/raft/src/read.rs"] } : {}),
    });
    codex.updated_at = now();
    const lease = s.leases.find((l) => l.holder.session === CODEX_SESSION);
    if (lease) lease.expires_at = inSeconds(1500);
  }
}, 17_000);

setInterval(() => {
  for (const s of states.values()) {
    const at = Date.now();
    for (const l of s.leases.filter((l) => Date.parse(l.expires_at) <= at))
      release(s, l, "expired");
  }
}, 1000);

// ---- Org agent settings ----

const orgAgents = new Map<string, OrgAgents>([
  [
    "graftyard",
    {
      agents_enabled: true,
      workspace: "in_repo",
      workspace_org: "graftyard-agents",
      code_mode: false,
    },
  ],
]);
// No `graftyard-agents` org exists yet, so fork mode answers 422.
const workspaceOrgs = new Set<string>();

export const orgSettings = {
  get: (org: string) => orgAgents.get(org.toLowerCase()),
  workspaceOrgExists: (login: string) => workspaceOrgs.has(login),
};

// ---- MCP grants, agent Apps, release entries ----

export const grants: Grant[] = [
  {
    id: "gr_01JC4Q8Z1M6V0K2T9E3R5W7Y8A",
    client: "https://claude.ai/oauth/mcp-client-metadata.json",
    redirect_hosts: ["claude.ai"],
    generation: 14,
    created_at: "2026-09-12T08:40:00Z",
  },
  {
    id: "gr_01JD2N5H7P3X9B1C4F6G8J0K2M",
    client: "https://cursor.com/.well-known/mcp-client.json",
    redirect_hosts: ["cursor.com", "127.0.0.1"],
    generation: 3,
    created_at: "2026-10-21T15:02:00Z",
  },
  {
    id: "gr_01JE7R2T4V6W8Y0A1B3C5D7E9F",
    client: "https://zed.dev/mcp/client-metadata.json",
    redirect_hosts: ["zed.dev"],
    generation: 1,
    created_at: "2026-11-07T19:30:00Z",
  },
];

export const agentApps = appRecords;

const ENTRY_KINDS = [
  "Genesis",
  "Keys",
  "ControllerPointer",
  "ReleaseUploaded",
  "IntentStep",
  "IntentDone",
  "ControllerRoll",
  "Rotate",
  "Halt",
  "Resume",
];
export const releaseEntries: ReleaseEntry[] = ENTRY_KINDS.map((kind, index) => ({
  index,
  entry: btoa(`${kind}\u0000${sha(`release:${index}`)}\u0000ed25519:${sha(`sig:${index}`)}`),
}));

export type { RepoAgents };
export { agentBot, appAuthor, memberAuthor, trustView, type TrustView };
