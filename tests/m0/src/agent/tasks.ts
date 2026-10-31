// M0-WP13: the task rules (design/DESIGN.md, Shape, agent block, `AgentTask`, `TaskKind`, `TaskApiState`, `SessionId`,
// and the task-creation paragraph; design/AGENT-LAYER.md, "Tasks and the activity feed" and "API reference"), the
// `graftyard_task` token request, the non-writer-edit pause, and the issue commands those rules read. Pure: the host
// passes the minted UUID, the org's `OrgPolicy`, and the entry's `at` and `index`.

import type { AgentTask, Authz, Cmd, Credential, Decision, Event, Fold, Issue, OrgPolicy, PermSet, Principal, Refusal,
  RefusalRule, TaskApiState } from "./model.ts";
import { FINAL_STATES, PROMPT_MAX, TASKS_OPEN_PER_REPO, TASKS_OPEN_PER_REQUESTER, actorKey } from "./model.ts";
import { agentApp } from "./classify.ts";
import { taskState } from "./decide.ts";

const no = (rule: RefusalRule, text: string): Decision => ({ ok: false, refusal: { rule, text } });
export const botLogin = (authz: Authz, app: number): string => `${authz.apps[app]?.slug ?? `app-${app}`}[bot]`;
const roleOf = (fold: Fold, p: Principal) => p.id.t === "User" ? fold.collaborators[`u${p.id.user}`] ?? null : null;
const isWriter = (fold: Fold, p: Principal): boolean => { const r = roleOf(fold, p); return r === "write" || r === "admin"; };
/** A host-paused task: a non-writer edited its issue after assignment, and no writer has reassigned it since. */
export const isPaused = (fold: Fold, session: string): boolean => fold.paused?.[session] != null;

const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
export const validExternalName = (n: string): boolean => NAME.test(n) && !n.includes("..") && !n.endsWith(".lock");

/** Open (not final) tasks, by requester, or by App for an External task with no requester. */
function openCount(fold: Fold, at: number, authz: Authz, f: (t: AgentTask) => boolean): number {
  let n = 0;
  for (const t of Object.values(fold.tasks)) if (f(t) && !FINAL_STATES.includes(taskState(t, at, authz))) n++;
  return n;
}

export interface TaskCtx { fold: Fold; authz: Authz; org: OrgPolicy; actor: Principal; at: number }

