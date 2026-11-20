// Load34, the hosted B0-34 client (`graft-m1-load`; decisions/DECISIONS.md, "`load34`"): a1.mjs's B0-34 load, run by N
// Durable Object clients inside Cloudflare instead of the orb (FRAME.md, "Operator decision, 2026-11-20", item 3). Each
// client runs its share (plan.mjs `clientPlan`) of journey 9's agent pushes, side commands, ref and API reads, and feed
// polls, and the clients that hold a role run a1's capped jobs: the tip mover, the clone probes, human pushes, and the
// hot networks' pushes and whole clones. Requests and their samples are a1.mjs's: the same bodies (`../rest/wire.mjs`),
// headers, kinds, phases and codes, and samples `[start on Cloudflare's clock, kind, ms, code]`.
//
// What differs from a1, each because the client runs in a Durable Object:
//   - At most SLOTS fetches per client wait for headers at once; a seventh would queue in the runtime and inflate its
//     sample, so the client queues it itself, and the wait is left out of every sample and reported (`slotWaits`).
//   - Feed polls go straight to the members over the sites' `Member` bindings, as the edge sends them, not through the
//     tools edge (a fetch, which the slot limit would hold for each 25 s poll).
//   - `tip:move` sends the next first-parent commit's thin pack, built on the orb and staged in R2 (stage.mjs), after the
//     receive-pack advertisement, as `git push` does; a1 ran `git push` from the bare clone.
//   - Each of a1's capped kinds (tip moves, clone probes, human pushes, each hot network's pushes and whole clones) runs
//     in one client of its own with a1's cap; the other clients share the uncapped kinds, the agents and the pollers.
// The orb (a1.mjs --hosted) does everything else: setup, the kill, k, leader and admission reads, the fill checks, and
// the row. It sends the members to poll with each progress read, and collects each client's output from R2.
import { DurableObject } from "cloudflare:workers";
import { Buffer } from "node:buffer";
import { cloneKeyHex, onePack, pkt, pushResult, receiveBody } from "../rest/wire.mjs";
import { PHASED, Slots, clientPlan, pct, phaseOf } from "./plan.mjs";

// The sites' regions, as the tools edge hints its member stubs (tests/m1/wp7/edge.ts, HINTS).
const MEMBER_HINTS = ["enam", "weur", "apac"];
const sleep = (ms) => new Promise((r) => setTimeout(r, Math.max(0, ms)));
const GIT_TIMEOUT_MS = 200_000;
// Past the edge's own worst case, as a1's: a push still in flight at the window's end gets this long to answer.
const TAIL_MS = 210_000;
const SAMPLED = Symbol("sampled");
const pick = (xs) => xs[Math.floor(Math.random() * xs.length)];
const exp = (perS) => -Math.log(1 - Math.random()) * 1000 / perS;
const masked = (s) => String(s ?? "").replace(/[0-9a-f]{12,}/g, "H").replace(/\d+/g, "N");
/** `n` random bytes, as a1's `randomBytes` (getRandomValues takes at most 64 KiB a call). */
function rnd(n) {
  const b = new Uint8Array(n);
  for (let o = 0; o < n; o += 65536) crypto.getRandomValues(b.subarray(o, Math.min(n, o + 65536)));
  return Buffer.from(b.buffer);
}
const auth = (tok) => ({ authorization: `Basic ${Buffer.from(`x:${tok}`).toString("base64")}` });
const LS_HEADERS = { "content-type": "application/x-git-upload-pack-request", "git-protocol": "version=2" };
const lsBody = (prefix) => Buffer.concat([pkt("command=ls-refs\n"), Buffer.from("0001"), pkt(`ref-prefix ${prefix}\n`), Buffer.from("0000")]);

/** One client's run of one spec. */
class Run {
  constructor(obj, spec) {
    this.obj = obj; this.env = obj.env; this.spec = spec;
    this.t0 = spec.t0; this.end = spec.t0 + spec.seconds * 1000;
    this.slots = new Slots();
    this.samples = []; this.pushes = []; this.misses = []; this.acks = []; this.seen = [];
    this.lsRefsBy = {}; this.requests = {}; this.errors = {};
    this.inflight = new Set(); this.inKind = {};
    this.members = spec.members ?? []; this.membersReadAt = 0;
    this.main = spec.main; this.missesSent = 0; this.state = "waiting"; this.stopped = false;
    this.pushSeq = 0; this.labelN = 0; this.leases = new Map(); this.lags = [];
  }
  now() { return Date.now(); }
  note(k, ms, code) {
    if (PHASED.test(k)) k += phaseOf(this.now(), this.t0, this.spec.marks);
    this.samples.push([this.now() - ms, k, ms, String(code)]);
  }
  count(kind) { this.requests[kind] = (this.requests[kind] ?? 0) + 1; }
  err(e) { const k = String(e).slice(0, 90); this.errors[k] = (this.errors[k] ?? 0) + 1; }

