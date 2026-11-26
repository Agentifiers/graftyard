// M1-WP7 test edge. The same source deploys as graft-m1-wp7-edge and graft-m1-wp7-edge-b (edge.jsonc,
// edge-b.jsonc), so BF-10 can create a chain from two edges at once. Each binds the `Member` and `Registry` classes of
// graft-m1-site-a/b/c with `script_name`.
//
// The caller's side of the design's group lookup (design/DESIGN.md, Shape, "Sites and placement", `ReplyMeta`):
//
// 1. The config this isolate cached from the group's last `NotLeader`, leader first.
// 2. Without one, the bootstrap voter names built from the chain's record, in record order.
// 3. If none answers with a config, the chain's row in the `MemberRegistry` of each record site.
// 4. Then steps 2 and 3 again for each `moving_from` record.
//
// The record comes from the route's source: a system chain's from the genesis view, which this test edge takes from
// its GENESIS variable (tests/m1/wp7/genesis.json, a test-only stand-in for the accepted signed genesis), and an
// owner chain's from the owner's `Placement` row, read through `fresh` on `Names` shard `account_shard(owner)`.
//
// Writes mint one RequestId per client request and re-send it until the request has an outcome. A missed receipt is
// hedged only by `suspect` reports to the other voters, never by sending the command to a second member.
//
// Every route takes `cold: true` to drop this isolate's cached configs, leaders, and records first, and `fresh: true`
// to read an owner record from its source rather than this isolate's cache.

interface Env {
  MEMBER_0: DurableObjectNamespace; MEMBER_1: DurableObjectNamespace; MEMBER_2: DurableObjectNamespace;
  REGISTRY_0: DurableObjectNamespace; REGISTRY_1: DurableObjectNamespace; REGISTRY_2: DurableObjectNamespace;
  TOKEN: string; GENESIS: string; EDGE_NAME: string;
}

type Record_ = { name: string; sites: number[]; home: number; generation: number; moving_from: Record_ | null; at: number };
type Voter = { site: number; do_name: string };
type NotLeader = { term: number; leader: number | null; config: { index: number; config: any } | null };
type Step = "cached" | `bootstrap${number}` | `registry${number}`;
type Trace = string[];

const CALL_DEADLINE = 1_000;
const COMMIT_HOLD = 1_000;
const OP_DEADLINE = 60_000;
const HINTS = ["enam", "weur", "apac"] as const;
const KINDS: Record<string, string[]> = { Tri: ["f", "f", "f"], Na3: ["f", "f", "f"], Us2w: ["f", "f", "w"], Eu2w: ["f", "f", "w"], Apac2w: ["f", "f", "w"] };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Per isolate: each chain's voters from its newest config, the leader its replies named, and owner records. */
const configs = new Map<string, { index: number; voters: Voter[] }>();
const leaders = new Map<string, number>();
const terms = new Map<string, number>();
const records = new Map<string, Record_>();

function forgetAll() { configs.clear(); leaders.clear(); terms.clear(); records.clear(); }

function bootstrapNames(chain: string, r: Record_): Voter[] {
  const kinds = KINDS[r.name] ?? r.sites.map(() => "f");
  return r.sites.map((site, slot) => ({ site, do_name: `${chain}#${r.generation}#${kinds[slot]}${slot}` }));
}

function* lookupOrder(r: Record_): Generator<Record_> {
  for (let x: Record_ | null = r; x; x = x.moving_from) yield x;
}

function learnConfig(chain: string, nl: NotLeader | null | undefined): void {
  const c = nl?.config;
  if (!c) return;
  const p = c.config.normal ?? c.config.degraded?.placement ?? c.config.joint?.new;
  if (!p?.voters) return;
  if ((configs.get(chain)?.index ?? -1) <= c.index) {
    configs.set(chain, { index: c.index, voters: p.voters.map((v: any) => ({ site: v.site, do_name: v.do_name })) });
  }
}

function learnMeta(chain: string, meta: any): void {
  if (!meta) return;
  terms.set(chain, Math.max(terms.get(chain) ?? 0, meta.term));
  if (meta.leader !== null && meta.leader !== undefined) leaders.set(chain, meta.leader);
}

async function sha256(bytes: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
}

