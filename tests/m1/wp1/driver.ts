// M1-WP1 check driver: joins the three bootstrap members of a chain from the real site scripts, then reads each site's
// registry row and the members' Rtt rows. Bound to every site's Member and Registry class with script_name.
// POST /join?chain=repo:42&gen=1  -> { joins: [...per member Rtt rows], rows: [...registry row per site] }

interface Env { [k: string]: DurableObjectNamespace | string; TOKEN: string }
const SITES = [
  { id: 0, hint: "enam" },
  { id: 1, hint: "weur" },
  { id: 2, hint: "apac" },
] as const;

async function registryShard(chain: string): Promise<number> {
  const h = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(chain)));
  return h[0] % 64;
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const chain = url.searchParams.get("chain") ?? "repo:42";
    const gen = Number(url.searchParams.get("gen") ?? "1");
    const record = { name: "Tri", sites: [0, 1, 2], home: 0, generation: gen, moving_from: null, at: 1 };
    const members = SITES.map((s, slot) => ({ site: s.id, kind: "full", slot, generation: gen, do_name: `${chain}#${gen}#f${slot}` }));
    const t0 = Date.now();
    const joins = await Promise.all(
      SITES.map(async (s, i) => {
        const ns = env[`MEMBER_${s.id}`] as DurableObjectNamespace;
        const stub = ns.get(ns.idFromName(members[i].do_name), { locationHint: s.hint });
        const r = await stub.fetch("https://member/join", { method: "POST", body: JSON.stringify({ chain, me: members[i], record }) });
        return { site: s.id, status: r.status, body: await r.json().catch(() => null) };
      }),
    );
    const shard = await registryShard(chain);
    const rows = await Promise.all(
      SITES.map(async (s) => {
        const ns = env[`REGISTRY_${s.id}`] as DurableObjectNamespace;
        const r = await ns.get(ns.idFromName(`registry#${shard}`), { locationHint: s.hint }).fetch(`https://registry/row?chain=${encodeURIComponent(chain)}`);
        return { site: s.id, status: r.status, body: await r.json().catch(() => null) };
      }),
    );
    // A member that reaches the wrong site's script must refuse: send site 1's identity to site 0's class.
    const ns0 = env.MEMBER_0 as DurableObjectNamespace;
    const wrong = await ns0.get(ns0.idFromName(`${chain}#${gen}#wrong`), { locationHint: "enam" })
      .fetch("https://member/join", { method: "POST", body: JSON.stringify({ chain, me: members[1], record }) });
    return Response.json({ ms: Date.now() - t0, joins, rows, wrongSite: wrong.status });
  },
};