export function decideTask(cmd: Cmd, c: TaskCtx): Decision | null {
  const { fold, authz, org, actor } = c;
  const index = (fold.index ?? 0) + 1;
  switch (cmd.t) {
    case "TaskCreate": {
      if (cmd.kind === "GraftyardRun") {
        if (!org.agentsEnabled) return no("AgentsDisabled", "Graftyard-run agents are not enabled for this organization");
        let requester: number | null = null;
        const aa = agentApp(authz, actor.app);
        const controller = actor.id.t === "App" && actor.app === authz.controllerApp && actor.cred === "Installation";
        if (controller) {
          // The one exception: the controller, from an issue whose latest `Assigned` entry names its bot.
          const iss = cmd.issue != null ? fold.issues?.[String(cmd.issue)] : undefined;
          const last = iss?.assigned.at(-1);
          if (!iss || !last || !last.assignees.includes(botLogin(authz, actor.app!)))
            return no("TaskCreator", "graftyard-agent creates tasks only from an issue whose latest assignment names its bot");
          const who = /^u(\d+)$/.exec(last.actor);
          if (!who) return no("TaskCreator", "the assignment was not made by a user");
          requester = Number(who[1]);
        } else {
          if (actor.id.t !== "User") return no("TaskCreator", "Graftyard-run tasks need a human user credential; installation tokens cannot create them");
          if (aa) return no("TaskCreator", "Graftyard-run tasks cannot be created with an agent App's user token");
          if (actor.demoted) return no("TaskCreator", "Graftyard-run tasks cannot be created by an actor demoted to the agent class");
          if (actor.sessionHeader) return no("TaskCreator", "Graftyard-run tasks cannot be created from an agent session");
          if (!isWriter(fold, actor) || (actor.perms && actor.perms.contents !== "write"))
            return no("TaskCreator", "Graftyard-run tasks need contents: write");
          requester = actor.id.user;
        }
        if (cmd.name != null) return invalid("session", "invalid", "session is set by the host for graftyard_run tasks");
        if (cmd.issue == null && cmd.prompt == null) return invalid("issue_number", "missing_field", "neither issue_number nor prompt");
        if (cmd.issue != null && !fold.issues?.[String(cmd.issue)]) return invalid("issue_number", "invalid", "issue not found");
        if (cmd.prompt != null && new TextEncoder().encode(cmd.prompt).length > PROMPT_MAX) return invalid("prompt", "invalid", "prompt over 64 KiB");
        if (openCount(fold, c.at, authz, (t) => t.requester === requester) >= TASKS_OPEN_PER_REQUESTER)
          return no("TaskCap", `requester has ${TASKS_OPEN_PER_REQUESTER} open tasks`);
        if (openCount(fold, c.at, authz, () => true) >= TASKS_OPEN_PER_REPO) return no("TaskCap", `repository has ${TASKS_OPEN_PER_REPO} open tasks`);
        const session = cmd.id;
        const app = authz.controllerApp!;
        const inst = Object.entries(authz.installations).find(([, i]) => i.app === app && i.active)?.[0];
        const task: AgentTask = { session, kind: "GraftyardRun", state: "Queued", prefix: `refs/heads/graftyard/${session}/`, requester,
          app, installation: Number(inst ?? 0), updatedAt: c.at, creator: actorKey(actor.id), issue: cmd.issue, snapshot: fold.index ?? 0,
          prompt: cmd.prompt, pr: null, summary: cmd.summary, createdAt: c.at, repo: fold.repo };
        return { ok: true, events: [{ t: "TaskCreated", task }, { t: "Deliver", event: "graftyard_task", action: "created", session }] };
      }
      // External: an AgentApp reports its own work. Fine-grained tokens and Apps need issues: write plus contents: read.
      const aa = agentApp(authz, actor.app);
      if (actor.id.t !== "App" || actor.cred !== "Installation" || !aa) return no("TaskCreator", "only an agent App's installation token creates external tasks");
      if (cmd.name == null || !validExternalName(cmd.name)) return invalid("session", cmd.name == null ? "missing_field" : "invalid", "bad session");
      const session = `${actor.app}.${cmd.name}`;
      if (fold.tasks[session]) return invalid("session", "already_exists", "session already exists");
      let requester: number | null = null;
      if (cmd.requester != null) {
        const u = Object.entries(authz.users).find(([, x]) => x.login === cmd.requester);
        if (!u) return invalid("requester", "invalid", "unknown requester");
        requester = Number(u[0]);
      }
      if (openCount(fold, c.at, authz, (t) => requester != null ? t.requester === requester : t.app === actor.app && t.requester == null) >= TASKS_OPEN_PER_REQUESTER)
        return no("TaskCap", "open-task cap");
      const task: AgentTask = { session, kind: "External", state: "InProgress", prefix: null, requester, app: actor.app!,
        installation: actor.id.installation, updatedAt: c.at, creator: actorKey(actor.id), issue: cmd.issue, snapshot: fold.index ?? 0,
        prompt: null, pr: null, summary: cmd.summary, createdAt: c.at, repo: fold.repo };
      return { ok: true, events: [{ t: "TaskCreated", task }] };
    }
    case "TaskPatch": {
      const t = fold.tasks[cmd.session];
      if (!t) return no("NotFound", "Not Found");
      if (!(actor.id.t === "App" && actor.app === t.app)) return no("TaskCaller", "only the task's App may update it");
      const st = taskState(t, c.at, authz);
      if (FINAL_STATES.includes(st)) return no("TaskFinal", `task is ${snake(st)}`);
      if (cmd.state === "Cancelled" || cmd.state === "TimedOut") return invalid("state", "invalid", `state ${snake(cmd.state)} is set only by the host`);
      if (isPaused(fold, t.session) && cmd.state != null && cmd.state !== "WaitingForUser" && !FINAL_STATES.includes(cmd.state))
        return no("TaskPaused", "task is paused: its issue was edited by a non-writer after assignment; a writer must reassign it");
      return { ok: true, events: [{ t: "TaskPatched", session: t.session, state: cmd.state, pr: cmd.pr, summary: cmd.summary }] };
    }
    case "IssueOpen": {
      const n = Math.max(0, ...Object.keys(fold.issues ?? {}).map(Number), ...Object.keys(fold.prs).map(Number)) + 1;
      const issue: Issue = { number: n, author: actorKey(actor.id), revisions: [{ index, title: cmd.title, body: cmd.body, actor: actorKey(actor.id) }],
                             assigned: [], createdAt: c.at };
      return { ok: true, events: [{ t: "IssueOpened", issue }] };
    }
    case "IssueEdit": {
      const iss = fold.issues?.[String(cmd.n)];
      if (!iss) return no("NotFound", "Not Found");
      const writer = isWriter(fold, actor);
      if (!writer && iss.author !== actorKey(actor.id)) return no("NotAllowed", "Must have triage access or be the author");
      const ev: Event[] = [{ t: "IssueEdited", n: cmd.n, title: cmd.title, body: cmd.body, actor: actorKey(actor.id), nonWriter: !writer }];
      if (!writer) {
        // A title or body edit by a non-writer after assignment pauses every live Graftyard-run task on the issue.
        for (const t of Object.values(fold.tasks))
          if (t.kind === "GraftyardRun" && t.issue === cmd.n && !FINAL_STATES.includes(taskState(t, c.at, authz)) && !isPaused(fold, t.session))
            ev.push({ t: "TaskPatched", session: t.session, state: "WaitingForUser", pr: null, summary: null, paused: true });
      }
      return { ok: true, events: ev };
    }
    case "Assign": {
      const iss = fold.issues?.[String(cmd.n)];
      if (!iss) return no("NotFound", "Not Found");
      if (!isWriter(fold, actor)) return no("NotAllowed", "Must have triage access to assign");
      const ev: Event[] = [{ t: "Assigned", n: cmd.n, assignees: cmd.assignees, actor: actorKey(actor.id) }];
      // A writer's reassignment to the controller's bot resumes a paused task with a new snapshot.
      if (authz.controllerApp != null && cmd.assignees.includes(botLogin(authz, authz.controllerApp)))
        for (const t of Object.values(fold.tasks))
          if (t.issue === cmd.n && isPaused(fold, t.session))
            ev.push({ t: "TaskPatched", session: t.session, state: "Queued", pr: null, summary: null, paused: false, snapshot: index });
      return { ok: true, events: ev };
    }
    case "CommentCreate": {
      if (!fold.issues?.[String(cmd.issue)] && !fold.prs[String(cmd.issue)]) return no("NotFound", "Not Found");
      const id = (fold.comments?.length ?? 0) + 1;
      return { ok: true, events: [{ t: "Commented", comment: { id, issue: cmd.issue, author: actorKey(actor.id), body: cmd.body, createdAt: c.at } }] };
    }
    default: return null;
  }
}

