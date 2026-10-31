// M0-WP9, B0-34 (build/BUILD-PLAN.md): git CPU and in-band fan-out on real containers, `graft-wp9-git`, its own script
// and its own bucket `graft-wp9-git`, so it touches no other bed.
//
// - `Shard9`: one git cache shard per (site, name), a container-backed Durable Object under the `durable_object`
//   policy (design/DESIGN.md, "Git cache shards (derived)"). It starts `wp9/git/Dockerfile` (git plus the shard server
//   in `wp9/git/main.go`) as `standard-4` with `enableInternet: false`, rehydrates a network by streaming its packs
//   from the bucket into the container (as B0-22), and forwards git work to the container's class-ordered work queue.
// - `Net9`: stands for a network group's leader. Its alarm reads `NetworkLoad` (the most loaded of its k shards' CPU
//   over 10 s, and their queue depth) and sets k per warm site with hysteresis, at most FANOUT_MAX, drawing extra
//   shards from the site's hot pool. No Cloudflare API call: shards start with `ctx.container.start()`.
// - `Pool9`: one per site, the hot pool of HOT_POOL slots with a per-owner budget.
// - `Drive9`: a load client created with a location hint and run from its alarm (as `src/rehydrate.ts` `ciStart`).
//
// Accelerated hysteresis, outside the design: up after 30 s above 0.75 (design 3 min), down after 120 s below 0.4
// (design 1 h), so a 25-minute run sees several decisions. The fetch guard below fails and counts any request to the
// Cloudflare API, which the gate requires to be zero.

import { DurableObject } from "cloudflare:workers";

interface Env {
  SHARD: DurableObjectNamespace<Shard9>; NET: DurableObjectNamespace<Net9>; POOL: DurableObjectNamespace<Pool9>;
  DRIVE: DurableObjectNamespace<Drive9>; GIT: R2Bucket; M0_TOKEN: string;
}

const FANOUT_MAX = 8, HOT_POOL = 7, COUNT = 2, UP = 0.75, DOWN = 0.4, UP_MS = 30_000, DOWN_MS = 120_000, TICK_MS = 10_000;
export type Site = "a" | "b" | "c";
const HINT: Record<Site, DurableObjectLocationHint> = { a: "enam", b: "weur", c: "apac" };
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ---- the API guard: the gate's "any Cloudflare API call" ----
let apiCalls = 0;
const realFetch = globalThis.fetch;
globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) => {
  const u = new URL(input instanceof Request ? input.url : String(input));
  if (u.hostname === "api.cloudflare.com") { apiCalls++; return Promise.reject(new Error("API unreachable (wp9 guard)")); }
  return realFetch(input, init);
}) as typeof fetch;

/** Rendezvous weight of (key, slot). */
function weight(key: string, slot: string): number {
  let h = 2166136261;
  for (const ch of `${key}|${slot}`) h = Math.imul(h ^ ch.charCodeAt(0), 16777619);
  return h >>> 0;
}

// ---- Shard9 ----

type Ctr = NonNullable<DurableObjectState["container"]> & { images?: Record<string, string> };

export class Shard9 extends DurableObject<Env> {
  private get c(): Ctr { return this.ctx.container as Ctr; }
  private nets = new Map<string, Promise<object>>();

  private async up(): Promise<number> {
    const t = Date.now();
    if (!this.c.running) this.c.start({ image: this.c.images!.git, instance: "standard-4", enableInternet: false, env: { SLOTS: "4", WP9_MISS_AT_BUILT: "1" } } as ContainerStartupOptions);
    await this.c.setInactivityTimeout(40 * 60_000);
    const port = this.c.getTcpPort(8080);
    for (let i = 0; i < 300; i++) {
      try { if ((await port.fetch("http://container/health")).ok) return Date.now() - t; } catch { /* starting */ }
      await sleep(200);
    }
    throw new Error("container did not answer on 8080 within 60 s");
  }

