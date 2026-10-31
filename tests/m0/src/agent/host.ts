// M0-WP10's host: the git smart-HTTP, REST, and GraphQL surfaces of one install, every write routed through the one
// `decide` (decide.ts). Git facts come from `gitd` (tests/m0/agent/container/gitd), real git, reached through
// `GitBackend`: the container's TCP port in the Worker (src/agent/worker.ts), or a local process in the harness
// (run/agent.ts local). The host holds the folds in memory; one lock serializes decide, the ref transaction, and apply,
// which is the group's "one entry in flight" with one command per entry.

import type { Cmd, Credential, Decision, Event, Fold, Principal, Provenance, RefOutcome, RefUpdate, Refusal, RefusalRule } from "./model.ts";
import { HOST_REFS, ZERO } from "./model.ts";
import { classify, resolveSession } from "./classify.ts";
import { apply, decide, isLive, type Ctx } from "./decide.ts";
import { validateFrom, type Inspect, type Validated } from "./validate.ts";
import { advertise, concat, lsRefs, parseCommandRequest, parseReceive, readBody, reportStatus, type RefRow } from "./smart.ts";
import { derive, hmac, mint, tokenOf, verify } from "./token.ts";
import { authz as fixtureAuthz, baseFold, forkFold, orgPolicy, withRepos, BASE, FORK, OTHER, MCP_CLIENT, MCP_GRANTS } from "./fixture.ts";
import type { AgentTask, Authz, Issue, IssueComment, OrgPolicy, Perm } from "./model.ts";
import { FINAL_STATES, TASK_IDLE, actorKey } from "./model.ts";
import { decideMint, issueAt, snake } from "./tasks.ts";
import { AGENT_MEDIA_TYPE, authorTrust, trustView, type TrustView } from "./trust.ts";

export interface GitBackend { fetch(path: string, init?: RequestInit): Promise<Response> }
/** M0-WP13: outbound webhook sends (the event pipeline's delivery step), and `waitUntil` for them. */
export interface Outbound { send(url: string, init: RequestInit): Promise<Response>; later(p: Promise<unknown>): void }
export interface Delivery { id: string; app: number; url: string; event: string; action: string; session: string | null; at: number;
                            status?: number; sentAt?: number; doneAt?: number; error?: string }
/** Durable state for the Worker: the object's SQLite. The local harness passes none and keeps everything in memory. */
export interface Persist {
  reset(epoch: number): void;
  saveFold(repo: string, fold: Fold): void;            // everything but `refs`
  saveRefs(repo: string, updates: RefUpdate[]): void;
  saveAudit(row: AuditRow): void;
  load(): { epoch: number; folds: Record<string, Fold>; kv?: Record<string, string> } | null;
  /** M0-WP13: small host rows that are not repository folds (OrgPolicy, MCP `Tokens` rows, delivery log). */
  saveKv?(k: string, v: string): void;
  audit(probe: string): AuditRow | null;
}

export interface AuditRow { probe: string; surface: string; op: string; actor: string; kind: string; admit: string; ok: boolean;
                            rule?: RefusalRule; text?: string; refs?: { name: string; ok: boolean; rule?: string }[];
                            sources?: { commit: string; t: string; key?: string; session?: string }[];
                            parseErrors?: number; parseFailed?: boolean; unpack?: string; status?: number }

const DOC = "https://docs.github.com/enterprise-server@3.22/rest";
const json = (status: number, body: unknown, headers: Record<string, string> = {}): Response =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json; charset=utf-8", ...headers } });

/** REST status for a refusal: 403 for the action refusals the design lists, 405 for an unmerged gate (GitHub's
 *  "not mergeable"), 422 for ref writes refused like a push (GitHub's rule-violation shape), 404 for missing. */
export function restStatus(r: RefusalRule): number {
  switch (r) {
    case "NotFound": return 404;
    case "GateUnsatisfied": case "DraftOnly": return 405;
    case "Scope": return 404;
    case "AgentsDisabled": case "Invalid": case "TaskCap": return 422;
    case "TaskFinal": case "TaskPaused": return 409;
    case "OutsidePrefix": case "NoLiveTask": case "Protected": case "Leased": case "HostOwned": case "Stale": case "Missing":
    case "Atomic": case "NotAllowed": return 422;
    default: return 403;
  }
}
export function gqlType(r: RefusalRule): string {
  switch (r) {
    case "NotFound": return "NOT_FOUND";
    case "OutsidePrefix": case "NoLiveTask": case "Protected": case "Leased": case "HostOwned": case "Stale": case "Missing":
    case "Atomic": case "NotAllowed": case "GateUnsatisfied": case "DraftOnly": case "Invalid": case "TaskCap": case "AgentsDisabled": return "UNPROCESSABLE";
    case "Scope": return "NOT_FOUND";
    default: return "FORBIDDEN";
  }
}

export class Host {
  folds: Record<string, Fold> = {};
  authz: Authz;
  epoch = 0;
  audit = new Map<string, AuditRow>();
  counts: Record<string, number> = {};
  private chain: Promise<unknown> = Promise.resolve();
  private seq = 0;
  /** Server-Timing of the last receive-pack, for diagnosis. */
  timing = "";

  private secret: string;
  private git: GitBackend;
  private persist: Persist | null;
  /** M0-WP13 host rows. */
  org: OrgPolicy = orgPolicy();
  deliveries: Delivery[] = [];
  /** Feed requests that reached this object, the group leader (L0-15: must stay zero). */
  feedAtLeader = 0;
  out: Outbound | null = null;
  constructor(secret: string, git: GitBackend, host: string, persist: Persist | null = null, out: Outbound | null = null) {
    this.secret = secret; this.git = git; this.persist = persist; this.out = out; this.authz = withRepos(fixtureAuthz(host));
    this.authz.grants = { ...MCP_GRANTS };
    const saved = persist?.load();
    if (saved) {
      this.epoch = saved.epoch; this.folds = saved.folds;
      const kv = saved.kv ?? {};
      if (kv.org) this.org = JSON.parse(kv.org);
      if (kv.tokens) this.authz.tokens = JSON.parse(kv.tokens);
      if (kv.deliveries) this.deliveries = JSON.parse(kv.deliveries);
      if (kv.feedAtLeader) this.feedAtLeader = Number(kv.feedAtLeader);
    }
  }
  private saveKv(k: string, v: unknown): void { this.persist?.saveKv?.(k, typeof v === "string" ? v : JSON.stringify(v)); }
  private commitFold(repo: string, refs: RefUpdate[]): void {
    if (!this.persist) return;
    if (refs.length) this.persist.saveRefs(repo, refs);
    this.persist.saveFold(repo, this.folds[repo]);
  }

  private locked<T>(f: () => Promise<T>): Promise<T> {
    const p = this.chain.then(f, f);
    this.chain = p.catch(() => undefined);
    return p;
  }
  private qid(): string { return `q${this.epoch}-${++this.seq}-${Math.random().toString(36).slice(2, 8)}`; }
  private gitJson = async <T>(path: string, body: unknown): Promise<T> => {
    const r = await this.git.fetch(path, { method: "POST", body: JSON.stringify(body), headers: { "content-type": "application/json" } });
    if (!r.ok) throw new Error(`gitd ${path}: ${r.status} ${await r.text()}`);
    return r.json() as Promise<T>;
  };

  async setup(): Promise<object> {
    const at = Date.now();
    const other = forkFold(); other.repo = OTHER; other.forkOf = null; other.collaborators = { ...baseFold(at).collaborators };
    this.folds = { [BASE]: baseFold(at), [FORK]: forkFold(), [OTHER]: other };
    this.epoch = at;
    this.audit.clear();
    this.counts = {};
    this.authz = withRepos(fixtureAuthz(this.authz.host));
    this.authz.grants = { ...MCP_GRANTS };
    this.org = orgPolicy(); this.deliveries = []; this.feedAtLeader = 0;
    await this.gitJson("/init", { repo: BASE });
    await this.gitJson("/init", { repo: FORK, alternates: BASE });
    await this.gitJson("/init", { repo: OTHER });
    this.persist?.reset(at);
    this.commitFold(BASE, []);
    this.commitFold(FORK, []);
    this.commitFold(OTHER, []);
    this.saveKv("org", this.org);
    return { epoch: this.epoch };
  }

  /** The fork is created from the base: its main points at the base's main tip (objects through alternates). */
  async forkSync(): Promise<object> {
    const tip = this.folds[BASE].refs["refs/heads/main"];
    const cur = this.folds[FORK].refs["refs/heads/main"] ?? null;
    const u: RefUpdate = { name: "refs/heads/main", old: cur, new: tip };
    const r = await this.gitJson<{ ok: boolean; error?: string }>("/finalize", { repo: FORK, id: this.qid(), updates: [wire(u)] });
    if (r.ok) { this.folds[FORK].refs["refs/heads/main"] = tip; this.commitFold(FORK, [u]); }
    return { ...r, tip };
  }