function invalid(field: string, code: string, text: string): Decision {
  return { ok: false, refusal: { rule: "Invalid", text: `${text}|${field}|${code}` } };
}
export const snake = (s: string): string => s.replace(/[A-Z]/g, (m, i) => (i ? "_" : "") + m.toLowerCase());

/** Applies the M0-WP13 events (the rest are in decide.ts's `apply`). */
export function applyTask(fold: Fold, e: Event, at: number, index: number): void {
  switch (e.t) {
    case "TaskCreated": fold.tasks[e.task.session] = e.task; break;
    case "TaskPatched": {
      const t = fold.tasks[e.session];
      if (e.state) t.state = e.state;
      if (e.pr != null) t.pr = e.pr;
      if (e.summary != null) t.summary = e.summary;
      if (e.snapshot != null) t.snapshot = e.snapshot;
      if (e.paused === true) (fold.paused ??= {})[e.session] = index;
      if (e.paused === false && fold.paused) delete fold.paused[e.session];
      t.updatedAt = at;
      break;
    }
    case "IssueOpened": (fold.issues ??= {})[String(e.issue.number)] = e.issue; break;
    case "IssueEdited": {
      const iss = fold.issues![String(e.n)];
      const last = iss.revisions.at(-1)!;
      iss.revisions.push({ index, title: e.title ?? last.title, body: e.body ?? last.body, actor: e.actor });
      break;
    }
    case "Assigned": fold.issues![String(e.n)].assigned.push({ index, assignees: e.assignees, actor: e.actor }); break;
    case "Commented": (fold.comments ??= []).push(e.comment); break;
    case "CheckRunStored": (fold.checkRuns ??= []).push(e.run); break;
    default: break;
  }
}