  /** A fetch through the client's slots: `{r, t}`, `t` when it was sent. The wait for a slot goes to `c.waited`. */
  async send(c, kind, url, init) {
    c.waited += await this.slots.take();
    let held = true;
    const give = () => { if (held) { held = false; this.slots.give(); } };
    this.count(kind);
    const t = this.now();
    try { const r = await fetch(url, init); give(); return { r, t }; } catch (e) { give(); throw e; }
  }

  // ---- a1.mjs's request helpers ----

  async api(c, method, path, tok, body, extra = {}) {
    const headers = { accept: "application/vnd.github+json", "user-agent": "graftyard-am1", ...extra };
    if (tok) headers.authorization = `token ${tok}`;
    if (body !== undefined) headers["content-type"] = "application/json";
    let t0 = null;
    for (let i = 0; ; i++) {
      try {
        const { r, t } = await this.send(c, "api", `${this.spec.edge}${path}`, { method, headers, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(180_000) });
        t0 ??= t;
        const text = await r.text();
        let b = null;
        try { b = text ? JSON.parse(text) : null; } catch { b = text.slice(0, 300); }
        return { status: r.status, body: b, headers: Object.fromEntries(r.headers), ms: this.now() - t0 };
      } catch (e) {
        if (i >= 2) return { status: 0, body: String(e).slice(0, 200), headers: {}, ms: this.now() - (t0 ?? this.now()) };
        await sleep(1000);
      }
    }
  }
  async git(c, path, tok, body, headers = {}, repo = this.spec.repo) {
    let t = this.now();
    try {
      const s = await this.send(c, "git", `${this.spec.edge}/${repo}.git/${path}`, { method: "POST", body,
        headers: { ...auth(tok), "user-agent": "git/2.56.0", ...headers }, signal: AbortSignal.timeout(GIT_TIMEOUT_MS) });
      t = s.t;
      const out = { status: s.r.status, text: Buffer.from(await s.r.arrayBuffer()).toString("latin1"), ms: this.now() - t, headers: s.r.headers };
      if (body.includes("command=ls-refs")) {
        const k = `${s.r.headers.get("x-graft-ls-refs") ?? "shard"} ${s.r.status}`;
        this.lsRefsBy[k] = (this.lsRefsBy[k] ?? 0) + 1;
      }
      return out;
    } catch (e) { return { status: 0, text: String(e).slice(0, 80), ms: this.now() - t, err: { name: e?.name, cause: e?.cause?.code ?? String(e?.cause ?? "").slice(0, 80) } }; }
  }
  lsRefs(c, tok, prefix, repo) { return this.git(c, "git-upload-pack", tok, lsBody(prefix), LS_HEADERS, repo); }
  async tipOf(c, ref, tok = this.spec.tokens.admin, repo = this.spec.repo) {
    const r = await this.lsRefs(c, tok, ref, repo);
    return r.text.match(new RegExp(`([0-9a-f]{40}) ${ref}\\n`))?.[1] ?? null;
  }
  /** The receive-pack advertisement, as `git push` reads it before its POST: `{status, text}` after the whole body. */
  async advert(c, tok) {
    try {
      const { r } = await this.send(c, "git", `${this.spec.edge}/${this.spec.repo}.git/info/refs?service=git-receive-pack`,
        { headers: { ...auth(tok), "user-agent": "git/2.56.0" }, signal: AbortSignal.timeout(GIT_TIMEOUT_MS) });
      return { status: r.status, text: Buffer.from(await r.arrayBuffer()).toString("latin1") };
    } catch (e) { return { status: `error ${String(e).slice(0, 30)}`, text: "" }; }
  }
  /** One push of `bytes` to `ref` from `from` (old tip or null): [result, ms, new tip, err, server-timing]. */
  async push(c, tok, ref, from, path, bytes, kind) {
    const { pack, commit } = onePack(path, bytes, from ?? this.main, `a1 ${ref}`, Math.floor(this.now() / 1000));
    const client = `${this.spec.run}-${this.spec.i}-${++this.pushSeq}`;
    const start = this.now();
    const r = await this.git(c, "git-receive-pack", tok, receiveBody(ref, from, commit, pack), { "content-type": "application/x-git-receive-pack-request", "x-graft-client-push": client });
    const res = r.status === 200 ? pushResult(r.text) : `http ${r.status} ${masked(r.text).slice(0, 160).trim()}`;
    const timing = r.headers?.get?.("server-timing") ?? null;
    if (res === "ok") this.acks.push([commit, this.now()]);
    this.pushes.push({ client, kind, ref, old: from ?? null, new: commit, start, end: this.now(), status: r.status, result: res,
      rid: r.headers?.get?.("x-github-request-id") ?? null, err: r.err ?? null, timing });
    return [res, r.ms, res === "ok" ? commit : from, r.err, timing];
  }
  st(r) { return String(r.status); }
  stages(r) {
    if (r.status === 200) for (const m of (r.headers?.["server-timing"] ?? "").matchAll(/([a-z_]+);dur=(\d+)/g)) this.note(`api:${m[1]}`, Number(m[2]), "200");
    return this.st(r);
  }
  openPr(c, tok, branch, draft) {
    return this.api(c, "POST", "/api/graphql", tok, { query: "mutation($i: CreatePullRequestInput!) { createPullRequest(input: $i) { pullRequest { id number } } }",
      variables: { i: { repositoryId: this.spec.repoId, baseRefName: "main", headRefName: branch, title: branch, body: "a1", draft } } });
  }
  /** a1's `cloneTtfb`: a v2 fetch of `want`, timed to its first byte; `firstByte` asks depth 1 and cancels after it. */
  async cloneTtfb(c, repo, want, kind, firstByte, tok = this.spec.tokens.admin) {
    let s = this.now();
    try {
      const sent = await this.send(c, "git", `${this.spec.edge}/${repo}.git/git-upload-pack`, { method: "POST", signal: AbortSignal.timeout(120_000),
        headers: { ...auth(tok), "git-protocol": "version=2", "content-type": "application/x-git-upload-pack-request" },
        body: Buffer.concat([pkt("command=fetch\n"), Buffer.from("0001"), pkt("no-progress\n"), ...(firstByte ? [pkt("ofs-delta\n")] : []),
          pkt(`want ${want}\n`), ...(firstByte ? [pkt("deepen 1\n")] : []), pkt("done\n"), Buffer.from("0000")]) });
      const r = sent.r;
      s = sent.t;
      const reader = r.body.getReader();
      await reader.read();
      const ms = this.now() - s;
      this.note(kind, ms, String(r.status));
      const edge = /total;dur=(\d+)/.exec(r.headers.get("server-timing") ?? "");
      if (edge) this.note(`${kind}:edge`, Number(edge[1]), String(r.status));
      if (firstByte) {
        const cache = r.headers.get("x-graft-clone-cache") ?? "nocache", compose = r.headers.get("x-graft-compose");
        this.note(`${kind}:${cache}`, ms, compose ?? String(r.status));
        // The edge clock split the same way, so hits and misses each have both clocks (clonelines.py).
        if (edge) this.note(`${kind}:${cache}:edge`, Number(edge[1]), compose ?? String(r.status));
        if (cache === "miss") {
          this.misses.push({ at: new Date(s).toISOString(), ms, status: r.status, compose, want,
            key: cloneKeyHex(Number(this.spec.chain.split(":")[1]), want), filled: null });
        }
      }
      if (firstByte) await reader.cancel();
      else {
        // A whole clone is read at most `hotReadMBps`, mt7's whole clones' rate through the orb, so it stays the slow
        // reader lock-split has to make harmless and its egress stays a1's (decisions/DECISIONS.md, `load34`).
        const rate = (this.spec.b034.hotReadMBps ?? 8) * 1e6 / 1000;
        let got = 0;
        for (const t1 = this.now(); ;) {
          const { done, value } = await reader.read();
          if (done) break;
          got += value.byteLength;
          if (rate > 0) await sleep(got / rate - (this.now() - t1));
        }
      }
      return String(r.status);
    } catch (e) { this.note(kind, this.now() - s, `error ${String(e).slice(0, 30)}`); return "error"; }
  }