  /** Makes `net` usable on this shard: start the container, stream `net/<src>/` from the bucket, set up the repository. */
  /** `tip` (third pass): the network's tip index now; setup builds base and parts up to it before the shard is ready. */
  async ensure(net: string, src: string, tip = 0): Promise<object> {
    if (!this.c.running) this.nets.clear(); // a stopped container lost its disk
    let p = this.nets.get(net);
    if (!p) {
      p = this.rehydrate(net, src, tip).catch((e) => { this.nets.delete(net); return { error: String(e).slice(0, 300) }; });
      this.nets.set(net, p);
    }
    return p;
  }

  private async rehydrate(net: string, src: string, tip: number): Promise<object> {
    const t0 = Date.now();
    const startMs = await this.up();
    const port = this.c.getTcpPort(8080);
    const have = await (await port.fetch("http://container/load")).json<{ nets: string[] }>();
    if (have.nets.includes(net)) return { net, startMs, already: true };
    const keys: { key: string; size: number }[] = [];
    let cursor: string | undefined;
    do {
      const l = await this.env.GIT.list({ prefix: `net/${src}/`, cursor });
      for (const o of l.objects) keys.push({ key: o.key, size: o.size });
      cursor = l.truncated ? l.cursor : undefined;
    } while (cursor);
    keys.sort((x, y) => y.size - x.size);
    const tCopy = Date.now();
    let next = 0;
    await Promise.all(Array.from({ length: 4 }, async () => {
      while (next < keys.length) {
        const k = keys[next++];
        const obj = await this.env.GIT.get(k.key);
        if (!obj) throw new Error(`missing ${k.key}`);
        const r = await port.fetch(`http://container/f?path=${encodeURIComponent(`in/${net}/${k.key.split("/").pop()}`)}`, { method: "PUT", body: obj.body });
        if (!r.ok) throw new Error(`${k.key}: ${r.status} ${await r.text()}`);
      }
    }));
    const copyMs = Date.now() - tCopy;
    const s = await port.fetch(`http://container/setup?net=${net}&tip=${tip}`, { method: "POST" });
    if (!s.ok) throw new Error(`setup: ${s.status} ${await s.text()}`);
    const bytes = keys.reduce((n, k) => n + k.size, 0);
    return { net, startMs, objects: keys.length, bytes, copyMs, copyMBps: Math.round(bytes / 1e3 / Math.max(1, copyMs)), totalMs: Date.now() - t0, setup: await s.json() };
  }

  async load(): Promise<object> {
    if (!this.c.running) return { running: false };
    try { return await (await this.c.getTcpPort(8080).fetch("http://container/load")).json(); } catch (e) { return { error: String(e).slice(0, 200) }; }
  }

  async kill(): Promise<void> { this.nets.clear(); if (this.c.running) await this.c.destroy(); }

  /** Git work: forwards `/op?...` to the container and streams the reply. */
  async fetch(req: Request): Promise<Response> {
    // 502, not 503: the container's queue answers 503 only when it sheds agent work.
    if (!this.c.running) return new Response("container not running", { status: 502 });
    const u = new URL(req.url);
    try { return await this.c.getTcpPort(8080).fetch(`http://container/op${u.search}`, { method: "POST" }); }
    catch (e) { return new Response(`container: ${String(e).slice(0, 200)}`, { status: 502 }); }
  }
}

// ---- Pool9: one per site ----

export class Pool9 extends DurableObject<Env> {
  /** Grants up to `want` free slots to `net`, in rendezvous order of (net, slot), while `owner` holds under `budget`. */
  async take(net: string, owner: string, budget: number, want: number): Promise<{ granted: number[]; denied: number; why?: string }> {
    const held = (await this.ctx.storage.get<Record<number, { net: string; owner: string }>>("slots")) ?? {};
    const ownerHeld = Object.values(held).filter((s) => s.owner === owner).length;
    const free = Array.from({ length: HOT_POOL }, (_, i) => i).filter((i) => !held[i]).sort((x, y) => weight(net, String(y)) - weight(net, String(x)));
    const n = Math.max(0, Math.min(want, free.length, budget - ownerHeld));
    const granted = free.slice(0, n);
    for (const i of granted) held[i] = { net, owner };
    await this.ctx.storage.put("slots", held);
    const why = n < want ? (budget - ownerHeld < want && budget - ownerHeld <= free.length ? "owner budget" : "pool full") : undefined;
    return { granted, denied: want - n, why };
  }
  async give(net: string, slots: number[]): Promise<void> {
    const held = (await this.ctx.storage.get<Record<number, { net: string; owner: string }>>("slots")) ?? {};
    for (const i of slots) if (held[i]?.net === net) delete held[i];
    await this.ctx.storage.put("slots", held);
  }
  async view(): Promise<object> { return (await this.ctx.storage.get("slots")) ?? {}; }
  async reset(): Promise<void> { await this.ctx.storage.deleteAll(); }
}

