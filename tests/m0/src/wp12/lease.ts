// M0-WP12: leases, tasks, actor kinds, and `place_learners`, as pure functions over a core-tier view (build/BUILD-PLAN.md,
// M0-WP12; design/AGENT-LAYER.md, Shape, first block, "Leases", and "Activity feed"; design/DESIGN.md, Shape, "Chains",
// `ExtendLeases`). The classification rules are those of src/agent/classify.ts (M0-WP10), copied, not imported, because
// another agent is editing src/agent/. `decideX` is pure: its result depends only on the command, the view (the core
// fold plus the earlier commands of the entry, plus the `deps` rows the leader read once through `fresh`), and `at`.

import { globMatch, globOverlap, literalPrefix } from "./glob.ts";

export const LEASE_TTL = 600_000;
export const TTL_MIN = 60_000;
export const TTL_MAX = 3_600_000;
export const EXCLUSIVE_MAX = 4 * 3_600_000;
export const LEASES_PER_HOLDER = 8;
export const LEASES_PER_APP = 1_000;
export const PLAN_LEASES_PER_REPO = 500;
export const LEASES_PER_REPO = 4_000;
/** A plan-block lease lives while its draft is open and at most 24 h after its last push. */
export const PLAN_TTL = 24 * 3_600_000;
export const MAX_LEARNERS = 16;
export const FEED_HELD_MAX = 1_000;

export type Cred =
  | { t: "Installation"; app: number; installation: number; task: string | null }
  | { t: "UserToServer"; user: number; app: number }
  | { t: "User"; user: number };
/** The credential the edge verified plus the opt-in `X-Graftyard-Agent-Session` header. */
export interface Who { cred: Cred; session?: string | null }
export type ActorId = { t: "User"; user: number } | { t: "App"; app: number; installation: number };
export interface LeaseHolder { actor: ActorId; app: number | null; session: string | null }
export type LeaseMode = "Shared" | "Exclusive";
export interface Lease {
  id: string; holder: LeaseHolder; paths: string[]; refs: string[]; mode: LeaseMode; intent: string;
  granted: number; expires: number; plan?: boolean;
}
export type TaskState = "Queued" | "InProgress" | "Completed" | "Failed" | "TimedOut" | "Cancelled";
export const FINAL: TaskState[] = ["Completed", "Failed", "TimedOut", "Cancelled"];
export interface Task { session: string; kind: "GraftyardRun" | "External"; state: TaskState; prefix: string | null; app: number;
                        installation: number; requester: number | null }
export interface AgentAppRow { app: number; kind: "GraftyardRun" | "ThirdParty" | "Mcp"; confine: string | null }
/** Rows of `Authz(Genesis.account)` every App command lists in `deps()`. `push` is the installation's push access to
 *  this repository (contents: write and the repository selected). */
export interface Install { app: number; active: boolean; push: boolean }
export interface AuthzRows { installs: Record<string, Install>; agentApps: Record<string, AgentAppRow> }
export type ActorKind = "User" | "Bot" | "Agent";
export type Enforce = "Advisory" | "AgentsExclusive";

/** What `decide` reads from the core tier. The member backs it with SQLite plus an overlay of the entry's earlier
 *  commands; every method is a core-tier read, so a metadata load can never come from here. */
export interface CoreView {
  task(session: string): Task | null;
  collab(user: number): "read" | "write" | "admin" | null;
  policy(): Enforce;
  lease(id: string): Lease | null;
  /** Live (at < expires) Exclusive-granted leases whose glob's literal prefix is one of `ancestors` or lies under one of
   *  `under`. The literal-prefix index (design/AGENT-LAYER.md, `LeaseEnforce`). */
  exclusiveNear(ancestors: string[], under: string[], at: number): Lease[];
  /** Live leases of one holder key, of one App, plan-block leases, and all live leases, counted. */
  holderLeases(holder: string, at: number): Lease[];
  countApp(app: number, at: number): number;
  countPlan(at: number): number;
  countRepo(at: number): number;
  authz: AuthzRows;
}