  // ---- members, for feed polls ----

  /** One call to a site's member object, as the tools edge's `/member` makes it: `{status, v}`, or `{status: null}`
   *  for a failed call or a missed deadline. */
  async member(site, path, body, deadline, name) {
    const ns = this.env[`MEMBER_${site}`];
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), deadline);
    this.count("member");
    try {
      const stub = ns.get(ns.idFromName(name ?? `${this.spec.chain}#1#f${site}`), { locationHint: MEMBER_HINTS[site] });
      const res = await stub.fetch(`https://member${path}`, body === undefined ? { signal: ctl.signal } : { method: "POST", body: JSON.stringify(body), signal: ctl.signal });
      const text = await res.text();
      let v = text;
      try { v = JSON.parse(text); } catch { /* text */ }
      return { status: res.status, v };
    } catch { return { status: null }; } finally { clearTimeout(timer); }
  }
  /** The members a poll may go to, as a1's `readMembers`: learners first, then the voters that do not lead. Used only
   *  after a `notServing` (a voter leads now), at most every 10 s; the orb sends the list with each progress read. */
  async readMembers() {
    if (this.now() - this.membersReadAt < 10_000) return;
    this.membersReadAt = this.now();
    const st = await Promise.all([0, 1, 2].map(async (s) => ({ site: s, ...((await this.member(s, "/status", undefined, 15_000)).v ?? {}) })));
    const newest = st.filter((x) => x.config?.config?.normal).sort((a, b) => (b.config.index ?? 0) - (a.config.index ?? 0))[0];
    const cfg = newest?.config?.config?.normal;
    if (!cfg) return;
    const names = Object.fromEntries(cfg.voters.map((v) => [v.site, v.do_name]));
    const leads = new Set((await Promise.all(cfg.voters.map(async (v) => ({ site: v.site, role: v.do_name === st[v.site]?.doName ? st[v.site]?.role
      : (await this.member(v.site, "/status", undefined, 15_000, names[v.site])).v?.role })))).filter((x) => x.role === "leader").map((x) => x.site));
    this.members = [...(cfg.learners ?? []).map((l) => ({ site: l.site, name: l.do_name, learner: true })),
      ...cfg.voters.filter((v) => !leads.has(v.site)).map((v) => ({ site: v.site, name: v.do_name, learner: false }))];
  }
  /** a1's `poll`: one feed poll routed as the edge's `activity` routes it. */
  async poll(k, cursor) {
    const ls = this.members.filter((m) => m.learner), vs = this.members.filter((m) => !m.learner);
    const order = [...ls.slice(k % Math.max(1, ls.length)), ...ls.slice(0, k % Math.max(1, ls.length)), ...vs];
    const body = (extra) => ({ cursor, wait_s: 25, paths: [`a1/${k % 50}/**`], token: `a1-${this.spec.run}-${k}`, ip: `a1ip-${k}`, ...extra });
    const answers = [], tried = [];
    let learnerAnswered = false, gone = null;
    for (const m of order) {
      if (answers.length >= 3) break;
      const r = await this.member(m.site, "/feed", body({ learner_answers: learnerAnswered && !m.learner }), 40_000, m.name);
      const v = r.v ?? {};
      tried.push(v.admit ?? `status ${r.status}`);
      if (r.status !== 200 || !v.admit) continue;
      if (v.admit === "hold" || v.admit === "tooMany") return { v, m, tried };
      if (v.admit === "gone") { gone ??= { v, m }; answers.push({ a: "gone", m }); }
      else if (v.admit === "full") answers.push({ a: "full", m, lag: 0 });
      else if (v.admit === "lagging") answers.push({ a: "lagging", m, lag: v.lagMs ?? Number.MAX_SAFE_INTEGER });
      else if (v.admit === "notServing" && m.learner) learnerAnswered = true;
      else if (v.admit === "notServing") void this.readMembers();
    }
    if (answers.some((x) => x.a === "full")) return { v: { admit: "429" }, tried };
    const others = answers.filter((x) => x.a !== "gone");
    if (gone && answers.length > 0 && others.length === 0) return { v: gone.v, m: gone.m, tried };
    const lag = others.filter((x) => x.a === "lagging");
    const least = lag.sort((a, b) => a.lag - b.lag)[0];
    if (least && lag.length === others.length && least.m.learner) {
      const r = await this.member(least.m.site, "/feed", body({ learner_answers: false, lagging_ok: true }), 40_000, least.m.name);
      if (r.v?.admit === "hold") return { v: r.v, m: least.m, tried: [...tried, "laggingOk"] };
    }
    return { v: { admit: "429" }, tried };
  }

  // ---- the scheduler ----

  live() { return this.now() < this.end && !this.stopped; }
  track(p) { this.inflight.add(p); void p.finally(() => this.inflight.delete(p)); return p; }
  /** a1's `every`: `kind` at Poisson times of `perS`, at most `cap` in flight; `f(c)` answers the sample's code, or
   *  SAMPLED when it noted its own. The sample leaves out `c.waited`, the time spent waiting for a slot. */
  async every(kind, perS, f, cap, from = this.t0) {
    if (!perS) return;
    await sleep(from - this.now());
    while (this.live()) {
      if ((this.inKind[kind] ?? 0) < cap) {
        this.inKind[kind] = (this.inKind[kind] ?? 0) + 1;
        const c = { waited: 0 }, s = this.now();
        this.track(f(c).then((code) => { if (code !== SAMPLED) this.note(kind, this.now() - s - c.waited, code); },
          (e) => this.note(kind, this.now() - s - c.waited, `error ${String(e).slice(0, 30)}`)).finally(() => { this.inKind[kind]--; }));
      } else this.note(kind, 0, "skipped: cap");
      await sleep(exp(perS));
    }
  }

  jobs() {
    const { spec } = this, p = spec.plan, admin = spec.tokens.admin, app = spec.tokens.app, R = spec.repo;
    const V1 = `/api/graftyard/v1/repos/${R}`;
    const hot = [spec.b034.second && { ...spec.b034.second, at: spec.marks.secondAt, tag: ":second" },
      ...(spec.b034.hostile ?? []).map((h, j) => ({ ...h, at: spec.marks.hostileAt, tag: `:hostile${j + 1}` }))].filter(Boolean)
      .map((h) => ({ ...h, pushPerS: spec.b034.hotPushPerS ?? 1, clonePerS: spec.b034.hotClonePerS ?? 3 }));
    const mine = clientPlan(spec.i, spec.n, p, hot);
    const tasks = spec.tasks ?? [];
    const agents = mine.agents.map((i) => ({ i, ref: `refs/heads/claude/a1-${spec.run}-${i}`, tip: null, group: i % 50, pr: false }));
    const out = [];
    const size = () => (Math.random() < p.bigShare ? p.bigBytes : p.packMin + Math.floor(Math.random() * (p.packMax - p.packMin)));
    // Agents: each pushes its own ref every pushEvery s, its start spread over the first interval.
    for (const a of agents) out.push((async () => {
      await sleep(this.t0 - this.now() + Math.random() * p.pushEvery * 1000);
      while (this.live()) {
        const s = this.now(), c = { waited: 0 };
        const [res, ms, nu] = await this.track(this.push(c, app, a.ref, a.tip, `a1/${a.group}/${a.i}/f.txt`, rnd(size()), "agent"));
        a.tip = nu;
        this.note("push:agent", ms, res);
        await sleep(Math.min(this.end - this.now(), p.pushEvery * 1000 - (this.now() - s)));
      }
    })());
    const side = {
      "side:taskEdit": async (c) => { const t = pick(tasks); return t ? this.st(await this.api(c, "PATCH", `${V1}/tasks/${t[1]}`, app, { summary: `a1 ${this.now()}` })) : "no task"; },
      "side:comment": async (c) => {
        const r = await this.api(c, "POST", "/api/graphql", app, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: spec.target.id, body: "a1" } } });
        return r.body?.data?.addComment ? "ok" : `gql ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 40)}`;
      },
      "side:label": async (c) => {
        const add = (this.labelN++ & 1) === 0;
        return this.st(await this.api(c, add ? "POST" : "DELETE", `/api/v3/repos/${R}/issues/${spec.target.number}/labels${add ? "" : "/a1"}`, app, add ? ["a1"] : undefined));
      },
      "side:standIn": async (c) => { const t = pick(tasks); return t ? this.st(await this.api(c, "PATCH", `${V1}/tasks/${t[1]}`, app, { summary: `a1 stand-in ${this.now()}` })) : "no task"; },
      "side:draftPr": async (c) => {
        const a = pick(agents.filter((x) => x.tip && !x.pr));
        if (!a) return "no branch";
        a.pr = true;
        const r = await this.openPr(c, app, a.ref.slice(11), true);
        return r.body?.data?.createPullRequest?.pullRequest ? "ok" : `gql ${String(r.body?.errors?.[0]?.message ?? r.status).slice(0, 40)}`;
      },
      "side:lease": async (c) => {
        const t = pick(tasks);
        if (!t) return "no task";
        const h = { "x-graftyard-agent-session": t[1] };
        if (this.leases.has(t[0])) { const r = await this.api(c, "DELETE", `${V1}/leases/${this.leases.get(t[0])}`, app, undefined, h); this.leases.delete(t[0]); return this.st(r); }
        const r = await this.api(c, "POST", `${V1}/leases`, app, { paths: [`a1/${t[0] % 50}/${t[0]}/x/**`], mode: "shared", ttl_seconds: 600 }, h);
        if (r.status === 201 && r.body?.lease?.id) this.leases.set(t[0], r.body.lease.id);
        return this.st(r);
      },
      "side:leaseRenew": async (c) => {
        const held = [...this.leases];
        const l = pick(held);
        const t = l && tasks.find((x) => x[0] === l[0]);
        return l ? this.st(await this.api(c, "POST", `${V1}/leases/${l[1]}/renew`, app, { ttl_seconds: 600 }, { "x-graftyard-agent-session": t[1] })) : "no lease";
      },
      "side:human": async (c) => {
        const r = await this.api(c, "POST", "/api/graphql", admin, { query: "mutation($i: AddCommentInput!) { addComment(input: $i) { subject { id } } }", variables: { i: { subjectId: spec.target.id, body: "a1 human" } } });
        return r.body?.data?.addComment ? "ok" : `gql ${r.status}`;
      },
      "read:ref": async (c) => this.st(await this.lsRefs(c, admin, "refs/heads/")),
      "read:api": async (c) => this.stages(Math.random() < 0.5
        ? await this.api(c, "GET", `/api/v3/repos/${R}/issues?per_page=30`, admin)
        : await this.api(c, "POST", "/api/graphql", admin, { query: `query { repository(owner: "${R.split("/")[0]}", name: "${R.split("/")[1]}") { pullRequests(last: 10) { nodes { number isDraft } } } }` })),
    };
    for (const u of mine.uncapped) out.push(this.every(u.kind, u.perS, side[u.kind], 400));
    for (const role of mine.roles) out.push(this.role(role, hot));
    for (const k of mine.pollers) out.push(this.poller(k));
    return out;
  }

  /** The time since `from` less the slot waits `c` gathered since `w0`. */
  lap(c, from, w0) { return this.now() - from - (c.waited - w0); }

  role(role, hot) {
    const { spec } = this, admin = spec.tokens.admin;
    if (role.role === "tip") return this.every(role.kind, role.perS, (c) => this.tipMove(c), role.cap);
    if (role.role === "probe") return this.every(role.kind, role.perS, async (c) => {
      const t = this.now();
      const want = await this.mainNow();
      c.waited += this.now() - t;
      return this.cloneTtfb(c, spec.repo, want, "read:cloneTtfb", true);
    }, role.cap);
    if (role.role === "human") {
      // Humans share 20 refs, so two pushes to one ref can race and the later one is `stale:race`, as a1 counts it.
      // `git-shaped-push`: the advertisement and the POST are the sample; the tip read before them is its own line.
      const inFlight = {};
      return this.every(role.kind, role.perS, async (c) => {
        const k = Math.floor(Math.random() * 20), ref = `refs/heads/a1h-${spec.run}-${k}`;
        inFlight[k] = (inFlight[k] ?? 0) + 1;
        let raced = inFlight[k] > 1;
        try {
          let t = this.now(), w = c.waited;
          const from = await this.tipOf(c, ref);
          this.note("push:human:tip", this.lap(c, t, w), from ? "ok" : "none");
          t = this.now(); w = c.waited;
          const adv = await this.advert(c, admin);
          if (adv.status !== 200) { this.note("push:human", this.lap(c, t, w), `adv ${adv.status}`); return SAMPLED; }
          const [res, , , , timing] = await this.push(c, admin, ref, from, `a1h/${k}.txt`, rnd(20_000), "human");
          this.note("push:human", this.lap(c, t, w), (raced ||= inFlight[k] > 1) && res === "stale" ? "stale:race" : res);
          const edge = /total;dur=(\d+)/.exec(timing ?? "");
          if (edge) this.note("push:human:edge", Number(edge[1]), res);
          return SAMPLED;
        } finally { inFlight[k]--; }
      }, role.cap);
    }
    const n = hot[role.net], tok = spec.tokens[n.who], from = this.t0 + n.at * 1000;
    const mainOf = (c) => this.tipOf(c, "refs/heads/main", tok, n.repo);
    if (role.role === "hotClone") return this.every(role.kind, role.perS, async (c) => {
      const want = await mainOf(c);
      return want ? this.cloneTtfb(c, n.repo, want, `hot:cloneTtfb${n.tag}`, false, tok) : "no main";
    }, role.cap, from);
    const tips = {}, refs = Array.from({ length: 20 }, (_, k) => k).filter((k) => k % role.parts === role.part);
    return this.every(role.kind, role.perS, async (c) => {
      const k = pick(refs), ref = `refs/heads/b034-${spec.run}-${k}`;
      const base = tips[ref] ?? await mainOf(c);
      const { pack, commit } = onePack(`b034/${k}.txt`, rnd(20_000), base, `b034 ${ref}`, Math.floor(this.now() / 1000));
      const r = await this.git(c, "git-receive-pack", tok, receiveBody(ref, tips[ref] ?? null, commit, pack), { "content-type": "application/x-git-receive-pack-request" }, n.repo);
      const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
      if (res === "ok") tips[ref] = commit;
      return res;
    }, role.cap, from);
  }

  /** The tip client's current main, for a clone probe (an RPC, not a fetch; its time is left out of the sample). */
  async mainNow() {
    if (this.spec.i === this.tipClient()) return this.main;
    try { return (await this.obj.peer(this.spec.run, this.tipClient()).main()) ?? this.main; } catch { return this.main; }
  }
  tipClient() { return 0; }

  /** `tip:move`: the advertisement (git push reads main there), then the staged thin pack of the next first-parent
   *  commit after it, as a receive-pack POST. A refused move is sent again from the advertised main next time. */
  async tipMove(c) {
    const { spec } = this, future = spec.b034.future;
    const adv = await this.advert(c, spec.tokens.admin);
    if (adv.status !== 200) return `adv ${adv.status}`;
    const cur = adv.text.match(/([0-9a-f]{40}) refs\/heads\/main[\0\n]/)?.[1] ?? null;
    const at = future.indexOf(cur);
    const next = at >= 0 ? at + 1 : (this.nextMove ?? 0);
    this.nextMove = next;
    const want = future[next];
    if (!want) return "no more history";
    const t = this.now();
    const pack = await this.packAt(next);
    c.waited += this.now() - t;
    if (!pack) return "no staged pack";
    void this.packAt(next + 1);
    const r = await this.git(c, "git-receive-pack", spec.tokens.admin, receiveBody("refs/heads/main", cur, want, pack), { "content-type": "application/x-git-receive-pack-request" });
    const res = r.status === 200 ? pushResult(r.text) : `http ${r.status}`;
    if (res === "ok") { this.main = want; return "ok"; }
    return `refused ${masked(res).slice(0, 80)}`;
  }
  packAt(i) {
    this.packs ??= new Map();
    if (!this.packs.has(i)) {
      this.packs.set(i, (async () => {
        await this.slots.take();
        try { const o = await this.env.BUCKET.get(`${this.spec.b034.packs}/${i}`); return o ? Buffer.from(await o.arrayBuffer()) : null; } finally { this.slots.give(); }
      })());
      for (const k of this.packs.keys()) if (k < i - 1) this.packs.delete(k);
    }
    return this.packs.get(i);
  }

  async poller(k) {
    await sleep(this.t0 - this.now() + (k % 500) * 4);
    let cursor = null, misses = {};
    while (this.live()) {
      const s = this.now();
      try {
        const { v, m, tried } = await this.track(this.poll(k, cursor));
        this.note("feed", this.now() - s, v.admit);
        const missed = misses;
        if (v.admit === "hold") misses = {};
        else for (const t of ["429", ...tried]) misses[t] = (misses[t] ?? 0) + 1;
        if (v.cursor) cursor = v.cursor;
        else if (v.admit === "gone" && v.oldest) cursor = v.oldest;
        for (const e of v.events ?? []) {
          const p = e[2]?.AgentPush;
          const head = p ? (typeof p.head === "string" ? p.head : Buffer.from(p.head).toString("hex")) : null;
          if (head) this.seen.push([head, this.now(), k, m?.site, missed]);
        }
        if (v.admit === "429") await sleep(10_000);
      } catch (e) { this.note("feed", this.now() - s, "error"); misses.error = (misses.error ?? 0) + 1; this.err(e); await sleep(1000); }
    }
  }

  // ---- the run ----

  async go() {
    try {
      this.colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace", { signal: AbortSignal.timeout(5_000) })).text())?.[1] ?? "?";
    } catch { this.colo = "?"; }
    this.startLateMs = this.now() - this.t0;
    this.state = "running";
    // The client's own lateness, as a1's `loopDelay`: a 500 ms timer's overrun. Clients can share an isolate's
    // thread, so a client whose timers run late is measuring its own isolate, not the edge.
    const lag = (async () => { while (this.live()) { const t = this.now(); await sleep(500); this.lags.push(this.now() - t - 500); } })();
    const saver = (async () => { while (this.live()) { await sleep(60_000); if (this.live()) await this.flush(false).catch((e) => this.err(e)); } })();
    await Promise.all(this.jobs());
    this.state = "tail";
    // In-flight requests end in their own answer, or are cut at the tail's end and counted.
    const cut = await Promise.race([Promise.allSettled([...this.inflight]).then(() => 0), sleep(TAIL_MS).then(() => this.inflight.size)]);
    this.cutAtTail = cut;
    // The saver flushes only while the window runs, so its last sleep is not waited for.
    void saver.catch(() => undefined);
    await lag;
    await this.flush(true);
    this.state = "done";
    // The spec holds the row's tokens: kept only while the run needs them.
    await this.obj.ctx.storage.delete("spec");
    await this.obj.ctx.storage.put("done", true);
  }
  summary() {
    const w = this.slots.waits;
    return { i: this.spec.i, colo: this.colo, hint: this.spec.hint, state: this.state, startLateMs: this.startLateMs, cutAtTail: this.cutAtTail ?? null,
      requests: this.requests, errors: this.errors,
      slotWaits: { n: w.length, p50: pct(w, 0.5), p99: pct(w, 0.99), max: w.length ? Math.max(...w) : 0 },
      lagMs: { n: this.lags.length, p50: pct(this.lags, 0.5), p99: pct(this.lags, 0.99), max: this.lags.length ? Math.max(...this.lags) : 0 } };
  }
  /** Writes what was gathered since the last flush as the next chunk, `runs/<run>/<i>/<seq>.json.gz`, and drops it from
   *  memory (one isolate can hold many clients, and 128 MB is the isolate's). The last chunk carries the summary. */
  async flush(done) {
    const chunk = { run: this.spec.run, i: this.spec.i, seq: this.seq = (this.seq ?? 0) + 1, done, samples: this.samples,
      pushes: this.pushes, acks: this.acks, seen: this.seen, ...(done ? { summary: { ...this.summary(), lsRefsBy: this.lsRefsBy }, misses: this.misses } : {}) };
    this.samples = []; this.pushes = []; this.acks = []; this.seen = [];
    const gz = await new Response(new Blob([JSON.stringify(chunk)]).stream().pipeThrough(new CompressionStream("gzip"))).arrayBuffer();
    await this.slots.take();
    try { await this.env.BUCKET.put(`runs/${this.spec.run}/${this.spec.i}/${String(chunk.seq).padStart(4, "0")}.json.gz`, gz); } finally { this.slots.give(); }
  }
  /** What the orb's progress read gets: the summary, and the clone-cache misses since the last read (for its R2 fill
   *  checks). `members` is the orb's latest read of the members a feed poll may go to. */
  progress(members) {
    if (members?.length) this.members = members;
    const misses = this.misses.slice(this.missesSent);
    this.missesSent = this.misses.length;
    return { ...this.summary(), misses };
  }
}

