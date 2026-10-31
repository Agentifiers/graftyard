// M0-WP6, B0-22: container rehydration of a 5 GB network from R2 (design/DESIGN.md, `rehydrate` and "Git cache shards
// (derived)"). `graft-m0-rehydrate` is its own script with its own bucket, so it touches no other bed.
//
// A `Shard` object, created with a location hint, starts its container (git plus a PUT sink, `b0-22/Dockerfile`) with
// `enableInternet: false`, so bytes reach the container only through the object, as the design requires. It lists the
// network's packs under `net/<name>/`, streams each `.pack` and `.idx` from R2 into the container over `getTcpPort`,
// a few at a time, then runs `git init --bare`, moves the packs in, writes refs, and runs `git fsck --connectivity-only`
// and `git rev-parse` on the tip. It reports bytes, time to copy, time to a usable repository, and where it ran.

import { DurableObject } from "cloudflare:workers";

interface Env { SHARD: DurableObjectNamespace<Shard>; BUCKET: R2Bucket; M0_TOKEN: string }

type Container = NonNullable<DurableObjectState["container"]> & { images?: Record<string, string> };

export class Shard extends DurableObject<Env> {
  private get c(): Container { return this.ctx.container as Container; }

  private async sh(cmd: string): Promise<{ out: string; err: string; ms: number }> {
    const t = Date.now();
    const p = await (this.c as unknown as { exec(a: string[]): Promise<{ output(): Promise<{ stdout: ArrayBuffer; stderr: ArrayBuffer }> }> })
      .exec(["sh", "-c", cmd]);
    const o = await p.output();
    return { out: new TextDecoder().decode(o.stdout), err: new TextDecoder().decode(o.stderr), ms: Date.now() - t };
  }

  private async up(instance: string): Promise<number> {
    const t = Date.now();
    if (!this.c.running) this.c.start({ image: this.c.images!.git, instance: instance as "standard-4", enableInternet: false });
    await this.c.setInactivityTimeout(20 * 60_000);
    const port = this.c.getTcpPort(8080);
    for (let i = 0; i < 300; i++) {
      try { if ((await port.fetch("http://container/health")).ok) return Date.now() - t; } catch { /* starting */ }
      await new Promise((r) => setTimeout(r, 200));
    }
    throw new Error("container did not answer on 8080 within 60 s");
  }