/** `account_shard(owner)`: SHA-256 of the tag and the id as big-endian i64, first four bytes mod 1024. */
async function accountShard(owner: number): Promise<number> {
  const tag = new TextEncoder().encode("graftyard/names/account/");
  const b = new Uint8Array(tag.length + 8);
  b.set(tag);
  new DataView(b.buffer).setBigInt64(tag.length, BigInt(owner));
  const h = await sha256(b);
  return new DataView(h.buffer).getUint32(0) % 1024;
}

/** `MemberRegistry::for_chain(site, chain).do_name()`: the first hash byte of the chain's text mod 64. */
async function registryName(chain: string): Promise<string> {
  const h = await sha256(new TextEncoder().encode(chain));
  return `registry#${h[0] % 64}`;
}

function coloId(colo: string): number {
  let n = 0;
  for (const ch of colo.slice(0, 4)) n = n * 256 + ch.charCodeAt(0);
  return n;
}

/** A `RequestId`: a ULID, 48 bits of milliseconds then 80 random bits, as 26 Crockford base-32 characters. */
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

const OPERATOR = { account: null, key: null };

class Edge {
  /** `skip` names lookup steps to skip on chain `skipOn` only (BF-11 forces the later steps with it). */
  constructor(private env: Env, private colo: number, private skip: Set<string>, private skipOn: string, private fresh: boolean) {}

  private skips(chain: string, step: string): boolean {
    return chain === this.skipOn && this.skip.has(step);
  }

  private ns(site: number, kind: "MEMBER" | "REGISTRY"): DurableObjectNamespace {
    return this.env[`${kind}_${site}` as "MEMBER_0"];
  }

  /** One member call with a deadline: `null` is a missed receipt or a failed call, else the status and body. */
  async member(v: Voter, path: string, body?: unknown, deadline = CALL_DEADLINE): Promise<{ status: number; v: any } | null> {
    const ns = this.ns(v.site, "MEMBER");
    const ctl = new AbortController();
    const timer = setTimeout(() => ctl.abort(), deadline);
    try {
      const init: RequestInit = body === undefined ? { signal: ctl.signal } : { method: "POST", body: JSON.stringify(body), signal: ctl.signal };
      const res = await ns.get(ns.idFromName(v.do_name), { locationHint: HINTS[v.site] }).fetch(`https://member${path}`, init);
      const text = await res.text();
      let parsed: any = text;
      try { parsed = JSON.parse(text); } catch { /* text body */ }
      return { status: res.status, v: parsed };
    } catch {
      return null;
    } finally {
      clearTimeout(timer);
    }
  }

  async registryRow(chain: string, site: number): Promise<any | null> {
    const ns = this.ns(site, "REGISTRY");
    try {
      const res = await ns.get(ns.idFromName(await registryName(chain)), { locationHint: HINTS[site] })
        .fetch(`https://registry/row?chain=${encodeURIComponent(chain)}`);
      if (res.status !== 200) { await res.body?.cancel(); return null; }
      return await res.json();
    } catch {
      return null;
    }
  }

  // ---- records ----

  genesis(): Record<string, Record_> {
    return JSON.parse(this.env.GENESIS).placements;
  }

  /** The chain's record from its source. `authz:<owner>` and `identity:<owner>` read the owner's row on `Names`. */
  async record(chain: string, tr: Trace): Promise<Record_ | null> {
    const g = this.genesis();
    if (chain.startsWith("names:")) return g.names;
    if (chain.startsWith("tokens:")) return g.tokens;
    if (chain === "ids") return g.ids;
    if (chain.startsWith("authz:") || chain.startsWith("identity:")) {
      const cached = this.fresh ? undefined : records.get(chain);
      if (cached) return cached;
      const owner = Number(chain.slice(chain.indexOf(":") + 1));
      const names = `names:${await accountShard(owner)}`;
      const r = await this.read(names, { placement: owner }, tr);
      const rec = r.ok ? (r.value?.placement ?? null) : null;
      if (rec) records.set(chain, rec);
      return rec;
    }
    throw new Error(`no record source for ${chain}`);
  }

  // ---- lookup ----