  private record(row: AuditRow): void {
    if (row.probe) { this.audit.set(row.probe, row); this.persist?.saveAudit(row); }
    const k = `${row.surface}:${row.ok ? "ok" : row.rule}`;
    this.counts[k] = (this.counts[k] ?? 0) + 1;
    if (this.audit.size > (this.persist ? 2_000 : 60_000)) this.audit.delete(this.audit.keys().next().value!);
  }

  // ---- entry ----
  async handle(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const p = url.pathname;
    const m = /^\/([^/]+)\/([^/]+?)\.git\/(info\/refs|git-upload-pack|git-receive-pack)$/.exec(p);
    if (m) return this.gitHttp(req, `${m[1]}/${m[2]}`, m[3], url);
    if (p === "/api/graphql") return this.graphql(req);
    const top = await this.restTop(req, url);
    if (top) return top;
    const sp = /^\/([^/]+)\/([^/]+)\/_graftyard\/sessions\/([^/]+)$/.exec(p);
    if (sp) return this.sessionPage(req, `${sp[1]}/${sp[2]}`, sp[3]);
    if (p.startsWith("/api/v3/") || p.startsWith("/api/graftyard/v1/")) return this.rest(req, url);
    return json(404, { message: "Not Found" });
  }

  private async principal(req: Request, repo: string, provenance: Provenance[] = []): Promise<{ p: Principal; cred: Credential; ctx: Ctx } | null> {
    const tok = tokenOf(req);
    const cred = tok ? await verify(this.secret, tok) : null;
    if (!cred || cred.t === "AppJwt") return null;
    if (cred.t === "Installation" && cred.exp != null && cred.exp < Date.now()) return null;
    const fold = this.folds[repo];
    const header = req.headers.get("x-graftyard-agent-session");
    const hasTask = cred.t === "Installation" && cred.task != null;
    const base = fold.forkOf && hasTask ? this.folds[fold.forkOf] : null;
    const app = cred.t === "Pat" ? null : cred.app;
    const at = Date.now();
    const p = classify(cred, header ? resolveSession(header, app) : null, this.authz, fold, base, provenance, at);
    return { p, cred, ctx: { fold, deps: { authz: this.authz, base, org: this.org }, actor: p, at } };
  }

  private canRead(fold: Fold, p: Principal): boolean {
    if (p.repos && !p.repos.includes(fold.repo)) return false;
    if (p.id.t === "App") {
      const inst = this.authz.installations[p.id.installation];
      return inst?.active !== false && (!inst?.repos || inst.repos.includes(fold.repo));
    }
    return fold.forkOf ? true : fold.collaborators[`u${p.id.user}`] != null;
  }
  /** Read permission on a resource class: the token's own permissions (a fine-grained PAT or a narrowed installation
   *  token) bound it, beside the role. `metadata` is implied by any access. */
  private canReadPerm(fold: Fold, p: Principal, perm: Perm): boolean {
    if (!this.canRead(fold, p)) return false;
    const own = p.perms ?? (p.id.t === "App" ? this.authz.installations[p.id.installation]?.perms : null);
    return perm === "metadata" || !own || own[perm] != null;
  }
  /** The permission ceiling: an installation token cannot exceed its App's grant, and a user cannot exceed their role
   *  (an MCP user-to-server token is also capped by the MCP App's grant). In fork mode only `graftyard-workspace` is
   *  installed with write on the workspace fork. */
  private ceiling(fold: Fold, p: Principal, perm: Perm): boolean {
    if (p.id.t === "App") {
      const inst = this.authz.installations[p.id.installation];
      if (!inst?.active) return false;
      if (inst.repos && !inst.repos.includes(fold.repo)) return false;
      if (p.repos && !p.repos.includes(fold.repo)) return false;
      if (fold.forkOf && p.app !== this.authz.workspaceApp) return false;
      if (p.perms && p.perms[perm] !== "write") return false;
      return inst.perms[perm] === "write";
    }
    if (fold.forkOf) return false;
    if (p.repos && !p.repos.includes(fold.repo)) return false;
    if (p.perms && p.perms[perm] !== "write") return false;
    const role = fold.collaborators[`u${p.id.user}`];
    if (perm === "administration" ? role !== "admin" : role !== "write" && role !== "admin") return false;
    if (p.app != null) return Object.values(this.authz.installations).find((i) => i.app === p.app)?.perms[perm] === "write";
    return true;
  }
  private canWrite(fold: Fold, p: Principal): boolean { return this.ceiling(fold, p, "contents"); }
  private ceilingRefusal(req: Request, surface: string, op: string, p: Principal): void {
    this.record({ probe: req.headers.get("x-m0-probe") ?? "", surface, op, actor: p.name, kind: p.kind.t, admit: p.admit, ok: false,
                  rule: "Ceiling", text: "Resource not accessible by integration" });
  }

  // ---- git smart HTTP ----
  private async gitHttp(req: Request, repo: string, op: string, url: URL): Promise<Response> {
    const fold = this.folds[repo];
    if (!fold) return new Response("Repository not found.\n", { status: 404 });
    const who = await this.principal(req, repo);
    if (!who) return new Response("Authentication required\n", { status: 401, headers: { "www-authenticate": 'Basic realm="GitHub"' } });
    const proto = /version=(\d)/.exec(req.headers.get("git-protocol") ?? "")?.[1];
    const v = (proto === "2" ? 2 : proto === "1" ? 1 : 0) as 0 | 1 | 2;
    if (!this.canRead(fold, who.p)) { this.ceilingRefusal(req, "git", "Scope", who.p); return new Response("Repository not found.\n", { status: 404 }); }
    if (op === "info/refs") {
      const service = url.searchParams.get("service");
      if (service !== "git-upload-pack" && service !== "git-receive-pack") return new Response("dumb HTTP is not supported\n", { status: 403 });
      if (service === "git-receive-pack" && !this.canWrite(fold, who.p)) {
        this.ceilingRefusal(req, "git", "push", who.p);
        return new Response(`Permission to ${repo}.git denied to ${who.p.name}.\n`, { status: 403 });
      }
      const body = advertise(service, service === "git-receive-pack" ? (v === 2 ? 0 : v) : v, this.refRows(fold), fold.refs[fold.defaultBranch] ? fold.defaultBranch : null);
      return new Response(body, { headers: { "content-type": `application/x-${service}-advertisement`, "cache-control": "no-cache" } });
    }
    if (op === "git-upload-pack") {
      const body = await readBody(req);
      if (v === 2) {
        const c = parseCommandRequest(body);
        if (c.command === "ls-refs") {
          return new Response(lsRefs(c.args, this.refRows(fold), fold.refs[fold.defaultBranch] ? fold.defaultBranch : null),
            { headers: { "content-type": "application/x-git-upload-pack-result" } });
        }
        // `fetch`: git hides refs/graftyard/ from want-ref; v2 serves a want by object id, as GitHub does.
        return this.git.fetch(`/upload-pack?repo=${enc(repo)}&proto=2&hide=1`, { method: "POST", body });
      }
      return this.git.fetch(`/upload-pack?repo=${enc(repo)}&proto=${v}&hide=1`, { method: "POST", body });
    }
    // receive-pack
    if (!this.canWrite(fold, who.p)) {
      this.ceilingRefusal(req, "git", "push", who.p);
      return new Response(`Permission to ${repo}.git denied to ${who.p.name}.\n`, { status: 403 });
    }
    const body = await readBody(req);
    const rr = parseReceive(body);
    // git's RPC probe for bodies over http.postBuffer is a bare flush: no commands, and receive-pack answers nothing.
    if (!rr.updates.length) return gitResult(new Uint8Array(0));
    const sideband = rr.caps.has("side-band-64k");
    const probe = req.headers.get("x-m0-probe") ?? "";
    const id = this.qid();
    let unpack = "ok";
    const t0 = Date.now();
    if (rr.pack.length) {
      const r = await this.git.fetch(`/pack?repo=${enc(repo)}&id=${id}`, { method: "POST", body: rr.pack });
      const j = await r.json() as { ok: boolean; error?: string };
      if (!j.ok) unpack = (j.error ?? "index-pack failed").split("\n").pop()!.slice(0, 200);
    }
    if (unpack !== "ok") {
      await this.gitJson("/discard", { id });
      this.record({ probe, surface: "git", op: "push", actor: who.p.name, kind: who.p.kind.t, admit: who.p.admit, ok: false, unpack });
      return gitResult(reportStatus(unpack, rr.updates.map((u) => ({ name: u.name, ok: false, text: "unpacker error" })), sideband, []));
    }
    const tPack = Date.now() - t0;
    const res = await this.push(repo, req, rr.updates, id, "git", probe, !rr.caps.has("atomic") ? false : true);
    this.timing = `pack;dur=${tPack}, ${this.timing}`;
    const remote = res.refs.filter((r) => !r.ok).map((r) => `error: ${(r as Extract<RefOutcome, { ok: false }>).refusal.text}`);
    const timing = this.timing;
    return gitResult(reportStatus("ok", res.refs.map((r) => r.ok ? { name: r.name, ok: true } : { name: r.name, ok: false, text: r.refusal.text }),
      sideband, remote), timing);
  }