export const actorKey = (a: ActorId) => (a.t === "User" ? `u:${a.user}` : `a:${a.app}:${a.installation}`);
export const holderKey = (h: LeaseHolder) => `${actorKey(h.actor)}|${h.session ?? ""}`;
const appOf = (c: Cred) => (c.t === "User" ? null : c.app);

/** `ActorKind` and the holder a token acquires as (src/agent/classify.ts rules). */
export function classify(w: Who, authz: AuthzRows): { kind: ActorKind; admit: "Human" | "Bot" | "Agent"; holder: LeaseHolder; confine: string | null } {
  const c = w.cred;
  if (c.t === "Installation") {
    const aa = authz.agentApps[c.app] ?? null;
    const actor: ActorId = { t: "App", app: c.app, installation: c.installation };
    // An App token names its session: a Graftyard-run token's `task` claim, or a third-party App's External task.
    const session = aa?.kind === "GraftyardRun" ? c.task : w.session ? (w.session.includes(".") ? w.session : `${c.app}.${w.session}`) : null;
    const kind: ActorKind = aa?.kind === "GraftyardRun" || (aa?.kind === "ThirdParty" && aa.confine) ? "Agent" : "Bot";
    return { kind, admit: aa ? "Agent" : "Bot", holder: { actor, app: c.app, session }, confine: aa?.confine ?? null };
  }
  const aa = c.t === "UserToServer" ? authz.agentApps[c.app] ?? null : null;
  return { kind: "User", admit: aa || w.session ? "Agent" : "Human",
           holder: { actor: { t: "User", user: c.user }, app: appOf(c), session: w.session ?? null }, confine: null };
}

/** A task is live when it is not final and its installation is active: uninstalling an App reads its tasks as
 *  cancelled with no sweep. */
export function taskLive(t: Task | null, authz: AuthzRows): boolean {
  return !!t && !FINAL.includes(t.state) && !!authz.installs[t.installation]?.active;
}
/** The state the task API reports. */
export const taskState = (t: Task, authz: AuthzRows): TaskState => (authz.installs[t.installation]?.active ? t.state : "Cancelled");

/** The holder still passes the owner `Authz` read: an App's installation is active with push access; a user keeps push
 *  access. An Exclusive lease also needs its task live. Otherwise the lease counts as released. */
export function holds(l: Lease, view: CoreView, at: number): boolean {
  if (at >= l.expires) return false;
  const a = l.holder.actor;
  if (a.t === "App") {
    const i = view.authz.installs[a.installation];
    if (!i?.active || !i.push) return false;
  } else {
    const p = view.collab(a.user);
    if (p !== "write" && p !== "admin") return false;
  }
  if (l.mode === "Exclusive" && l.holder.session && !taskLive(view.task(l.holder.session), view.authz)) return false;
  return true;
}
export const modeAt = (l: Lease, at: number): LeaseMode => (l.mode === "Exclusive" && at - l.granted < EXCLUSIVE_MAX ? "Exclusive" : "Shared");

/** Every literal prefix of a path: `a/b/c` gives `a`, `a/b`, `a/b/c`. */
export function prefixesOf(path: string): string[] {
  const segs = path.split("/").filter(Boolean), out: string[] = [];
  for (let i = 1; i <= segs.length; i++) out.push(segs.slice(0, i).join("/"));
  return out;
}