  /** Candidates in the rule's order, each with the step that named it. Registry rows are read lazily. */
  async *candidates(chain: string, record: Record_): AsyncGenerator<[Voter, Step]> {
    const c = configs.get(chain);
    if (c) {
      const l = leaders.get(chain);
      for (const v of [...c.voters].sort((a, b) => Number(b.site === l) - Number(a.site === l))) yield [v, "cached"];
    }
    let depth = 0;
    for (const r of lookupOrder(record)) {
      if (!this.skips(chain, `bootstrap${depth}`)) for (const v of bootstrapNames(chain, r)) yield [v, `bootstrap${depth}`];
      if (!this.skips(chain, `registry@${depth}`)) {
        for (const s of r.sites) {
          const row = await this.registryRow(chain, s);
          if (row) yield [{ site: s, do_name: row.do_name }, `registry${depth}`];
        }
      }
      depth++;
    }
  }

  /** Finds a member of the group that answers `path` as its leader would: walks the rule, follows `NotLeader`, and
   *  skips members that are not joined or hold no config. Returns that member, its answer, and the step. */
  async find(chain: string, record: Record_, path: string, body: unknown, tr: Trace, deadline = CALL_DEADLINE):
    Promise<{ voter: Voter; v: any; step: Step } | null> {
    const tried = new Set<string>();
    const queue: [Voter, Step][] = [];
    let hedged = false;
    const gen = this.candidates(chain, record);
    for (;;) {
      let next = queue.shift();
      if (!next) {
        const n = await gen.next();
        if (n.done) return null;
        next = n.value;
      }
      const [v, step] = next;
      if (tried.has(v.do_name)) continue;
      tried.add(v.do_name);
      const r = await this.member(v, path, body, deadline);
      if (!r) { tr.push(`${v.do_name} (${step}): no answer`); continue; }
      if (r.status !== 200) { tr.push(`${v.do_name} (${step}): ${r.status}`); continue; }
      learnMeta(chain, r.v?.meta);
      const nl: NotLeader | undefined = r.v?.error?.notLeader ?? r.v?.done?.notLeader;
      if (nl) {
        learnConfig(chain, nl);
        if (!nl.config) { tr.push(`${v.do_name} (${step}): no config`); continue; }
        tr.push(`${v.do_name} (${step}): not leader, leader ${nl.leader}`);
        // A `NotLeader` naming no leader is reported, once per lookup, to every voter: a member whose promise lapsed
        // campaigns, and a restarted leader resumes its term. The report carries no command.
        let led: Voter | null = null;
        if (nl.leader === null && !hedged) { hedged = true; led = await this.hedge(chain, v, tr, true); }
        const voters = configs.get(chain)?.voters ?? [];
        const first = led?.site ?? nl.leader;
        const sorted = [...voters].sort((a, b) => Number(b.site === first) - Number(a.site === first));
        if (led) tried.delete(led.do_name);
        queue.unshift(...sorted.map((x) => [x, step] as [Voter, Step]));
        continue;
      }
      if (r.v?.busy) { tr.push(`${v.do_name}: busy`); tried.delete(v.do_name); await sleep(50); queue.push([v, step]); continue; }
      return { voter: v, v: r.v, step };
    }
  }

  /** The hedge: `suspect` about `about` to every other voter (every voter with `all`), in parallel. Returns the member that answered it
   *  leads. Carries no command. */
  async hedge(chain: string, about: Voter, tr: Trace, all = false): Promise<Voter | null> {
    const vs = (configs.get(chain)?.voters ?? []).filter((v) => all || v.site !== about.site);
    const term = terms.get(chain) ?? 0;
    const rs = await Promise.all(vs.map(async (v) => [v, await this.member(v, "/suspect", { colo: this.colo, about: about.site, term })] as const));
    for (const [v, r] of rs) {
      tr.push(`suspect ${about.site} to ${v.site}: ${r?.v?.verdict ?? "no reply"}`);
      if (r?.status === 200 && ["leader", "won", "resumed"].includes(r.v?.verdict)) { leaders.set(chain, v.site); return v; }
    }
    return null;
  }

