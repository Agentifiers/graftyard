// BF-50's scratch Worker on graft-m6a (`bf50.sh`; `tests/m6/results/m6-wp1.md`, "BF-50"). A separate script that binds
// every site's `Member` and `Registry` classes with `script_name`, spares included, so deploying it creates no site
// version. It reads, sets the test-faults site fault, and drives a light load; it signs nothing and calls no
// Cloudflare API.
//
// Routes (bearer token), every body JSON:
//   POST /snapshot {sites, shards, from, count}   every registry row on `sites`' shards [from, from+count), and each
//                                                 chain's committed config (`/status` of the row's member, then of the
//                                                 member a `notLeader` names): `{rows, chains: {chain: {index, cfg}}}`
//   POST /configs {chains: [{chain, site, do_name}]}  each chain's committed config, as /snapshot reads it
//   POST /fault {site, for_ms} | {off: true}      BM-1's site fault (`SiteFault`), on every site's `faults` registry
//   GET  /fault                                    each site's current plan
//   POST /create {chains, home, name?, sites?}    test-register repo chains (no owner), Tri on [0, 1, 2] at `home`, as
//                                                 M1-WP2's edge creates them, then one warm `put`; `{"name": "Solo",
//                                                 "sites": [1], "home": 1}` makes a control chain with no voter on 0
//   POST /read {chains: [{chain, site, do_name}], key} each chain's register value at `key`, read-index on its leader
//   POST /load {chains: [{chain, site, do_name}], seconds, rate}
//                                                 `put` commands at `rate` per second on random chains, through the
//                                                 member each chain's config names as leader; the members' own calls
//                                                 feed `SiteHealth`. `last` is each chain's last acknowledged value
//   POST /view {sites}                             the `Release` group's pages from each named site's member, at the
//                                                 generation its config names: `[{site, pages}]` for `relview`
//   POST /member {site, do_name, path, body}       one raw call to a member
//   POST /op {at: {site, do_name}, cmd}            one operator command through the chain's leader (the R66 probe's
//                                                 seed only): `{done, by}`
//   GET  /health?since=MS[&site=N]                 each voter site's `site_health` sums since MS

interface Env {
  [k: string]: unknown;
  TOKEN: string;
  SITES: string;
}