export type XCmd =
  | { t: "Acquire"; who: Who; paths: string[]; refs: string[]; mode: LeaseMode; ttl?: number; intent?: string }
  | { t: "Renew"; who: Who; id: string; ttl?: number }
  | { t: "Release"; who: Who; ids: string[] }
  | { t: "PlanBlock"; who: Who; pr: number; paths: string[] }
  /** A push or a REST ref write (`surface`), as a compare-and-set on `ref` with the commit's `changed` paths. */
  | { t: "Push"; who: Who; ref: string; expect: string | null; value: string; changed: string[]; surface: "git" | "rest" }
  /** A merge-queue merge of PR `pr` into `ref` (`refs/heads/main`), compare-and-set on its tip. */
  | { t: "Merge"; who: Who; pr: number; ref: string; expect: string | null; value: string }
  | { t: "TaskCancel"; who: Who; session: string }
  /** Setup and operator rows, written as given (the runner's bed: tasks, collaborators, policy, PRs, authz rows). */
  | { t: "Seed"; events: XEv[] }
  /** A metadata-only write (a comment, a PR body, a `PathSet` row, a task summary): appended, decided without a read. */
  | { t: "Meta"; kind: string; body: string };

export type XEv =
  | { t: "LeaseAcquire"; lease: Lease; plan: boolean }
  | { t: "LeaseRenew"; id: string; expires: number }
  | { t: "LeaseRelease"; ids: string[]; by: string }
  | { t: "TaskCreated"; task: Task }
  | { t: "TaskUpdated"; session: string; state: TaskState }
  | { t: "Collab"; user: number; perm: "read" | "write" | "admin" | null }
  | { t: "Policy"; leases: Enforce }
  | { t: "Install"; installation: number; app: number; active: boolean; push: boolean }
  | { t: "AgentApp"; row: AgentAppRow }
  | { t: "PrOpened"; n: number; head: string; agentApp: number | null }
  | { t: "Review"; n: number; reviewer: number; state: "APPROVED" | "CHANGES_REQUESTED"; commit: string }
  | { t: "Check"; sha: string; name: string; conclusion: "success" | "failure" }
  | { t: "Merged"; n: number; commit: string }
  | { t: "Gate"; n: number; head: string }
  /** The register write a push or merge makes (the ref move), core. */
  | { t: "Ref"; ref: string; expect: string | null; value: string; changed?: string[]; pack?: number }
  | { t: "Meta"; kind: string; body: string };

/** `tier_of`: Meta is metadata only, Merged and PrOpened touch both tiers (here only their core part is modeled, and
 *  their metadata part is a `Meta` event in the same entry), the rest is core. */
export const tierOf = (e: XEv): "core" | "meta" => (e.t === "Meta" ? "meta" : "core");

export type XOut = { ok: true; status: number; lease?: Lease; overlaps?: Lease[]; text?: string; claims?: unknown }
  | { ok: false; status: number; rule: string; text: string; conflicts?: Lease[] };

/** Core-tier reads `decide` needs for merges. */
export interface MergeView { pull(n: number): { head: string; state: "open" | "merged" | "closed"; agentApp: number | null } | null;
  approvedOn(n: number, commit: string): boolean; checksGreen(sha: string): boolean; gate(n: number, head: string): boolean; ref(k: string): string | null }

const refuse = (status: number, rule: string, text: string, conflicts?: Lease[]): XOut => ({ ok: false, status, rule, text, conflicts });

function hasPush(who: Who, cls: ReturnType<typeof classify>, view: CoreView): boolean {
  const a = cls.holder.actor;
  if (a.t === "App") { const i = view.authz.installs[a.installation]; return !!i?.active && i.push; }
  const p = view.collab(a.user);
  return p === "write" || p === "admin";
}

/** Exclusive leases of another holder that overlap `globs`, live and still Exclusive at `at`, and still held. Another
 *  holder's lease is one whose `session` differs from the pusher's task, or, for a holder with no task, whose actor
 *  differs (design/AGENT-LAYER.md, `LeaseEnforce`). */