  private refRows(fold: Fold): RefRow[] { return Object.entries(fold.refs).map(([name, oid]) => ({ name, oid })); }

  /** The write path for every ref write: git receive-pack, REST ref writes, and GraphQL ref mutations. The staged
   *  pack is quarantine `id` (empty for REST and GraphQL ref writes, which run `validate` on an empty staged pack). */
  async push(repo: string, req: Request, updates: RefUpdate[], id: string, surface: string, probe: string, atomic: boolean):
      Promise<{ refs: RefOutcome[]; decision: Decision; principal: Principal }> {
    const fold = this.folds[repo];
    const who0 = (await this.principal(req, repo))!;
    const policyFold = who0.ctx.deps.base ?? fold;
    const defaultTip = policyFold.refs[policyFold.defaultBranch] ?? "";
    let ins: Inspect;
    const t0 = Date.now();
    // Open PRs with AgentMeta into each updated ref: git reports which heads the new tip contains (a merge by ref write).
    const heads: Record<string, string[]> = {};
    for (const u of updates) heads[u.name] = Object.values(fold.prs).filter((x) => x.open && x.meta && x.base === u.name).map((x) => x.headOid);
    try { ins = await this.gitJson<Inspect>("/inspect", { repo, id, updates: updates.map(wire), defaultTip, heads }); }
    catch { ins = { facts: [], commits: [] }; }
    const tIns = Date.now() - t0;
    const val: Validated = validateFrom(ins, updates, who0.p, policyFold, this.authz);
    // Admission class depends on the trailers (App tokens only), so classify again with the provenance.
    const who = (await this.principal(req, repo, val.provenance))!;
    const cmd: Cmd = { t: "Push", via: surface as Cmd extends { via: infer V } ? V : never, updates, atomic, changed: val.changed,
                       protected: val.protected, provenance: val.provenance, facts: val.facts };
    const tQ = Date.now();
    return this.locked(async () => {
      const tL = Date.now();
      const ctx: Ctx = { ...who.ctx, fold: this.folds[repo], at: Date.now() };
      const d = decide(cmd, ctx);
      let refs = d.refs ?? [];
      let tFin = 0, tPersist = 0;
      if (d.ok) {
        const refsEv = d.events.find((e) => e.t === "Refs") as Extract<Event, { t: "Refs" }>;
        const tf = Date.now();
        const fin = await this.gitJson<{ ok: boolean; error?: string }>("/finalize", { repo, id, updates: refsEv.updates.map(wire) });
        tFin = Date.now() - tf;
        if (!fin.ok) {
          refs = refs.map((r) => r.ok ? { name: r.name, ok: false, refusal: { rule: "Stale", text: "failed to update ref" } } : r);
          this.record({ probe, surface, op: "push", actor: who.p.name, kind: who.p.kind.t, admit: who.p.admit, ok: false, rule: "Stale", text: fin.error });
          return { refs, decision: { ok: false, refusal: { rule: "Stale", text: fin.error ?? "" } }, principal: who.p };
        }
        this.committed(repo, d.events, ctx.at, who.p);
        const tp = Date.now();
        this.commitFold(repo, refsEv.updates);
        tPersist = Date.now() - tp;
      } else {
        await this.gitJson("/discard", { id }).catch(() => undefined);
      }
      const prov = d.ok ? (d.events.find((e) => e.t === "Provenance") as Extract<Event, { t: "Provenance" }> | undefined)?.list ?? [] : val.provenance;
      this.record({ probe, surface, op: "push", actor: who.p.name, kind: who.p.kind.t, admit: who.p.admit, ok: refs.every((r) => r.ok) && d.ok,
        rule: d.ok ? (refs.find((r) => !r.ok) as Extract<RefOutcome, { ok: false }> | undefined)?.refusal.rule : d.refusal.rule,
        text: d.ok ? undefined : d.refusal.text,
        refs: refs.map((r) => ({ name: r.name, ok: r.ok, rule: r.ok ? undefined : r.refusal.rule })),
        sources: prov.flatMap((x) => x.sources.map((s) => ({ commit: x.commit, t: s.t, key: s.t === "Claimed" ? s.key : undefined,
                                                             session: s.t === "Session" ? s.session : undefined }))),
        parseErrors: val.parseErrors, parseFailed: val.parseFailed });
      this.timing = `inspect;dur=${tIns}, lockwait;dur=${tL - tQ}, finalize;dur=${tFin}, persist;dur=${tPersist}, locked;dur=${Date.now() - tL}`;
      return { refs, decision: d, principal: who.p };
    });
  }

  /** After `decide` accepts: apply, count the actor's write for DEMOTE_PER_MIN (in the fold, so a leader change keeps
   *  it), and hand each `Deliver` event to the event pipeline, which sends it only to the controller's webhook. */
  private committed(repo: string, events: Event[], at: number, p: Principal): void {
    const fold = this.folds[repo];
    apply(fold, events, at);
    const k = actorKey(p.id);
    const w = (fold.writes ??= {});
    w[k] = [...(w[k] ?? []).filter((t) => at - t < 60_000), at];
    for (const e of events) if (e.t === "Deliver") this.deliver(repo, e, at);
  }

  /** The first-party `graftyard_task` delivery: to the install's `graftyard-agent` webhook URL only, signed with its
   *  webhook secret. Every other App's subscription is ignored for this event, whatever it asks for. */
  private deliver(repo: string, e: Extract<Event, { t: "Deliver" }>, at: number): void {
    const app = this.authz.controllerApp!;
    const hook = this.authz.hooks?.[app];
    const task = this.folds[repo].tasks[e.session];
    const id = crypto.randomUUID();
    const d: Delivery = { id, app, url: hook?.url ?? "", event: e.event, action: e.action, session: e.session, at };
    this.deliveries.push(d);
    if (this.deliveries.length > 5_000) this.deliveries.splice(0, this.deliveries.length - 5_000);
    if (!hook || !this.out) return;
    const body = JSON.stringify({ action: e.action, task: taskJson(task, true), repository: { id: repo, full_name: repo },
                                  installation: { id: task.installation }, sender: { login: task.creator ?? "" } });
    const out = this.out;
    const send = (async () => {
      const sig = [...await hmac(await derive(this.secret, `webhook:${app}`), body)].map((b) => b.toString(16).padStart(2, "0")).join("");
      d.sentAt = Date.now();
      try {
        const r = await out.send(hook.url, { method: "POST", body, headers: { "content-type": "application/json", "x-github-event": e.event,
          "x-github-delivery": id, "x-hub-signature-256": `sha256=${sig}`, "user-agent": "GitHub-Hookshot/graftyard" } });
        d.status = r.status; await r.arrayBuffer();
      } catch (err) { d.error = String(err).slice(0, 200); }
      d.doneAt = Date.now();
      this.saveKv("deliveries", this.deliveries.slice(-2_000));
    })();
    out.later(send);
  }

  /** A non-push command: decide, apply, record. */
  private async command(repo: string, req: Request, cmd: Cmd, surface: string): Promise<{ d: Decision; p: Principal }> {
    const who = (await this.principal(req, repo))!;
    const probe = req.headers.get("x-m0-probe") ?? "";
    return this.locked(async () => {
      const ctx: Ctx = { ...who.ctx, fold: this.folds[repo], at: Date.now() };
      const d = decide(cmd, ctx);
      if (d.ok) { this.committed(repo, d.events, ctx.at, who.p); this.commitFold(repo, []); }
      this.record({ probe, surface, op: cmd.t, actor: who.p.name, kind: who.p.kind.t, admit: who.p.admit, ok: d.ok,
                    rule: d.ok ? undefined : d.refusal.rule, text: d.ok ? undefined : d.refusal.text });
      return { d, p: who.p };
    });
  }

  /** A REST or GraphQL ref write: one update, `validate` on an empty staged pack (or the quarantine of a server-made
   *  commit), then the push path. Returns the refusal, if any. */
  private async refWrite(repo: string, req: Request, u: RefUpdate, force: boolean, surface: string, id?: string): Promise<Refusal | null> {
    const fold = this.folds[repo];
    const probe = req.headers.get("x-m0-probe") ?? "";
    const qid = id ?? this.qid();
    if (u.new && u.old && !force) {
      const ins = await this.gitJson<Inspect>("/inspect", { repo, id: qid, updates: [wire(u)], defaultTip: "" });
      const f = ins.facts[0];
      if (f?.missing) return { rule: "Missing", text: "Object does not exist" };
      if (f && !f.ff) return { rule: "NotAllowed", text: "Update is not a fast forward" };
    }
    if (u.old == null && u.new && fold.refs[u.name]) return { rule: "Stale", text: "Reference already exists" };
    const r = await this.push(repo, req, [u], qid, surface, probe, true);
    if (r.decision.ok && r.refs.every((x) => x.ok)) return null;
    const bad = r.refs.find((x) => !x.ok) as Extract<RefOutcome, { ok: false }> | undefined;
    return bad?.refusal ?? (r.decision.ok ? null : r.decision.refusal);
  }