  async rehydrate(net: string, instance: string, parallel: number): Promise<object> {
    const t0 = Date.now();
    const doColo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1];
    const startMs = await this.up(instance);
    const env = await this.sh("tr '\\0' '\\n' < /proc/1/environ | grep -E '^CLOUDFLARE_(REGION|LOCATION)='; nproc; df -m /data | tail -1");
    await this.sh("rm -rf /data/*");
    const keys: { key: string; size: number }[] = [];
    let cursor: string | undefined;
    do {
      const l = await this.env.BUCKET.list({ prefix: `net/${net}/`, cursor });
      for (const o of l.objects) keys.push({ key: o.key, size: o.size });
      cursor = l.truncated ? l.cursor : undefined;
    } while (cursor);
    const refs = keys.find((k) => k.key.endsWith("/refs"));
    const files = keys.filter((k) => k.key.endsWith(".pack") || k.key.endsWith(".idx")).sort((a, b) => b.size - a.size);
    const bytes = files.reduce((n, f) => n + f.size, 0);
    // Copy: each object streams from R2 through this object into the container's disk.
    const tCopy = Date.now();
    const port = this.c.getTcpPort(8080);
    let next = 0;
    const copyErr: string[] = [];
    await Promise.all(Array.from({ length: parallel }, async () => {
      while (next < files.length) {
        const f = files[next++];
        const obj = await this.env.BUCKET.get(f.key);
        if (!obj) { copyErr.push(`missing ${f.key}`); continue; }
        const r = await port.fetch(`http://container/f?path=${encodeURIComponent(`in/${f.key.split("/").pop()}`)}`, { method: "PUT", body: obj.body });
        if (!r.ok) copyErr.push(`${f.key}: ${r.status} ${await r.text()}`);
      }
    }));
    const copyMs = Date.now() - tCopy;
    // Usable repository: packs and their indexes in place, refs written, connectivity checked from the tip.
    const refText = refs ? await (await this.env.BUCKET.get(refs.key))!.text() : "";
    const tGit = Date.now();
    const setup = await this.sh(`set -e; git init -q --bare /data/r.git; mv /data/in/*.pack /data/in/*.idx /data/r.git/objects/pack/;
      printf '%s' '${refText.replace(/'/g, "")}' | while read sha ref; do [ -n "$ref" ] && git -C /data/r.git update-ref "$ref" "$sha"; done; echo ok`);
    const setupMs = Date.now() - tGit;
    const fsck = await this.sh("git -C /data/r.git fsck --connectivity-only --no-progress > /data/fsck.txt 2>&1; echo exit=$?; tail -3 /data/fsck.txt");
    const tip = await this.sh("git -C /data/r.git for-each-ref --count=3 --format='%(objectname) %(refname)'; git -C /data/r.git count-objects -vH");
    const gitMs = Date.now() - tGit;
    return { net, instance, parallel, doColo, container: env.out.trim().split("\n"), startMs, objects: files.length, bytes, copyMs,
      copyMBps: Math.round(bytes / 1e6 / (copyMs / 1000)), gitMs, setupMs, fsckMs: fsck.ms, totalMs: Date.now() - t0, copyErr: copyErr.slice(0, 5),
      setup: setup.out.trim().slice(-200) + setup.err.slice(-300), fsck: fsck.out.trim().slice(-300), tip: tip.out.trim().slice(0, 600) };
  }

  /** Seeds network `net`: the container generates about `mb` MB of random blobs as packs of at most `packMb` MB with
   *  `git fast-import`, and this object streams each `.pack` and `.idx`, then the refs, into the bucket. */
  async seed(net: string, mb: number, packMb: number, instance: string): Promise<object> {
    const t0 = Date.now();
    await this.up(instance);
    const g = await this.sh(`set -e; rm -rf /data/*; git init -q --bare /data/s.git; /sink gen ${mb} 50 |
      git -C /data/s.git -c fastimport.unpackLimit=0 fast-import --quiet --max-pack-size=${packMb}m; cd /data/s.git/objects/pack; ls -l | awk 'NR>1 {print $5, $9}'`);
    const genMs = Date.now() - t0;
    const files = g.out.trim().split("\n").filter(Boolean).map((l) => { const [size, name] = l.split(" "); return { size: Number(size), name }; });
    if (!files.length) throw new Error(`gen failed: ${g.err.slice(-400)}`);
    return { genMs, ...(await this.upload(net, files)) };
  }

  private async upload(net: string, files: { size: number; name: string }[]): Promise<object> {
    const port = this.c.getTcpPort(8080);
    const tUp = Date.now();
    let next = 0;
    await Promise.all(Array.from({ length: 4 }, async () => {
      while (next < files.length) {
        const f = files[next++];
        const r = await port.fetch(`http://container/f?path=${encodeURIComponent(`s.git/objects/pack/${f.name}`)}`);
        if (!r.ok || !r.body) throw new Error(`read ${f.name}: ${r.status}`);
        // R2 needs a known length for a streamed put.
        const body = r.body.pipeThrough(new FixedLengthStream(f.size));
        await this.env.BUCKET.put(`net/${net}/${f.name}`, body);
      }
    }));
    const refs = await this.sh("git -C /data/s.git for-each-ref --format='%(objectname) %(refname)'");
    await this.env.BUCKET.put(`net/${net}/refs`, refs.out);
    const bytes = files.reduce((n, f) => n + f.size, 0);
    return { net, files: files.length, bytes, uploadMs: Date.now() - tUp, uploadMBps: Math.round(bytes / 1e6 / ((Date.now() - tUp) / 1000)), refs: refs.out.trim() };
  }

  async stop(): Promise<void> { if (this.c.running) await this.c.destroy(); }

  // ---- B0-23, clone cache ----

  /** Builds monorepo `net` on this shard's disk: one commit of `files` text files of 1 to 4 KB, then `commits` commits
   *  that each change 3 files, packed with bitmaps. Returns the commits oldest first: the tips the clients clone. */
  async mono(net: string, files: number, commits: number, instance: string): Promise<object> {
    const startMs = await this.up(instance);
    await this.c.setInactivityTimeout(60 * 60_000);
    const g = await this.sh(`set -e; rm -rf /data/${net}.git /data/cc; git init -q --bare /data/${net}.git;
      /sink mono ${files} ${commits} | git -C /data/${net}.git fast-import --quiet; git -C /data/${net}.git repack -a -d -b -q;
      du -sm /data/${net}.git | cut -f1`);
    const tips = (await this.sh(`git -C /data/${net}.git rev-list --reverse main`)).out.trim().split("\n");
    return { startMs, genMs: g.ms, repoMb: Number(g.out.trim()), err: g.err.slice(-300), tips };
  }

  /** A clone-cache miss: the container runs one `upload-pack` per key however many misses arrive for it (src/sink
   *  `/clone`), and once it finishes this object writes the response to the bucket as `replicas` copies. */
  async fetch(req: Request): Promise<Response> {
    const u = new URL(req.url);
    const key = u.searchParams.get("key")!, net = u.searchParams.get("net")!, replicas = Number(u.searchParams.get("replicas"));
    if (!this.c.running) return new Response("container not running", { status: 503 });
    // At most FOLLOW_MAX streams per key go through this object; later misses on a key being filled are told to wait
    // for its replica 0 in the bucket, so a herd on a new tip costs this object one short answer each.
    const streams = this.streams.get(key) ?? 0;
    if (this.filling.has(key) && streams >= FOLLOW_MAX) return new Response(null, { status: 202, headers: { "x-cc": "wait" } });
    this.streams.set(key, streams + 1);
    const release = () => { const n = (this.streams.get(key) ?? 1) - 1; if (n) this.streams.set(key, n); else this.streams.delete(key); };
    const port = this.c.getTcpPort(8080);
    let r: Response;
    try { r = await port.fetch(`http://container/clone?repo=${net}.git&key=${key}`, { method: "POST", body: req.body }); }
    catch (e) { release(); return new Response(`container: ${String(e).slice(0, 300)}`, { status: 503 }); }
    const state = r.headers.get("x-cc") ?? "?";
    if (state === "gen" && !this.filling.has(key)) {
      this.filling.add(key);
      this.ctx.waitUntil(this.fill(key, replicas).finally(() => this.filling.delete(key)));
    }
    const { readable, writable } = new TransformStream();
    this.ctx.waitUntil(r.body!.pipeTo(writable).catch(() => undefined).finally(release));
    return new Response(readable, { status: r.status, headers: { "x-cc": state } });
  }

  private filling = new Set<string>();
  private streams = new Map<string, number>();

  private async fill(key: string, replicas: number): Promise<void> {
    const t0 = Date.now();
    const port = this.c.getTcpPort(8080);
    const w = await port.fetch(`http://container/wait?key=${key}`);
    const done = w.ok ? await w.json<{ size: number; error?: string }>() : { size: 0, error: `wait: ${w.status} ${await w.text()}` };
    const tPut = Date.now();
    // Replica 0 first, alone, so the edge can fall back to it while the others are written.
    const put = async (i: number) => {
      const f = await port.fetch(`http://container/f?path=cc/${key}`);
      await this.env.BUCKET.put(`cc/${key}/${i}`, f.body!.pipeThrough(new FixedLengthStream(done.size)));
    };
    let firstMs = 0;
    if (!done.error) {
      await put(0);
      firstMs = Date.now() - tPut;
      await Promise.all(Array.from({ length: replicas - 1 }, (_, i) => put(i + 1)));
    }
    await this.ctx.storage.put(`fill:${key}`, { at: t0, genMs: tPut - t0, firstMs, putMs: Date.now() - tPut, size: done.size, replicas, error: done.error });
  }

  async fills(): Promise<object[]> { return [...(await this.ctx.storage.list({ prefix: "fill:" })).values()] as object[]; }

  /** A CI load client, created with a location hint and run from its alarm: `rate` clones a second for `seconds`.
   *  The tip moves every `everyMs` from `start`, the same schedule on every client, and each clone wants the tip as of
   *  `lagMs` ago (0: the current tip; more: the time from a push to its CI job's checkout). */
  async ciStart(cfg: CiCfg): Promise<void> {
    await this.ctx.storage.deleteAll();
    await this.ctx.storage.put("ci", cfg);
    await this.ctx.storage.setAlarm(Math.max(Date.now(), cfg.start));
  }

  async alarm(): Promise<void> {
    const cfg = await this.ctx.storage.get<CiCfg>("ci");
    if (!cfg) return;
    const colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1];
    const rows: object[] = [];
    const end = cfg.start + cfg.seconds * 1000;
    const inflight = new Set<Promise<void>>();
    // With `prefill`, client 0 also plays the push hook: when tip k appears, it asks the shard to fill k's entry.
    if (cfg.prefill && cfg.index === 0) {
      const env = this.env;
      inflight.add((async () => {
        for (let k = 0; cfg.start + k * cfg.everyMs < end; k++) {
          const at = cfg.start + k * cfg.everyMs;
          if (at > Date.now()) await new Promise((r) => setTimeout(r, at - Date.now()));
          const tip = cfg.tips[Math.min((cfg.offset ?? 0) + k, cfg.tips.length - 1)];
          await cloneCache({ net: cfg.net, tip, run: cfg.run, replicas: cfg.replicas }, env, true).catch(() => undefined);
        }
      })());
    }
    for (let n = 0; ; n++) {
      const at = cfg.start + (n + Math.random()) * (1000 / cfg.rate);
      if (at >= end) break;
      if (at > Date.now()) await new Promise((r) => setTimeout(r, at - Date.now()));
      // An alarm that wakes late (an evicted or restarted object) skips the slots it missed rather than firing them
      // all at once, and records the gap.
      if (Date.now() - at > 5000) { rows.push({ t: at - cfg.start, idx: -1, status: -1, error: "skipped: client late", ms: 0 }); continue; }
      // Tip index as of `lag` ago, lag uniform in [lagMs, lagMaxMs]; tips before the run's start are history, so the
      // run opens with the steady state rather than every client on tip 0.
      const lag = cfg.lagMs + Math.random() * ((cfg.lagMaxMs ?? cfg.lagMs) - cfg.lagMs);
      const idx = Math.max(0, (cfg.offset ?? 0) + Math.floor((Date.now() - lag - cfg.start) / cfg.everyMs));
      const spec: CloneSpec = { net: cfg.net, tip: cfg.tips[Math.min(idx, cfg.tips.length - 1)], run: cfg.run, replicas: cfg.replicas, read: cfg.read, claim: cfg.claim };
      const t0 = Date.now();
      const p = (async () => {
        try {
          const r = await fetch(`${SELF}/cc`, { method: "POST", headers: { authorization: `Bearer ${this.env.M0_TOKEN}` }, body: JSON.stringify(spec),
            signal: AbortSignal.timeout(60_000) });
          if (r.status !== 200) {
            rows.push({ t: t0 - cfg.start, idx, status: r.status, cc: r.headers.get("x-cc"), error: (await r.text()).slice(0, 160), ms: Date.now() - t0 });
            return;
          }
          let bytes = 0, ttfb = -1;
          const reader = r.body!.getReader();
          for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            if (ttfb < 0) ttfb = Date.now() - t0;
            bytes += value.byteLength;
            if (cfg.read && bytes >= cfg.read) { await reader.cancel(); break; }
          }
          rows.push({ t: t0 - cfg.start, idx, status: r.status, cc: r.headers.get("x-cc"), r2Ms: Number(r.headers.get("x-r2-ms")), ttfb, bytes, ms: Date.now() - t0 });
        } catch (e) { rows.push({ t: t0 - cfg.start, idx, status: 0, error: String(e).slice(0, 200), ms: Date.now() - t0 }); }
      })();
      inflight.add(p);
      p.finally(() => inflight.delete(p));
    }
    await Promise.all(inflight);
    await this.ctx.storage.put("rows", { colo, rows });
  }

  async ciResult(): Promise<object | undefined> { return this.ctx.storage.get("rows"); }
}

