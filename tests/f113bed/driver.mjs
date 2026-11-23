// f113bed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with one driver route in front.
// `/drive?replaced=1` joins one test-register group (`repo:<n>`, which `Kind::of` hosts as the register and
// `Source::Given` trusts) on the three sites' `Member` bindings, elects site 0, commits a put, and with `replaced=1`
// replaces site 1's voter through the leader's own `/replace` (the placement retry m3fk2B's setup ran: a new
// generation on the same site, `Degraded` then `Normal`). Then it sends read-index reads to site 1's first-generation
// member, as the edge's spread pass does, and to its replacement, and reports each answer and its time. Every other
// request goes to the site script unchanged; the members' answers are the product's.
import site, * as classes from "./out/index.js";

export const { Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// Local workerd's sites are under a millisecond apart, so `Placement::new` refuses every try under MIN_PAIR_RTT (30 ms).
// A `/ping`, which a member's round-trip measure times, waits PING_MS first, as a region's round trip would; nothing
// else is delayed.
export class Member extends classes.Member {
  async fetch(req) {
    if (new URL(req.url).pathname === "/ping") await sleep(Number(this.env?.PING_MS ?? 40));
    return super.fetch(req);
  }
}

function rid() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

async function drive(env, chain, replaced) {
  const record = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
  const call = async (s, name, path, body) => {
    const t = Date.now();
    const r = await env[`MEMBER_${s}`].getByName(name).fetch(`https://member${path}`,
      body === undefined ? {} : { method: "POST", body: JSON.stringify(body) });
    const x = await r.text(); let v = x; try { v = JSON.parse(x); } catch {}
    return { status: r.status, body: v, ms: Date.now() - t };
  };
  const first = (s) => `${chain}#1#f${s}`;
  for (const s of record.sites) {
    const me = { site: s, kind: "full", slot: s, generation: 1, do_name: first(s) };
    const j = await call(s, first(s), "/join", { chain, me, record });
    if (j.status !== 200) return { join: s, status: j.status, body: j.body };
  }
  let role = null;
  for (let i = 0; i < 50 && role !== "leader"; i++) {
    await call(0, first(0), "/campaign", {});
    role = (await call(0, first(0), "/status")).body?.role ?? null;
    if (role !== "leader") await sleep(200);
  }
  const put = await call(0, first(0), "/commit", { rid: rid(), cmd: { kind: "put", key: "k", value: "v" } });
  let replace = null;
  if (replaced) replace = await call(0, first(0), "/replace", { site: 1 });
  // The edge's spread read: `{"q": .., "index": true}` to a follower.
  const read = (name) => call(1, name, "/read", { key: "k", index: true });
  // Until the replaced member answers `removed` and three reads after it, or 24 reads: its first check can find the
  // replacement still on the `Degraded` config, and it asks again at most every SUPERSEDED_CHECK_MS (5 s).
  const reads = [];
  for (let i = 0, after = 0; i < 24 && after < 4; i++) {
    const r = await read(first(1));
    const nl = r.body?.error?.notLeader;
    const x = { ms: r.ms, nl: nl ? { leader: nl.leader, configIndex: nl.config?.index ?? null } : null,
      overloaded: r.body?.overloaded !== undefined, value: r.body?.value ?? null, removed: r.body?.meta?.removed ?? false };
    reads.push(x);
    if (x.nl || after) after++;
    if (!replaced && i >= 3) break;
    await sleep(300);
  }
  const leader = (await call(0, first(0), "/status")).body;
  const voter1 = leader?.config?.config?.normal?.voters?.find((m) => m.site === 1)?.do_name ?? null;
  let fresh = null;
  if (voter1 && voter1 !== first(1)) {
    const r = await read(voter1);
    fresh = { name: voter1, ms: r.ms, value: r.body?.value ?? null, nl: !!r.body?.error?.notLeader, overloaded: r.body?.overloaded !== undefined };
  }
  const stats = (await call(1, first(1), "/stats")).body;
  return { role, put: put.status, replace: replace && { status: replace.status, body: replace.body },
    configIndex: leader?.config?.index ?? null, voter1, reads, fresh, removedReads: stats?.removedReads ?? null,
    followerLate: stats?.followerLate ?? null };
}

export default class extends site {
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/drive") {
      return Response.json(await drive(this.env, url.searchParams.get("chain") ?? "repo:900001",
        url.searchParams.get("replaced") === "1"));
    }
    return super.fetch(req);
  }
}
