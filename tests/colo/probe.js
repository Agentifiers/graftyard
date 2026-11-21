// Scratch probe for the Workers Cache API on the hostname kind the test edge uses (b034-colo). Not product code.
// GET /bench?n=N&size=B : N rounds of put(unique key) then match(it), plus one match of a missing key per round.
// GET /put?k=K          : put K (max-age 30) with the put time in its body.
// GET /match?k=K        : match K; answers whether it hit and the body's put time.
// Every answer names the colo and an isolate token, so the client can tell cross-isolate hits from same-isolate ones.
let ISO = null;
const origin = (req) => new URL(req.url).origin;
const key = (req, k) => new Request(`${new URL(req.url).searchParams.get("o") ?? origin(req)}/__gate/${encodeURIComponent(k)}`);
const value = (at, size) =>
  new Response(JSON.stringify({ at, pad: "x".repeat(size) }), {
    headers: { "cache-control": `max-age=${globalThis.MAXAGE ?? 30}`, "content-type": "application/json" },
  });

async function timed(f) {
  const t = Date.now();
  const v = await f();
  return [Date.now() - t, v];
}

export default {
  async fetch(req, env) {
    ISO ??= crypto.randomUUID().slice(0, 8);
    globalThis.MAXAGE = +(new URL(req.url).searchParams.get("ma") || 30);
    const u = new URL(req.url);
    if (u.searchParams.get("t") !== env.TOKEN) return new Response("no\n", { status: 401 });
    const c = u.searchParams.get("named") ? await caches.open("graft-gate") : caches.default;
    const colo = req.cf?.colo ?? null;
    const host = u.host;
    if (u.pathname === "/put") {
      const k = u.searchParams.get("k");
      const [ms] = await timed(() => c.put(key(req, k), value(Date.now(), 64)));
      return Response.json({ colo, iso: ISO, host, ms });
    }
    if (u.pathname === "/match") {
      const k = u.searchParams.get("k");
      const [ms, r] = await timed(() => c.match(key(req, k)));
      const at = r ? (await r.json()).at : null;
      return Response.json({ colo, iso: ISO, host, ms, hit: !!r, at, age: at ? Date.now() - at : null });
    }
    if (u.pathname === "/delete") {
      const k = u.searchParams.get("k");
      const [ms, gone] = await timed(() => c.delete(key(req, k)));
      return Response.json({ colo, iso: ISO, host, ms, gone });
    }
    if (u.pathname === "/bench") {
      const n = Math.min(+u.searchParams.get("n") || 20, 200);
      const size = +u.searchParams.get("size") || 64;
      const put = [], match = [], miss = [];
      let hits = 0;
      for (let i = 0; i < n; i++) {
        const k = `${ISO}-${Date.now()}-${i}`;
        const [p] = await timed(() => c.put(key(req, k), value(Date.now(), size)));
        put.push(p);
        const [m, r] = await timed(() => c.match(key(req, k)));
        match.push(m);
        if (r) hits++;
        const [x] = await timed(() => c.match(key(req, `missing-${k}`)));
        miss.push(x);
      }
      return Response.json({ colo, iso: ISO, host, n, hits, put, match, miss });
    }
    return new Response("probe\n");
  },
};