type CiCfg = { net: string; tips: string[]; run: string; replicas: number; read: number; rate: number; seconds: number; start: number;
  everyMs: number; lagMs: number; lagMaxMs?: number; offset?: number; prefill?: boolean; index?: number; claim?: boolean };

// ---- B0-20 / B0-21, the edge's own push cap ----
// A minimal smart-HTTP receive-pack: an empty advertisement, then a POST read as a stream and counted. Past `cap` MB
// the edge stops reading and answers with a side-band report-status that names SSH; under it, it reports `unpack ok`
// and refuses the ref, so nothing is stored. Path: /push/<cap MB>/<repo>.git/...
const sb = (band: number, s: string) => pkt(String.fromCharCode(band) + s);
async function pushCap(req: Request, url: URL): Promise<Response> {
  const cap = Number(url.pathname.split("/")[2]) * 1024 * 1024;
  if (url.pathname.endsWith("/info/refs")) {
    const adv = pkt("# service=git-receive-pack\n") + "0000" +
      pkt("0000000000000000000000000000000000000000 capabilities^{}\0report-status side-band-64k quiet object-format=sha1 agent=graft-m0\n") + "0000";
    return new Response(adv, { headers: { "content-type": "application/x-git-receive-pack-advertisement", "cache-control": "no-cache" } });
  }
  let n = 0, refused = false;
  const reader = req.body!.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    n += value.byteLength;
    if (n > cap) { refused = true; await reader.cancel().catch(() => undefined); break; }
  }
  const mb = (cap / 1048576).toFixed(0);
  const msg = refused
    ? sb(2, `Push refused: over ${mb} MB, this plan's HTTPS push limit. Push over SSH: git remote set-url --push origin git@HOST:o/r.git\n`) +
      sb(1, pkt(`unpack push exceeds ${mb} MB over HTTPS, use SSH\n`) + pkt("ng refs/heads/main push over HTTPS cap, use SSH\n") + "0000")
    : sb(1, pkt("unpack ok\n") + pkt(`ng refs/heads/main test edge stores nothing (${n} bytes read)\n`) + "0000");
  return new Response(msg + "0000", { headers: { "content-type": "application/x-git-receive-pack-result", "x-read": String(n), "x-refused": String(refused) } });
}

