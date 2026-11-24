// quietbed's entry (`run.sh`): the site script's own build (`workers/site`, test-faults) with one driver route in
// front. `/drive?chain=repo:<n>&refuse=1` joins one test-register group (`repo:<n>`, which `Kind::of` hosts as the
// register and `Source::Given` trusts) on sites 0 and 2, and on site 1 too unless `refuse=1`, so site 1's member
// answers every AppendEntries 409 "member not joined", as a half-made chain's unjoined member does. It elects site 0,
// commits a put, waits until the leader's drain-check alarm has stopped (DRAIN_IDLE_MS after the put, plus margin),
// then counts the leader's calls to site 1 over WINDOW_MS. Every other request goes to the site script unchanged.
import site, * as classes from "./out/index.js";

export const { Member, Registry, GitShard, HookSequencer, Inbox, LogRelay, RunnerBroker, SearchShard } = classes;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The leader's alarm re-arms only while it served in the last DRAIN_IDLE_MS (60 s); 75 s leaves its last run behind.
const SETTLE_MS = 75_000;
const WINDOW_MS = 15_000;

function rid() {
  const A = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let v = BigInt(Date.now()) & 0xffff_ffff_ffffn;
  for (const b of crypto.getRandomValues(new Uint8Array(10))) v = (v << 8n) | BigInt(b);
  let out = "";
  for (let i = 25; i >= 0; i--) out += A[Number((v >> BigInt(5 * i)) & 31n)];
  return out;
}

async function drive(env, chain, refuse) {
  const record = { name: "Tri", sites: [0, 1, 2], home: 0, generation: 1, moving_from: null, at: 1 };
  const call = async (s, name, path, body) => {
    const r = await env[`MEMBER_${s}`].getByName(name).fetch(`https://member${path}`,
      body === undefined ? {} : { method: "POST", body: JSON.stringify(body) });
    const x = await r.text(); let v = x; try { v = JSON.parse(x); } catch {}
    return { status: r.status, body: v };
  };
  const name = (s) => `${chain}#1#f${s}`;
  for (const s of refuse ? [0, 2] : [0, 1, 2]) {
    const me = { site: s, kind: "full", slot: s, generation: 1, do_name: name(s) };
    const j = await call(s, name(s), "/join", { chain, me, record });
    if (j.status !== 200) return { join: s, status: j.status, body: j.body };
  }
  let role = null;
  for (let i = 0; i < 50 && role !== "leader"; i++) {
    await call(0, name(0), "/campaign", {});
    role = (await call(0, name(0), "/status")).body?.role ?? null;
    if (role !== "leader") await sleep(200);
  }
  const put = await call(0, name(0), "/commit", { rid: rid(), cmd: { kind: "put", key: "k", value: "v" } });
  // `/stats` and `/status` only read: neither steps the core, so neither keeps the ticker up. An object with no input,
  // no alarm and no timer is evicted and restores with a new `boot` on its next request, as a follower with no calls.
  const boot = async () => (await call(0, name(0), "/status")).body?.boot ?? null;
  const sent = async () => (await call(0, name(0), "/stats")).body?.peers?.["1"]?.sent ?? 0;
  const bootPut = await boot();
  await sleep(SETTLE_MS);
  const bootSettled = await boot();
  const before = await sent();
  await sleep(WINDOW_MS);
  const after = await sent();
  const status = (await call(0, name(0), "/status")).body;
  const unjoined = (await call(1, name(1), "/status")).body?.joined === false;
  // A quiet leader still commits: the next input starts its ticker again.
  const put2 = await call(0, name(0), "/commit", { rid: rid(), cmd: { kind: "put", key: "k", value: "w" } });
  return { role, put: put.status, evicted: bootPut !== null && bootSettled !== bootPut, before, after,
    windowCalls: after - before, leaderAfter: status?.role ?? null,
    commit: status?.commit ?? null, site1Unjoined: unjoined, put2: put2.status, put2Done: !!put2.body?.done?.ok };
}

export default class extends site {
  async fetch(req) {
    const url = new URL(req.url);
    if (url.pathname === "/drive") {
      return Response.json(await drive(this.env, url.searchParams.get("chain") ?? "repo:900301",
        url.searchParams.get("refuse") === "1"));
    }
    return super.fetch(req);
  }
}