export function exclusiveConflicts(globs: string[], mine: LeaseHolder, view: CoreView, at: number): Lease[] {
  const anc = new Set<string>(), under = new Set<string>();
  for (const g of globs) { const lp = literalPrefix(g); for (const p of prefixesOf(lp)) anc.add(p); under.add(lp); }
  const other = (l: Lease) => (mine.session ? l.holder.session !== mine.session : actorKey(l.holder.actor) !== actorKey(mine.actor) || l.holder.session !== null);
  return view.exclusiveNear([...anc], [...under], at).filter((l) => modeAt(l, at) === "Exclusive" && other(l) && holds(l, view, at)
    && l.paths.some((lp) => globs.some((g) => globOverlap(lp, g))));
}

/** `decide` for the agent-layer commands. `rid` makes the lease id deterministic, so followers recompute nothing. */
export function decideX(cmd: XCmd, view: CoreView, mv: MergeView, at: number, rid: string): { events: XEv[]; out: XOut } {
  const none = (out: XOut) => ({ events: [] as XEv[], out });
  switch (cmd.t) {
    case "Seed": return { events: cmd.events, out: { ok: true, status: 200 } };
    case "Meta": return { events: [{ t: "Meta", kind: cmd.kind, body: cmd.body }], out: { ok: true, status: 201 } };
    case "Acquire": {
      const cls = classify(cmd.who, view.authz);
      if (!hasPush(cmd.who, cls, view)) return none(refuse(403, "NoPush", "refused: a lease needs push access"));
      if (cmd.who.cred.t === "Installation" && !cls.holder.session) return none(refuse(422, "NoSession", "refused: an App token must name its session"));
      if (!cmd.paths.length && !cmd.refs.length) return none(refuse(422, "Empty", "refused: a lease needs a path or a ref"));
      if (cmd.paths.length > 32) return none(refuse(422, "TooMany", "refused: at most 32 globs"));
      const task = cls.holder.session ? view.task(cls.holder.session) : null;
      if (cls.kind === "Agent") {
        const scope = task?.prefix ?? cls.confine;
        for (const r of cmd.refs) if (!scope || !globMatch(scope.endsWith("/") ? scope : scope, literalPrefix(r) + "/x"))
          return none(refuse(422, "RefScope", `refused: ${r} is outside the task's prefix`));
      }
      if (cmd.mode === "Exclusive") {
        if (!task || !taskLive(task, view.authz)) return none(refuse(422, "NoLiveTask", "refused: an Exclusive lease needs a live task"));
        // Prototype rule, not in the design: a user-to-server token names a task of its own App (an `AgentApp`'s External
        // task, as `Graftyard MCP` would create), and a plain user token a task that user requested.
        const c = cmd.who.cred;
        if ((c.t === "UserToServer" && task.app !== c.app) || (c.t === "User" && task.requester !== c.user))
          return none(refuse(422, "NoLiveTask", "refused: an Exclusive lease needs a live task of the holder"));
        for (const g of cmd.paths) {
          const first = g.split("/")[0];
          if (!first || first.includes("*")) return none(refuse(422, "Literal", `refused: ${g} needs a literal first segment`));
        }
        const own = view.holderLeases(holderKey(cls.holder), at).filter((l) => l.mode === "Exclusive" && modeAt(l, at) === "Exclusive" && holds(l, view, at));
        if (own.length) return none(refuse(422, "OnePerTask", `refused: task ${task.session} already holds Exclusive lease ${own[0].id}`));
      }
      const ttl = Math.min(TTL_MAX, Math.max(TTL_MIN, cmd.ttl ?? LEASE_TTL));
      const mine = view.holderLeases(holderKey(cls.holder), at).filter((l) => holds(l, view, at));
      if (mine.length >= LEASES_PER_HOLDER) return none(refuse(422, "Cap", "refused: LEASES_PER_HOLDER"));
      if (cls.holder.app !== null && view.countApp(cls.holder.app, at) >= LEASES_PER_APP) return none(refuse(422, "Cap", "refused: LEASES_PER_APP"));
      if (view.countRepo(at) >= LEASES_PER_REPO) return none(refuse(422, "Cap", "refused: LEASES_PER_REPO"));
      if (cmd.mode === "Exclusive") {
        const c = exclusiveConflicts(cmd.paths, cls.holder, view, at);
        if (c.length) return none(refuse(409, "Conflict", `conflict: ${c[0].paths[0]} is leased Exclusive by ${actorKey(c[0].holder.actor)}`, c));
      }
      const lease: Lease = { id: `L${rid}`, holder: cls.holder, paths: cmd.paths, refs: cmd.refs, mode: cmd.mode,
        intent: (cmd.intent ?? "").slice(0, 280), granted: at, expires: at + ttl };
      return { events: [{ t: "LeaseAcquire", lease, plan: false }], out: { ok: true, status: 201, lease } };
    }
    case "Renew": {
      const l = view.lease(cmd.id), cls = classify(cmd.who, view.authz);
      if (!l || !holds(l, view, at)) return none(refuse(404, "NotFound", "lease not found"));
      if (holderKey(l.holder) !== holderKey(cls.holder)) return none(refuse(403, "NotHolder", "refused: only the holder renews"));
      const expires = at + Math.min(TTL_MAX, Math.max(TTL_MIN, cmd.ttl ?? LEASE_TTL));
      return { events: [{ t: "LeaseRenew", id: l.id, expires }], out: { ok: true, status: 200, lease: { ...l, expires } } };
    }
    case "Release": {
      const cls = classify(cmd.who, view.authz);
      const admin = cls.holder.actor.t === "User" && view.collab(cls.holder.actor.user) === "admin";
      const ids = cmd.ids.filter((id) => { const l = view.lease(id); return l && (admin || holderKey(l.holder) === holderKey(cls.holder)); });
      if (!ids.length) return none(refuse(404, "NotFound", "lease not found"));
      return { events: [{ t: "LeaseRelease", ids, by: actorKey(cls.holder.actor) }], out: { ok: true, status: 204 } };
    }
    case "PlanBlock": {
      // The block parser stays GHES-compatible: a block from a non-writer is ignored and the body is still accepted.
      const cls = classify(cmd.who, view.authz);
      if (!hasPush(cmd.who, cls, view)) return none({ ok: true, status: 200, text: "plan block ignored: editor has no push access" });
      if (view.countPlan(at) >= PLAN_LEASES_PER_REPO) return none({ ok: true, status: 200, text: "plan block ignored: PLAN_LEASES_PER_REPO" });
      if (view.holderLeases(holderKey(cls.holder), at).filter((l) => holds(l, view, at)).length >= LEASES_PER_HOLDER)
        return none({ ok: true, status: 200, text: "plan block ignored: LEASES_PER_HOLDER" });
      const lease: Lease = { id: `L${rid}`, holder: cls.holder, paths: cmd.paths, refs: [], mode: "Shared", intent: `plan block on #${cmd.pr}`,
        granted: at, expires: at + PLAN_TTL, plan: true };
      return { events: [{ t: "LeaseAcquire", lease, plan: true }], out: { ok: true, status: 200, lease } };
    }
    case "TaskCancel": {
      const t = view.task(cmd.session);
      if (!t) return none(refuse(404, "NotFound", "task not found"));
      const ids = view.holderLeases("*session:" + t.session, at).map((l) => l.id);
      return { events: [{ t: "TaskUpdated", session: t.session, state: "Cancelled" }, ...(ids.length ? [{ t: "LeaseRelease" as const, ids, by: "cancel" }] : [])],
               out: { ok: true, status: 202 } };
    }
    case "Merge": {
      const p = mv.pull(cmd.pr);
      if (!p || p.state !== "open") return none(refuse(422, "NotOpen", `pull request #${cmd.pr} is not open`));
      if (!mv.approvedOn(cmd.pr, p.head)) return none(refuse(405, "Review", "refused: an approving review is required on the head"));
      if (!mv.checksGreen(p.head)) return none(refuse(405, "Checks", "refused: required checks have not passed"));
      if (p.agentApp !== null && !mv.gate(cmd.pr, p.head)) return none(refuse(405, "GateUnsatisfied", "refused: graftyard/agent-gate is not satisfied"));
      if (mv.ref(cmd.ref) !== cmd.expect) return none({ ok: false, status: 409, rule: "Stale", text: "stale" });
      return { events: [{ t: "Ref", ref: cmd.ref, expect: cmd.expect, value: cmd.value }, { t: "Merged", n: cmd.pr, commit: cmd.value },
                        { t: "Meta", kind: "timeline", body: `merged #${cmd.pr}` }], out: { ok: true, status: 200 } };
    }
    case "Push": {
      const cls = classify(cmd.who, view.authz);
      const ng = (rule: string, text: string) => none(refuse(cmd.surface === "git" ? 200 : 422, rule, cmd.surface === "git" ? `ng ${cmd.ref} ${text}` : text));
      if (!hasPush(cmd.who, cls, view)) return ng("NoPush", "refused: permission to push denied");
      if (cls.kind === "Agent") {
        const app = view.authz.agentApps[cls.holder.app!];
        const task = cls.holder.session ? view.task(cls.holder.session) : null;
        if (app?.kind === "GraftyardRun") {
          if (!taskLive(task, view.authz)) return ng("NoLiveTask", "refused: no live task for this token");
          if (!task!.prefix || !cmd.ref.startsWith(task!.prefix)) return ng("OutsidePrefix", `refused: may push only ${task!.prefix ?? "(none)"}**`);
        } else if (cls.confine && !globMatch(cls.confine, cmd.ref)) return ng("OutsidePrefix", `refused: may push only ${cls.confine}`);
        if (view.policy() === "AgentsExclusive") {
          const c = exclusiveConflicts(cmd.changed, cls.holder, view, at);
          if (c.length) {
            const l = c[0];
            return ng("Leased", `refused: ${l.paths[0]} is leased Exclusive by ${actorKey(l.holder.actor)} (task ${l.holder.session}) until ${new Date(l.expires).toISOString()}`);
          }
        }
      }
      // A ref already at the pushed value is up to date, as git reports a re-push of the same commit.
      if (mv.ref(cmd.ref) === cmd.value) return none({ ok: true, status: 200, text: "up to date" });
      if (mv.ref(cmd.ref) !== cmd.expect) return none({ ok: false, status: 409, rule: "Stale", text: "stale" });
      return { events: [{ t: "Ref", ref: cmd.ref, expect: cmd.expect, value: cmd.value, changed: cmd.changed }], out: { ok: true, status: 200 } };
    }
  }
}