const FOLLOW_MAX = 8;
const CLAIM_STALE = 15_000;
const pkt = (s: string) => (s.length + 4).toString(16).padStart(4, "0") + s;
const hex = async (s: string) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(s)))]
  .map((b) => b.toString(16).padStart(2, "0")).join("");
const SELF = "https://graft-m0-rehydrate.<workers-subdomain>.workers.dev";
type CloneSpec = { net: string; tip: string; run: string; replicas: number; depth?: number; read?: number; claim?: boolean };

/** The edge side of a CI clone: `CloneKey` from (network, wants, depth, capabilities), here salted with the run so
 *  each run starts cold. A random replica is read from the bucket; on a miss the network's shard answers. */
async function cloneCache(spec: CloneSpec, env: Env, prefill = false): Promise<Response> {
  const depth = spec.depth ?? 1;
  const key = await hex(`${spec.net}|${spec.tip}|${depth}|thin-pack,ofs-delta,no-progress|${spec.run}`);
  const t = Date.now();
  if (prefill) {
    const stub = env.SHARD.get(env.SHARD.idFromName(`cc-${spec.net}`), { locationHint: "enam" });
    const body = pkt("command=fetch") + pkt("object-format=sha1") + "0001" + pkt("thin-pack") + pkt("no-progress") +
      pkt("ofs-delta") + pkt(`deepen ${depth}`) + pkt(`want ${spec.tip}\n`) + pkt("done\n") + "0000";
    const r = await stub.fetch(`http://shard/cc?key=${key}&net=${spec.net}&replicas=${spec.replicas}`, { method: "POST", body });
    await r.body?.cancel();
    return Response.json({ status: r.status, cc: r.headers.get("x-cc"), ms: Date.now() - t });
  }
  // A random replica, then replica 0, which the fill writes first; a miss on both goes to the shard.
  const i = Math.floor(Math.random() * spec.replicas);
  const o = await env.BUCKET.get(`cc/${key}/${i}`) ?? (i ? await env.BUCKET.get(`cc/${key}/0`) : null);
  if (o) return new Response(o.body, { headers: { "x-cc": o.key.endsWith(`/${i}`) ? "hit" : "hit0", "x-r2-ms": String(Date.now() - t) } });
  const body = pkt("command=fetch") + pkt("object-format=sha1") + "0001" + pkt("thin-pack") + pkt("no-progress") +
    pkt("ofs-delta") + pkt(`deepen ${depth}`) + pkt(`want ${spec.tip}\n`) + pkt("done\n") + "0000";
  // With `claim`, a miss first makes a create-only claim object for the key. Only the edge that creates it calls the
  // shard; every other miss waits for replica 0 in the bucket, and takes over the fill only if the claim is older than
  // CLAIM_STALE with no replica. So the shard object sees about one request per key, not the herd.
  if (spec.claim) {
    const claimed = await env.BUCKET.put(`cc/${key}/claim`, String(Date.now()), { onlyIf: new Headers({ "if-none-match": "*" }) });
    if (!claimed) {
      for (const end = Date.now() + CLAIM_STALE; Date.now() < end;) {
        await new Promise((res) => setTimeout(res, 100));
        const w = await env.BUCKET.get(`cc/${key}/0`);
        if (w) return new Response(w.body, { headers: { "x-cc": "waited", "x-r2-ms": String(Date.now() - t) } });
      }
    }
  }
  const stub = env.SHARD.get(env.SHARD.idFromName(`cc-${spec.net}`), { locationHint: "enam" });
  const r = await stub.fetch(`http://shard/cc?key=${key}&net=${spec.net}&replicas=${spec.replicas}`, { method: "POST", body })
    .catch((e: Error) => new Response(`stub: ${String(e).slice(0, 300)}`, { status: 502 }));
  if (r.status === 202) {
    // Being filled: wait for replica 0, which the fill writes first.
    for (const end = Date.now() + 20_000; Date.now() < end;) {
      await new Promise((res) => setTimeout(res, 150));
      const w = await env.BUCKET.get(`cc/${key}/0`);
      if (w) return new Response(w.body, { headers: { "x-cc": "waited", "x-r2-ms": String(Date.now() - t) } });
    }
    return new Response("fill did not land in 20 s", { status: 504, headers: { "x-cc": "wait" } });
  }
  return new Response(r.body, { status: r.status, headers: { "x-cc": r.headers.get("x-cc") ?? "?", "x-r2-ms": String(Date.now() - t) } });
}

