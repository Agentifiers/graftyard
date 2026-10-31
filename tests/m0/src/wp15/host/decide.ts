// `decide` for the agent block (design/DESIGN.md, Shape, "Agent load and agent safety"), as a pure function of the
// command, the fold, the `deps` folds read once through `fresh`, the classified actor, and the entry's `at`. `apply`
// folds the events back. Nothing here reads a clock, the network, or storage.

import type { AgentTask, Authz, Cmd, Decision, Deps, Event, Fold, Lease, Principal, Pr, Provenance, RefOutcome, Refusal,
  RefusalRule, Source } from "./model.ts";
import { EXCLUSIVE_MAX, FINAL_STATES, HOST_REFS, SESSION_REFS, TASK_IDLE, ZERO, actorKey } from "./model.ts";
import { globMatch, globOverlap, literalPrefix } from "./glob.ts";
import { agentApp } from "./classify.ts";
import { sessionFromUrl } from "./provenance.ts";
import { applyTask, decideTask } from "./tasks.ts";

/** The org's `OrgPolicy` when the host passes none: `agents_enabled` and `code_mode` off, their defaults. */
export const ORG_DEFAULT = { org: "acme", agentsEnabled: false, codeMode: false, workspace: "Fork" as const, admins: [] };

export interface Ctx { fold: Fold; deps: Deps; actor: Principal; at: number }

const refuse = (rule: RefusalRule, text: string): Decision => ({ ok: false, refusal: { rule, text } });
const R = (rule: RefusalRule, text: string): Refusal => ({ rule, text });

/** The fold that owns the actor's task: the base's when a token with a `task` claim writes to a fork. */
export function taskFold(c: Ctx): Fold {
  return c.fold.forkOf && c.deps.base && c.actor.kind.t === "Agent" && c.actor.kind.agent.task ? c.deps.base : c.fold;
}

/** Live: not final, updated within TASK_IDLE, and its installation still passes the owner Authz read. */
export function taskState(t: AgentTask, at: number, authz: Authz): AgentTask["state"] {
  if (authz.installations[t.installation]?.active === false) return "Cancelled";
  if (!FINAL_STATES.includes(t.state) && at - t.updatedAt >= TASK_IDLE) return "TimedOut";
  return t.state;
}
export const isLive = (t: AgentTask, at: number, authz: Authz): boolean => !FINAL_STATES.includes(taskState(t, at, authz));

/** The holder still passes the owner Authz read: an App holder's installation is active, and a holder that names a task
 *  holds only while that task is live (cancelling releases leases; a derived cancel reads the same way). */
function leaseHolds(l: Lease, c: Ctx, tasks: Fold["tasks"]): boolean {
  if (l.holder.actor.t === "App" && c.deps.authz.installations[l.holder.actor.installation]?.active === false) return false;
  if (l.holder.session) {
    const t = tasks[l.holder.session];
    if (t && !isLive(t, c.at, c.deps.authz)) return false;
  }
  return true;
}
export const leaseLive = (l: Lease, at: number): boolean => at < l.expires;
export const leaseMode = (l: Lease, at: number): Lease["mode"] => l.mode === "Exclusive" && at - l.granted < EXCLUSIVE_MAX ? "Exclusive" : "Shared";

function ownLease(l: Lease, a: Principal): boolean {
  const s = a.kind.t === "Agent" ? a.kind.agent.task : null;
  if (s && l.holder.session === s) return true;
  return actorKey(l.holder.actor) === actorKey(a.id) && l.holder.session === s;
}

function holderName(l: Lease, authz: Authz): string {
  const h = l.holder.actor;
  const who = h.t === "App" ? authz.apps[h.app]?.slug ?? `app ${h.app}` : authz.users[h.user]?.login ?? `user ${h.user}`;
  return l.holder.session ? `${who} (task ${l.holder.session})` : who;
}