  /** A fresh read through the group's leader. */
  async read(chain: string, q: unknown, tr: Trace): Promise<{ ok: boolean; value?: any; index?: number; step?: Step; error?: string; ms: number }> {
    const t0 = Date.now();
    const record = await this.record(chain, tr);
    if (!record) return { ok: false, error: "no record", ms: Date.now() - t0 };
    while (Date.now() - t0 < OP_DEADLINE) {
      const f = await this.find(chain, record, "/read", { q }, tr, 3 * CALL_DEADLINE + 500);
      if (f && "value" in f.v) return { ok: true, value: f.v.value, index: f.v.index, step: f.step, ms: Date.now() - t0 };
      if (f) tr.push(`read: ${JSON.stringify(f.v).slice(0, 200)}`);
      leaders.delete(chain);
      await sleep(250);
    }
    return { ok: false, error: "no group answered", ms: Date.now() - t0 };
  }

  /** One client write to an outcome, re-sent under `id`. The target is the member the rule finds; after a missed
   *  receipt the edge reports `suspect` and sends again to the same member unless another answered that it leads. */
  async commit(chain: string, cmd: unknown, id: string, tr: Trace): Promise<{ ok: boolean; outcome?: any; step?: Step; sends: number; error?: string; ms: number }> {
    const t0 = Date.now();
    const record = await this.record(chain, tr);
    if (!record) return { ok: false, error: "no record", sends: 0, ms: 0 };
    let sends = 0;
    let target: Voter | null = null;
    let step: Step | undefined;
    while (Date.now() - t0 < OP_DEADLINE) {
      sends++;
      let r: { status: number; v: any } | null;
      if (target === null) {
        // The lookup carries no command: `outcome_by_request` is answered only by the leader, and answers the
        // request's outcome if a re-send finds it already decided. The command goes to one member at a time.
        const f = await this.find(chain, record, "/outcome_by_request", { rid: id, cmd }, tr);
        if (!f) { tr.push("commit: no group answered"); await sleep(250); continue; }
        target = f.voter; step = f.step;
        if (f.v?.done && !f.v.done.notLeader) { leaders.set(chain, target.site); return { ok: true, outcome: f.v.done, step, sends, ms: Date.now() - t0 }; }
      }
      r = await this.member(target, "/commit", { rid: id, cmd }, COMMIT_HOLD + CALL_DEADLINE);
      if (r?.status === 200) learnMeta(chain, r.v?.meta);
      if (r === null || r.status !== 200) {
        tr.push(`commit to ${target.do_name}: ${r === null ? "missed" : r.status}`);
        const led = await this.hedge(chain, target, tr);
        if (led) target = led;
        else { await sleep(Math.min(1_000, 100 * 2 ** Math.min(sends, 4))); if (r !== null) target = null; }
        continue;
      }
      const nl: NotLeader | undefined = r.v?.error?.notLeader ?? r.v?.done?.notLeader;
      if (nl) { learnConfig(chain, nl); target = null; continue; }
      if (r.v?.error) return { ok: false, error: JSON.stringify(r.v.error), sends, ms: Date.now() - t0 };
      let done = r.v?.done;
      if (r.v?.admitted) {
        const o = await this.member(target, "/outcome", { ticket: r.v.admitted, wait_ms: 10_000 }, 11_000);
        if (o === null || o.status !== 200) { const led = await this.hedge(chain, target, tr); if (led) target = led; continue; }
        done = o.v?.done;
        if (!done || done.notLeader) { if (done?.notLeader) { learnConfig(chain, done.notLeader); target = null; } continue; }
      }
      leaders.set(chain, target.site);
      return { ok: true, outcome: done, step, sends, ms: Date.now() - t0 };
    }
    return { ok: false, error: `gave up after ${OP_DEADLINE} ms`, sends, ms: Date.now() - t0 };
  }