// ---- Net9: the network's leader, fanout in band ----

/** Third pass: `alike` sizes every warm site from the larger of the warm sites' `NetworkLoad` (the adopted rule), and
 *  `upMs`/`downMs` set the hysteresis (default the accelerated 30 s and 120 s; the design's is 180 s and 1 h). */
interface NetCfg { net: string; src: string; owner: string; budget: number; warm: Site[]; spare: Site[]; run: string; alike?: boolean; upMs?: number; downMs?: number;
  /** The tip clock (third pass): tip index = (now - tipStart) / 1000 * tipPerS. */
  tipStart?: number; tipPerS?: number }
interface SiteState { base: string; pool: number[]; ready: string[]; highSince: number; lowSince: number; dead?: boolean }
interface NetState { cfg: NetCfg; sites: Partial<Record<Site, SiteState>>; timeline: object[]; denials: object[] }

export class Net9 extends DurableObject<Env> {
  private st?: NetState;
  private ensuring = new Set<string>();

  private shard(site: Site, name: string) { return this.env.SHARD.get(this.env.SHARD.idFromName(`${this.st!.cfg.run}-${site}-${name}`), { locationHint: HINT[site] }); }
  private shardNames(site: Site, s: SiteState) { return [s.base, ...s.pool.map((i) => `pool-${i}`)]; }
  private async save() { await this.ctx.storage.put("st", this.st!); }
  private async load(): Promise<NetState | undefined> { return (this.st ??= await this.ctx.storage.get<NetState>("st")); }

  async init(cfg: NetCfg): Promise<void> {
    const base = `base-${weight(cfg.net, "count") % COUNT}`;
    this.st = { cfg, sites: {}, timeline: [], denials: [] };
    for (const s of cfg.warm) this.st.sites[s] = { base, pool: [], ready: [], highSince: 0, lowSince: 0 };
    await this.save();
    for (const s of cfg.warm) this.start(s, base);
    await this.ctx.storage.setAlarm(Date.now() + TICK_MS);
  }

  private start(site: Site, name: string): void {
    const key = `${site}/${name}`;
    if (this.ensuring.has(key)) return;
    this.ensuring.add(key);
    const t = Date.now();
    const c = this.st!.cfg, tip = c.tipPerS && c.tipStart ? Math.max(0, Math.floor(((Date.now() - c.tipStart) / 1000) * c.tipPerS)) : 0;
    void this.shard(site, name).ensure(c.net, c.src, tip).then(async (r) => {
      this.ensuring.delete(key);
      const s = this.st!.sites[site];
      if (s && !s.dead && !("error" in r) && !s.ready.includes(name) && this.shardNames(site, s).includes(name)) s.ready.push(name);
      this.st!.timeline.push({ at: Date.now(), ev: "ready", site, name, ms: Date.now() - t, r });
      await this.save();
    }, async (e) => { this.ensuring.delete(key); this.st!.timeline.push({ at: Date.now(), ev: "ensure failed", site, name, e: String(e).slice(0, 200) }); await this.save(); });
  }