/** The allowed prefix of an Agent: its task's `prefix` for a Graftyard-run App, the App's `confine` for a third party. */
function allowedPrefix(c: Ctx): { pattern: string | null; task: AgentTask | null; live: boolean; why: string } {
  const a = c.actor.kind.t === "Agent" ? c.actor.kind.agent : null;
  if (!a) return { pattern: null, task: null, live: true, why: "" };
  const aa = agentApp(c.deps.authz, a.app);
  if (aa?.kind === "GraftyardRun") {
    const t = a.task ? taskFold(c).tasks[a.task] ?? null : null;
    if (!t) return { pattern: null, task: null, live: false, why: a.task ? `task ${a.task} does not exist` : "the token names no task" };
    const st = taskState(t, c.at, c.deps.authz);
    // M0-WP13: a task paused by a non-writer's edit of its issue pushes nothing until a writer reassigns it.
    if (!FINAL_STATES.includes(st) && taskFold(c).paused?.[t.session] != null)
      return { pattern: t.prefix ? t.prefix + "**" : null, task: t, live: false, why: `task ${t.session} is paused: its issue was edited by a non-writer after assignment` };
    return { pattern: t.prefix ? t.prefix + "**" : null, task: t, live: !FINAL_STATES.includes(st), why: `task ${t.session} is ${st.toLowerCase()}` };
  }
  return { pattern: aa?.confine ?? null, task: null, live: true, why: "" };
}

const isAgentPrefix = (ref: string, authz: Authz): boolean =>
  ref.startsWith("refs/heads/graftyard/") || authz.agentApps.some((a) => a.confine != null && globMatch(a.confine, ref));
export { isAgentPrefix };

/** Linking rule: `Session` only when the commit's pusher has a matching AgentTask or AgentMeta; every other trailer
 *  stays `Claimed`. */
export function linkProvenance(list: Provenance[], c: Ctx, refs: string[]): Provenance[] {
  const a = c.actor;
  const tasks = taskFold(c).tasks;
  const own = new Set<string>();
  if (a.kind.t === "Agent" && a.kind.agent.task) own.add(a.kind.agent.task);
  if (a.id.t === "App") {
    for (const t of Object.values(tasks)) if (t.kind === "External" && t.app === a.app) own.add(t.session);
    for (const pr of Object.values(c.fold.prs))
      if (pr.open && pr.meta && pr.meta.session && pr.meta.app === a.app && refs.includes(pr.headRef)) own.add(pr.meta.session);
  }
  const repo = c.fold.forkOf && c.deps.base ? c.deps.base.repo : c.fold.repo;
  return list.map((p) => ({ commit: p.commit, sources: p.sources.map((s): Source => {
    if (s.t !== "Claimed" || s.key !== "GraftyardSession" || a.id.t !== "App") return s;
    const sid = sessionFromUrl(s.value, c.deps.authz.host, repo);
    return sid && own.has(sid) ? { t: "Session", session: sid } : s;
  }) }));
}