  /** Creates a chain's group from `record` (or the source's): joins its bootstrap members, optionally probes each
   *  one that holds no committed config with a vote and a read, then campaigns on the record's home until a member
   *  leads. A join the members refuse (another generation claimed the chain) falls back to the lookup. */
  async create(chain: string, given: Record_ | null, o: { probe?: boolean; no_claim?: boolean; campaign?: boolean; campaign_at?: number; lookup?: boolean }, tr: Trace) {
    const t0 = Date.now();
    const record = given ?? await this.record(chain, tr);
    if (!record) return { ok: false, error: "no record" };
    // Create on first use: only when the lookup finds no group.
    if (o.lookup !== false) {
      const existing = await this.find(chain, record, "/read", { q: {} }, tr, 3 * CALL_DEADLINE + 500);
      if (existing && "value" in existing.v) {
        return { ok: true, existed: true, record: { generation: record.generation }, group: { do_name: existing.voter.do_name, step: existing.step }, ms: Date.now() - t0 };
      }
    }
    const probe = !!o.probe, noClaim = !!o.no_claim;
    const names = bootstrapNames(chain, record);
    const joins = await Promise.all(names.map(async (v, slot) => {
      const me = { site: v.site, kind: "full", slot, generation: record.generation, do_name: v.do_name };
      const r = await this.member(v, "/join", { chain, me, record, ...(noClaim ? { no_claim: true } : {}) }, 30_000);
      return { site: v.site, status: r?.status ?? 0, refused: r?.status === 409 ? r.v?.refused ?? r.v : undefined };
    }));
    const probes: any[] = [];
    if (probe) {
      for (const [i, v] of names.entries()) {
        if (joins[i].status !== 200) continue;
        const pv = await this.member(v, "/probe_vote", {}, 5_000);
        if (pv?.status !== 200) { probes.push({ site: v.site, skipped: pv?.status ?? 0 }); continue; }
        probes.push({ site: v.site, granted: pv.v.granted, deny: pv.v.deny, served: pv.v.readServed, readConfig: pv.v.readConfig });
      }
    }
    let leader: number | null = null;
    let verdict = null;
    const ok = joins.filter((j) => j.status === 200).length;
    if (o.campaign === false) return { ok: true, joined: ok, joins, probes, record: { generation: record.generation }, ms: Date.now() - t0 };
    if (ok >= 2) {
      const home = names.find((v) => v.site === (o.campaign_at ?? record.home))!;
      for (let i = 0; i < 60 && leader === null; i++) {
        verdict = (await this.member(home, "/campaign", {}))?.v?.verdict ?? null;
        const st = await this.member(home, "/status");
        if (st?.v?.role === "leader") leader = home.site;
        else if (st?.v?.leader !== null && st?.v?.leader !== undefined && st?.v?.config) leader = st.v.leader;
        else await sleep(200);
      }
    }
    // Whether this edge built the group or another generation did, the group is what the rule finds.
    const f = await this.find(chain, record, "/read", { q: {} }, tr, 3 * CALL_DEADLINE + 500);
    return {
      ok: f !== null && "value" in (f?.v ?? {}),
      record: { generation: record.generation, home: record.home, moving: record.moving_from !== null },
      joins, probes, verdict, leader, ms: Date.now() - t0,
      group: f ? { do_name: f.voter.do_name, step: f.step } : null,
    };
  }