  async alarm(): Promise<void> {
    const st = await this.load();
    if (!st) return;
    await this.ctx.storage.setAlarm(Date.now() + TICK_MS);
    const now = Date.now(), upMs = st.cfg.upMs ?? UP_MS, downMs = st.cfg.downMs ?? DOWN_MS;
    const live = (Object.keys(st.sites) as Site[]).filter((site) => !st.sites[site]!.dead);
    // NetworkLoad per warm site: the most loaded of its ready shards, CPU over the last 10 s, and their queue depth.
    const per = new Map<Site, { cpu: number; queue: number }>();
    await Promise.all(live.map(async (site) => {
      const s = st.sites[site]!;
      const loads = await Promise.all(s.ready.map((n) => this.shard(site, n).load().catch(() => ({})) as Promise<{ cpu10?: number; queue?: Record<string, number>; running?: boolean }>));
      // A shard whose container stopped (crash, eviction) leaves `ready` and is rehydrated, as the design's shard object
      // restarts its container on the next request.
      const lost = s.ready.filter((_, i) => loads[i].running === false);
      if (lost.length) {
        s.ready = s.ready.filter((n) => !lost.includes(n));
        for (const n of lost) { st.timeline.push({ at: now, ev: "container lost", site, name: n }); this.start(site, n); }
      }
      per.set(site, { cpu: Math.max(0, ...loads.map((l) => l.cpu10 ?? 0)), queue: loads.reduce((a, l) => a + Object.values(l.queue ?? {}).reduce((x, y) => x + y, 0), 0) });
    }));
    // The adopted rule: with `alike`, every warm site is judged by the larger of the sites' loads, so the second warm
    // site holds the same k as the loaded one and a site loss finds k shards already warm there.
    const top = [...per.values()].reduce((m, l) => (l.cpu > m.cpu ? l : m), { cpu: 0, queue: 0 });
    const topK = Math.max(1, ...live.map((site) => 1 + st.sites[site]!.pool.length));
    for (const site of live) {
      const s = st.sites[site]!;
      const own = per.get(site)!;
      const cpu = st.cfg.alike ? top.cpu : own.cpu;
      const k = 1 + s.pool.length;
      s.highSince = cpu > UP ? (s.highSince || now) : 0;
      s.lowSince = cpu < DOWN ? (s.lowSince || now) : 0;
      let want = k;
      // With `alike` the busiest site's k is the reference: k * cpu over UP of it, so both sites converge on one k.
      const refK = st.cfg.alike ? topK : k;
      if (s.highSince && now - s.highSince >= upMs) want = Math.min(FANOUT_MAX, Math.max(refK + 1, Math.ceil((refK * cpu) / UP)));
      else if (st.cfg.alike && k < topK && !s.lowSince) want = topK; // a site behind the other catches up at once
      else if (s.lowSince && now - s.lowSince >= downMs && k > 1) want = k - 1;
      if (want > k) {
        const pool = this.env.POOL.get(this.env.POOL.idFromName(`${st.cfg.run}-${site}`), { locationHint: HINT[site] });
        const g = await pool.take(st.cfg.net, st.cfg.owner, st.cfg.budget, want - k);
        s.pool.push(...g.granted);
        for (const i of g.granted) this.start(site, `pool-${i}`);
        if (g.denied) st.denials.push({ at: now, site, asked: want - k, denied: g.denied, why: g.why });
        s.highSince = 0;
      } else if (want < k) {
        const i = s.pool.pop()!;
        s.ready = s.ready.filter((n) => n !== `pool-${i}`);
        await this.env.POOL.get(this.env.POOL.idFromName(`${st.cfg.run}-${site}`), { locationHint: HINT[site] }).give(st.cfg.net, [i]);
        s.lowSince = 0;
        // The freed shard keeps the network on disk; a real shard would evict it by LRU. Not stopped here, because
        // other networks may hold the same pool slot later.
      }
      st.timeline.push({ at: now, site, cpu: Math.round(own.cpu * 100) / 100, judged: Math.round(cpu * 100) / 100, queue: own.queue, k: 1 + s.pool.length, ready: s.ready.length });
    }
    st.timeline.push({ at: now, apiCalls });
    await this.save();
  }

  /** Kills a warm site: its shards for this network are destroyed and the next spare site becomes warm. */
  async killSite(site: Site): Promise<object> {
    const st = (await this.load())!;
    const s = st.sites[site];
    if (!s || s.dead) return { already: true };
    s.dead = true;
    s.ready = [];
    await this.env.POOL.get(this.env.POOL.idFromName(`${st.cfg.run}-${site}`), { locationHint: HINT[site] }).give(st.cfg.net, s.pool);
    const spare = st.cfg.spare.find((x) => !st.sites[x]);
    if (spare) { st.sites[spare] = { base: s.base, pool: [], ready: [], highSince: 0, lowSince: 0 }; this.start(spare, s.base); }
    // With `alike`, the spare is brought to the surviving sites' k on the next tick (it catches up at once).
    st.timeline.push({ at: Date.now(), ev: "site killed", site, spare });
    await this.save();
    return { killed: site, spare };
  }