// ---- Push ----
function decidePush(cmd: Extract<Cmd, { t: "Push" }>, c: Ctx): Decision {
  const { fold, actor } = c;
  const agent = actor.kind.t === "Agent";
  const ap = allowedPrefix(c);
  const authz = c.deps.authz;
  const policyFold = c.fold.forkOf && c.deps.base ? c.deps.base : fold;
  const tasks = policyFold.tasks;
  const outcomes: RefOutcome[] = [];
  for (const u of cmd.updates) {
    const fact = cmd.facts.find((f) => f.name === u.name);
    const cur = fold.refs[u.name] ?? null;
    const one = (): Refusal | null => {
      // Host-owned refs, for every actor. Only `graftyard-workspace` may append (create or fast-forward) its own task's
      // session ref, under the same live-task rule as its other pushes.
      if (u.name.startsWith(HOST_REFS)) {
        const ws = actor.id.t === "App" && actor.app === authz.workspaceApp && agent;
        const mine = ap.task && u.name === SESSION_REFS + ap.task.session;
        if (!ws) return R("HostOwned", `refused: ${HOST_REFS} is host-owned; only graftyard-workspace appends session logs`);
        if (!mine) return R("HostOwned", `refused: graftyard-workspace may append only ${SESSION_REFS}${ap.task?.session ?? "<its task>"}`);
        if (!ap.live) return R("NoLiveTask", `refused: ${ap.why}`);
        if (u.new == null || (cur != null && !fact?.ff)) return R("HostOwned", `refused: ${u.name} takes appends only`);
      }
      if ((u.old ?? null) !== cur) return R("Stale", "fetch first");
      if (u.new != null && fact?.missing) return R("Missing", "missing necessary objects");
      if (agent && !u.name.startsWith(HOST_REFS)) {
        if (u.name.startsWith("refs/tags/") && u.new != null) return R("Release", "refused: agents may not create releases or tags");
        if (!ap.live) return R("NoLiveTask", `refused: push needs a live task; ${ap.why}`);
        if (!ap.pattern || !globMatch(ap.pattern, u.name))
          return R("OutsidePrefix", `refused: ${actor.name} may push only ${ap.pattern ?? "(no prefix)"}`);
      }
      // Rulesets: deletion and non-fast-forward. An Agent may never use a bypass.
      const nonFf = u.new == null || (cur != null && fact != null && !fact.ff);
      if (nonFf && cur != null) {
        for (const rs of policyFold.rulesets) {
          if (!rs.refs.some((p) => globMatch(p, u.name))) continue;
          const listed = actor.app != null && rs.bypass.includes(actor.app);
          if (listed && agent) return R("Bypass", `refused: agents may not use ruleset bypass (ruleset ${rs.id})`);
          if (!listed) return R("NotAllowed", `Cannot ${u.new == null ? "delete" : "force-push to"} this branch (ruleset ${rs.id})`);
        }
      }
      // M0-WP13: a ref write that lands an open agent PR's head on its base merges that PR, so it takes `Merge`'s rules:
      // the MCP WriteScope, and `AgentGate` on that exact head. (git, REST `PATCH /git/refs`, `POST /merges`.)
      for (const h of fact?.contains ?? []) {
        const pr = Object.values(fold.prs).find((x) => x.open && x.meta && x.base === u.name && x.headOid === h);
        if (!pr) continue;
        if (agent) return R("Merge", "refused: agents may not merge pull requests");
        if (actor.mcpWrite && actor.mcpWrite !== "WriteApproveMerge") return R("McpScope", `refused: this MCP grant does not include merge (the update merges #${pr.number})`);
        const why = gateHolds(pr, c, authz);
        if (why) return R("GateUnsatisfied", `refused: the update merges #${pr.number}, and graftyard/agent-gate is not satisfied for ${pr.headOid.slice(0, 7)}: ${why}`);
      }
      if (agent && u.new != null) {
        const hit = cmd.protected.find((h) => h.refname === u.name && h.paths.length);
        if (hit) return R("Protected", `refused: ${hit.paths[0]} is a protected path; agents may not change it`);
        if (policyFold.policy.leases === "AgentsExclusive") {
          const ch = cmd.changed.find((x) => x.refname === u.name);
          if (ch) {
            for (const l of policyFold.leases) {
              if (!leaseLive(l, c.at) || leaseMode(l, c.at) !== "Exclusive" || ownLease(l, actor) || !leaseHolds(l, c, tasks)) continue;
              for (const g of l.scope.paths) {
                const lp = literalPrefix(g);
                const p = ch.paths.find((x) => (x.startsWith(lp) || lp.startsWith(x.replace(/\/$/, ""))) &&
                  (x.endsWith("/") && ch.truncated ? globOverlap(x, g) : globMatch(g, x)));
                if (p) return R("Leased", `refused: ${g} is leased Exclusive by ${holderName(l, authz)} until ${new Date(l.expires).toISOString()}`);
              }
            }
          }
        }
      }
      return null;
    };
    const r = one();
    outcomes.push(r ? { name: u.name, ok: false, refusal: r } : { name: u.name, ok: true });
  }
  const failed = outcomes.filter((o) => !o.ok);
  if (failed.length && cmd.atomic) {
    const first = (failed[0] as Extract<RefOutcome, { ok: false }>).refusal;
    return { ok: false, refusal: first, refs: outcomes.map((o) => o.ok ? { name: o.name, ok: false, refusal: R("Atomic", "atomic push failed") } : o) };
  }
  const okRefs = new Set(outcomes.filter((o) => o.ok).map((o) => o.name));
  if (!outcomes.length) return { ok: true, events: [], refs: [] };
  if (!okRefs.size) return { ok: false, refusal: (failed[0] as Extract<RefOutcome, { ok: false }>).refusal, refs: outcomes };
  const updates = cmd.updates.filter((u) => okRefs.has(u.name));
  const events: Event[] = [{ t: "Refs", updates, actor: actorKey(actor.id) }];
  const prov = linkProvenance(cmd.provenance, c, [...okRefs]);
  if (prov.length) events.push({ t: "Provenance", list: prov });
  for (const u of updates) {
    if (u.new && isAgentPrefix(u.name, authz)) {
      const ch = cmd.changed.find((x) => x.refname === u.name);
      events.push({ t: "PathSet", refname: u.name, paths: ch?.paths ?? [], truncated: ch?.truncated ?? false });
    }
    for (const pr of Object.values(fold.prs)) if (pr.open && pr.headRef === u.name && u.new) events.push({ t: "PrSync", pr: pr.number, head: u.new, pusher: actorKey(actor.id) });
    const fact = cmd.facts.find((f) => f.name === u.name);
    for (const pr of Object.values(fold.prs))
      if (pr.open && pr.base === u.name && fact?.contains?.includes(pr.headOid)) events.push({ t: "PrMerged", pr: pr.number });
  }
  return { ok: true, events, refs: outcomes };
}

