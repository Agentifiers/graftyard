// lifeboatbed's install front (`run.sh`): the one address the lifeboat reaches the install at, standing in for the
// internet between the customer's host and Cloudflare. Every request that is not the bed's own goes to the edge. Bed
// routes:
// - `/_s3/<bucket>[/<key>]`: R2's S3 API over the export bucket, as the lifeboat's bucket-scoped read token sees it:
//   `ListObjectsV2` (`list-type=2`, `prefix`, `start-after`, `max-keys`) and `GetObject`. Each request's SigV4
//   signature is recomputed here from the bed's key pair and refused 403 if it differs, so a signer bug fails the bed.
//   `GET /_feed?stop=1|0` stops and restarts it (R2 down: 503); every request it answered is counted.
// - `GET /_edge?block=1|0`: blocks the edge for the lifeboat (503 to any request that is not the bed's).
// - `GET /_sitecron`: site 0's `scheduled` handler, which kicks a feed round.
// - `GET /_site/<path>`: site 0's script (exportbed's driver).
// - `GET /_stats?chain=C`: each of the chain's three members' `/status` role and commit, and `/stats` inbound
//   counts by route (`reset=1` restarts them).
// - `POST /_put?key=K`: plants an object in the export bucket.
const enc = new TextEncoder();
const hexOf = (b) => [...new Uint8Array(b)].map((x) => x.toString(16).padStart(2, "0")).join("");
const sha = async (s) => hexOf(await crypto.subtle.digest("SHA-256", typeof s === "string" ? enc.encode(s) : s));
async function hmac(key, msg) {
  const k = await crypto.subtle.importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", k, enc.encode(msg)));
}
const uri = (s, path) =>
  [...enc.encode(s)].map((b) => {
    const c = String.fromCharCode(b);
    if (/[A-Za-z0-9\-_.~]/.test(c) || (path && c === "/")) return c;
    return "%" + b.toString(16).toUpperCase().padStart(2, "0");
  }).join("");

// SigV4 as AWS documents it, independently of `lifeboat/src/feed.rs`: true if `req` is signed by KEY_ID/SECRET.
async function signed(req, url, env) {
  const auth = req.headers.get("authorization") ?? "";
  const m = auth.match(/^AWS4-HMAC-SHA256 Credential=([^/]+)\/(\d{8})\/([^/]+)\/s3\/aws4_request, SignedHeaders=([^,]+), Signature=([0-9a-f]{64})$/);
  if (!m || m[1] !== env.FEED_KEY_ID || m[4] !== "host;x-amz-content-sha256;x-amz-date") return false;
  const [, , day, region, , sig] = m;
  const date = req.headers.get("x-amz-date") ?? "";
  const body = req.headers.get("x-amz-content-sha256") ?? "";
  if (!date.startsWith(day) || Math.abs(Date.parse(`${date.slice(0, 4)}-${date.slice(4, 6)}-${date.slice(6, 8)}T${date.slice(9, 11)}:${date.slice(11, 13)}:${date.slice(13, 15)}Z`) - Date.now()) > 900_000) return false;
  const q = [...url.searchParams].map(([k, v]) => [uri(k, false), uri(v, false)]).sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : 1));
  const canonical = ["GET", uri(decodeURIComponent(url.pathname), true), q.map(([k, v]) => `${k}=${v}`).join("&"),
    `host:${req.headers.get("host")}`, `x-amz-content-sha256:${body}`, `x-amz-date:${date}`, "",
    "host;x-amz-content-sha256;x-amz-date", body].join("\n");
  const scope = `${day}/${region}/s3/aws4_request`;
  const toSign = `AWS4-HMAC-SHA256\n${date}\n${scope}\n${await sha(canonical)}`;
  let k = await hmac(enc.encode(`AWS4${env.FEED_SECRET}`), day);
  for (const p of [region, "s3", "aws4_request"]) k = await hmac(k, p);
  return hexOf(await hmac(k, toSign)) === sig;
}