  async view(): Promise<{ sites: Partial<Record<Site, { ready: string[]; k: number; dead?: boolean }>> }> {
    const st = await this.load();
    const out: Partial<Record<Site, { ready: string[]; k: number; dead?: boolean }>> = {};
    for (const [site, s] of Object.entries(st?.sites ?? {})) out[site as Site] = { ready: s!.ready, k: 1 + s!.pool.length, dead: s!.dead };
    return { sites: out };
  }
  async dump(): Promise<object | undefined> { return await this.load(); }
  async setTip(tipStart: number, tipPerS: number): Promise<void> { const st = (await this.load())!; st.cfg.tipStart = tipStart; st.cfg.tipPerS = tipPerS; await this.save(); }
  async stop(): Promise<void> { await this.ctx.storage.deleteAlarm(); }
}

// ---- Drive9: load clients ----

/** One stream of git work. `site` "near" routes to the client's own warm site, else the network's home site. */
export interface Stream { net: string; kind: "push" | "human" | "merge" | "miss" | "hit"; cls: string; rate: number; from: number; to: number; mode?: string; near?: boolean }
/** `bed` names the networks and shards; `run` names this phase's clients. */
export interface DriveSpec { bed: string; run: string; index: number; clients: number; start: number; seconds: number; site: Site; home: Record<string, Site>; streams: Stream[]; args: Record<string, { commits: string[]; merges: string[] }>;
  /** Third pass: tip moves a second on `linux` (the merge queue's 10 a minute); each request names the tip's position. */
  tipPerS?: number }

/** [t ms, stream, code 0 ok 1 shed 2 error 3 no shard, ms, ttfb ms, queue ms, cpu s, site 0/1/2] */
type Row = number[];

export class Drive9 extends DurableObject<Env> {
  async startRun(spec: DriveSpec): Promise<void> {
    await this.ctx.storage.deleteAll();
    await this.ctx.storage.put("spec", spec);
    await this.ctx.storage.setAlarm(Math.max(Date.now(), spec.start));
  }
  /** All segments' rows, once the last segment has run. */
  async result(): Promise<object | undefined> {
    if (!(await this.ctx.storage.get("done"))) return undefined;
    const segs = [...(await this.ctx.storage.list<{ colo: string; rows: Row[]; errs: Record<string, number> }>({ prefix: "seg-" })).values()];
    const errs: Record<string, number> = {};
    for (const s of segs) for (const [e, c] of Object.entries(s.errs)) errs[e] = (errs[e] ?? 0) + c;
    return { colo: segs[0]?.colo, rows: segs.flatMap((s) => s.rows), errs, segments: segs.length };
  }