/** The issue as of `index` (the task's snapshot): the last revision at or before it. */
export function issueAt(iss: Issue, index: number): { title: string; body: string } {
  let r = iss.revisions[0];
  for (const x of iss.revisions) if (x.index <= index) r = x;
  return { title: r.title, body: r.body };
}

// ---- The `graftyard_task` token request (`POST /app/installations/{id}/access_tokens`) ----
export interface MintReq { installation: number; repositories: string[] | null; permissions: PermSet | null; graftyardTask: string | null }
/** GHES's rules (the App owns the installation, repositories and permissions within it) plus the additive field's. */
export function decideMint(jwt: Extract<Credential, { t: "AppJwt" }>, req: MintReq, authz: Authz, folds: Record<string, Fold>, at: number):
    { ok: true; cred: Extract<Credential, { t: "Installation" }> } | { ok: false; status: number; refusal: Refusal } {
  const inst = authz.installations[req.installation];
  if (!inst || inst.app !== jwt.app) return { ok: false, status: 404, refusal: { rule: "NotFound", text: "Not Found" } };
  if (!inst.active) return { ok: false, status: 403, refusal: { rule: "NotAllowed", text: "This installation has been suspended" } };
  const all = inst.repos ?? [];
  const repos = req.repositories ?? all;
  if (repos.some((r) => !all.includes(r)))
    return { ok: false, status: 422, refusal: { rule: "Invalid", text: "There is at least one repository that does not exist or is not accessible to the parent installation." } };
  const perms: PermSet = req.permissions ?? inst.perms;
  for (const [k, v] of Object.entries(perms)) {
    const have = inst.perms[k as keyof PermSet];
    if (!have || (v === "write" && have !== "write"))
      return { ok: false, status: 422, refusal: { rule: "Invalid", text: `The permissions requested are not granted to this installation.` } };
  }
  let task: string | null = null;
  if (req.graftyardTask != null) {
    let t: AgentTask | null = null; let home: Fold | null = null;
    for (const f of Object.values(folds)) if (f.tasks[req.graftyardTask]) { t = f.tasks[req.graftyardTask]; home = f; }
    const bad = (text: string) => ({ ok: false as const, status: 422, refusal: { rule: "Invalid" as const, text } });
    if (!t || !home || t.kind !== "GraftyardRun") return bad("graftyard_task must name a Graftyard-run task");
    const st = taskState(t, at, authz);
    if (FINAL_STATES.includes(st as TaskApiState)) return bad(`graftyard_task names a task that is ${snake(st)}`);
    if (isPaused(home, t.session)) return bad("graftyard_task names a paused task");
    if (jwt.app !== t.app && jwt.app !== authz.workspaceApp) return bad("graftyard_task names another App's task");
    const fork = Object.values(folds).find((f) => f.forkOf === home!.repo)?.repo;
    const outside = repos.filter((r) => r !== home!.repo && r !== fork);
    if (!repos.length || outside.length) return bad(`a token for graftyard_task may cover only ${home.repo}${fork ? ` and ${fork}` : ""}; it covers ${outside.join(", ") || "no repository"}`);
    task = t.session;
  }
  return { ok: true, cred: { t: "Installation", app: jwt.app, installation: req.installation, task, repos, perms,
                             jti: `j${at.toString(36)}${Math.random().toString(36).slice(2, 8)}`, exp: at + 3_600_000 } };
}

// ---- The activity feed's edge check (L0-15). WP12 builds the feed itself. ----
export const FEED_POLLS = { token: 2, ip: 50 } as const;
/** What the edge answers before any member sees the poll: 401 anonymous, 403 without `contents: read`, else null. */
export function feedGate(cred: Credential | null, canReadContents: boolean): { status: 401 | 403; message: string } | null {
  if (!cred) return { status: 401, message: "Requires authentication" };
  if (!canReadContents) return { status: 403, message: "Resource not accessible: the activity feed needs contents: read" };
  return null;
}
