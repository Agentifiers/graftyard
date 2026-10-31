// graft-m0-transport: does the way two members talk change the round trip that sets B0-01's commit latency?
// One Durable Object class, objects placed with the Raft prototype's hints (enam, weur, apac). From one object, time
// sequential round trips to another by RPC (what the prototype uses), by stub.fetch, and over a WebSocket opened once
// through stub.fetch. A separate script, so measuring it creates no version of graft-raft-* or graft-site-*.

import { DurableObject } from "cloudflare:workers";

interface Env { PEER: DurableObjectNamespace<Peer>; M0_TOKEN: string }

const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const summary = (xs: number[]) => ({ p50: pct(xs, 0.5), p90: pct(xs, 0.9), p99: pct(xs, 0.99), max: Math.max(...xs), n: xs.length });

export class Peer extends DurableObject<Env> {
  private colo = "";
  private async where(): Promise<string> {
    if (!this.colo) this.colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] ?? "?";
    return this.colo;
  }

  ping(body?: string): number { return body?.length ?? 0; }
  async colo_(): Promise<string> { return this.where(); }

  async fetch(req: Request): Promise<Response> {
    if (req.headers.get("Upgrade") === "websocket") {
      const [client, server] = Object.values(new WebSocketPair());
      server.accept();
      server.addEventListener("message", (m) => server.send(m.data as string)); // echo
      return new Response(null, { status: 101, webSocket: client });
    }
    return new Response(await req.text());
  }

  /** `n` sequential round trips to `peer` by each transport, with `bytes` of payload each way. */
  async probe(peer: string, hint: DurableObjectLocationHint, n: number, bytes: number) {
    const stub = this.env.PEER.get(this.env.PEER.idFromName(peer), { locationHint: hint });
    const body = "x".repeat(bytes);
    const peerColo = await stub.colo_();
    // Date.now() inside Workers advances only on I/O, which is what is timed here.
    const timeIO = async (f: () => Promise<unknown>) => { const xs: number[] = []; for (let i = 0; i < n; i++) { const t = Date.now(); await f(); xs.push(Date.now() - t); } return xs; };
    await stub.ping(body); // warm
    const rpc = await timeIO(() => stub.ping(body));
    const viaFetch = await timeIO(async () => (await stub.fetch("https://peer/", { method: "POST", body })).text());
    const up = await stub.fetch("https://peer/ws", { headers: { Upgrade: "websocket" } });
    const ws = up.webSocket!;
    ws.accept();
    let waiter: (() => void) | null = null;
    ws.addEventListener("message", () => { waiter?.(); waiter = null; });
    await new Promise<void>((r) => { waiter = r; ws.send(body); }); // warm
    const viaWs = await timeIO(() => new Promise<void>((r) => { waiter = r; ws.send(body); }));
    ws.close();
    return { from: await this.where(), to: peerColo, bytes, rpc: summary(rpc), fetch: summary(viaFetch), ws: summary(viaWs) };
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    const { run, n = 100, bytes = 100 } = await req.json<{ run: string; n?: number; bytes?: number }>();
    const a = env.PEER.get(env.PEER.idFromName(`${run}-a`), { locationHint: "enam" });
    const out = [
      await a.probe(`${run}-b`, "weur", n, bytes),
      await a.probe(`${run}-c`, "apac", n, bytes),
    ];
    return Response.json(out);
  },
} satisfies ExportedHandler<Env>;