/** A load client, pinned by its location hint at creation, running one spec from its alarm. */
export class Load34 extends DurableObject {
  async begin(spec) {
    await this.ctx.storage.deleteAll();
    this.run = null;
    await this.ctx.storage.put("spec", spec);
    await this.ctx.storage.setAlarm(Math.max(Date.now(), spec.t0 - 20_000));
  }
  /** Starts the run and returns: an alarm handler may run 15 minutes at most, and the run is longer. Its fetches keep
   *  the object in memory [measured 2026-11-20: a loop started from an alarm ran past 20 minutes]. */
  async alarm() {
    if (this.run || (await this.ctx.storage.get("done"))) return;
    const spec = await this.ctx.storage.get("spec");
    if (!spec) return;
    this.run = new Run(this, spec);
    void this.run.go().catch((e) => { this.run.state = `failed ${String(e).slice(0, 120)}`; });
  }
  peer(run, i) { return this.env.LOAD.get(this.env.LOAD.idFromName(`${run}-${i}`)); }
  main() { return this.run?.main ?? null; }
  async progress(members) {
    if (this.run) return this.run.progress(members);
    // A spec past its start with no run here: the object restarted and lost the run (its flushed chunks remain).
    const spec = await this.ctx.storage.get("spec");
    return { state: (await this.ctx.storage.get("done")) ? "done" : !spec ? "idle" : spec.t0 < Date.now() ? "lost" : "waiting" };
  }
  stop() { if (this.run) this.run.stopped = true; }
  /** After the orb has collected the run: nothing of it stays in the object. */
  async forget() { this.run = null; await this.ctx.storage.deleteAlarm(); await this.ctx.storage.deleteAll(); }
}