// ---- Merge and the gate ----
/** Approvals `AgentGate` needs: one, plus `extra_human_approvals`, plus one more when `AgentMeta.requester` is empty,
 *  because the requester cannot be excluded (design/DESIGN.md, Shape, `AgentGate`; decisions A23). */
export function approvalsNeeded(pr: Pr, policy: Fold["policy"]): number {
  return 1 + policy.extraHumanApprovals + (pr.meta && pr.meta.requester == null ? 1 : 0);
}

function gateHolds(pr: Pr, c: Ctx, authz: Authz): string | null {
  void authz;
  const rows = pr.approvals ?? (pr.gate ? [pr.gate] : []);
  if (!rows.length) return "no approval from a human writer";
  const ok = new Set<number>();
  let why = "";
  for (const g of rows) {
    if (g.head !== pr.headOid) { why = `approval was for ${g.head.slice(0, 7)}, head is ${pr.headOid.slice(0, 7)}`; continue; }
    if (pr.meta?.requester === g.satisfiedBy) { why = "the approver is the task's requester"; continue; }
    if (pr.pushers.includes(`u${g.satisfiedBy}`)) { why = "the approver pushed to this head"; continue; }
    const role = c.fold.collaborators[`u${g.satisfiedBy}`];
    if (role !== "write" && role !== "admin") { why = "the approver is not a writer"; continue; }
    ok.add(g.satisfiedBy);
  }
  const need = approvalsNeeded(pr, c.fold.policy);
  if (ok.size >= need) return null;
  if (ok.size === 0) return why || "no approval from a human writer";
  return `${ok.size} of ${need} approvals from human writers on ${pr.headOid.slice(0, 7)}` +
    (pr.meta?.requester == null ? " (no recorded requester, so one more is needed)" : "");
}

/** Whether an APPROVE review by this actor writes `AgentGate`: a user credential that is neither an AgentApp's nor
 *  MCP-issued, from a writer who is not the requester and did not push to that head. */
function approvalWritesGate(pr: Pr, c: Ctx): boolean {
  const a = c.actor;
  if (a.id.t !== "User") return false;
  if (a.app != null && agentApp(c.deps.authz, a.app)) return false;
  const role = c.fold.collaborators[`u${a.id.user}`];
  if (role !== "write" && role !== "admin") return false;
  if (pr.meta?.requester === a.id.user) return false;
  if (pr.pushers.includes(`u${a.id.user}`)) return false;
  return true;
}

function prOf(fold: Fold, n: number): Pr | null { return fold.prs[String(n)] ?? null; }

