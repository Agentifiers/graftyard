// vaultbed's front (`run.sh`): the one address the vault reaches the install at, as the internet carries its HTTPS
// pulls from the vault account to the install's front. Every request that is not the bed's own goes to the edge. The
// bed's routes:
// - `POST /_put?b=SITE0|EXPORT&key=K[&meta=<json>]`: plants an object in the install's buckets.
// - `GET /_site/<path>`: exportbed's driver on site 0 (`tests/exportbed/driver.mjs`: `/drive`, `/objects`).
// - `GET /_sitecron`: site 0's `scheduled` handler, which kicks a feed round; `GET /_cron`: the vault's, one pass loop.
// - `GET /_vault`: every object in the vault's bucket with its size and SHA-256, and its state; `GET /_export`: the
//   same for the install's export bucket.
// - `GET /_dropstate`: deletes the vault's `state/vault`, so the next pass starts from no state and writes every object
//   again; `GET /_meta?key=`: an object's size and `sha256` metadata.
async function listed(bucket, prefix) {
  const out = [];
  let cursor;
  do {
    const l = await bucket.list({ prefix, cursor });
    for (const o of l.objects) {
      const b = await (await bucket.get(o.key)).arrayBuffer();
      const d = new Uint8Array(await crypto.subtle.digest("SHA-256", b));
      out.push({ key: o.key, size: o.size, sha256: [...d].map((x) => x.toString(16).padStart(2, "0")).join("") });
    }
    cursor = l.truncated ? l.cursor : undefined;
  } while (cursor);
  return out;
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    const q = (k) => url.searchParams.get(k);
    if (url.pathname === "/_put") {
      const bucket = { SITE0: env.SITE0, EXPORT: env.EXPORT }[q("b")];
      await bucket.put(q("key"), await req.arrayBuffer(), q("meta") ? { customMetadata: JSON.parse(q("meta")) } : {});
      return new Response("ok");
    }
    if (url.pathname.startsWith("/_site/")) {
      return env.SITE.fetch(new Request(`https://site${url.pathname.slice(6)}${url.search}`));
    }
    if (url.pathname === "/_sitecron" || url.pathname === "/_cron") {
      const t0 = Date.now();
      const r = await (url.pathname === "/_cron" ? env.VAULT : env.SITE).scheduled({ cron: q("cron") ?? "* * * * *" });
      return Response.json({ ms: Date.now() - t0, outcome: r?.outcome ?? null });
    }
    if (url.pathname === "/_dropstate") {
      await env.VAULTB.delete("state/vault");
      return new Response("ok");
    }
    if (url.pathname === "/_meta") {
      const o = await env.VAULTB.head(q("key"));
      return Response.json(o ? { size: o.size, sha256: o.customMetadata?.sha256 ?? null } : null);
    }
    if (url.pathname === "/_vault") {
      const s = await env.VAULTB.get("state/vault");
      return Response.json({ objects: await listed(env.VAULTB, ""), state: s ? JSON.parse(await s.text()) : null });
    }
    if (url.pathname === "/_export") return Response.json(await listed(env.EXPORT, "export/"));
    return env.EDGE.fetch(req);
  },
};