let feedStopped = false, edgeBlocked = false;
const counts = { s3: 0, s3_refused: 0, s3_stopped: 0, edge_blocked: 0 };

async function s3(req, url, env) {
  if (feedStopped) { counts.s3_stopped++; return new Response("<Error><Code>ServiceUnavailable</Code></Error>", { status: 503 }); }
  if (!(await signed(req, url, env))) { counts.s3_refused++; return new Response("<Error><Code>SignatureDoesNotMatch</Code></Error>", { status: 403 }); }
  counts.s3++;
  const [, , bucket, ...rest] = url.pathname.split("/");
  if (bucket !== env.FEED_BUCKET) return new Response("<Error><Code>NoSuchBucket</Code></Error>", { status: 404 });
  if (rest.length) {
    const o = await env.EXPORT.get(decodeURIComponent(rest.join("/")));
    return o ? new Response(o.body) : new Response("<Error><Code>NoSuchKey</Code></Error>", { status: 404 });
  }
  const p = url.searchParams;
  const max = Number(p.get("max-keys") ?? 1000);
  const after = p.get("start-after") ?? "";
  const keys = [];
  let cursor;
  do {
    const l = await env.EXPORT.list({ prefix: p.get("prefix") ?? "", cursor });
    for (const o of l.objects) if (o.key > after) keys.push(o.key);
    cursor = l.truncated ? l.cursor : undefined;
  } while (cursor);
  keys.sort();
  const page = keys.slice(0, max);
  const xml = `<?xml version="1.0" encoding="UTF-8"?><ListBucketResult><Name>${bucket}</Name><IsTruncated>${keys.length > max}</IsTruncated>` +
    page.map((k) => `<Contents><Key>${k}</Key></Contents>`).join("") + "</ListBucketResult>";
  return new Response(xml, { headers: { "content-type": "application/xml" } });
}

async function stats(env, chain, reset) {
  const out = [];
  for (const s of [0, 1, 2]) {
    const stub = env[`MEMBER_${s}`].getByName(`${chain}#1#f${s}`);
    const st = await (await stub.fetch("https://member/status")).json().catch(() => ({}));
    const sx = await (await stub.fetch(`https://member/stats${reset ? "?reset=1" : ""}`)).json().catch(() => ({}));
    out.push({ site: s, role: st.role ?? null, commit: st.commit ?? null, inbound: sx.inbound ?? {} });
  }
  return out;
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    const q = (k) => url.searchParams.get(k);
    if (url.pathname.startsWith("/_s3/")) return s3(req, url, env);
    if (url.pathname === "/_feed") {
      if (q("stop") !== null) feedStopped = q("stop") === "1";
      return Response.json({ stopped: feedStopped, ...counts });
    }
    if (url.pathname === "/_edge") {
      if (q("block") !== null) edgeBlocked = q("block") === "1";
      return Response.json({ blocked: edgeBlocked, ...counts });
    }
    if (url.pathname === "/_sitecron") {
      await env.SITE.scheduled({ cron: q("cron") ?? "*/15 * * * *" });
      return new Response("ok");
    }
    if (url.pathname.startsWith("/_site/")) return env.SITE.fetch(new Request(`https://site${url.pathname.slice(6)}${url.search}`));
    if (url.pathname === "/_stats") return Response.json(await stats(env, q("chain"), q("reset") === "1"));
    if (url.pathname === "/_put") {
      await env.EXPORT.put(q("key"), await req.arrayBuffer());
      return new Response("ok");
    }
    if (url.pathname === "/_export") {
      const l = await env.EXPORT.list({ prefix: "export/" });
      return Response.json(l.objects.map((o) => o.key));
    }
    if (edgeBlocked && !req.headers.get("x-bed-token")) {
      counts.edge_blocked++;
      return new Response("blocked by the bed\n", { status: 503 });
    }
    return env.EDGE.fetch(req);
  },
};