const json = (v, status = 200) => Response.json(v, { status });

export default {
  async fetch(req, env) {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const stub = (run, i, hint) => env.LOAD.get(env.LOAD.idFromName(`${run}-${i}`), hint ? { locationHint: hint } : undefined);
    try {
      switch (url.pathname) {
        case "/start": {
          // Each client gets the spec with its own index; `hints` go round the clients.
          const { spec, hints } = await req.json();
          await Promise.all(Array.from({ length: spec.n }, (_, i) => {
            const hint = hints?.length ? hints[i % hints.length] : undefined;
            return stub(spec.run, i, hint).begin({ ...spec, i, hint: hint ?? null });
          }));
          return json({ started: spec.n });
        }
        case "/progress": {
          const { run, n, members } = await req.json();
          return json(await Promise.all(Array.from({ length: n }, (_, i) => stub(run, i).progress(members).catch((e) => ({ i, state: `unreachable ${String(e).slice(0, 80)}` })))));
        }
        case "/forget": {
          const { run, n } = await req.json();
          await Promise.all(Array.from({ length: n }, (_, i) => stub(run, i).forget().catch(() => undefined)));
          return json({ forgot: n });
        }
        case "/stop": {
          const { run, n } = await req.json();
          await Promise.all(Array.from({ length: n }, (_, i) => stub(run, i).stop().catch(() => undefined)));
          return json({ stopped: n });
        }
        case "/list": {
          const prefix = url.searchParams.get("prefix") ?? "";
          if (!prefix.startsWith("runs/")) return json({ error: "runs/ keys only" }, 400);
          const keys = [];
          let cursor;
          do { const l = await env.BUCKET.list({ prefix, cursor, limit: 1000 }); keys.push(...l.objects.map((o) => o.key)); cursor = l.truncated ? l.cursor : undefined; } while (cursor);
          return json({ keys });
        }
        case "/get": {
          const key = url.searchParams.get("key") ?? "";
          if (!key.startsWith("runs/")) return json({ error: "runs/ keys only" }, 400);
          const o = await env.BUCKET.get(key);
          return o ? new Response(o.body, { headers: { "content-type": "application/gzip" } }) : json({ error: "not found" }, 404);
        }
        case "/stage": {
          const key = url.searchParams.get("key") ?? "";
          if (!key.startsWith("tip/")) return json({ error: "tip/ keys only" }, 400);
          if (req.method === "HEAD") return new Response(null, { status: (await env.BUCKET.head(key)) ? 200 : 404 });
          await env.BUCKET.put(key, await req.arrayBuffer());
          return json({ ok: true });
        }
        case "/purge": {
          const { prefix } = await req.json();
          if (!/^(runs|tip)\/./.test(prefix ?? "")) return json({ error: "runs/ or tip/ prefix" }, 400);
          const l = await env.BUCKET.list({ prefix, limit: 1000 });
          if (l.objects.length) await env.BUCKET.delete(l.objects.map((o) => o.key));
          return json({ deleted: l.objects.length, more: l.truncated });
        }
      }
    } catch (e) { return json({ error: String(e) }, 500); }
    return json({ error: "not found" }, 404);
  },
};