export function decide(cmd: Cmd, c: Ctx): Decision {
  const agent = c.actor.kind.t === "Agent";
  const authz = c.deps.authz;
  const policy = (c.fold.forkOf && c.deps.base ? c.deps.base : c.fold).policy;
  switch (cmd.t) {
    case "Push": return decidePush(cmd, c);
    case "Review": {
      const pr = prOf(c.fold, cmd.pr);
      if (!pr) return refuse("NotFound", "Not Found");
      if (cmd.event === "APPROVE" && agent) return refuse("Approve", "refused: agents may not approve pull requests");
      if (cmd.event === "APPROVE" && c.actor.mcpWrite && c.actor.mcpWrite !== "WriteApproveMerge")
        return refuse("McpScope", "refused: this MCP grant does not include approve");
      const ev: Event[] = [{ t: "Review", pr: pr.number, event: cmd.event, by: actorKey(c.actor.id) }];
      if (cmd.event === "APPROVE" && pr.meta && approvalWritesGate(pr, c) && c.actor.id.t === "User")
        ev.push({ t: "AgentGate", gate: { pr: pr.number, head: pr.headOid, satisfiedBy: c.actor.id.user } });
      return { ok: true, events: ev };
    }
    case "Merge": {
      const pr = prOf(c.fold, cmd.pr);
      if (!pr || !pr.open) return refuse("NotFound", "Not Found");
      if (agent) return refuse("Merge", "refused: agents may not merge pull requests");
      if (c.actor.mcpWrite && c.actor.mcpWrite !== "WriteApproveMerge") return refuse("McpScope", "refused: this MCP grant does not include merge");
      if (cmd.sha && cmd.sha !== pr.headOid) return refuse("Stale", "Head branch was modified. Review and try the merge again.");
      if (pr.draft) return refuse("NotAllowed", "Pull Request is still a draft");
      if (pr.meta) {
        const why = gateHolds(pr, c, authz);
        if (why) return refuse("GateUnsatisfied", `refused: graftyard/agent-gate is not satisfied for ${pr.headOid.slice(0, 7)}: ${why}`);
      }
      return { ok: true, events: [{ t: "PrMerged", pr: pr.number }] };
    }
    case "ReadyForReview": {
      const pr = prOf(c.fold, cmd.pr);
      if (!pr) return refuse("NotFound", "Not Found");
      if (agent) return refuse("ReadyForReview", "refused: agents may not mark pull requests ready for review");
      return { ok: true, events: [{ t: "PrReady", pr: pr.number }] };
    }
    case "RulesetEdit": case "BranchProtectionEdit":
      if (agent) return refuse("Bypass", "refused: agents may not edit rulesets, branch protection, or bypass lists");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "RepositoryDispatch": case "WorkflowDispatch":
      if (agent) return refuse("Dispatch", "refused: agents may not dispatch workflows");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "WorkflowRerun":
      if (agent) return refuse("Rerun", "refused: agents may not re-run workflows");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "ReleaseCreate":
      if (agent) return refuse("Release", "refused: agents may not create releases or tags");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "LabelWrite": {
      const bad = agent ? cmd.labels.find((l) => !policy.labels.includes(l)) : undefined;
      if (bad !== undefined) return refuse("Label", `refused: label ${JSON.stringify(bad)} is not in this repository's agent label allowlist`);
      return { ok: true, events: [{ t: "Labeled", issue: cmd.issue, labels: cmd.labels }] };
    }
    case "StatusCreate": case "CheckRunCreate": {
      const ctxName = cmd.t === "StatusCreate" ? cmd.context : cmd.name;
      if (agent && ctxName.startsWith("graftyard/")) return refuse("GraftyardStatus", `refused: agents may not write ${ctxName}; graftyard/ contexts are host-owned`);
      const ev: Event[] = [{ t: "Status", sha: cmd.sha, context: ctxName }];
      if (cmd.t === "CheckRunCreate" && cmd.output)
        ev.push({ t: "CheckRunStored", run: { id: (c.fold.checkRuns?.length ?? 0) + 1, sha: cmd.sha, name: cmd.name, author: actorKey(c.actor.id), ...cmd.output } });
      return { ok: true, events: ev };
    }
    case "BranchRename":
      if (agent) return refuse("Rename", "refused: agents may not rename branches");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "HookEdit":
      if (agent) return refuse("Hook", "refused: agents may not edit webhooks");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "DeployKeyEdit":
      if (agent) return refuse("DeployKey", "refused: agents may not edit deploy keys");
      return { ok: true, events: [{ t: "Recorded", what: cmd.t }] };
    case "PrOpen": {
      const aa = agentApp(authz, c.actor.app);
      if (aa?.kind === "GraftyardRun" && !cmd.draft) return refuse("DraftOnly", "refused: graftyard-agent opens draft pull requests only");
      const head = c.fold.refs[cmd.head];
      if (!head) return refuse("NotFound", "head ref not found");
      let meta = null;
      if (aa) {
        // Session header accepted only for a task whose `app` is the same App.
        const s = c.actor.sessionHeader;
        const t = s ? taskFold(c).tasks[s] : undefined;
        const ok = t && t.app === c.actor.app;
        meta = { app: aa.app, session: ok ? t!.session : null, requester: ok ? t!.requester : (aa.kind === "Mcp" && c.actor.id.t === "User" ? c.actor.id.user : null), repo: c.fold.repo };
      }
      const n = Math.max(c.fold.nextPr, ...Object.keys(c.fold.issues ?? {}).map((k) => Number(k) + 1));
      const pr: Pr = { number: n, headRef: cmd.head, headOid: head, base: cmd.base, draft: cmd.draft, open: true, merged: false,
                       meta, gate: null, author: actorKey(c.actor.id), pushers: [], body: cmd.body ?? "" };
      return { ok: true, events: [{ t: "PrOpened", pr }] };
    }
    case "TaskCancel": {
      const t = c.fold.tasks[cmd.session];
      if (!t) return refuse("NotFound", "Not Found");
      const st = taskState(t, c.at, authz);
      if (FINAL_STATES.includes(st)) return refuse("TaskFinal", `task is ${st.replace(/[A-Z]/g, (m, i) => (i ? "_" : "") + m.toLowerCase())}`);
      return { ok: true, events: [{ t: "TaskUpdated", session: t.session, state: "Cancelled" },
        { t: "LeaseRelease", ids: c.fold.leases.filter((l) => l.holder.session === t.session).map((l) => l.id) },
        ...(t.kind === "GraftyardRun" ? [{ t: "Deliver" as const, event: "graftyard_task", action: "cancelled", session: t.session }] : [])] };
    }
    default:
      return decideTask(cmd, { fold: c.fold, authz, org: c.deps.org ?? ORG_DEFAULT, actor: c.actor, at: c.at }) ?? refuse("NotFound", "Not Found");
  }
}