  // ---- REST ----
  private async rest(req: Request, url: URL): Promise<Response> {
    const p = url.pathname;
    const m = /^\/api\/(?:v3|graftyard\/v1)\/repos\/([^/]+)\/([^/]+)\/(.*)$/.exec(p);
    if (!m) return json(404, { message: "Not Found", documentation_url: DOC });
    const repo = `${m[1]}/${m[2]}`;
    const fold = this.folds[repo];
    if (!fold) return json(404, { message: "Not Found", documentation_url: DOC });
    const who = await this.principal(req, repo);
    if (!who) return json(401, { message: "Bad credentials", documentation_url: DOC });
    // A token whose repository list or installation does not cover the repository sees it as missing, as on GitHub.
    if (!this.canRead(fold, who.p)) { this.ceilingRefusal(req, "rest", "Scope", who.p); return json(404, { message: "Not Found", documentation_url: DOC }); }
    const rest = m[3];
    const method = req.method;
    const body = method === "GET" || method === "DELETE" ? {} : await req.json().catch(() => ({})) as Record<string, any>;
    const refuse = (r: Refusal) => json(restStatus(r.rule), r.rule === "NotFound" ? { message: "Not Found", documentation_url: DOC }
      : restStatus(r.rule) === 422 ? { message: r.text, documentation_url: `${DOC}/git/refs`, status: "422" }
      : { message: r.text, documentation_url: DOC });
    const deny = (op: string) => { this.ceilingRefusal(req, "rest", op, who.p); return json(403, { message: "Resource not accessible by integration", documentation_url: DOC }); };
    const run = async (cmd: Cmd, ok: (d: Extract<Decision, { ok: true }>) => Response) => {
      if (!this.ceiling(fold, who.p, permOf(cmd))) return deny(cmd.t);
      const { d } = await this.command(repo, req, cmd, "rest");
      return d.ok ? ok(d) : refuse(d.refusal);
    };
    let x: RegExpExecArray | null;
    if (method === "GET" && (x = /^git\/(?:ref|refs)\/(.+)$/.exec(rest))) {
      const name = `refs/${x[1]}`;
      return fold.refs[name] ? json(200, { ref: name, object: { sha: fold.refs[name], type: "commit" } }) : json(404, { message: "Not Found", documentation_url: DOC });
    }
    if (method === "GET" && (x = /^pulls\/(\d+)$/.exec(rest))) {
      const pr = fold.prs[x[1]];
      return pr ? json(200, { number: pr.number, draft: pr.draft, state: pr.open ? "open" : "closed", merged: pr.merged, head: { ref: pr.headRef.replace("refs/heads/", ""), sha: pr.headOid } })
        : json(404, { message: "Not Found", documentation_url: DOC });
    }
    if (method === "POST" && rest === "pulls") {
      return run({ t: "PrOpen", head: `refs/heads/${body.head}`, base: `refs/heads/${body.base ?? "main"}`, draft: !!body.draft, body: typeof body.body === "string" ? body.body : "" },
        (d) => { const e = d.events[0] as Extract<Event, { t: "PrOpened" }>; return json(201, { number: e.pr.number, draft: e.pr.draft, head: { sha: e.pr.headOid } }); });
    }
    if (method === "POST" && (x = /^pulls\/(\d+)\/reviews$/.exec(rest)))
      return run({ t: "Review", pr: Number(x[1]), event: body.event ?? "COMMENT" }, (d) => json(200, { state: body.event === "APPROVE" ? "APPROVED" : "COMMENTED",
        // Opt-in only: no GHES 3.22 response changes for a client that does not send the agent media type (M0-WP15 L0-11).
        ...((req.headers.get("accept") ?? "").includes(AGENT_MEDIA_TYPE) ? { graftyard_gate: d.events.some((e) => e.t === "AgentGate") } : {}) }));
    if (method === "PUT" && (x = /^pulls\/(\d+)\/merge$/.exec(rest)))
      return run({ t: "Merge", pr: Number(x[1]), sha: body.sha ?? null, mergeOid: null }, () => json(200, { merged: true, message: "Pull Request successfully merged" }));
    if (method === "PUT" && (x = /^rulesets\/(\d+)$/.exec(rest))) return run({ t: "RulesetEdit", id: Number(x[1]) }, () => json(200, { id: Number(x![1]) }));
    if (method === "PUT" && (x = /^branches\/(.+)\/protection$/.exec(rest))) return run({ t: "BranchProtectionEdit", branch: x[1] }, () => json(200, {}));
    if (method === "POST" && rest === "dispatches") return run({ t: "RepositoryDispatch" }, () => new Response(null, { status: 204 }));
    if (method === "POST" && (x = /^actions\/workflows\/([^/]+)\/dispatches$/.exec(rest)))
      return run({ t: "WorkflowDispatch", workflow: x[1], ref: body.ref ?? "main" }, () => new Response(null, { status: 204 }));
    if (method === "POST" && (x = /^actions\/(?:runs|jobs)\/(\d+)\/(?:rerun|rerun-failed-jobs)$/.exec(rest)))
      return run({ t: "WorkflowRerun", run: Number(x[1]) }, () => json(201, {}));
    if (method === "POST" && rest === "releases") return run({ t: "ReleaseCreate", tag: body.tag_name ?? "" }, () => json(201, { tag_name: body.tag_name }));
    if (method === "POST" && (x = /^issues\/(\d+)\/labels$/.exec(rest)))
      return run({ t: "LabelWrite", issue: Number(x[1]), labels: Array.isArray(body) ? body : body.labels ?? [] }, () => json(200, []));
    if (method === "POST" && (x = /^statuses\/([0-9a-f]{40})$/.exec(rest)))
      return run({ t: "StatusCreate", sha: x[1], context: body.context ?? "default" }, () => json(201, { context: body.context ?? "default", state: body.state }));
    if (method === "POST" && rest === "check-runs")
      return run({ t: "CheckRunCreate", sha: body.head_sha ?? ZERO, name: body.name ?? "",
                  output: body.output ? { title: String(body.output.title ?? ""), summary: String(body.output.summary ?? ""), text: String(body.output.text ?? "") } : undefined },
        (d) => { const e = d.events.find((x) => x.t === "CheckRunStored") as Extract<Event, { t: "CheckRunStored" }> | undefined;
                 return json(201, { id: e?.run.id ?? null, name: body.name, head_sha: body.head_sha }); });
    if (method === "POST" && (x = /^branches\/(.+)\/rename$/.exec(rest)))
      return run({ t: "BranchRename", from: x[1], to: body.new_name ?? "" }, () => json(201, {}));
    if (method === "POST" && rest === "hooks") return run({ t: "HookEdit" }, () => json(201, {}));
    if (method === "POST" && rest === "keys") return run({ t: "DeployKeyEdit" }, () => json(201, {}));
    // Ref writes: POST and PATCH /git/refs, DELETE /git/refs.
    if (rest === "git/refs" && method === "POST") {
      if (!this.canWrite(fold, who.p)) return deny("Push");
      const r = await this.refWrite(repo, req, { name: body.ref, old: null, new: body.sha }, false, "rest");
      return r ? refuse(r) : json(201, { ref: body.ref, object: { sha: body.sha, type: "commit" } });
    }
    if ((x = /^git\/refs\/(.+)$/.exec(rest)) && (method === "PATCH" || method === "DELETE")) {
      if (!this.canWrite(fold, who.p)) return deny("Push");
      const name = `refs/${x[1]}`;
      if (!fold.refs[name]) return json(422, { message: "Reference does not exist", documentation_url: `${DOC}/git/refs`, status: "422" });
      const u: RefUpdate = { name, old: fold.refs[name], new: method === "DELETE" ? null : body.sha };
      const r = await this.refWrite(repo, req, u, !!body.force || method === "DELETE", "rest");
      return r ? refuse(r) : method === "DELETE" ? new Response(null, { status: 204 }) : json(200, { ref: name, object: { sha: body.sha, type: "commit" } });
    }
    const w13 = await this.rest13(req, url, repo, fold, who.p, rest, method, body, refuse, deny);
    if (w13) return w13;
    return json(404, { message: "Not Found", documentation_url: DOC });
  }

