// load34.test.mjs's stand-ins, in the same local workerd as the client: a fake edge (`fetch`, which the client reaches
// at `spec.edge`) and a fake member class bound as MEMBER_0..2. Every request the edge gets is kept, with the body's
// sha256 for receive-packs, so the test can check what the client sent. The edge answers like Graftyard's: ls-refs and
// the receive-pack advertisement from its refs, report-status for receive-packs, a clone-cache hit or miss header on
// upload-pack fetches with `server-timing`, and JSON for the API. `/_fake/log` returns what it saw.
import { DurableObject } from "cloudflare:workers";
import { Buffer } from "node:buffer";
import { onePack } from "../rest/wire.mjs";
import loader, { Load34 } from "./load34.mjs";

export { Load34 };

const enc = new TextEncoder();
const pkt = (s) => { const b = enc.encode(s); return (b.length + 4).toString(16).padStart(4, "0") + s; };
const hex = async (b) => [...new Uint8Array(await crypto.subtle.digest("SHA-256", b))].map((x) => x.toString(16).padStart(2, "0")).join("");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The edge's state and log, one object so every client's request lands in one place. */
export class FakeEdge extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS log (e TEXT)");
    ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS refs (ref TEXT PRIMARY KEY, sha TEXT)");
  }
  get refs() {
    const sql = this.ctx.storage.sql;
    return { get: (r) => sql.exec("SELECT sha FROM refs WHERE ref = ?", r).toArray()[0]?.sha,
      set: (r, v) => sql.exec("INSERT OR REPLACE INTO refs VALUES (?, ?)", r, v),
      [Symbol.iterator]: () => sql.exec("SELECT ref, sha FROM refs").toArray().map((x) => [x.ref, x.sha])[Symbol.iterator]() };
  }
  async fetch(req) {
    const url = new URL(req.url), body = req.method === "POST" || req.method === "PUT" ? await req.arrayBuffer() : new ArrayBuffer(0);
    if (url.pathname === "/_fake/log") return Response.json({ log: this.ctx.storage.sql.exec("SELECT e FROM log").toArray().map((x) => JSON.parse(x.e)) });
    if (url.pathname === "/_fake/main") { this.refs.set("refs/heads/main", url.searchParams.get("sha")); return new Response("ok"); }
    {
      const text = new TextDecoder("latin1").decode(body);
      const e = { at: Date.now(), method: req.method, path: url.pathname + url.search, auth: req.headers.get("authorization") ?? "", bytes: body.byteLength };
      // The edge's wire gate: every surface needs a User-Agent (`graftyard_wire::serve`), and workerd adds none.
      const answer = req.headers.get("user-agent")?.trim() ? await this.answer(url, text, body, e)
        : new Response("Request forbidden by administrative rules. Please make sure your request has a User-Agent header.\n", { status: 403 });
      this.ctx.storage.sql.exec("INSERT INTO log VALUES (?)", JSON.stringify(e));
      return answer;
    }
  }
  async answer(url, text, body, e) {
    {
      await sleep(Number(this.env.EDGE_DELAY_MS ?? 20));
      if (url.pathname.endsWith("/info/refs")) {
        const main = this.refs.get("refs/heads/main") ?? "0".repeat(40);
        return new Response(`${pkt(`${main} refs/heads/main\0report-status\n`)}0000`);
      }
      if (url.pathname.endsWith("/git-receive-pack")) {
        e.sha256 = await hex(body);
        const m = text.match(/^[0-9a-f]{4}([0-9a-f]{40}) ([0-9a-f]{40}) (\S+)\0/);
        e.ref = m?.[3]; e.old = m?.[1]; e.new = m?.[2];
        if (m) this.refs.set(m[3], m[2]);
        return new Response(`${pkt("unpack ok\n")}${pkt(`ok ${m?.[3]}\n`)}0000`, { headers: { "server-timing": "total;dur=7" } });
      }
      if (url.pathname.endsWith("/git-upload-pack")) {
        if (text.includes("command=ls-refs")) {
          const prefix = text.match(/ref-prefix (\S+)\n/)?.[1] ?? "";
          e.lsRefs = prefix;
          const lines = [...this.refs].filter(([r]) => r.startsWith(prefix)).map(([r, s]) => pkt(`${s} ${r}\n`)).join("");
          return new Response(`${lines}0000`, { headers: { "x-graft-ls-refs": "edge" } });
        }
        e.want = text.match(/want ([0-9a-f]{40})/)?.[1];
        e.deepen = text.includes("deepen 1");
        const miss = e.want?.endsWith("0");
        return new Response(new Uint8Array(4096), { headers: { "x-graft-clone-cache": miss ? "miss" : "hit", ...(miss ? { "x-graft-compose": "composed" } : {}), "server-timing": `front;dur=4, ${miss ? "" : "chain_open;dur=6, "}total;dur=11` } });
      }
      if (url.pathname.startsWith("/api/")) return Response.json(url.pathname.endsWith("/leases") ? { lease: { id: `L${e.at}${Math.random()}` } } : { data: { addComment: {}, createPullRequest: { pullRequest: { id: "p", number: 1 } } } }, { status: url.pathname.endsWith("/leases") ? 201 : 200, headers: { "server-timing": "auth;dur=3, total;dur=5" } });
      return new Response("not found", { status: 404 });
    }
  }
}

/** A member: `/status` names a config with one learner, and `/feed` holds 50 ms then answers `hold` with one event for
 *  the newest agent push the edge took, so feed latency can be joined. */
export class FakeMember extends DurableObject {
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/status") return Response.json({ role: "follower", doName: "x", config: { index: 1, config: { normal: { voters: [0, 1, 2].map((s) => ({ site: s, do_name: `f${s}` })), learners: [{ site: 0, do_name: "l0" }] } } } });
    if (url.pathname === "/feed") {
      const b = await req.json();
      await sleep(50);
      const log = (await (await this.env.EDGE.get(this.env.EDGE.idFromName("edge")).fetch("https://edge/_fake/log")).json()).log;
      const last = log.filter((x) => x.ref?.includes("/claude/")).pop();
      return Response.json({ admit: "hold", cursor: (b.cursor ?? 0) + 1, events: last ? [[0, 0, { AgentPush: { head: last.new } }]] : [] });
    }
    return new Response("?", { status: 404 });
  }
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    // Edge requests come to this script's own origin under /edge.
    if (url.pathname.startsWith("/edge/") || url.pathname === "/edge") {
      const u = new URL(req.url); u.pathname = u.pathname.slice(5) || "/";
      const body = req.method === "GET" || req.method === "HEAD" ? undefined : await req.arrayBuffer();
      return env.EDGE.get(env.EDGE.idFromName("edge")).fetch(u, { method: req.method, headers: req.headers, body });
    }
    // wire.mjs's builder run in workerd, on load34.test.mjs's fixed input.
    if (url.pathname === "/_fake/pack") {
      const fixed = Buffer.from(Array.from({ length: 40_000 }, (_, i) => (i * 31 + (i >> 7)) & 255));
      const ps = [onePack("a1/3/7/f.txt", fixed, "1234567890abcdef1234567890abcdef12345678", "fixed", 1_700_000_000),
        onePack("b034/4.txt", Buffer.from("x\n"), null, "root", 1_700_000_001, [["dir/sub/a.txt", Buffer.from("a")], ["dir/b", Buffer.from("b")]])];
      return Response.json(await Promise.all(ps.map(async (p) => [await hex(p.pack), p.commit, p.pack.length])));
    }
    return loader.fetch(req, env);
  },
};
