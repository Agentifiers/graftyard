// coldwrite's site entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with two additions.
// Every call to a `Member` that is not member-to-member Raft waits the site's `CALL_MS` first, a stand-in for the
// edge-to-member round trip to that site's region, and logs one `cwcall` line: when it arrived, how long it took, the
// site, the object's name, the path, and the edge's `x-graft-sent-by`. So an edge request's member calls, and which of
// them ran one after another, can be read from the log. `/mk?chain=C&record=R` joins a chain's three bootstrap members
// under record R and campaigns until one leads, as `graftyard_lookup::create` would.
import site, * as classes from "./out/index.js";

export const { Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const RAFT = new Set(["/raft", "/campaign", "/suspect", "/ping", "/rtt", "/join"]);

export class Member extends classes.Member {
  constructor(state, env) {
    super(state, env);
    this.cwName = state.id.name ?? "?";
    this.cwSite = env.SITE_ID;
    this.cwMs = Number(env.CALL_MS ?? 0);
  }
  async fetch(req) {
    const path = new URL(req.url).pathname;
    if (RAFT.has(path) || req.headers.get("x-cw-direct") === "1") return super.fetch(req);
    const t0 = Date.now();
    if (this.cwMs > 0) await sleep(this.cwMs);
    const r = await super.fetch(req);
    console.log(`cwcall ${t0} ${Date.now() - t0} ${this.cwSite} ${this.cwName} ${path} ${req.headers.get("x-graft-sent-by") ?? "-"} ${r.status}`);
    return r;
  }
}

async function mk(env, chain, record) {
  const stub = (s) => env[`MEMBER_${s}`].getByName(`${chain}#${record.generation}#f${s}`);
  const call = async (s, path, body) => {
    const r = await stub(s).fetch(`https://member${path}`, {
      method: body === undefined ? "GET" : "POST",
      headers: { "x-cw-direct": "1" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const t = await r.text();
    let v = t;
    try { v = JSON.parse(t); } catch {}
    return { status: r.status, body: v };
  };
  const joins = [];
  for (const s of record.sites) {
    const me = { site: s, kind: "full", slot: s, generation: record.generation, do_name: `${chain}#${record.generation}#f${s}` };
    joins.push((await call(s, "/join", { chain, me, record })).status);
  }
  let role = null;
  for (let i = 0; i < 50 && role !== "leader"; i++) {
    await call(record.home, "/campaign", {});
    role = (await call(record.home, "/status")).body?.role ?? null;
    if (role !== "leader") await sleep(200);
  }
  return { chain, joins, role };
}

export default class extends site {
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/mk") {
      const record = JSON.parse(url.searchParams.get("record"));
      return Response.json(await mk(this.env, url.searchParams.get("chain"), record));
    }
    return super.fetch(req);
  }
}
