// M0-WP16 predictor shard, `graft-wp16-shard`: one container class, `PredictShard`, under the `durable_object` scheduling
// policy, as src/rehydrate.ts's `Shard` (M0-WP6), which it copies in part. Its own script and its own bucket
// `graft-wp16-net`, so nothing here touches another bed.
//
// A shard object starts a `standard-4` container (git and wp16/shard/main.go) with `enableInternet: false`, streams the
// replay network's pack and refs from R2 into it, indexes them, and then runs `predict` batches: real
// `git merge-tree --write-tree --name-only` pairs at most PREDICT_CPU * vCPU at once. CPU is the container's own cgroup
// counter, read before and after each batch.
//
// The edge (`fetch`, behind the harness bearer token) also takes the pack into R2 by multipart upload from the runner,
// and forwards `predict`, `cpu`, and `leads` calls to a named shard. `leads` marks the shard's site as the network's
// leading site, which pauses `predict` (`design/AGENT-LAYER.md`, Shape, `GitCacheShard::predict`).

import { DurableObject } from "cloudflare:workers";

interface Env { SHARD: DurableObjectNamespace<PredictShard>; NET: R2Bucket; M0_TOKEN: string }

type Container = NonNullable<DurableObjectState["container"]>;

export class PredictShard extends DurableObject<Env> {
  private get c(): Container { return this.ctx.container!; }

  private async up(): Promise<number> {
    const t = Date.now();
    if (!this.c.running) this.c.start({ image: (this.c as unknown as { images: Record<string, string> }).images.git, instance: "standard-4", enableInternet: false } as never);
    await (this.c as unknown as { setInactivityTimeout(ms: number): Promise<void> }).setInactivityTimeout(30 * 60_000);
    const port = this.c.getTcpPort(8080);
    for (let i = 0; i < 300; i++) {
      try { if ((await port.fetch("http://container/health")).ok) return Date.now() - t; } catch { /* starting */ }
      await new Promise((r) => setTimeout(r, 200));
    }
    throw new Error("container did not answer within 60 s");
  }

  private async sh(cmd: string): Promise<string> {
    const p = await (this.c as unknown as { exec(a: string[]): Promise<{ output(): Promise<{ stdout: ArrayBuffer }> }> }).exec(["sh", "-c", cmd]);
    return new TextDecoder().decode((await p.output()).stdout);
  }

  /** Starts the container and loads the network from R2: `net/<name>/pack.pack` and `net/<name>/refs`. */
  async load(name: string): Promise<object> {
    const t0 = Date.now();
    const startMs = await this.up();
    const env = await this.sh("tr '\\0' '\\n' < /proc/1/environ | grep -E '^CLOUDFLARE_(REGION|LOCATION)='; nproc");
    const port = this.c.getTcpPort(8080);
    const tCopy = Date.now();
    let bytes = 0;
    for (const [key, path] of [[`net/${name}/pack.pack`, "pack.pack"], [`net/${name}/refs`, "refs"], [`net/${name}/shallow`, "shallow"]]) {
      const o = await this.env.NET.get(key);
      if (!o) throw new Error(`missing ${key}`);
      bytes += o.size;
      const r = await port.fetch(`http://container/f?path=${path}`, { method: "PUT", body: o.body, headers: { "content-length": String(o.size) } });
      if (!r.ok) throw new Error(`put ${path}: ${r.status} ${await r.text()}`);
    }
    const copyMs = Date.now() - tCopy;
    const setup = await (await port.fetch("http://container/setup", { method: "POST" })).text();
    return { startMs, copyMs, bytes, setup, env, totalMs: Date.now() - t0 };
  }

  /** Whether this shard's site leads the network's groups. Stored, so it survives an object restart. */
  async setLeads(leads: boolean): Promise<object> { await this.ctx.storage.put("leads", leads); return { leads }; }

  /** One InsightJob: `head` against `others`, each landed on `base` first (wp16/shard/main.go). Paused while the site
   * leads: every pair is `Stale` and the container runs nothing. */
  async predict(job: { base: string; head: string; others: string[] }, budget: number, vcpu: number): Promise<object> {
    if (await this.ctx.storage.get<boolean>("leads")) return { paused: true, verdicts: job.others.map(() => ({ v: "Stale" })) };
    await this.up();
    const r = await this.c.getTcpPort(8080).fetch("http://container/predict", { method: "POST", body: JSON.stringify({ ...job, budget, vcpu }) });
    if (!r.ok) throw new Error(`predict: ${r.status} ${await r.text()}`);
    return r.json();
  }

  async cpu(): Promise<object> {
    if (!this.c.running) return { running: false };
    return (await this.c.getTcpPort(8080).fetch("http://container/cpu")).json();
  }

  async stop(): Promise<void> { if (this.c.running) await (this.c as unknown as { destroy(): Promise<void> }).destroy(); }
}

const json = (v: unknown, status = 200) => Response.json(v, { status });

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url), q = url.searchParams;
    try {
      // Multipart upload of the network's pack from the runner: start, part, done.
      if (url.pathname === "/mpu/start") return json({ uploadId: (await env.NET.createMultipartUpload(q.get("key")!)).uploadId });
      if (url.pathname === "/mpu/part") {
        const u = env.NET.resumeMultipartUpload(q.get("key")!, q.get("uploadId")!);
        return json(await u.uploadPart(Number(q.get("n")), req.body!));
      }
      if (url.pathname === "/mpu/done") {
        const u = env.NET.resumeMultipartUpload(q.get("key")!, q.get("uploadId")!);
        const o = await u.complete(await req.json());
        return json({ key: o.key, size: o.size });
      }
      if (url.pathname === "/put") { const o = await env.NET.put(q.get("key")!, req.body); return json({ size: o?.size }); }
      const body = req.method === "POST" ? await req.json<Record<string, any>>() : {};
      const stub = env.SHARD.get(env.SHARD.idFromName(body.shard ?? "s0"), { locationHint: body.hint ?? "weur" });
      switch (url.pathname) {
        case "/load": return json(await stub.load(body.net));
        case "/predict": return json(await stub.predict(body as { base: string; head: string; others: string[] }, body.budget ?? 0.5, body.vcpu ?? 4));
        case "/cpu": return json(await stub.cpu());
        case "/leads": return json(await stub.setLeads(!!body.leads));
        case "/stop": await stub.stop(); return json({ ok: true });
      }
    } catch (e) {
      return json({ error: String(e) }, 500);
    }
    return json({ error: "not found" }, 404);
  },
} satisfies ExportedHandler<Env>;
