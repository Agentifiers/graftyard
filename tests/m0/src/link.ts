// M0-WP5, B0-17: the account-separated link (design/DESIGN.md, `SiteLink::Https`). One source deployed as three
// throwaway scripts, graft-m0-link-a/b/c, each with one SQLite-backed `Member` class and a public workers.dev hostname.
// From a member placed on one site, time the same member call two ways to a member placed on another site:
//
//   - binding: the cross-script Durable Object binding with `script_name`, as members call each other by default;
//   - https: an Ed25519-signed POST to the other site Worker's public hostname. That Worker checks the signature and
//     the timestamp and forwards to its member over its own binding. The caller runs with `global_fetch_strictly_public`,
//     so the fetch goes out through the front door as a request from another account would.
//
// The member call is a ping that writes nothing, so the difference is the link, not storage. Separate scripts, so this
// creates no version of graft-raft-*, graft-wp4-*, or graft-site-*.

import { DurableObject } from "cloudflare:workers";

interface Env {
  SITE: "a" | "b" | "c";
  M0_TOKEN: string;
  /** PKCS8 base64 Ed25519 private key this site signs with, and the SPKI base64 public keys of every site. */
  LINK_KEY: string;
  LINK_PUBS: string;
  SUBDOMAIN: string;
  MEMBER_A: DurableObjectNamespace<Member>;
  MEMBER_B: DurableObjectNamespace<Member>;
  MEMBER_C: DurableObjectNamespace<Member>;
}

const HINT = { a: "enam", b: "weur", c: "apac" } as const;
const SKEW_MS = 30_000;
const b64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const summary = (xs: number[]) => ({ p50: pct(xs, 0.5), p90: pct(xs, 0.9), p99: pct(xs, 0.99), max: Math.max(...xs), n: xs.length });

let signKey: CryptoKey | undefined;
const verifyKeys = new Map<string, CryptoKey>();
async function signer(env: Env): Promise<CryptoKey> {
  return (signKey ??= await crypto.subtle.importKey("pkcs8", b64(env.LINK_KEY), { name: "Ed25519" }, false, ["sign"]));
}
async function verifier(env: Env, site: string): Promise<CryptoKey | undefined> {
  if (!verifyKeys.has(site)) {
    const pub = (JSON.parse(env.LINK_PUBS) as Record<string, string>)[site];
    if (!pub) return undefined;
    verifyKeys.set(site, await crypto.subtle.importKey("spki", b64(pub), { name: "Ed25519" }, false, ["verify"]));
  }
  return verifyKeys.get(site);
}
const member = (env: Env, site: "a" | "b" | "c", name: string) => {
  const ns = env[`MEMBER_${site.toUpperCase()}` as "MEMBER_A"];
  return ns.get(ns.idFromName(name), { locationHint: HINT[site] });
};

export class Member extends DurableObject<Env> {
  private colo = "";
  private async where(): Promise<string> {
    if (!this.colo) this.colo = /^colo=(\w+)$/m.exec(await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text())?.[1] ?? "?";
    return this.colo;
  }
  ping(body: string): number { return body.length; }
  async coloOf(): Promise<string> { return this.where(); }

  /** `n` alternating round trips to `peer` on `to`, by binding and by signed HTTPS, each with `bytes` of payload. */
  async probe(to: "a" | "b" | "c", peer: string, n: number, bytes: number) {
    const body = "x".repeat(bytes), stub = member(this.env, to, peer);
    const url = `https://graft-m0-link-${to}.${this.env.SUBDOMAIN}.workers.dev/member`;
    const key = await signer(this.env);
    const https = async () => {
      const msg = JSON.stringify({ from: this.env.SITE, at: Date.now(), peer, body });
      const sig = btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.sign("Ed25519", key, new TextEncoder().encode(msg)))));
      const r = await fetch(url, { method: "POST", body: msg, headers: { "x-link-sig": sig } });
      if (!r.ok) throw new Error(`https ${r.status} ${await r.text()}`);
      return r.text();
    };
    await stub.ping(body); await https(); // warm both paths
    const bind: number[] = [], web: number[] = [];
    let signMs = 0;
    for (let i = 0; i < n; i++) {
      let t = Date.now(); await stub.ping(body); bind.push(Date.now() - t);
      t = Date.now(); await https(); web.push(Date.now() - t);
    }
    { const t = performance.now(); for (let i = 0; i < 20; i++) await crypto.subtle.sign("Ed25519", key, new TextEncoder().encode(body)); signMs = (performance.now() - t) / 20; }
    return { from: await this.where(), to: await stub.coloOf(), bytes, binding: summary(bind), https: summary(web),
      overheadP50: pct(web, 0.5) - pct(bind, 0.5), signMs };
  }
}

export default {
  async fetch(req: Request, env: Env): Promise<Response> {
    const url = new URL(req.url);
    if (url.pathname === "/member" && req.method === "POST") {
      // The receiving site Worker: check the signature and the clock, then call its own member over its binding.
      const msg = await req.text(), sig = req.headers.get("x-link-sig") ?? "";
      const m = JSON.parse(msg) as { from: string; at: number; peer: string; body: string };
      const key = await verifier(env, m.from);
      if (!key || !(await crypto.subtle.verify("Ed25519", key, b64(sig), new TextEncoder().encode(msg)))) return new Response("bad signature", { status: 401 });
      if (Math.abs(Date.now() - m.at) > SKEW_MS) return new Response("stale", { status: 401 });
      return new Response(String(await member(env, env.SITE, m.peer).ping(m.body)));
    }
    if (req.headers.get("authorization") !== `Bearer ${env.M0_TOKEN}`) return new Response("unauthorized", { status: 401 });
    // Runner entry: from a member on `from`, probe a member on `to`.
    const { run, from, to, n = 50, bytes = 1024 } = await req.json<{ run: string; from: "a" | "b" | "c"; to: "a" | "b" | "c"; n?: number; bytes?: number }>();
    return Response.json(await member(env, from, `${run}-${from}`).probe(to, `${run}-${to}`, n, bytes));
  },
} satisfies ExportedHandler<Env>;
