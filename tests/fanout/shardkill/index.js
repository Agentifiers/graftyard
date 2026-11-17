// teardown.py --shardkill: a product install's edge has no `/_bed/kill`, so the teardown goes through this scratch
// Worker, bound to each site script's `GitShard` class, to the shard's own `/teardown` (a product route the edge does
// not forward), as M4's `graft-m4a-shardkill` did (tests/am1/results/install-m4a.md). teardown.py deploys it with a
// one-time TOKEN and deletes it after.
//   POST /?site=S&name=git#S#net:R   with header x-t: <TOKEN>
export default {
  async fetch(req, env) {
    if (req.headers.get("x-t") !== env.TOKEN) return new Response("no", { status: 403 });
    const u = new URL(req.url);
    const ns = env["SHARD_" + u.searchParams.get("site")];
    const r = await ns.get(ns.idFromName(u.searchParams.get("name"))).fetch("https://shard/teardown", { method: "POST" });
    return new Response(await r.text(), { status: r.status });
  },
};
