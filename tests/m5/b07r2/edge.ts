// B0-07 r2's scratch Worker on graft-m4a (`graft-m4a-b07r2-edge`; `tests/m5/b07r2/row.sh`). It binds every site's
// `Member` and `Registry` classes, as M5-WP4 row B's `graft-m4a-ck-edge` did, and adds what row B lacked: each member's
// election trace, read by name, so a chain's leader change is timed from its own members' records, and every bench
// write kept with its chain and Cloudflare time, so a chain's activity before the drain is known. Deploying it creates
// no site version. Every time it returns is this Worker's `Date.now()` or a member's clock: Cloudflare time.
//
// Routes (bearer token), every body JSON:
//   POST /create {chains, home}        create each register group (Tri, home site `home`) and warm it
//   POST /voters {chains, home}        each chain's committed voters as the home member names them: {chain: [name a, b, c]}
//   POST /status {names, sites}        every named member's /status: {at, groups: {chain: [[site, role, term, leader, ledTerm]]}}
//   POST /elections {names, since}     every named member's /elections rows after `since`:
//                                      {at, rows: {chain: [[site, at, kind, term, peer, index, extra]]}, missed: [[chain, site]]}
//   POST /bench {chains, seconds, workers, rate}
//                                      `workers` writers, each `rate` puts/s on random chains, for `seconds`:
//                                      {colo, ops: [[chain, start ms, ms, ok]]}
//   POST /fault {site, rate, hang_ms, for_ms} | {off: true}
//                                      the gray fault (`SiteFault`) on every site's `faults` registry
//   GET  /now                          this Worker's clock, for the orb's offset from Cloudflare time

import { Edge, rid } from "../../m1/wp2/edge.ts";

interface Env {
  MEMBER_0: DurableObjectNamespace; MEMBER_1: DurableObjectNamespace; MEMBER_2: DurableObjectNamespace;
  REGISTRY_0: DurableObjectNamespace; REGISTRY_1: DurableObjectNamespace; REGISTRY_2: DurableObjectNamespace;
  TOKEN: string;
}

const HINTS = ["enam", "weur", "apac"] as const;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function registry(env: Env, site: number, name: string): DurableObjectStub {
  const ns = env[`REGISTRY_${site}` as "REGISTRY_0"];
  return ns.get(ns.idFromName(name), { locationHint: HINTS[site] });
}

/** One observation call to the member named `name` on `site`: `/status` and `/elections` pass the site fault. */
async function observe(env: Env, site: number, name: string, path: string, ms = 5_000): Promise<any | null> {
  const ns = env[`MEMBER_${site}` as "MEMBER_0"];
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), ms);
  try {
    const r = await ns.get(ns.idFromName(name), { locationHint: HINTS[site] }).fetch(`https://member${path}`, { signal: ctl.signal });
    if (r.status !== 200) { await r.body?.cancel(); return null; }
    return await r.json();
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/** Runs `f` over `xs`, at most `n` at once. */
async function pool<T, R>(xs: T[], n: number, f: (x: T) => Promise<R>): Promise<R[]> {
  const out: R[] = new Array(xs.length);
  let i = 0;
  await Promise.all(Array.from({ length: Math.min(n, xs.length) }, async () => {
    while (i < xs.length) { const k = i++; out[k] = await f(xs[k]); }
  }));
  return out;
}

/** The (chain, site, name) triples of `names` on `sites`. */
function members(names: Record<string, string[]>, sites: number[]): [string, number, string][] {
  return Object.entries(names).flatMap(([c, ns]) => sites.map((s) => [c, s, ns[s]] as [string, number, string]));
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body: Record<string, any> = req.method === "POST" ? await req.json() : {};
    const colo = (req.cf as { colo?: string } | undefined)?.colo ?? "edge";
    const edge = (chain: string, home = 0) => new Edge(env as any, chain, 1, 0x4c4158, home);
    const json = (v: unknown) => Response.json(v);
    try {
      switch (url.pathname) {
        case "/now": return json({ now: Date.now(), colo });
        case "/create":
          return json(await pool(body.chains as string[], 6, async (c) => {
            const r: any = await edge(c, body.home ?? 0).create();
            return { chain: c, ok: !!r.warm?.ok, verdict: r.verdict, ms: r.leaderMs };
          }));
        case "/voters": {
          // The home member is never re-placed (`/place` replaces only its followers), so its bootstrap name holds.
          const home: number = body.home;
          const out: Record<string, (string | null)[]> = {};
          await pool(body.chains as string[], 6, async (c) => {
            const st = await observe(env, home, `${c}#1#f${home}`, "/status");
            const cfg = st?.config?.config;
            const p = cfg?.normal ?? cfg?.degraded?.placement ?? cfg?.joint?.new;
            const names: (string | null)[] = [null, null, null];
            for (const v of p?.voters ?? []) names[v.site] = v.do_name;
            out[c] = names;
          });
          return json(out);
        }
        case "/status": {
          const sites: number[] = body.sites ?? [0, 1, 2];
          const out: Record<string, unknown[][]> = {};
          const rs = await pool(members(body.names, sites), 6, async ([c, s, n]) => [c, s, await observe(env, s, n, "/status")] as const);
          for (const [c, s, r] of rs) (out[c] ??= []).push(r ? [s, r.role, r.term, r.leader, r.ledTerm] : [s, null]);
          return json({ at: Date.now(), groups: out });
        }
        case "/elections": {
          const since: number = body.since ?? 0;
          const rows: Record<string, number[][]> = {};
          const missed: [string, number][] = [];
          await pool(members(body.names, [0, 1, 2]), 6, async ([c, s, n]) => {
            const r = await observe(env, s, n, `/elections?since=${since}`);
            if (!r) { missed.push([c, s]); return; }
            for (const row of r.rows ?? []) (rows[c] ??= []).push([s, ...row]);
          });
          return json({ at: Date.now(), rows, missed });
        }
        case "/bench": {
          const chains: string[] = body.chains, end = Date.now() + 1000 * (body.seconds ?? 15);
          const ops: [string, number, number, boolean][] = [];
          const per = 1000 / (body.rate ?? 1);
          await Promise.all(Array.from({ length: body.workers ?? 5 }, async () => {
            await sleep(Math.random() * per);
            while (Date.now() < end) {
              const c = chains[Math.floor(Math.random() * chains.length)], t = Date.now();
              const r = await edge(c).write({ kind: "put", key: "b07r2", value: `${t}` }, rid(), []);
              ops.push([c, t, Date.now() - t, r.ok]);
              await sleep(Math.max(0, per - (Date.now() - t)));
            }
          }));
          return json({ colo, ops });
        }
        case "/fault": {
          const now = Date.now();
          const plan = body.off ? { site: null, from: 0, until: 0 }
            : { site: body.site, from: now, until: now + (body.for_ms ?? 3_600_000), rate: body.rate ?? null, hang_ms: body.hang_ms ?? null };
          const sites = await Promise.all([0, 1, 2].map(async (s) => {
            const r = await registry(env, s, "faults").fetch("https://registry/site_fault", { method: "PUT", body: JSON.stringify(plan) });
            return { site: s, status: r.status };
          }));
          return json({ now, plan, sites });
        }
      }
    } catch (err) {
      return Response.json({ error: String(err) }, { status: 500 });
    }
    return Response.json({ error: "not found" }, { status: 404 });
  },
};