  /** Every name a chain's group may have on each site, generations 1 to `gens`: each member's status. */
  async census(chain: string, gens: number) {
    const out: any[] = [];
    await Promise.all([0, 1, 2].flatMap((site) => Array.from({ length: gens }, async (_, g) => {
      const v = { site, do_name: `${chain}#${g + 1}#f${site}` };
      const r = await this.member(v, "/status", undefined, 10_000);
      const s = r?.v ?? {};
      out.push({ site, gen: g + 1, joined: s.joined ?? null, doName: v.do_name, boot1: s.boot1 ?? null, commit: s.commit ?? null,
        config: s.config ? s.config.index : null, role: s.role ?? null, last: s.last ?? null });
    })));
    const rows = await Promise.all([0, 1, 2].map(async (site) => ({ site, row: await this.registryRow(chain, site) })));
    return { members: out.sort((a, b) => a.gen - b.gen || a.site - b.site), rows };
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body: Record<string, any> = req.method === "POST" ? await req.json() : {};
    if (body.cold) forgetAll();
    const colo = (req.cf as { colo?: string } | undefined)?.colo ?? "edge";
    const e = new Edge(env, coloId(colo), new Set(body.skip ?? []), String(body.chain ?? ""), !!body.fresh);
    const json = (v: unknown, status = 200) => Response.json(v, { status });
    const tr: Trace = [];
    const chain = String(body.chain ?? "");
    try {
      switch (url.pathname) {
        case "/whoami": return json({ edge: env.EDGE_NAME, colo });
        case "/create": return json({ edge: env.EDGE_NAME, ...(await e.create(chain, body.record ?? null, body, tr)), trace: body.trace ? tr : undefined });
        case "/commit": {
          const r = await e.commit(chain, body.cmd, body.rid ?? rid(), tr);
          return json({ edge: env.EDGE_NAME, ...r, trace: r.ok && !body.trace ? undefined : tr });
        }
        case "/commits": {
          // Several independent client requests on one chain, `conc` at a time, each with its own RequestId.
          const cmds: unknown[] = body.cmds;
          const out: any[] = new Array(cmds.length);
          let next = 0;
          await Promise.all(Array.from({ length: body.conc ?? 8 }, async () => {
            while (next < cmds.length) {
              const i = next++;
              const t: Trace = [];
              const r = await e.commit(chain, cmds[i], rid(), t);
              out[i] = { ...r, trace: r.ok ? undefined : t.slice(-12) };
            }
          }));
          return json({ edge: env.EDGE_NAME, results: out });
        }
        case "/read": {
          const r = await e.read(chain, body.q ?? {}, tr);
          return json({ edge: env.EDGE_NAME, ...r, trace: r.ok && !body.trace ? undefined : tr });
        }
        case "/record": return json({ record: await e.record(chain, tr), trace: tr });
        case "/commit_many": {
          // A batch to the group's leader in one call, for BF-15's queue. `leader` ({site, do_name}) names it for a
          // chain this edge has no record source for, such as a repository's (BF-52 over SSH, B0-24a), and `rids`
          // gives each command its request id, else one is minted.
          let lead: Voter | undefined = body.leader;
          if (!lead) {
            const record = await e.record(chain, tr);
            if (!record) return json({ error: "no record" }, 404);
            const f = await e.find(chain, record, "/read", { q: {} }, tr, 3 * CALL_DEADLINE + 500);
            if (!f) return json({ error: "no group", trace: tr }, 404);
            lead = f.voter;
          }
          const cmds = (body.cmds as unknown[]).map((cmd, i) => ({ rid: body.rids?.[i] ?? rid(), cmd }));
          const r = await e.member(lead, "/commit_many", { cmds, hold_deps_ms: body.hold_deps_ms, hold_ms: body.hold_ms }, (body.hold_ms ?? 1_000) + 30_000);
          return json({ leader: lead, rids: cmds.map((c) => c.rid), status: r?.status ?? null, ...(r?.v ?? {}) });
        }
        case "/member": {
          // A raw call to one member, by name or by the chain's generation and site.
          const v = { site: Number(body.site), do_name: body.do_name ?? `${chain}#${body.gen ?? 1}#f${body.site}` };
          const r = await e.member(v, body.path, body.body, body.deadline ?? 30_000);
          return json({ voter: v, ...(r ?? { status: null }) });
        }
        case "/leader": {
          const record = await e.record(chain, tr);
          if (!record) return json({ error: "no record" }, 404);
          const f = await e.find(chain, record, "/read", { q: {} }, tr, 3 * CALL_DEADLINE + 500);
          if (!f) return json({ error: "no group", trace: tr }, 404);
          return json({ leader: f.voter, step: f.step, config: configs.get(chain) ?? null, trace: body.trace ? tr : undefined });
        }
        case "/census": return json(await e.census(chain, body.gens ?? 3));
        case "/registry": return json(await e.registryRow(chain, Number(body.site)));
        case "/ssh_kill":
        case "/ssh_status": {
          // B0-24a (tests/m6/results/m6-wp2.md): a site's `SshFront` through its Registry object `ssh`, as graft-ops
          // reads it: `/ssh_kill` is the test-faults `POST /ssh_front_kill` (`{at, running, open}`), `/ssh_status`
          // the `GET /ssh_front` status. The front's answer passes through with its status.
          const site = Number(body.site);
          const ns = env[`REGISTRY_${site}` as "REGISTRY_0"];
          const stub = ns.get(ns.idFromName("ssh"), { locationHint: HINTS[site] });
          const kill = url.pathname === "/ssh_kill";
          const r = await stub.fetch(kill ? "https://registry/ssh_front_kill" : "https://registry/ssh_front", { method: kill ? "POST" : "GET" });
          const text = await r.text();
          let v: unknown = text;
          try { v = JSON.parse(text); } catch { /* text body */ }
          return json({ site, status: r.status, v });
        }
      }
    } catch (err) {
      return json({ error: String(err), stack: (err as Error)?.stack, trace: tr }, 500);
    }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;

export { OPERATOR };
