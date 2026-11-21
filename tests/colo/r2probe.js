// Scratch probe (b034-colo): R2 list of a chain prefix against 12-byte ranged gets, in one invocation. Read-only.
// GET /nets            : the first compose/<net>/ prefixes in the bucket.
// GET /bench?net=N&n=K : K rounds of one list of compose/N/ and, beside it, every .pack's 12-byte ranged get,
//                        4 at a time (as open_chain does).
export default {
  async fetch(req, env) {
    const u = new URL(req.url);
    if (u.searchParams.get("t") !== env.TOKEN) return new Response("no\n", { status: 401 });
    const b = env.BUCKET;
    if (u.pathname === "/nets") {
      const l = await b.list({ prefix: "compose/", delimiter: "/", limit: 50 });
      return Response.json({ prefixes: l.delimitedPrefixes });
    }
    if (u.pathname === "/bench") {
      const prefix = `compose/${u.searchParams.get("net")}/`;
      const n = Math.min(+u.searchParams.get("n") || 10, 50);
      const list = [], checks = [];
      let files = 0, packs = 0, etagsAgree = true;
      for (let i = 0; i < n; i++) {
        let t = Date.now();
        const l = await b.list({ prefix, limit: 1000 });
        list.push(Date.now() - t);
        files = l.objects.length;
        const ps = l.objects.filter((o) => o.key.endsWith(".pack"));
        packs = ps.length;
        t = Date.now();
        for (let j = 0; j < ps.length; j += 4) {
          const got = await Promise.all(
            ps.slice(j, j + 4).map(async (o) => {
              const r = await b.get(o.key, { range: { offset: 0, length: 12 } });
              if (r) await r.arrayBuffer();
              return [o.etag, r?.etag];
            }),
          );
          for (const [a, c] of got) if (a !== c) etagsAgree = false;
        }
        checks.push(Date.now() - t);
      }
      return Response.json({ colo: req.cf?.colo, prefix, files, packs, etagsAgree, list, checks });
    }
    return new Response("r2probe\n");
  },
};