/** Applies committed events to the fold, in place. */
export function apply(fold: Fold, events: Event[], at: number): void {
  const index = fold.index = (fold.index ?? 0) + 1;
  for (const e of events) {
    applyTask(fold, e, at, index);
    switch (e.t) {
      case "Refs": for (const u of e.updates) { if (u.new == null) delete fold.refs[u.name]; else fold.refs[u.name] = u.new; } break;
      case "PrSync": { const pr = fold.prs[String(e.pr)]; pr.headOid = e.head; if (e.pusher.startsWith("u") && !pr.pushers.includes(e.pusher)) pr.pushers.push(e.pusher); break; }
      case "AgentGate": { const pr = fold.prs[String(e.gate.pr)]; pr.gate = e.gate;
        pr.approvals = [...(pr.approvals ?? []).filter((g) => g.satisfiedBy !== e.gate.satisfiedBy), e.gate]; break; }
      case "PrMerged": { const pr = fold.prs[String(e.pr)]; pr.open = false; pr.merged = true; break; }
      case "PrReady": fold.prs[String(e.pr)].draft = false; break;
      case "PrOpened": fold.prs[String(e.pr.number)] = e.pr; fold.nextPr = e.pr.number + 1; break;
      case "TaskUpdated": { const t = fold.tasks[e.session]; t.state = e.state; t.updatedAt = at; break; }
      case "LeaseRelease": fold.leases = fold.leases.filter((l) => !e.ids.includes(l.id)); break;
      default: break; // Provenance, PathSet, Review, Labeled, Status, Recorded: kept in the audit rows only.
    }
  }
}

export { ZERO };