/** One CI client: fetches the clone through the public hostname, as a runner would, and times the first byte. Reads
 *  `read` bytes then cancels, or the whole response when `read` is 0. */
async function cloneClient(spec: CloneSpec, auth: string, colo: string | undefined): Promise<Response> {
  const t0 = Date.now();
  const r = await fetch(`${SELF}/cc`, { method: "POST", headers: { authorization: auth }, body: JSON.stringify(spec) });
  const headersMs = Date.now() - t0;
  let bytes = 0, ttfb = -1;
  const reader = r.body!.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (ttfb < 0) ttfb = Date.now() - t0;
    bytes += value.byteLength;
    if (spec.read && bytes >= spec.read) { await reader.cancel(); break; }
  }
  return Response.json({ status: r.status, cc: r.headers.get("x-cc"), r2Ms: Number(r.headers.get("x-r2-ms")), headersMs, ttfb, bytes,
    ms: Date.now() - t0, colo });
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    // git sends the token as the Basic password.
    const basic = `Basic ${btoa(`m0:${env.M0_TOKEN}`)}`;
    if (url.pathname.startsWith("/push/")) {
      if (req.headers.get("authorization") !== basic) return new Response("unauthorized", { status: 401, headers: { "www-authenticate": 'Basic realm="m0"' } });
      return pushCap(req, url);
    }
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    // Upload one object to the bucket by streaming the request body (the runner seeds the network this way).
    if (req.method === "PUT" && url.pathname === "/put") {
      const key = url.searchParams.get("key")!;
      const o = await env.BUCKET.put(key, req.body, { onlyIf: undefined });
      return Response.json({ key, size: o?.size });
    }
    if (url.pathname === "/cc") return cloneCache(await req.json<CloneSpec>(), env);
    if (url.pathname === "/load") return cloneClient(await req.json<CloneSpec>(), req.headers.get("authorization")!, (req.cf as { colo?: string } | undefined)?.colo);
    const body = await req.json<{ net: string; hint?: DurableObjectLocationHint; name?: string; instance?: string; parallel?: number;
      op?: string; mb?: number; packMb?: number; files?: number; commits?: number; clients?: number; hints?: DurableObjectLocationHint[]; cfg?: CiCfg }>();
    const { net, hint = "enam", name, instance = "standard-4", parallel = 4, op, mb = 5120, packMb = 512, files = 10000, commits = 200 } = body;
    if (op === "mono") return Response.json(await env.SHARD.get(env.SHARD.idFromName(`cc-${net}`), { locationHint: hint }).mono(net, files, commits, instance));
    if (op === "ci" || op === "ciResult") {
      const { clients = 0, hints = ["enam"], cfg } = body as Required<typeof body>;
      const stubs = Array.from({ length: clients }, (_, i) => env.SHARD.get(env.SHARD.idFromName(`ci-${cfg.run}-${i}`), { locationHint: hints[i % hints.length] }));
      if (op === "ci") { await Promise.all(stubs.map((s, index) => s.ciStart({ ...cfg, index }))); return Response.json({ started: clients }); }
      return Response.json(await Promise.all(stubs.map((s) => s.ciResult())));
    }
    if (op === "fills") return Response.json(await env.SHARD.get(env.SHARD.idFromName(`cc-${net}`), { locationHint: hint }).fills());
    const stub = env.SHARD.get(env.SHARD.idFromName(name ?? `b0-22-${hint}`), { locationHint: hint });
    if (op === "stop") { await stub.stop(); return Response.json({ stopped: true }); }
    if (op === "seed") return Response.json(await stub.seed(net, mb, packMb, instance));
    return Response.json(await stub.rehydrate(net, instance, parallel));
  },
} satisfies ExportedHandler<Env>;
