// B0-13 probe (build/BUILD-PLAN.md, M0-WP1). Its own script, so deploying it never creates a site-script version.
// One `Probe` Durable Object per region hint reports its colo and times R2 reads through the binding of every test
// bucket. A bucket sits where its reads are fastest; the run compares that with the get-bucket API's `location`.

import { DurableObject } from "cloudflare:workers";
import { median } from "./sites.ts";

type ProbeEnv = { M0_TOKEN: string; PROBE: DurableObjectNamespace<Probe> } & Record<string, unknown>;

const KEY = "b0-13-probe";
const SAMPLES = 9;

export class Probe extends DurableObject<ProbeEnv> {
  async colo(): Promise<string> {
    const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
    return /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
  }

  /** Median read time per bucket binding, in ms, with the first (cold) read dropped. */
  async time(bindings: string[]): Promise<Record<string, number | string>> {
    const out: Record<string, number | string> = {};
    for (const b of bindings) {
      const bucket = this.env[b] as R2Bucket;
      try {
        const samples: number[] = [];
        for (let i = 0; i <= SAMPLES; i++) {
          const t0 = Date.now();
          const obj = await bucket.get(KEY);
          await obj?.arrayBuffer();
          if (i > 0) samples.push(Date.now() - t0);
        }
        out[b] = median(samples);
      } catch (e) {
        out[b] = `error: ${e}`;
      }
    }
    return out;
  }
}

export default {
  async fetch(req, env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const url = new URL(req.url);
    const body = await req.json<{ hint?: DurableObjectLocationHint; name?: string; bindings?: string[] }>();
    if (url.pathname === "/seed") {
      // Writes the probe object once per bucket, from wherever this request runs.
      await Promise.all(body.bindings!.map((b) => (env[b] as R2Bucket).put(KEY, "x".repeat(64))));
      return Response.json({ ok: true });
    }
    const stub = env.PROBE.get(env.PROBE.idFromName(body.name ?? `probe-${body.hint}`), { locationHint: body.hint });
    if (url.pathname === "/colo") return Response.json({ colo: await stub.colo() });
    if (url.pathname === "/time") return Response.json({ colo: await stub.colo(), ms: await stub.time(body.bindings!) });
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<ProbeEnv>;
