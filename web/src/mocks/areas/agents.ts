import { HttpResponse } from "msw";
import { createOpenApiHttp } from "openapi-msw";
import {
  AGENT_MEDIA_TYPE,
  GRAFTYARD_BASE,
  TASK_STATES,
  type ActivityEvent,
  type AgentMediaPaths,
  type GraftyardPaths,
  type GithubError,
  type Lease,
  type Task,
  type TaskState,
} from "@/areas/agents/api";
import { anyOverlap, globProblem, literalFirstSegment, MAX_GLOBS } from "@/areas/agents/globs";
import { failing } from "../faults";
import { gate } from "../http";
import { users } from "../world";
import { db as core } from "./core-db";
import {
  FINAL,
  LEASES_PER_HOLDER,
  PROMPT_MAX,
  TASKS_OPEN_PER_REQUESTER,
  acquire,
  agentApps,
  agentsState,
  appAuthor,
  cancelTask,
  compactRing,
  createExternalTask,
  createTask,
  decodeCursor,
  derivedState,
  emit,
  encodeCursor,
  grants,
  liveLeases,
  makeLease,
  memberAuthor,
  newestIndex,
  oldestIndex,
  orgSettings,
  overlapping,
  pullAgentFields,
  readTask,
  recordDerived,
  release,
  releaseEntries,
  trustView,
  type RepoAgents,
} from "./agents-db";

// Every /api/graftyard/v1 path in design/AGENT-LAYER.md, "API reference", and the opt-in
// agent media type on two GHES reads. The browser user is mira, a human with push and
// admin rights, so App-only calls (PATCH a task, insights, workspace) need the mock's
// installation token: `Authorization: token ghs_<app_id>`.

const gy = createOpenApiHttp<GraftyardPaths>({ baseUrl: GRAFTYARD_BASE });
const media = createOpenApiHttp<AgentMediaPaths>({ baseUrl: "/api/v3" });

const DOCS = "https://docs.graftyard.dev/api/graftyard/v1";
const viewer = users.mira;

const error = (message: string, errors?: GithubError["errors"]): GithubError => ({
  message,
  documentation_url: DOCS,
  ...(errors ? { errors } : {}),
});
const invalid = (
  resource: string,
  field: string,
  code: "missing_field" | "invalid" | "already_exists" = "invalid",
) => error("Validation Failed", [{ resource, field, code }]);
const notFound = () => error("Not Found");

// The App behind an installation token, if the request carries one.
function callerApp(request: Request): number | undefined {
  const m = /^(?:token|Bearer) ghs_(\d+)/.exec(request.headers.get("authorization") ?? "");
  return m ? Number(m[1]) : undefined;
}

// Lists take per_page (default 30, at most 100) and an opaque `after`, with a Link header.
function paginate<T>(items: T[], request: Request) {
  const url = new URL(request.url);
  const perPage = Math.min(100, Math.max(1, Number(url.searchParams.get("per_page")) || 30));
  const after = url.searchParams.get("after");
  const start = after ? Number(atob(after)) || 0 : 0;
  const slice = items.slice(start, start + perPage);
  const headers = new Headers();
  if (start + perPage < items.length) {
    url.searchParams.set("after", btoa(String(start + perPage)));
    headers.set("Link", `<${url.toString()}>; rel="next"`);
  }
  return { slice, headers };
}

const byCreatedDesc = (a: Task, b: Task) => b.created_at.localeCompare(a.created_at);
const openFor = (s: RepoAgents, login: string) =>
  s.tasks.filter((t) => t.requester?.login === login && !FINAL.has(derivedState(t))).length;

function withLeases(s: RepoAgents, t: Task): Task {
  return { ...readTask(t), leases: liveLeases(s).filter((l) => l.holder.session === t.id) };
}

// ---- The activity feed ----

const MAX_WAIT = 25;
const FEED_POLLS = 2;
const held = new Map<string, number>();
let wrapped = false;

// Each event's paths, for the `paths` filter. Events without paths pass every filter.
function eventPaths(e: ActivityEvent): string[] | undefined {
  if (e.type === "lease") return e.lease.paths;
  if (e.type === "agent_push") return e.paths;
  if (e.type === "verdict") return e.files;
  return undefined;
}

