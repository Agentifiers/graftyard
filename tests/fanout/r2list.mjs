// Lists graft-m2b's (or R2_BUCKET's, such as graft-m1-packs-{site}) site buckets under a prefix through the R2 API (a driver tool, not a request path):
//   node tests/fanout/r2list.mjs PREFIX     one JSON line per site bucket: {site, keys: [{key, size, modified}]}
// B0-34 round 26: the composed-miss chain (`compose/<net>/`) after the warm, and each clone-cache miss's entry
// (`clone/<net>/<key>/0`), which the shard's spool puts whether or not the client read to the end.
const SITES = ["a", "b", "c"];

export async function r2list(prefix, timeoutMs = 20_000) {
  return Promise.all(SITES.map(async (site) => {
    const url = `https://api.cloudflare.com/client/v4/accounts/${process.env.CLOUDFLARE_ACCOUNT_ID}/r2/buckets/${(process.env.R2_BUCKET ?? "graft-m2b-site-{site}-packs").replace("{site}", site)}/objects?prefix=${encodeURIComponent(prefix)}&per_page=100`;
    try {
      const r = await fetch(url, { headers: { authorization: `Bearer ${process.env.CLOUDFLARE_API_TOKEN}` }, signal: AbortSignal.timeout(timeoutMs) });
      const d = await r.json();
      if (!d.success) return { site, error: JSON.stringify(d.errors).slice(0, 120) };
      return { site, keys: (d.result ?? []).map((o) => ({ key: o.key, size: o.size, modified: o.last_modified })) };
    } catch (e) { return { site, error: String(e).slice(0, 120) }; }
  }));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  for (const row of await r2list(process.argv[2])) console.log(JSON.stringify(row));
}