type Site = { id: number; region: string; spare: boolean; script: string };
type M = { site: number; do_name: string };

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** A `RequestId`: a ULID, 48 bits of milliseconds then 80 random bits, as 26 Crockford base-32 characters. */
function rid(): string {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

function sites(env: Env): Site[] {
  return JSON.parse(env.SITES);
}

function stub(env: Env, cls: "MEMBER" | "REGISTRY", site: number, name: string): DurableObjectStub {
  const s = sites(env).find((x) => x.id === site)!;
  const ns = env[`${cls}_${site}`] as DurableObjectNamespace;
  return ns.get(ns.idFromName(name), { locationHint: s.region as DurableObjectLocationHint });
}

async function call(env: Env, cls: "MEMBER" | "REGISTRY", site: number, name: string, path: string, body?: unknown,
  ms = 5_000): Promise<{ status: number; v: any }> {
  try {
    const init: RequestInit = body === undefined ? {} : { method: "POST", body: JSON.stringify(body) };
    const r = await Promise.race([
      stub(env, cls, site, name).fetch(`https://x${path}`, init),
      sleep(ms).then(() => null),
    ]);
    if (!r) return { status: 0, v: "timeout" };
    const t = await r.text();
    try {
      return { status: r.status, v: JSON.parse(t) };
    } catch {
      return { status: r.status, v: t.slice(0, 200) };
    }
  } catch (e) {
    return { status: 0, v: String(e).slice(0, 200) };
  }
}

async function pool<T, R>(xs: T[], n: number, f: (x: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(xs.length);
  let i = 0;
  await Promise.all(Array.from({ length: Math.min(n, xs.length) }, async () => {
    while (i < xs.length) { const k = i++; out[k] = await f(xs[k]); }
  }));
  return out;
}

function voters(cfg: any): M[] {
  const c = cfg?.config ?? cfg;
  const halves = c?.normal ? [c.normal] : c?.degraded ? [c.degraded.placement] : c?.joint ? [c.joint.old, c.joint.new] : [];
  const out: M[] = [];
  for (const p of halves) for (const v of p.voters ?? []) if (!out.some((m) => m.do_name === v.do_name)) out.push(v);
  return out;
}

/** A chain's committed config from its members' `/status`: the highest index any answering member reports. */
async function config(env: Env, first: M): Promise<{ index: number; cfg: any; leader: number | null } | { error: string }> {
  const tried = new Set<string>();
  let best: { index: number; cfg: any; leader: number | null } | null = null;
  const queue: M[] = [first];
  while (queue.length && tried.size < 6) {
    const m = queue.shift()!;
    if (tried.has(m.do_name)) continue;
    tried.add(m.do_name);
    const r = await call(env, "MEMBER", m.site, m.do_name, "/status");
    const st = r.v;
    const c = st?.config;
    if (r.status === 200 && c && typeof c.index === "number") {
      if (!best || c.index > best.index) best = { index: c.index, cfg: c.config, leader: st.leader ?? null };
      for (const v of voters(c.config)) queue.push(v);
    }
  }
  return best ?? { error: `no config from ${[...tried].join(",")}` };
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body: any = req.method === "POST" ? await req.json() : {};
    const json = (v: unknown) => Response.json(v);
    try {
      switch (url.pathname) {
        case "/snapshot": {
          const rows: any[] = [];
          const shards = Array.from({ length: body.count ?? 8 }, (_, i) => (body.from ?? 0) + i);
          for (const s of body.sites as number[]) {
            await pool(shards, 6, async (sh) => {
              const r = await call(env, "REGISTRY", s, `registry#${sh}`, "/all_rows", undefined, 15_000);
              if (r.status !== 200) throw new Error(`rows ${s}/${sh}: ${r.status} ${JSON.stringify(r.v).slice(0, 120)}`);
              for (const row of r.v) rows.push({ ...row, site: s });
            });
          }
          return json({ rows });
        }
        case "/joined": {
          // Each named member's `/status`: whether it has joined, so `before` can exclude a chain with an unjoined one.
          const out: Record<string, unknown> = {};
          await pool(body.members as M[], 8, async (m) => {
            const r = await call(env, "MEMBER", m.site, m.do_name, "/status");
            out[`${m.site}|${m.do_name}`] = r.status === 200 ? !!r.v?.joined : `HTTP ${r.status}`;
          });
          return json(out);
        }
        case "/configs": {
          const out: Record<string, unknown> = {};
          await pool(body.chains as (M & { chain: string })[], 6, async (c) => {
            out[c.chain] = await config(env, { site: c.site, do_name: c.do_name });
          });
          return json(out);
        }
        case "/fault": {
          const every = sites(env).map((s) => s.id);
          if (req.method === "GET") {
            return json(await Promise.all(every.map(async (s) => ({ site: s, ...(await call(env, "REGISTRY", s, "faults", "/site_fault")) }))));
          }
          const now = Date.now();
          const plan = body.off ? { site: null, from: 0, until: 0 } : { site: body.site, from: now, until: now + (body.for_ms ?? 3_600_000) };
          const put = await Promise.all(every.map(async (s) => {
            const r = await stub(env, "REGISTRY", s, "faults").fetch("https://registry/site_fault", { method: "PUT", body: JSON.stringify(plan) });
            return { site: s, status: r.status };
          }));
          return json({ now, plan, put });
        }
        case "/create": {
          const home = body.home ?? 0;
          const rec = { name: body.name ?? "Tri", sites: body.sites ?? [0, 1, 2], home, generation: 1, moving_from: null, at: 1 };
          return json(await pool(body.chains as string[], 4, async (chain) => {
            const joins = await Promise.all(rec.sites.map(async (site, slot) => {
              const me = { site, kind: "full", slot, generation: 1, do_name: `${chain}#1#f${slot}` };
              return (await call(env, "MEMBER", site, me.do_name, "/join", { chain, me, record: rec }, 15_000)).status;
            }));
            const lead = { site: home, do_name: `${chain}#1#f${rec.sites.indexOf(home)}` };
            let role = null;
            for (let i = 0; i < 50 && role !== "leader"; i++) {
              await call(env, "MEMBER", lead.site, lead.do_name, "/campaign", {});
              role = (await call(env, "MEMBER", lead.site, lead.do_name, "/status")).v?.role ?? null;
              if (role !== "leader") await sleep(200);
            }
            const w = await call(env, "MEMBER", lead.site, lead.do_name, "/commit",
              { rid: rid(), cmd: { kind: "put", key: "warm", value: "1" } }, 10_000);
            return { chain, joins, role, warm: w.status === 200 && !!w.v?.done?.ok };
          }));
        }
        case "/read": {
          // Each chain's value on its home voter as leader: an idle group's followers name no leader and answer a
          // read-index read notLeader (m6aL's home read: 0 of 37 read that way, 37 of 37 this way), so the home is
          // asked to campaign first, which an elected leader answers `leader` or `resumed`. `{chain, site, do_name}`
          // names any member; the config it reports names the home.
          const out: Record<string, unknown> = {};
          await pool(body.chains as (M & { chain: string })[], 6, async (c) => {
            const cf = await config(env, c);
            const cfg = "error" in cf ? null : cf.cfg;
            const n = cfg?.normal ?? null;
            const home = n ? voters(cfg).find((v: M & { site: number }) => v.site === n.home) : null;
            const m = home ?? c;
            let r = { status: 0, v: null as any };
            for (let t = 0; t < 4; t++) {
              await call(env, "MEMBER", m.site, m.do_name, "/campaign", {}, 8_000);
              r = await call(env, "MEMBER", m.site, m.do_name, "/read", { q: { key: body.key } }, 8_000);
              if (r.status === 200 && r.v?.value !== undefined) break;
              await new Promise((ok) => setTimeout(ok, 1_000));
            }
            out[c.chain] = { status: r.status, by: m.do_name, v: r.v?.value ?? r.v };
          });
          return json(out);
        }
        case "/load": {
          const chains = body.chains as (M & { chain: string; leader?: M })[];
          const end = Date.now() + 1000 * (body.seconds ?? 60);
          const per = 1000 / (body.rate ?? 2);
          let ok = 0, bad = 0;
          const last: Record<string, string> = {};
          const errs = new Map<string, number>();
          while (Date.now() < end) {
            const t = Date.now();
            const c = chains[Math.floor(Math.random() * chains.length)];
            const target = c.leader ?? c;
            const r = await call(env, "MEMBER", target.site, target.do_name, "/commit",
              { rid: rid(), cmd: { kind: "put", key: body.key ?? "bf50", value: `${t}` } }, 4_000);
            // A member's `/commit` answers `{done: {ok}}`, or `{done: {refused}}`, `{pending}` or `{error}`.
            if (r.status === 200 && r.v?.done?.ok) { ok++; last[c.chain] = `${t}`; }
            else {
              bad++;
              const k = `${r.status} ${JSON.stringify(r.v).slice(0, 60)}`;
              errs.set(k, (errs.get(k) ?? 0) + 1);
              const nl = r.v?.notLeader ?? r.v?.error?.notLeader ?? r.v?.done?.notLeader;
              // An idle group's follower names no leader (its leader sends nothing while idle), so try the config's home
              // voter, where the group's leader sits until a fault moves it.
              const cfg = nl?.config?.config;
              const home = cfg?.normal?.home ?? cfg?.degraded?.placement?.home;
              const at = typeof nl?.leader === "number" ? nl.leader : home;
              if (nl && typeof at === "number" && at !== target.site) {
                const v = voters(nl.config?.config).find((m) => m.site === at);
                if (v) c.leader = v;
              }
            }
            await sleep(Math.max(0, per - (Date.now() - t)));
          }
          return json({ ok, bad, last, errs: [...errs].slice(0, 8) });
        }
        case "/member": {
          return json(await call(env, "MEMBER", body.site, body.do_name, body.path, body.body, body.ms ?? 10_000));
        }
        case "/op": {
          // One operator command (a `cred` naming no account) on a chain that exists, through its leader: the stand-in
          // for the operator route R14 lacks, used only to seed the R66 probe's owner (`bf50-probe.mjs`). `at` names
          // any row member; one request id is re-sent until an outcome, and the config's home is asked to campaign
          // while no member names a leader.
          const c = await config(env, body.at);
          if ("error" in c) return json(c);
          const vs = voters(c.cfg);
          const home = vs.find((v: any) => v.site === (c.cfg?.normal ?? c.cfg?.degraded?.placement)?.home) ?? vs[0];
          let target: M = vs.find((v: any) => v.site === c.leader) ?? home;
          let ask = c.leader === null;
          const id = rid();
          let last: unknown = null;
          for (let t = 0; t < 8; t++) {
            if (ask) await call(env, "MEMBER", target.site, target.do_name, "/campaign", {});
            let r = await call(env, "MEMBER", target.site, target.do_name, "/commit", { rid: id, cmd: body.cmd }, 15_000);
            if (r.status === 200 && r.v?.admitted) {
              r = await call(env, "MEMBER", target.site, target.do_name, "/outcome", { ticket: r.v.admitted, wait_ms: 10_000 }, 12_000);
            }
            const nl = r.v?.error?.notLeader ?? r.v?.done?.notLeader;
            if (r.status === 200 && r.v?.done && !nl) return json({ done: r.v.done, by: target.do_name });
            last = r.v;
            const named = typeof nl?.leader === "number" ? vs.find((v: any) => v.site === nl.leader) : undefined;
            target = named ?? home;
            ask = !named;
            await sleep(500);
          }
          return json({ error: "no outcome", last });
        }
        case "/view": {
          // The group's voters from any member that answers `/status` with a config, then each named site's pages.
          let vs: M[] = [0, 1, 2].map((s) => ({ site: s, do_name: `release#1#f${s}` }));
          const c = await config(env, vs.find((v) => v.site !== body.skip) ?? vs[1]);
          if ("cfg" in c) vs = voters(c.cfg);
          const out = await Promise.all(vs.filter((v) => (body.sites as number[]).includes(v.site)).map(async (v) => {
            const pages: unknown[] = [];
            for (let from = 0; pages.length < 200; ) {
              const r = await call(env, "MEMBER", v.site, v.do_name, "/view", { from }, 10_000);
              const p = r.v?.ok ?? r.v;
              if (r.status !== 200 || !p?.entries) return { site: v.site, do_name: v.do_name, error: `${r.status} ${JSON.stringify(r.v).slice(0, 120)}`, pages };
              pages.push({ count: p.count, entries: p.entries });
              from += p.entries.length;
              if (!p.entries.length || from >= p.count) break;
            }
            return { site: v.site, do_name: v.do_name, pages };
          }));
          return json(out);
        }
        case "/health": {
          const since = url.searchParams.get("since") ?? "0";
          const obs = sites(env).filter((s) => !s.spare).map((s) => s.id);
          return json(await Promise.all(obs.map(async (o) => ({ observer: o, ...(await call(env, "REGISTRY", o, "site_health", `/site_health?since=${since}`)) }))));
        }
      }
    } catch (err) {
      return Response.json({ error: String(err) }, { status: 500 });
    }
    return Response.json({ error: "not found" }, { status: 404 });
  },
};
