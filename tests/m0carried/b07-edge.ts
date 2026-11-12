// B0-07's test edge on graft-m3c (`graft-m3c-b07-edge`), with the M1-WP2 test edge's caller (`../m1/wp2/edge.ts`:
// one RequestId per write, a missed receipt hedged only by `suspect`). It binds every site's `Member` and `Registry`
// classes. The site modes and `SiteHealth` it reads and writes are the site's test-faults stand-ins
// (`workers/site/src/faults.rs`, `SiteModes` and `site_health`), because the hosted code runs no `Release` group or
// `graft-ops` drain yet.
//
// Routes (bearer token), every body JSON:
//   POST /create {chains}                 create each register group (Tri, home site 0)
//   POST /lead {chains, to}               transfer each group's leadership to site `to`
//   POST /status {chains}                 every member's /status of each group
//   POST /bench {chains, seconds, workers, rate, observer}
//                                         `workers` writers, each `rate` puts/s on random groups; every member call's
//                                         outcome counted into site `observer`'s health object, as an edge there would
//   POST /modes {drained, limited}        the site modes, on every site's `faults` registry
//   POST /fault {site, rate, hang_ms, for_ms} | {off: true}
//                                         the gray fault (`SiteFault`), on every site's `faults` registry
//   GET  /health?since=MS                 each observer site's health sums since MS, per target

import { Edge, rid } from "../m1/wp2/edge.ts";

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

async function everySite(env: Env, path: string, body: unknown): Promise<unknown[]> {
  return Promise.all([0, 1, 2].map(async (s) => {
    const r = await registry(env, s, "faults").fetch(`https://registry${path}`, { method: "PUT", body: JSON.stringify(body) });
    return { site: s, status: r.status, body: await r.json().catch(() => null) };
  }));
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

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body: Record<string, any> = req.method === "POST" ? await req.json() : {};
    const colo = (req.cf as { colo?: string } | undefined)?.colo ?? "edge";
    const edge = (chain: string) => new Edge(env as any, chain, 1, 0x4c4158);
    const json = (v: unknown) => Response.json(v);
    try {
      switch (url.pathname) {
        case "/create":
          return json(await pool(body.chains as string[], 6, async (c) => {
            const r: any = await edge(c).create();
            return { chain: c, ok: !!r.warm?.ok, verdict: r.verdict, ms: r.leaderMs };
          }));
        case "/lead":
          return json(await pool(body.chains as string[], 6, async (c) => {
            const e = edge(c);
            const st = await e.all<any>("/status", 3_000);
            const l = st.find((m) => m.r?.role === "leader")?.site;
            if (l === undefined) return { chain: c, error: "no leader" };
            if (l === body.to) return { chain: c, already: true };
            const t = await e.call(l, "/transfer", { to: body.to }, 3_000);
            return { chain: c, from: l, transfer: t?.transfer ?? null };
          }));
        case "/status": {
          // `sites` limits the members asked (default all three), so a poll of many groups stays inside one
          // invocation's six concurrent calls.
          const sites: number[] = body.sites ?? [0, 1, 2];
          const pairs = (body.chains as string[]).flatMap((c) => sites.map((s) => [c, s] as const));
          const rs = await pool(pairs, 6, async ([c, s]) => [c, s, await edge(c).call(s, "/status", undefined, 3_000)] as const);
          const out: Record<string, (number | string | null)[][]> = {};
          for (const [c, s, r] of rs) (out[c] ??= []).push(r ? [s, r.role, r.term, r.leader] : [s, null]);
          return json({ at: Date.now(), groups: out });
        }
        case "/bench": {
          const chains: string[] = body.chains, end = Date.now() + 1000 * (body.seconds ?? 15);
          const counts = new Map<number, [number, number]>();
          const onCall = (site: number, ok: boolean) => {
            const c = counts.get(site) ?? [0, 0];
            c[0]++; if (!ok) c[1]++;
            counts.set(site, c);
          };
          let flushed = Date.now();
          const flush = async () => {
            if (!counts.size) return;
            const rows = [...counts].map(([t, [c, f]]) => [t, c, f]);
            counts.clear();
            flushed = Date.now();
            await registry(env, body.observer, "health").fetch("https://registry/health", { method: "PUT", body: JSON.stringify({ counts: rows }) }).catch(() => {});
          };
          const ops: [number, number, number, boolean][] = [];
          const per = 1000 / (body.rate ?? 1);
          await Promise.all(Array.from({ length: body.workers ?? 5 }, async () => {
            await sleep(Math.random() * per);
            while (Date.now() < end) {
              const k = Math.floor(Math.random() * chains.length), t = Date.now();
              const e = edge(chains[k]);
              e.onCall = onCall;
              const r = await e.write({ kind: "put", key: "b07", value: `${t}` }, rid(), []);
              ops.push([k, t, Date.now() - t, r.ok]);
              if (Date.now() - flushed >= 1_000) await flush();
              await sleep(Math.max(0, per - (Date.now() - t)));
            }
          }));
          await flush();
          return json({ colo, ops });
        }
        case "/modes": return json(await everySite(env, "/site_modes", { drained: body.drained ?? [], limited: body.limited ?? [] }));
        case "/fault": {
          const now = Date.now();
          const plan = body.off ? { site: null, from: 0, until: 0 }
            : { site: body.site, from: now + (body.after_ms ?? 0), until: now + (body.after_ms ?? 0) + (body.for_ms ?? 3_600_000), rate: body.rate ?? null, hang_ms: body.hang_ms ?? null };
          return json({ now, plan, sites: await everySite(env, "/site_fault", plan) });
        }
        case "/health": {
          const since = url.searchParams.get("since") ?? "0";
          return json(await Promise.all([0, 1, 2].map(async (o) => {
            const r = await registry(env, o, "health").fetch(`https://registry/health?since=${since}`);
            return { observer: o, rows: r.status === 200 ? await r.json() : { status: r.status } };
          })));
        }
      }
    } catch (err) {
      return Response.json({ error: String(err) }, { status: 500 });
    }
    return Response.json({ error: "not found" }, { status: 404 });
  },
};