/** Minting a task token (`fresh` read on the leader): the task's core row and the `Authz` rows, never metadata. */
export function mint(session: string, view: CoreView): XOut {
  const t = view.task(session);
  if (!taskLive(t, view.authz)) return { ok: false, status: 422, rule: "NoLiveTask", text: "refused: no live task" };
  return { ok: true, status: 201, claims: { task: t!.session, prefix: t!.prefix, installation: t!.installation, exp: 3_600 } };
}

/** design/DESIGN.md, `place_learners`, with the agent layer's feed rule (design/AGENT-LAYER.md, "Activity feed"): add
 *  learners until the peak held count fits in half of FEED_HELD_MAX each, up to MAX_LEARNERS. `peakHeld` is what each
 *  serving member reported. With `refusedHeld` (outside the design, the M0-WP12 variant) the demand also counts polls
 *  that every tried member refused while full, at their mean hold time, because a full member's peak is FEED_HELD_MAX
 *  however many polls arrive. */
export function placeLearners(load: { peakHeld: number[]; learners: number; refusedHeld?: number }): number {
  const demand = load.peakHeld.reduce((a, b) => a + b, 0) + (load.refusedHeld ?? 0);
  return Math.min(MAX_LEARNERS, Math.max(load.learners, Math.ceil(demand / (FEED_HELD_MAX / 2))));
}