  /** One 20-minute alarm lost its first 15 minutes of rows in r1 (likely the alarm handler's wall-time limit), so the
   *  client runs in segments of at most SEG_MS, each stored as it ends, and re-arms its alarm for the next. A Poisson
   *  schedule restarted at a segment boundary is the same process, so nothing is skipped or doubled. */
  async alarm(): Promise<void> {
    const SEG_MS = 9 * 60_000;
    const spec = await this.ctx.storage.get<DriveSpec>("spec");
    if (!spec || (await this.ctx.storage.get("done"))) return;
    const segStart = Math.max(Date.now(), spec.start), seg = Math.round((segStart - spec.start) / 1000);
    const segEnd = Math.min(spec.start + spec.seconds * 1000, segStart + SEG_MS);
    if (await this.ctx.storage.get(`seg-${String(seg).padStart(6, "0")}`)) return;
    const colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] ?? "?";
    const rows: Row[] = [], errs: Record<string, number> = {};
    const views = new Map<string, { at: number; v: Awaited<ReturnType<Net9["view"]>> }>();
    const view = async (net: string) => {
      const c = views.get(net);
      if (c && Date.now() - c.at < 5_000) return c.v;
      const v = await this.env.NET.get(this.env.NET.idFromName(`${spec.bed}-${net}`)).view();
      views.set(net, { at: Date.now(), v });
      return v;
    };
    const end = segEnd;
    const inflight = new Set<Promise<void>>();
    let seq = 0;
    const one = async (si: number, st: Stream, t0: number) => {
      const n = seq++;
      const args = spec.args[st.net === "linux" ? "linux" : "git"];
      try {
        if (st.kind === "hit") {
          // A clone-cache hit: the edge streams a cached pack from the bucket.
          // Third pass: `linux`'s base.pack went up in byte parts, so its first part stands for the cached pack.
          const src = st.net === "linux" ? "linux" : "git";
          const o = (await this.env.GIT.get(`net/${src}/base.pack`)) ?? (await this.env.GIT.get(`net/${src}/base.pack.part000`));
          const rd = o!.body.getReader();
          const first = await rd.read();
          const ttfb = Date.now() - t0;
          await rd.cancel();
          rows.push([t0 - spec.start, si, first.done ? 2 : 0, Date.now() - t0, ttfb, 0, 0, 0]);
          return;
        }
        const v = await view(st.net);
        const live = (Object.entries(v.sites) as [Site, { ready: string[]; dead?: boolean }][]).filter(([, s]) => !s.dead && s.ready.length);
        const pick = (st.near ? live.find(([s]) => s === spec.site) : undefined) ?? live.find(([s]) => s === spec.home[st.net]) ?? live[0];
        if (!pick) { rows.push([t0 - spec.start, si, 3, Date.now() - t0, -1, 0, 0, -1]); return; }
        const [site, s] = pick;
        const name = s.ready.reduce((b, x) => (weight(`${spec.index}-${n}`, x) > weight(`${spec.index}-${n}`, b) ? x : b), s.ready[0]);
        const arg = st.kind === "merge" ? args.merges[n % args.merges.length] : args.commits[(spec.index * 7919 + n) % args.commits.length];
        const stub = this.env.SHARD.get(this.env.SHARD.idFromName(`${spec.bed}-${site}-${name}`));
        const tip = st.net === "linux" && spec.tipPerS ? Math.max(0, Math.floor(((Date.now() - spec.start) / 1000) * spec.tipPerS)) : 0;
        const r = await stub.fetch(`http://shard/op?net=${st.net}&kind=${st.kind}&cls=${st.cls}&mode=${st.mode ?? ""}&tip=${tip}&arg=${encodeURIComponent(arg)}`, { signal: AbortSignal.timeout(180_000) });
        const code = r.status === 503 ? 1 : r.ok ? 0 : 2;
        let ttfb = -1, text = "";
        if (r.body) {
          const rd = r.body.getReader(), dec = new TextDecoder();
          for (;;) {
            const { done, value } = await rd.read();
            if (done) break;
            if (ttfb < 0) ttfb = Date.now() - t0;
            text = (text + dec.decode(value, { stream: true })).slice(-2000);
          }
        }
        let stats: { queueMs?: number; cpu?: number; recvCpu?: number; mergeCpu?: number } = {};
        const j = st.kind === "miss" ? text.split("WP9STATS")[1] : text;
        try { if (code === 0 && j) stats = JSON.parse(j); } catch { /* not json */ }
        if (code === 2) errs[text.slice(0, 80)] = (errs[text.slice(0, 80)] ?? 0) + 1;
        const cpu = stats.cpu ?? (stats.recvCpu ?? 0) + (stats.mergeCpu ?? 0);
        rows.push([t0 - spec.start, si, code, Date.now() - t0, ttfb, stats.queueMs ?? -1, Math.round(cpu * 1000) / 1000, "abc".indexOf(site)]);
      } catch (e) {
        errs[String(e).slice(0, 80)] = (errs[String(e).slice(0, 80)] ?? 0) + 1;
        rows.push([t0 - spec.start, si, 2, Date.now() - t0, -1, -1, 0, -1]);
      }
    };
    // One timer per stream: exponential gaps at this client's share of the stream's rate.
    await Promise.all(spec.streams.map(async (st, si) => {
      const rate = st.rate / spec.clients;
      if (rate <= 0) return;
      let at = Math.max(segStart, spec.start + st.from * 1000) + (-Math.log(1 - Math.random()) * 1000) / rate;
      const stop = Math.min(end, spec.start + st.to * 1000);
      while (at < stop) {
        if (at > Date.now()) await sleep(at - Date.now());
        if (inflight.size < 300) {
          const p = one(si, st, Date.now());
          inflight.add(p);
          void p.finally(() => inflight.delete(p));
        } else rows.push([Date.now() - spec.start, si, 2, 0, -1, -1, 0, -1]);
        at += (-Math.log(1 - Math.random()) * 1000) / rate;
      }
    }));
    await Promise.all(inflight);
    await this.ctx.storage.put(`seg-${String(seg).padStart(6, "0")}`, { colo, rows, errs });
    if (segEnd >= spec.start + spec.seconds * 1000) await this.ctx.storage.put("done", true);
    else await this.ctx.storage.setAlarm(segEnd);
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
    const net = (n: string) => env.NET.get(env.NET.idFromName(`${body.bed}-${n}`));
    switch (url.pathname) {
      case "/init": await net(body.cfg.net).init({ ...body.cfg, run: body.bed }); return Response.json({ ok: true });
      case "/tip": await net(body.net).setTip(body.tipStart, body.tipPerS); return Response.json({ ok: true });
      case "/view": return Response.json(await Promise.all((body.nets as string[]).map(async (n) => ({ net: n, ...(await net(n).dump()) }))));
      case "/kill": {
        const out = await Promise.all((body.nets as string[]).map((n) => net(n).killSite(body.site)));
        // Destroy every container of the site: its base shards and its whole pool, as a site loss would.
        const names = [...Array.from({ length: COUNT }, (_, i) => `base-${i}`), ...Array.from({ length: HOT_POOL }, (_, i) => `pool-${i}`)];
        await Promise.all(names.map((n) => env.SHARD.get(env.SHARD.idFromName(`${body.bed}-${body.site}-${n}`)).kill().catch(() => undefined)));
        return Response.json(out);
      }
      case "/start": {
        const spec = body.spec as Omit<DriveSpec, "index" | "site">, sites = body.sites as Site[];
        await Promise.all(Array.from({ length: spec.clients }, (_, i) => env.DRIVE.get(env.DRIVE.idFromName(`${spec.run}-d${i}`), { locationHint: HINT[sites[i % sites.length]] })
          .startRun({ ...spec, index: i, site: sites[i % sites.length] })));
        return Response.json({ started: spec.clients });
      }
      case "/result": return Response.json(await Promise.all(Array.from({ length: body.clients as number }, (_, i) => env.DRIVE.get(env.DRIVE.idFromName(`${body.run}-d${i}`)).result())));
      case "/load": {
        const out: Record<string, object> = {};
        for (const site of ["a", "b", "c"] as Site[]) for (const n of [...Array.from({ length: COUNT }, (_, i) => `base-${i}`), ...Array.from({ length: HOT_POOL }, (_, i) => `pool-${i}`)])
          out[`${site}-${n}`] = await env.SHARD.get(env.SHARD.idFromName(`${body.bed}-${site}-${n}`)).load();
        const pools: Record<string, object> = {};
        for (const site of ["a", "b", "c"]) pools[site] = await env.POOL.get(env.POOL.idFromName(`${body.bed}-${site}`)).view();
        return Response.json({ shards: out, pools });
      }
      case "/stop": {
        await Promise.all((body.nets as string[]).map((n) => net(n).stop()));
        for (const site of ["a", "b", "c"] as Site[]) {
          await Promise.all([...Array.from({ length: COUNT }, (_, i) => `base-${i}`), ...Array.from({ length: HOT_POOL }, (_, i) => `pool-${i}`)]
            .map((n) => env.SHARD.get(env.SHARD.idFromName(`${body.bed}-${site}-${n}`)).kill().catch(() => undefined)));
          await env.POOL.get(env.POOL.idFromName(`${body.bed}-${site}`)).reset();
        }
        return Response.json({ stopped: true });
      }
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