function eventsAfter(s: RepoAgents, cursor: number, paths: string[]) {
  return s.ring
    .filter((r) => r.index > cursor)
    .filter((r) => {
      const p = eventPaths(r.event);
      return !paths.length || !p || anyOverlap(p, paths);
    });
}

// A burst the client never saw, then a ring that dropped it: the next poll gets 410.
function wrapRing(s: RepoAgents) {
  for (let i = 0; i < 4; i++) {
    emit(s, { type: "task", task_id: s.tasks[i % s.tasks.length].id, state: "in_progress" });
  }
  compactRing(s);
}

async function hold(s: RepoAgents, signal: AbortSignal, seconds: number) {
  if (seconds <= 0) return;
  await new Promise<void>((resolve) => {
    const done = () => {
      clearTimeout(timer);
      s.waiters.delete(wake);
      signal.removeEventListener("abort", done);
      resolve();
    };
    // FEED_BATCH: after the first event, wait a moment for more.
    const wake = () => {
      s.waiters.delete(wake);
      setTimeout(done, 400);
    };
    const timer = setTimeout(done, seconds * 1000);
    s.waiters.add(wake);
    signal.addEventListener("abort", done);
  });
}

export const handlers = [
  // The opt-in variants come first and fall through for every other Accept value, so a
  // client that does not opt in gets the pulls and issues handlers' bytes.
  media.get("/repos/{owner}/{repo}/pulls/{pull_number}", async ({ params, request, response }) => {
    if (!request.headers.get("accept")?.includes(AGENT_MEDIA_TYPE)) return undefined;
    const fault = await gate("agents-trust");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const number = Number(params.pull_number);
    const pr = core.state(params.owner, params.repo)?.pulls.find((p) => p.number === number);
    if (!s || !pr) return response(404).json(notFound());
    const fields = pullAgentFields(s, number);
    const author = pr.user.type === "Bot" ? appAuthor() : memberAuthor(pr.user.login);
    return response(200).json(
      {
        ...pr,
        graftyard_trust: trustView(pr.body ?? "", {
          ...author,
          is_agent: author.is_agent || Boolean(fields.agent),
        }),
        graftyard_agent: fields.agent,
        graftyard_overlaps: fields.overlaps,
        graftyard_provenance: { sessions: fields.provenance },
      },
      { headers: { Vary: "Accept", "Content-Type": AGENT_MEDIA_TYPE } },
    );
  }),

  media.get(
    "/repos/{owner}/{repo}/issues/{issue_number}",
    async ({ params, request, response }) => {
      if (!request.headers.get("accept")?.includes(AGENT_MEDIA_TYPE)) return undefined;
      const fault = await gate("agents-trust");
      if (fault) return response.untyped(fault);
      const issue = core
        .state(params.owner, params.repo)
        ?.issues.find((i) => i.number === Number(params.issue_number));
      if (!issue) return response(404).json(notFound());
      const author =
        issue.user?.type === "Bot" ? appAuthor() : memberAuthor(issue.user?.login ?? "");
      return response(200).json(
        { ...issue, graftyard_trust: trustView(issue.body ?? "", author) },
        { headers: { Vary: "Accept", "Content-Type": AGENT_MEDIA_TYPE } },
      );
    },
  ),

  // ---- Tasks ----

  gy.get("/repos/{owner}/{repo}/tasks", async ({ params, query, request, response }) => {
    const fault = await gate("agents-tasks");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const state = query.get("state");
    const app = query.get("app_id");
    const requester = query.get("requester");
    const issue = query.get("issue_number");
    const items = s.tasks
      .map(readTask)
      .filter((t) => !state || t.state === state)
      .filter((t) => !app || t.app_id === Number(app))
      .filter((t) => !requester || t.requester?.login.toLowerCase() === requester.toLowerCase())
      .filter((t) => !issue || t.issue_number === Number(issue))
      .sort(byCreatedDesc);
    const { slice, headers } = paginate(items, request);
    return response(200).json(slice, { headers });
  }),

  gy.post("/repos/{owner}/{repo}/tasks", async ({ params, request, response }) => {
    const fault = await gate("agents-task-create");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    const app = callerApp(request);

    if (body.kind === "external") {
      const row = agentApps.find((a) => a.row.app_id === app)?.row;
      if (!row)
        return response(403).json(
          error("Only an App's installation token may create an external task."),
        );
      if (!body.session) return response(422).json(invalid("Task", "session", "missing_field"));
      if (
        !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(body.session) ||
        body.session.includes("..") ||
        body.session.endsWith(".lock")
      )
        return response(422).json(invalid("Task", "session"));
      if (s.tasks.some((t) => t.id === `${row.app_id}.${body.session}`))
        return response(422).json(invalid("Task", "session", "already_exists"));
      const requester = Object.values(users).find((u) => u.login === body.requester) ?? null;
      return response(201).json(
        createExternalTask(s, {
          app: row,
          name: body.session,
          requester,
          summary: body.summary ?? "",
        }),
      );
    }

    if (app !== undefined && app !== 9001)
      return response(403).json(error("This credential may not create a graftyard_run task."));
    const owner = s.repo.owner.login;
    if (s.repo.owner.type === "Organization" && !orgSettings.get(owner)?.agents_enabled) {
      return response(422).json(
        error(
          `Graftyard-run agents are off for ${owner}. An organization owner can turn on agents_enabled.`,
        ),
      );
    }
    const hasIssue = body.issue_number !== undefined && body.issue_number !== null;
    const prompt = body.prompt?.trim();
    if (!hasIssue && !prompt)
      return response(422).json(invalid("Task", "issue_number", "missing_field"));
    if (prompt && new TextEncoder().encode(body.prompt).length > PROMPT_MAX)
      return response(422).json(invalid("Task", "prompt"));
    if (hasIssue) {
      const issue = core
        .state(params.owner, params.repo)
        ?.issues.find((i) => i.number === body.issue_number);
      if (!issue || issue.pull_request) return response(422).json(invalid("Task", "issue_number"));
    }
    if (openFor(s, viewer.login) >= TASKS_OPEN_PER_REQUESTER)
      return response(422).json(
        error(`${viewer.login} has ${TASKS_OPEN_PER_REQUESTER} open tasks, the most allowed.`),
      );
    const t = createTask(s, {
      issue_number: hasIssue ? body.issue_number : undefined,
      prompt: prompt ? body.prompt : undefined,
      requester: viewer,
    });
    return response(201).json(readTask(t));
  }),

  gy.get("/repos/{owner}/{repo}/tasks/{id}", async ({ params, response }) => {
    const fault = await gate("agents-task");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const t = s?.tasks.find((x) => x.id === params.id);
    return s && t ? response(200).json(withLeases(s, t)) : response(404).json(notFound());
  }),

  gy.patch("/repos/{owner}/{repo}/tasks/{id}", async ({ params, request, response }) => {
    const fault = await gate("agents-task-update");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const t = s?.tasks.find((x) => x.id === params.id);
    if (!s || !t) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    if (callerApp(request) !== t.app_id)
      return response(403).json(error("Only the task's App may update it."));
    const state = recordDerived(t);
    if (FINAL.has(state) || state === "waiting_for_user")
      return response(409).json({ ...error(`The task is ${state}.`), state });
    const body = await request.json();
    if (body.state === "cancelled" || body.state === "timed_out")
      return response(422).json(invalid("Task", "state"));
    if (body.state && !TASK_STATES.includes(body.state))
      return response(422).json(invalid("Task", "state"));
    if (body.summary !== undefined) t.summary = body.summary;
    if (body.pr_number !== undefined) t.pr_number = body.pr_number;
    t.updated_at = new Date().toISOString();
    if (body.state && body.state !== t.state) {
      t.state = body.state;
      emit(s, { type: "task", task_id: t.id, state: t.state });
    }
    return response(200).json(withLeases(s, t));
  }),

  gy.post("/repos/{owner}/{repo}/tasks/{id}/cancel", async ({ params, response }) => {
    const fault = await gate("agents-task-cancel");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const t = s?.tasks.find((x) => x.id === params.id);
    if (!s || !t) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const state: TaskState = recordDerived(t);
    if (FINAL.has(state))
      return response(409).json({
        ...error(`The task is already ${state.replace("_", " ")}.`),
        state,
      });
    cancelTask(s, t);
    return response(200).json(withLeases(s, t));
  }),

  gy.get("/repos/{owner}/{repo}/tasks/{id}/log", async ({ params, response }) => {
    const fault = await gate("agents-log");
    if (fault) return response.untyped(fault);
    const log = agentsState(params.owner, params.repo)?.logs.get(params.id);
    return log
      ? response(200).json(log)
      : response(404).json(error("The session log has not been pushed yet."));
  }),

  // ---- Leases ----

  gy.get("/repos/{owner}/{repo}/leases", async ({ params, query, request, response }) => {
    const fault = await gate("agents-leases");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const paths = query.getAll("paths");
    const items = (paths.length ? overlapping(s, paths) : liveLeases(s)).sort((a, b) =>
      a.expires_at.localeCompare(b.expires_at),
    );
    const { slice, headers } = paginate(items, request);
    return response(200).json(slice, { headers });
  }),

  gy.post("/repos/{owner}/{repo}/leases", async ({ params, request, response }) => {
    const fault = await gate("agents-lease-acquire");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    const session = body.session ?? request.headers.get("X-Graftyard-Agent-Session") ?? null;
    const mode = body.mode ?? "shared";
    const paths = body.paths ?? [];
    if (paths.length < 1 || paths.length > MAX_GLOBS || paths.some((p) => globProblem(p)))
      return response(422).json(invalid("Lease", "paths"));
    if ((body.refs ?? []).length > 2) return response(422).json(invalid("Lease", "refs"));
    const ttl = body.ttl_seconds ?? 600;
    if (!Number.isInteger(ttl) || ttl < 60 || ttl > 3600)
      return response(422).json(invalid("Lease", "ttl_seconds"));
    if (new TextEncoder().encode(body.intent ?? "").length > 280)
      return response(422).json(invalid("Lease", "intent"));
    const mine = (l: Lease) => l.holder.actor.type === "User" && l.holder.actor.id === viewer.id;
    if (liveLeases(s).filter(mine).length >= LEASES_PER_HOLDER)
      return response(422).json(
        error(`A holder may keep ${LEASES_PER_HOLDER} live leases on a repository.`),
      );
    if (mode === "exclusive") {
      if (!session) return response(422).json(invalid("Lease", "session", "missing_field"));
      const t = s.tasks.find((x) => x.id === session);
      if (!t || FINAL.has(derivedState(t))) return response(422).json(invalid("Lease", "session"));
      if (!paths.every(literalFirstSegment)) return response(422).json(invalid("Lease", "paths"));
      if (liveLeases(s).some((l) => l.mode === "exclusive" && l.holder.session === session))
        return response(422).json(error("A task may hold one Exclusive lease."));
      const conflicts = overlapping(s, paths).filter((l) => l.mode === "exclusive" && !mine(l));
      if (conflicts.length) {
        return response(409).json({
          ...error("An Exclusive lease of another holder overlaps these paths."),
          conflicts,
        });
      }
    }
    const lease = makeLease({
      holder: { actor: { type: "User", id: viewer.id }, app_id: null, session },
      paths,
      refs: body.refs,
      mode,
      ttl,
      intent: body.intent ?? "",
      author: memberAuthor(viewer.login),
    });
    const overlaps = overlapping(s, paths);
    acquire(s, lease);
    return response(201).json({ lease, overlaps });
  }),

  gy.post("/repos/{owner}/{repo}/leases/{id}/renew", async ({ params, request, response }) => {
    const fault = await gate("agents-lease-renew");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const lease = s && liveLeases(s).find((l) => l.id === params.id);
    if (!lease) return response(404).json(error("The lease was released or has expired."));
    const { ttl_seconds = 600 } = await request.json();
    lease.expires_at = new Date(Date.now() + ttl_seconds * 1000).toISOString();
    return response(200).json(lease);
  }),

  gy.delete("/repos/{owner}/{repo}/leases/{id}", async ({ params, response }) => {
    const fault = await gate("agents-lease-release");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    const lease = s?.leases.find((l) => l.id === params.id);
    if (s && lease) release(s, lease);
    return response(204).empty();
  }),

  // ---- The activity feed ----

  gy.get("/repos/{owner}/{repo}/activity", async ({ params, query, request, response }) => {
    const fault = await gate("agents-activity");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    // A Mock panel switch: the ring drops a burst of events this client has not read.
    const wrap = failing("agents-activity-wrap");
    if (wrap && !wrapped) wrapRing(s);
    wrapped = wrap;

    const raw = query.get("cursor");
    const cursor = raw === null ? newestIndex(s) : decodeCursor(s, raw);
    if (cursor === undefined || cursor < oldestIndex(s)) {
      return response(410).json({
        ...error("The cursor is older than the activity ring."),
        oldest_cursor: encodeCursor(s, oldestIndex(s)),
      });
    }
    const key = s.repo.full_name;
    if ((held.get(key) ?? 0) >= FEED_POLLS) {
      return response(429).json(error("You have exceeded a secondary rate limit."), {
        headers: { "Retry-After": "10" },
      });
    }
    const paths = query.getAll("paths");
    const wait = Math.min(MAX_WAIT, Math.max(0, Number(query.get("wait")) || 0));
    if (!eventsAfter(s, cursor, paths).length) {
      held.set(key, (held.get(key) ?? 0) + 1);
      try {
        await hold(s, request.signal, wait);
      } finally {
        held.set(key, (held.get(key) ?? 1) - 1);
      }
    }
    const events = eventsAfter(s, cursor, paths);
    const last = events.at(-1)?.index ?? Math.max(cursor, newestIndex(s));
    return response(200).json({
      cursor: encodeCursor(s, last),
      events: events.map((e) => e.event),
    });
  }),

  // ---- Controller and insights calls (App tokens only) ----

  gy.post("/repos/{owner}/{repo}/workspace", async ({ params, response }) => {
    const fault = await gate("agents-workspace");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const org = orgSettings.get(s.repo.owner.login);
    if (!org?.agents_enabled)
      return response(422).json(error("Graftyard-run agents are off for this organization."));
    if (org.workspace === "in_repo")
      return response(422).json(error("The organization runs agents in-repo."));
    const full_name = `${org.workspace_org}/${s.repo.name}`;
    if (s.workspaceCreated) return response(200).json({ full_name, created: false });
    s.workspaceCreated = true;
    return response(201).json({ full_name, created: true });
  }),

  gy.post("/repos/{owner}/{repo}/insights", async ({ params, request, response }) => {
    const fault = await gate("agents-insights");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    const branch = body.ref.replace("refs/heads/", "");
    const tip = core.state(params.owner, params.repo)?.pulls.find((p) => p.head.ref === branch)
      ?.head.sha;
    if (tip && tip !== body.head_sha) return response(409).json(error("The ref's head moved."));
    const next = {
      head: body.head_sha,
      checks: [...body.checks].sort().join(","),
      cluster: body.cluster ?? null,
    };
    const prev = s.verdicts.get(body.ref);
    if (
      prev &&
      prev.head === next.head &&
      prev.checks === next.checks &&
      prev.cluster === next.cluster
    )
      return response(200).json({ ref: body.ref, head_sha: body.head_sha, written: false });
    s.verdicts.set(body.ref, next);
    emit(s, { type: "verdict", ref: body.ref, verdict: "clean" });
    return response(201).json({ ref: body.ref, head_sha: body.head_sha, written: true });
  }),

  // ---- Policy and org settings ----

  gy.get("/repos/{owner}/{repo}/agents/policy", async ({ params, response }) => {
    const fault = await gate("agents-policy");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    return s
      ? response(200).json(s.policy)
      : response.untyped(HttpResponse.json(notFound(), { status: 404 }));
  }),

  gy.put("/repos/{owner}/{repo}/agents/policy", async ({ params, request, response }) => {
    const fault = await gate("agents-policy-save");
    if (fault) return response.untyped(fault);
    const s = agentsState(params.owner, params.repo);
    if (!s) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    s.policy = {
      ci: body.ci,
      labels: [...new Set(body.labels)],
      extra_paths: [...new Set(body.extra_paths)],
      extra_human_approvals: Math.max(0, Math.min(255, Math.trunc(body.extra_human_approvals))),
      leases: body.leases,
    };
    return response(200).json(s.policy);
  }),

  gy.get("/orgs/{org}/agents", async ({ params, response }) => {
    const fault = await gate("agents-org");
    if (fault) return response.untyped(fault);
    const org = orgSettings.get(params.org);
    return org
      ? response(200).json(org)
      : response.untyped(HttpResponse.json(notFound(), { status: 404 }));
  }),

  gy.patch("/orgs/{org}/agents", async ({ params, request, response }) => {
    const fault = await gate("agents-org-save");
    if (fault) return response.untyped(fault);
    const org = orgSettings.get(params.org);
    if (!org) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    if (body.workspace === "fork" && !orgSettings.workspaceOrgExists(org.workspace_org)) {
      return response(422).json({
        ...invalid("OrgAgents", "workspace"),
        message: `The workspace organization ${org.workspace_org} does not exist.`,
      });
    }
    if (body.agents_enabled !== undefined) org.agents_enabled = body.agents_enabled;
    if (body.workspace !== undefined) org.workspace = body.workspace;
    if (body.code_mode !== undefined) org.code_mode = body.code_mode;
    return response(200).json(org);
  }),

  // ---- Agent Apps and release entries (site admin, graftyard-agent) ----

  gy.get("/agents/apps", async ({ response }) => {
    const fault = await gate("agents-apps");
    if (fault) return response.untyped(fault);
    return response(200).json(agentApps.map((a) => a.row));
  }),

  gy.put("/agents/apps/{app_id}", async ({ params, request, response }) => {
    const fault = await gate("agents-app-save");
    if (fault) return response.untyped(fault);
    const rec = agentApps.find((a) => a.row.app_id === Number(params.app_id));
    if (!rec) return response.untyped(HttpResponse.json(notFound(), { status: 404 }));
    const body = await request.json();
    if ((body.kind === "graftyard_run" || body.kind === "mcp") && !rec.owned)
      return response(422).json(invalid("AgentApp", "kind"));
    rec.row = { ...rec.row, kind: body.kind, confine: body.confine ?? null };
    return response(200).json(rec.row);
  }),

  gy.delete("/agents/apps/{app_id}", async ({ params, response }) => {
    const fault = await gate("agents-app-delete");
    if (fault) return response.untyped(fault);
    const i = agentApps.findIndex((a) => a.row.app_id === Number(params.app_id));
    if (i >= 0) agentApps.splice(i, 1);
    return response(204).empty();
  }),

  gy.get("/release/entries", async ({ query, response }) => {
    const fault = await gate("agents-release");
    if (fault) return response.untyped(fault);
    const after = Number(query.get("after") ?? -1);
    const perPage = Math.min(100, Number(query.get("per_page")) || 30);
    return response(200).json(releaseEntries.filter((e) => e.index > after).slice(0, perPage));
  }),

  // ---- MCP grants ----

  gy.get("/mcp/grants", async ({ response }) => {
    const fault = await gate("agents-grants");
    if (fault) return response.untyped(fault);
    return response(200).json([...grants].sort((a, b) => b.created_at.localeCompare(a.created_at)));
  }),

  gy.post("/mcp/grants", async ({ request, response }) => {
    const fault = await gate("agents-grant-create");
    if (fault) return response.untyped(fault);
    const body = await request.json();
    const grant = {
      id: `gr_${crypto.randomUUID().replace(/-/g, "").slice(0, 26).toUpperCase()}`,
      client: body.client,
      redirect_hosts: body.redirect_hosts,
      generation: 0,
      created_at: new Date().toISOString(),
    };
    grants.push(grant);
    return response(201).json(grant);
  }),

  gy.post("/mcp/grants/{id}/generation", async ({ params, request, response }) => {
    const fault = await gate("agents-grant-generation");
    if (fault) return response.untyped(fault);
    const i = grants.findIndex((g) => g.id === params.id);
    if (i < 0) return response(409).json({ ...error("The grant was revoked."), revoked: true });
    const { generation } = await request.json();
    if (generation < grants[i].generation) {
      grants.splice(i, 1);
      return response(409).json({
        ...error("A refresh reused an old generation. The grant is revoked."),
        revoked: true,
      });
    }
    grants[i] = { ...grants[i], generation: grants[i].generation + 1 };
    return response(200).json(grants[i]);
  }),

  gy.delete("/mcp/grants/{id}", async ({ params, response }) => {
    const fault = await gate("agents-grant-revoke");
    if (fault) return response.untyped(fault);
    const i = grants.findIndex((g) => g.id === params.id);
    if (i >= 0) grants.splice(i, 1);
    return response(204).empty();
  }),
];