  // ---- GraphQL ----
  private async graphql(req: Request): Promise<Response> {
    const body = await req.json().catch(() => null) as { query?: string; variables?: Record<string, any> } | null;
    const q = body?.query ?? "";
    const mm = /mutation\b[^{]*\{\s*(\w+)\s*\(/.exec(q);
    if (!mm) return json(200, { errors: [{ type: "PARSE", message: "Only single mutations are served by this prototype" }] });
    const name = mm[1];
    const input = (body?.variables?.input ?? {}) as Record<string, any>;
    const repo = gqlRepo(input) ?? BASE;
    const fold = this.folds[repo];
    if (!fold) return json(200, { data: { [name]: null }, errors: [{ type: "NOT_FOUND", path: [name], message: "Could not resolve to a Repository" }] });
    const who = await this.principal(req, repo);
    if (!who) return json(401, { message: "Bad credentials", documentation_url: "https://docs.github.com/enterprise-server@3.22/graphql" });
    const err = (r: Refusal) => json(200, { data: { [name]: null }, errors: [{ type: gqlType(r.rule), path: [name],
      locations: [{ line: 1, column: q.indexOf(name) + 1 }], message: r.text }] });
    const okData = (v: unknown) => json(200, { data: { [name]: v } });
    if (!this.canRead(fold, who.p)) { this.ceilingRefusal(req, "graphql", "Scope", who.p);
      return json(200, { data: { [name]: null }, errors: [{ type: "NOT_FOUND", path: [name], message: `Could not resolve to a Repository with the name '${repo}'.` }] }); }
    const deny = (op: string) => { this.ceilingRefusal(req, "graphql", op, who.p); return err({ rule: "Ceiling", text: "Resource not accessible by integration" }); };
    if (["createRef", "updateRef", "deleteRef", "updateRefs", "createCommitOnBranch"].includes(name) && !this.canWrite(fold, who.p)) return deny("Push");
    const prNum = (id: unknown) => Number(String(id ?? "").replace(/^PR_/, ""));
    const run = async (cmd: Cmd, v: unknown) => {
      if (!this.ceiling(fold, who.p, permOf(cmd))) return deny(cmd.t);
      const { d } = await this.command(repo, req, cmd, "graphql"); return d.ok ? okData(v) : err(d.refusal);
    };
    switch (name) {
      case "addPullRequestReview": case "submitPullRequestReview":
        return run({ t: "Review", pr: prNum(input.pullRequestId), event: input.event ?? "COMMENT" }, { pullRequestReview: { state: input.event } });
      case "mergePullRequest": case "enqueuePullRequest":
        return run({ t: "Merge", pr: prNum(input.pullRequestId), sha: input.expectedHeadOid ?? null, mergeOid: null }, { pullRequest: { merged: true } });
      case "markPullRequestReadyForReview": return run({ t: "ReadyForReview", pr: prNum(input.pullRequestId) }, { pullRequest: { isDraft: false } });
      case "updateRepositoryRuleset": case "createRepositoryRuleset":
        return run({ t: "RulesetEdit", id: Number(String(input.repositoryRulesetId ?? "0").replace(/\D/g, "")) }, { ruleset: {} });
      case "updateBranchProtectionRule": case "createBranchProtectionRule": return run({ t: "BranchProtectionEdit", branch: "" }, { branchProtectionRule: {} });
      case "rerequestCheckSuite": return run({ t: "WorkflowRerun", run: Number(String(input.checkSuiteId ?? "0").replace(/\D/g, "")) }, { checkSuite: {} });
      case "addLabelsToLabelable":
        return run({ t: "LabelWrite", issue: prNum(input.labelableId), labels: (input.labelIds ?? []).map((l: string) => String(l).replace(/^LA_/, "")) }, { labelable: {} });
      case "createCheckRun": return run({ t: "CheckRunCreate", sha: input.headSha ?? ZERO, name: input.name ?? "" }, { checkRun: { name: input.name } });
      case "createRef": {
        const r = await this.refWrite(repo, req, { name: input.name, old: null, new: input.oid }, false, "graphql");
        return r ? err(r) : okData({ ref: { name: input.name } });
      }
      case "updateRef": case "deleteRef": {
        const nm = refFromId(input.refId);
        if (!fold.refs[nm]) return err({ rule: "NotFound", text: "Could not resolve to a Ref" });
        const r = await this.refWrite(repo, req, { name: nm, old: fold.refs[nm], new: name === "deleteRef" ? null : input.oid }, !!input.force || name === "deleteRef", "graphql");
        return r ? err(r) : okData({ ref: { name: nm } });
      }
      case "updateRefs": {
        for (const u of input.refUpdates ?? []) {
          const nm = String(u.name);
          const r = await this.refWrite(repo, req, { name: nm, old: fold.refs[nm] ?? null, new: u.afterOid === ZERO ? null : u.afterOid }, !!u.force, "graphql");
          if (r) return err(r);
        }
        return okData({ clientMutationId: null });
      }
      case "createCommitOnBranch": {
        const nm = `refs/heads/${input.branch?.branchName ?? ""}`;
        const head = fold.refs[nm];
        if (!head) return err({ rule: "NotFound", text: `Could not resolve to a Ref "${nm}"` });
        if (input.expectedHeadOid !== head) return err({ rule: "Stale", text: `Expected branch to point to "${input.expectedHeadOid}" but it did not` });
        const add: Record<string, string> = {};
        for (const a of input.fileChanges?.additions ?? []) add[a.path] = a.contents;
        const id = this.qid();
        const msg = [input.message?.headline ?? "commit", input.message?.body].filter(Boolean).join("\n\n") + "\n";
        const c = await this.gitJson<{ ok: boolean; oid?: string; error?: string }>("/commit", { repo, id, parent: head, add,
          del: (input.fileChanges?.deletions ?? []).map((d: { path: string }) => d.path), message: msg, author: who.p.name });
        if (!c.ok) return err({ rule: "NotAllowed", text: c.error ?? "commit failed" });
        const r = await this.refWrite(repo, req, { name: nm, old: head, new: c.oid! }, false, "graphql", id);
        return r ? err(r) : okData({ commit: { oid: c.oid } });
      }
      default:
        return json(200, { errors: [{ type: "UNDEFINED", message: `Field '${name}' doesn't exist on type 'Mutation'` }] });
    }
  }

  // ---- M0-WP13: top-level REST (minting, scoped tokens, org policy, MCP grants) ----
  private async restTop(req: Request, url: URL): Promise<Response | null> {
    const p = url.pathname;
    const method = req.method;
    let x: RegExpExecArray | null;
    // POST /app/installations/{id}/access_tokens, with the additive `graftyard_task` field. An App JWT only.
    if (method === "POST" && (x = /^\/api\/v3\/app\/installations\/(\d+)\/access_tokens$/.exec(p))) {
      const tok = tokenOf(req);
      const cred = tok ? await verify(this.secret, tok) : null;
      if (!cred || cred.t !== "AppJwt") return json(401, { message: "A JSON web token could not be decoded", documentation_url: DOC });
      const b = await req.json().catch(() => ({})) as Record<string, any>;
      const repos = Array.isArray(b.repositories) ? b.repositories.map((r: string) => r.includes("/") ? r : `acme/${r}`) : null;
      const r = decideMint(cred, { installation: Number(x[1]), repositories: repos, permissions: b.permissions ?? null,
                                   graftyardTask: typeof b.graftyard_task === "string" ? b.graftyard_task : null }, this.authz, this.folds, Date.now());
      const probe = req.headers.get("x-m0-probe") ?? "";
      this.record({ probe, surface: "rest", op: "Mint", actor: this.authz.apps[cred.app]?.slug ?? "", kind: "Bot", admit: "Bot", ok: r.ok,
                    rule: r.ok ? undefined : r.refusal.rule, text: r.ok ? undefined : r.refusal.text });
      if (!r.ok) return json(r.status, r.status === 422 ? { message: r.refusal.text, documentation_url: DOC, errors: [{ resource: "Installation", field: "graftyard_task", code: "invalid" }] }
        : { message: r.refusal.text, documentation_url: DOC });
      return json(201, { token: await mint(this.secret, r.cred), expires_at: new Date(r.cred.exp!).toISOString(), permissions: r.cred.perms,
                         repository_selection: "selected", repositories: r.cred.repos?.map((full_name) => ({ full_name })), graftyard_task: r.cred.task });
    }
    // POST /applications/{client_id}/token/scoped from Graftyard MCP, with `graftyard_grant` and `graftyard_write`.
    if (method === "POST" && (x = /^\/api\/v3\/applications\/([^/]+)\/token\/scoped$/.exec(p))) {
      const basic = /^Basic (.+)$/i.exec(req.headers.get("authorization") ?? "");
      const [cid, csec] = basic ? atob(basic[1]).split(":") : ["", ""];
      if (cid !== MCP_CLIENT.id || x[1] !== MCP_CLIENT.id || csec !== await derive(this.secret, MCP_CLIENT.secretLabel))
        return json(401, { message: "Bad credentials", documentation_url: DOC });
      const b = await req.json().catch(() => ({})) as Record<string, any>;
      const user = typeof b.access_token === "string" ? await verify(this.secret, b.access_token) : null;
      if (!user || user.t !== "UserToServer" || user.app !== this.authz.agentApps.find((a) => a.kind === "Mcp")?.app) return json(404, { message: "Not Found", documentation_url: DOC });
      const grant = this.authz.grants?.[b.graftyard_grant];
      const order = ["Read", "Write", "WriteApproveMerge"];
      const want = { read: "Read", write: "Write", write_approve_merge: "WriteApproveMerge" }[String(b.graftyard_write)] as WriteScopeT | undefined;
      if (!grant || grant.user !== user.user || !want || order.indexOf(want) > order.indexOf(grant.write))
        return json(422, { message: "graftyard_grant and graftyard_write must name a live grant of this user and a write scope within it",
                           documentation_url: DOC, errors: [{ resource: "Token", field: "graftyard_write", code: "invalid" }] });
      const jti = `m${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
      (this.authz.tokens ??= {})[jti] = { user: user.user, grant: b.graftyard_grant, write: want };
      this.saveKv("tokens", this.authz.tokens);
      const repos = Array.isArray(b.repositories) ? b.repositories.map((r: string) => r.includes("/") ? r : `acme/${r}`) : undefined;
      // The `write` claim is deliberately not set: the host reads WriteScope from the Tokens row only.
      const token = await mint(this.secret, { t: "UserToServer", user: user.user, app: user.app, write: null, jti, repos });
      return json(200, { token, scoped: true, graftyard_write: snake(want).toLowerCase() });
    }
    // GET and PATCH /api/graftyard/v1/orgs/{org}/agents (org admin; GET also with the Graftyard MCP client credentials).
    if ((x = /^\/api\/graftyard\/v1\/orgs\/([^/]+)\/agents$/.exec(p))) {
      if (x[1] !== this.org.org) return json(404, { message: "Not Found", documentation_url: DOC });
      const basic = /^Basic (.+)$/i.exec(req.headers.get("authorization") ?? "");
      const tok = tokenOf(req);
      const cred = !basic && tok ? await verify(this.secret, tok) : null;
      const admin = cred && cred.t === "Pat" && this.org.admins.includes(cred.user);
      const client = basic && atob(basic[1]) === `${MCP_CLIENT.id}:${await derive(this.secret, MCP_CLIENT.secretLabel)}`;
      const view = () => json(200, { agents_enabled: this.org.agentsEnabled, workspace: snake(this.org.workspace), workspace_org: `${this.org.org}-agents`, code_mode: this.org.codeMode });
      if (method === "GET") return admin || client ? view() : json(cred || basic ? 403 : 401, { message: cred || basic ? "Must be an organization admin" : "Requires authentication", documentation_url: DOC });
      if (method === "PATCH") {
        if (!admin) return json(cred ? 403 : 401, { message: "Must be an organization admin", documentation_url: DOC });
        const b = await req.json().catch(() => ({})) as Record<string, any>;
        if (typeof b.agents_enabled === "boolean") this.org.agentsEnabled = b.agents_enabled;
        if (typeof b.code_mode === "boolean") this.org.codeMode = b.code_mode;
        this.saveKv("org", this.org);
        return view();
      }
    }
    return null;
  }

  /** The leader-side count of feed requests (L0-15): the edge answers refused polls, so this must stay zero. */
  feedReachedLeader(): void { this.feedAtLeader++; this.saveKv("feedAtLeader", String(this.feedAtLeader)); }

  // ---- M0-WP13: repository REST (task API, issues, comments, commits, check runs, merges, PR reads) ----
  private async rest13(req: Request, url: URL, repo: string, fold: Fold, p: Principal, rest: string, method: string, body: Record<string, any>,
                       refuse: (r: Refusal) => Response, deny: (op: string) => Response): Promise<Response | null> {
    let x: RegExpExecArray | null;
    const agentMedia = (req.headers.get("accept") ?? "").includes(AGENT_MEDIA_TYPE);
    const vary = { vary: "Accept" };
    const notFound = () => json(404, { message: "Not Found", documentation_url: DOC });
    const forbidden = (m: string) => json(403, { message: m, documentation_url: DOC });
    const run = async (cmd: Cmd, perm: Perm, ok: (d: Extract<Decision, { ok: true }>) => Response) => {
      // `metadata`: any reader may open issues and comment (GHES); for Apps, `issues: write` as GHES requires.
      const allowed = perm === "metadata" ? (p.id.t === "User" ? this.canRead(fold, p) : this.ceiling(fold, p, "issues")) : this.ceiling(fold, p, perm);
      if (!allowed) return deny(cmd.t);
      const { d } = await this.command(repo, req, cmd, rest.startsWith("tasks") ? "task-api" : "rest");
      return d.ok ? ok(d) : taskRefusal(d.refusal, refuse);
    };
    const isApi13 = url.pathname.startsWith("/api/graftyard/v1/");
    // ---- Task API (/api/graftyard/v1 only) ----
    if (isApi13 && rest === "tasks" && method === "POST") {
      const ext = body.kind === "external";
      if (ext && !(this.canReadPerm(fold, p, "contents") && this.ceiling(fold, p, "issues"))) return deny("TaskCreate");
      if (!this.canRead(fold, p)) return notFound();
      const cmd: Cmd = { t: "TaskCreate", kind: ext ? "External" : "GraftyardRun", id: crypto.randomUUID(),
        issue: typeof body.issue_number === "number" ? body.issue_number : null, prompt: typeof body.prompt === "string" ? body.prompt : null,
        name: typeof body.session === "string" ? body.session : "prefix" in body && !ext ? null : null,
        requester: typeof body.requester === "string" ? body.requester : null, summary: typeof body.summary === "string" ? body.summary : "" };
      if (!ext && typeof body.session === "string") (cmd as Extract<Cmd, { t: "TaskCreate" }>).name = body.session;
      const { d } = await this.command(repo, req, cmd, "task-api");
      if (!d.ok) return taskRefusal(d.refusal, refuse);
      const t = (d.events.find((e) => e.t === "TaskCreated") as Extract<Event, { t: "TaskCreated" }>).task;
      return json(201, taskJson(this.folds[repo].tasks[t.session], true, this.folds[repo]));
    }
    if (isApi13 && rest === "tasks" && method === "GET") {
      if (!this.canReadPerm(fold, p, "metadata")) return notFound();
      const q = url.searchParams;
      const contents = this.canReadPerm(fold, p, "contents");
      const rows = Object.values(fold.tasks).filter((t) => (!q.get("state") || snake(this.taskStateNow(fold, t)) === q.get("state"))
        && (!q.get("app_id") || String(t.app) === q.get("app_id")) && (!q.get("issue_number") || String(t.issue) === q.get("issue_number")));
      return json(200, rows.slice(0, Math.min(100, Number(q.get("per_page") ?? 30))).map((t) => taskJson(t, contents, fold, this.taskStateNow(fold, t))));
    }
    if (isApi13 && (x = /^tasks\/([^/]+)(\/cancel|\/log)?$/.exec(rest))) {
      const sid = resolveSession(decodeURIComponent(x[1]), p.app);
      const t = fold.tasks[sid];
      if (!this.canReadPerm(fold, p, "metadata")) return notFound();
      if (!t) return notFound();
      if (x[2] === "/log" && method === "GET") {
        if (!this.canReadPerm(fold, p, "contents")) return forbidden("Resource not accessible: session logs need contents: read");
        this.record({ probe: req.headers.get("x-m0-probe") ?? "", surface: "task-api", op: "LogRead", actor: p.name, kind: p.kind.t, admit: p.admit, ok: true });
        return json(404, { message: "Not Found: the session log has not been pushed", documentation_url: DOC });
      }
      if (x[2] === "/cancel" && method === "POST") {
        const okCaller = (p.id.t === "User" && !p.app && (p.id.user === t.requester || fold.collaborators[`u${p.id.user}`] === "admin")) || p.app === t.app;
        if (!okCaller) return forbidden("Must be the requester, a repository admin, or the task's App");
        if (t.kind === "External" && !this.ceiling(fold, p, "issues")) return deny("TaskCancel");
        const { d } = await this.command(repo, req, { t: "TaskCancel", session: t.session }, "task-api");
        return d.ok ? json(200, taskJson(this.folds[repo].tasks[t.session], true, this.folds[repo])) : taskRefusal(d.refusal, refuse, t, this);
      }
      if (!x[2] && method === "GET") {
        const contents = this.canReadPerm(fold, p, "contents");
        if (url.searchParams.get("fields") === "prompt" && !contents) return forbidden("Resource not accessible: prompts need contents: read");
        const j = taskJson(t, contents, fold, this.taskStateNow(fold, t));
        return json(200, { ...j, leases: fold.leases.filter((l) => l.holder.session === t.session).map((l) => ({ id: l.id, mode: snake(l.mode), paths: l.scope.paths })) });
      }
      if (!x[2] && method === "PATCH") {
        const st = typeof body.state === "string" ? fromSnake(body.state) : null;
        if (body.state != null && !st) return json(422, { message: "Validation Failed", documentation_url: DOC, errors: [{ resource: "Task", field: "state", code: "invalid" }] });
        const { d } = await this.command(repo, req, { t: "TaskPatch", session: t.session, state: st, pr: typeof body.pr_number === "number" ? body.pr_number : null,
          summary: typeof body.summary === "string" ? body.summary : null }, "task-api");
        return d.ok ? json(200, taskJson(this.folds[repo].tasks[t.session], true, this.folds[repo])) : taskRefusal(d.refusal, refuse, t, this);
      }
    }
    // ---- The activity feed: the edge refuses before any member sees the poll. WP12 serves the rest. ----
    if (isApi13 && rest === "activity" && method === "GET") {
      // Unreachable for refused polls: the Worker's edge answers them (worker.ts, `feedEdge`). Count any that arrive.
      this.feedReachedLeader();
      return json(503, { message: "the activity feed is served by learners and followers (M0-WP12)" });
    }
    // ---- Issues ----
    if (rest === "issues" && method === "POST") {
      if (!this.canRead(fold, p)) return notFound();
      return run({ t: "IssueOpen", title: String(body.title ?? ""), body: String(body.body ?? "") }, "metadata", (d) => {
        const e = d.events[0] as Extract<Event, { t: "IssueOpened" }>;
        return json(201, this.issueJson(this.folds[repo], this.folds[repo].issues![String(e.issue.number)], agentMedia));
      });
    }
    if ((x = /^issues\/(\d+)$/.exec(rest))) {
      const iss = fold.issues?.[x[1]];
      if (!iss) return notFound();
      if (method === "GET") return this.canRead(fold, p) ? json(200, this.issueJson(fold, iss, agentMedia), vary) : notFound();
      if (method === "PATCH") return run({ t: "IssueEdit", n: iss.number, title: typeof body.title === "string" ? body.title : null, body: typeof body.body === "string" ? body.body : null },
        "metadata", () => json(200, this.issueJson(this.folds[repo], this.folds[repo].issues![x![1]], agentMedia), vary));
    }
    if (method === "POST" && (x = /^issues\/(\d+)\/assignees$/.exec(rest)))
      return run({ t: "Assign", n: Number(x[1]), assignees: Array.isArray(body.assignees) ? body.assignees.map(String) : [] }, "issues",
        () => json(201, this.issueJson(this.folds[repo], this.folds[repo].issues![x![1]], false)));
    if ((x = /^issues\/(\d+)\/comments$/.exec(rest))) {
      if (method === "POST") return run({ t: "CommentCreate", issue: Number(x[1]), body: String(body.body ?? "") }, "metadata", (d) => {
        const e = d.events[0] as Extract<Event, { t: "Commented" }>;
        return json(201, this.commentJson(this.folds[repo], e.comment, agentMedia));
      });
      if (method === "GET") {
        if (!this.canRead(fold, p)) return notFound();
        return json(200, (fold.comments ?? []).filter((c) => c.issue === Number(x![1])).map((c) => this.commentJson(fold, c, agentMedia)), vary);
      }
    }
    if (method === "GET" && (x = /^issues\/comments\/(\d+)$/.exec(rest))) {
      const c = (fold.comments ?? []).find((y) => y.id === Number(x![1]));
      return c && this.canRead(fold, p) ? json(200, this.commentJson(fold, c, agentMedia), vary) : notFound();
    }
    // ---- Pulls (single read with the trust view) ----
    if (method === "GET" && (x = /^pulls\/(\d+)$/.exec(rest))) {
      const pr = fold.prs[x[1]];
      if (!pr || !this.canRead(fold, p)) return notFound();
      const tv = agentMedia ? { graftyard_trust: this.tv(fold, pr.author, pr.body ?? "") } : {};
      return json(200, { number: pr.number, draft: pr.draft, state: pr.open ? "open" : "closed", merged: pr.merged, body: pr.body ?? "",
                         head: { ref: pr.headRef.replace("refs/heads/", ""), sha: pr.headOid }, base: { ref: pr.base.replace("refs/heads/", "") },
                         ...(agentMedia && pr.meta ? { graftyard_agent: { app_id: pr.meta.app, session: pr.meta.session, requester: pr.meta.requester,
                           gate: pr.gate ? { head_sha: pr.gate.head, satisfied: pr.gate.head === pr.headOid } : null } } : {}), ...tv }, vary);
    }
    // ---- Commits ----
    if (method === "GET" && (x = /^commits\/([0-9a-f]{40})$/.exec(rest))) {
      if (!this.canReadPerm(fold, p, "contents")) return notFound();
      const r = await this.git.fetch(`/cat?repo=${enc(repo)}&oid=${x[1]}`);
      if (!r.ok || r.headers.get("x-object-type") !== "commit") return notFound();
      const raw = new Uint8Array(await r.arrayBuffer());
      const message = commitMsg(raw);
      const tv = agentMedia ? { graftyard_trust: { visible: trustView(message, authorTrust({ role: null, owner: false, isBot: false, isAgent: false, firstContribution: false })).visible,
        hidden: trustView(message, authorTrust({ role: null, owner: false, isBot: false, isAgent: false, firstContribution: false })).hidden,
        author: authorTrust({ role: null, owner: false, isBot: false, isAgent: false, firstContribution: false }) } } : {};
      return json(200, { sha: x[1], commit: { message }, ...tv }, vary);
    }
    // ---- Check runs ----
    if (method === "GET" && (x = /^check-runs\/(\d+)$/.exec(rest))) {
      const c = (fold.checkRuns ?? []).find((y) => y.id === Number(x![1]));
      if (!c || !this.canRead(fold, p)) return notFound();
      const joined = [c.title, c.summary, c.text].join("\n\n");
      return json(200, { id: c.id, name: c.name, head_sha: c.sha, output: { title: c.title, summary: c.summary, text: c.text },
                         ...(agentMedia ? { graftyard_trust: this.tv(fold, c.author, joined) } : {}) }, vary);
    }
    // ---- POST /merges: a server-made merge commit, then the ref write path (validate on its staged commit) ----
    if (method === "POST" && rest === "merges") {
      if (!this.canWrite(fold, p)) return deny("Push");
      const name = `refs/heads/${String(body.base ?? "")}`;
      const baseTip = fold.refs[name];
      const head = /^[0-9a-f]{40}$/.test(String(body.head)) ? String(body.head) : fold.refs[`refs/heads/${String(body.head ?? "")}`];
      if (!baseTip || !head) return json(404, { message: "Base or head does not exist", documentation_url: DOC });
      const id = `q${this.epoch}-m${Math.random().toString(36).slice(2, 10)}`;
      const m = await this.gitJsonPub<{ ok: boolean; oid?: string; noop?: boolean; conflict?: boolean }>("/merge", { repo, id, base: baseTip, head, message: String(body.commit_message ?? `Merge ${body.head} into ${body.base}`) + "\n" });
      if (m.noop) return new Response(null, { status: 204 });
      if (!m.ok) return json(409, { message: "Merge conflict", documentation_url: DOC });
      const r = await this.refWritePub(repo, req, { name, old: baseTip, new: m.oid! }, false, "rest", id);
      return r ? refuse(r) : json(201, { sha: m.oid, commit: { message: body.commit_message ?? "" } });
    }
    return null;
  }

  private taskStateNow(fold: Fold, t: AgentTask): AgentTask["state"] { return taskStateOf(t, Date.now(), this.authz, fold); }
  private gitJsonPub<T>(path: string, b: unknown): Promise<T> { return this.gitJson<T>(path, b); }
  private refWritePub(repo: string, req: Request, u: RefUpdate, force: boolean, surface: string, id?: string) { return this.refWrite(repo, req, u, force, surface, id); }

  /** `AuthorTrust` for an actorKey on this repository. */
  private authorOf(fold: Fold, key: string, firstContribution = false) {
    const app = /^a(\d+)i/.exec(key);
    const isAgent = app ? this.authz.agentApps.some((a) => a.app === Number(app[1])) : false;
    const role = app ? null : fold.collaborators[key] ?? null;
    return authorTrust({ role, owner: role === "admin", isBot: !!app, isAgent, firstContribution });
  }
  tv(fold: Fold, authorKey: string, text: string): TrustView { return trustView(text, this.authorOf(fold, authorKey)); }
  private issueJson(fold: Fold, iss: Issue, agentMedia: boolean): object {
    const cur = iss.revisions.at(-1)!;
    return { number: iss.number, title: cur.title, body: cur.body, user: { login: loginOf(this.authz, iss.author) },
             assignees: (iss.assigned.at(-1)?.assignees ?? []).map((login) => ({ login })),
             ...(agentMedia ? { graftyard_trust: this.tv(fold, iss.author, cur.body) } : {}) };
  }
  private commentJson(fold: Fold, c: IssueComment, agentMedia: boolean): object {
    return { id: c.id, body: c.body, user: { login: loginOf(this.authz, c.author) }, ...(agentMedia ? { graftyard_trust: this.tv(fold, c.author, c.body) } : {}) };
  }

  /** The controller's prompt for a task: the trust view of the issue snapshot, comments only from human authors who can
   *  push, and counts of what was left out. (AGENTS.md from the default branch is left to M0-WP14's controller.) */
  prompt(repo: string, session: string): object | null {
    const fold = this.folds[repo];
    const t = fold?.tasks[session];
    if (!t) return null;
    const paused = fold.paused?.[session] != null;
    const iss = t.issue != null ? fold.issues?.[String(t.issue)] : undefined;
    const snap = iss ? issueAt(iss, t.snapshot ?? 0) : null;
    const comments = (fold.comments ?? []).filter((c) => c.issue === t.issue);
    const kept = comments.filter((c) => this.authorOf(fold, c.author).trusted);
    return { session, paused, state: snake(this.taskStateNow(fold, t)), snapshot: t.snapshot,
             title: snap ? trustView(snap.title, this.authorOf(fold, iss!.author)) : null,
             body: snap ? trustView(snap.body, this.authorOf(fold, iss!.author)) : null, prompt: t.prompt ?? null,
             comments: kept.map((c) => trustView(c.body, this.authorOf(fold, c.author))), comments_left_out: comments.length - kept.length };
  }

  /** The session page and its JSON: the viewer's `contents: read` on the base, then the log (none pushed in M0). */
  private async sessionPage(req: Request, repo: string, id: string): Promise<Response> {
    const fold = this.folds[repo];
    if (!fold) return json(404, { message: "Not Found" });
    const who = await this.principal(req, repo);
    if (!who) return json(401, { message: "Requires authentication", documentation_url: DOC });
    if (!this.canReadPerm(fold, who.p, "metadata")) return json(404, { message: "Not Found" });
    if (!this.canReadPerm(fold, who.p, "contents")) return json(403, { message: "Resource not accessible: session pages need contents: read", documentation_url: DOC });
    return fold.tasks[resolveSession(id, who.p.app)] ? json(404, { message: "Not Found: the session log has not been pushed" }) : json(404, { message: "Not Found" });
  }

  deliveryLog(): Delivery[] { return this.deliveries; }

  /** Harness reads. */
  state(): object {
    const f = (k: string) => ({ refs: this.folds[k]?.refs, prs: this.folds[k]?.prs, tasks: Object.fromEntries(Object.entries(this.folds[k]?.tasks ?? {})
      .map(([s, t]) => [s, { state: t.state, live: isLive(t, Date.now(), this.authz) }])), leases: this.folds[k]?.leases.length });
    return { epoch: this.epoch, base: f(BASE), fork: f(FORK), counts: this.counts, auditSize: this.audit.size };
  }
  auditRows(probes: string[]): (AuditRow | null)[] { return probes.map((p) => this.audit.get(p) ?? this.persist?.audit(p) ?? null); }
  /** After a restart: the fold's refs against what git holds on the container's disk. */
  async check(): Promise<object> {
    const out: Record<string, object> = {};
    for (const repo of Object.keys(this.folds)) {
      const r = await this.git.fetch(`/refs?repo=${enc(repo)}`);
      const git = Object.fromEntries((await r.text()).split("\n").filter(Boolean).map((l) => { const [o, n] = l.split(" "); return [n, o]; }));
      const fold = this.folds[repo].refs;
      const diff = [...new Set([...Object.keys(git), ...Object.keys(fold)])].filter((n) => git[n] !== fold[n]);
      out[repo] = { status: r.status, fold: Object.keys(fold).length, git: Object.keys(git).length, differ: diff.length, sample: diff.slice(0, 3) };
    }
    return out;
  }
  /** For the B0-37 hidden-ref check: what git itself holds under refs/graftyard/, beside the fold. */
  hostRefs(repo: string): Record<string, string> {
    return Object.fromEntries(Object.entries(this.folds[repo]?.refs ?? {}).filter(([n]) => n.startsWith(HOST_REFS)));
  }
}

/** The GitHub App permission each command needs (GHES 3.22's "Permissions required for GitHub Apps"). */
export function permOf(cmd: Cmd): Perm {
  switch (cmd.t) {
    case "Push": case "Merge": case "ReleaseCreate": case "RepositoryDispatch": return "contents";
    case "Review": case "ReadyForReview": case "PrOpen": return "pull_requests";
    case "LabelWrite": return "issues";
    case "StatusCreate": return "statuses";
    case "CheckRunCreate": return "checks";
    case "WorkflowDispatch": case "WorkflowRerun": return "actions";
    case "TaskCancel": return "issues";
    default: return "administration";
  }
}

function gitResult(b: Uint8Array, timing = ""): Response {
  return new Response(b, { headers: { "content-type": "application/x-git-receive-pack-result", "cache-control": "no-cache", ...(timing ? { "server-timing": timing } : {}) } });
}
const enc = encodeURIComponent;
const wire = (u: RefUpdate) => ({ name: u.name, old: u.old ?? ZERO, new: u.new ?? ZERO });
function gqlRepo(input: Record<string, any>): string | null {
  const r = input.repositoryId ?? input.branch?.repositoryNameWithOwner ?? (typeof input.refId === "string" ? input.refId.split(":")[0].replace(/^REF_/, "") : null);
  if (!r) return null;
  return String(r).replace(/^R_/, "");
}
const refFromId = (id: unknown): string => String(id ?? "").split(":").slice(1).join(":");
type WriteScopeT = "Read" | "Write" | "WriteApproveMerge";

const STATES: Record<string, AgentTask["state"]> = { queued: "Queued", in_progress: "InProgress", completed: "Completed", failed: "Failed",
  idle: "Idle", waiting_for_user: "WaitingForUser", timed_out: "TimedOut", cancelled: "Cancelled" };
const fromSnake = (s: string): AgentTask["state"] | null => STATES[s] ?? null;
function taskStateOf(t: AgentTask, at: number, authz: Authz, fold: Fold): AgentTask["state"] {
  if (authz.installations[t.installation]?.active === false) return "Cancelled";
  if (!FINAL_STATES.includes(t.state) && at - t.updatedAt >= TASK_IDLE) return "TimedOut";
  void fold;
  return t.state;
}
/** The task API's `task` object (design/AGENT-LAYER.md, "API reference"). `prompt` only with `contents: read`. */
export function taskJson(t: AgentTask, contents: boolean, fold?: Fold, state?: AgentTask["state"]): object {
  return { id: t.session, kind: t.kind === "GraftyardRun" ? "graftyard_run" : "external", state: snake(state ?? t.state), app_id: t.app,
           installation_id: t.installation, requester: t.requester != null ? { id: t.requester } : null, creator: t.creator ?? null,
           issue_number: t.issue ?? null, ...(contents ? { prompt: t.prompt ?? null } : {}), pr_number: t.pr ?? null, summary: t.summary ?? "",
           prefix: t.prefix, paused: fold?.paused?.[t.session] != null, created_at: t.createdAt ? new Date(t.createdAt).toISOString() : null,
           updated_at: new Date(t.updatedAt).toISOString() };
}
/** Task-rule refusals in the API reference's shapes: 422 with `errors` for a bad field, `message` only for a policy. */
function taskRefusal(r: Refusal, refuse: (r: Refusal) => Response, t?: AgentTask, h?: Host): Response {
  if (r.rule === "Invalid") {
    const [message, field, code] = r.text.split("|");
    return json(422, { message: "Validation Failed", documentation_url: DOC, errors: [{ resource: "Task", field, code, message }] });
  }
  if (r.rule === "AgentsDisabled" || r.rule === "TaskCap") return json(422, { message: r.text, documentation_url: DOC });
  if (r.rule === "TaskFinal" || r.rule === "TaskPaused") return json(409, { message: r.text, documentation_url: DOC, state: t ? snake(t.state) : undefined });
  if (r.rule === "TaskCreator" || r.rule === "TaskCaller") return json(403, { message: r.text, documentation_url: DOC });
  void h;
  return refuse(r);
}
const loginOf = (authz: Authz, key: string): string => {
  const a = /^a(\d+)i/.exec(key);
  if (a) return `${authz.apps[a[1]]?.slug ?? `app-${a[1]}`}[bot]`;
  return authz.users[key.slice(1)]?.login ?? key;
};
function commitMsg(raw: Uint8Array): string {
  let i = 0;
  for (; i + 1 < raw.length; i++) if (raw[i] === 10 && raw[i + 1] === 10) break;
  return new TextDecoder().decode(raw.subarray(Math.min(raw.length, i + 2)));
}
export { concat };
