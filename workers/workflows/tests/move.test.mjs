// M6-WP1's Workflows glue (`../move.mjs`) on fake bindings and a fake `step`. Run with
// `node --test 'workers/workflows/tests/*.test.mjs'`. A step's result is kept by name, as the platform keeps it, so a
// replayed run skips the steps it finished.
import { test } from "node:test";
import assert from "node:assert/strict";
import { candidates, coordinate, expire, group, handle } from "../move.mjs";

const SITES = [
  { id: 0, region: "enam", spare: false },
  { id: 1, region: "weur", spare: false },
  { id: 2, region: "apac", spare: false },
  { id: 3, region: "wnam", spare: true },
];

class Step {
  done = new Map();
  log = [];
  async do(name, _cfg, fn) {
    if (this.done.has(name)) return this.done.get(name);
    const v = await fn();
    this.done.set(name, v);
    this.log.push(name);
    return v;
  }
  async sleep() {}
}

// One class on one site: `idFromName` is the name, and `get` hands the handler the name and the request.
function ns(handler) {
  return { idFromName: (n) => n, get: (n) => ({ fetch: (url, init) => handler(n, url, init) }) };
}

const ok = (v) => new Response(JSON.stringify(v), { status: 200 });

function workflow(log) {
  const status = new Map();
  return {
    status,
    async createBatch({ instances }) {
      const created = [];
      const errors = [];
      instances.forEach((x, index) => {
        if (status.has(x.id)) errors.push({ index, id: x.id, code: 10405, message: "exists" });
        else {
          status.set(x.id, { status: "running", params: x.params });
          created.push({ id: x.id });
        }
      });
      log.push(`createBatch ${instances.length}`);
      return { created, errors };
    },
    async get(id) {
      if (!status.has(id)) throw new Error("not found");
      return { status: async () => status.get(id) };
    },
    async create({ id, params }) {
      if (status.has(id)) throw new Error("instance exists (instance.already_exists)");
      status.set(id, { status: "running", params });
    },
  };
}

// Two surviving registries (sites 1 and 2). `a` joined on site 0 at generation 1, then moved to {1, 2, 0} at
// generation 2: site 1's shard still holds the generation 1 row, site 2's the generation 2 one. `b` never touched site
// 0. A `/refill` pass answers two pages.
function coordinatorEnv(log) {
  const rows = {
    1: { 5: [{ chain: "repo:7", do_name: "repo:7#1#f1", generation: 1, sites: [1, 2, 3] }],
      9: [{ chain: "repo:8", do_name: "repo:8#1#f1", generation: 1, sites: [1, 2] }] },
    2: { 5: [{ chain: "repo:7", do_name: "repo:7#2#f2", generation: 2, sites: [1, 2, 0] }] },
  };
  const env = { sites: SITES, GROUP: workflow(log) };
  for (const s of [1, 2]) {
    env[`REGISTRY_${s}`] = ns(async (name, url, init) => {
      if (url.endsWith("/refill")) {
        const b = JSON.parse(init.body);
        log.push(`refill ${b.single ? "single" : "all"} ${b.from}`);
        return ok({ done: 1, single: b.single ? 1 : 0, failed: [], next: b.from === null ? 40 : null });
      }
      const shard = Number(name.split("#")[1]);
      return ok(rows[s][shard] ?? []);
    });
  }
  return env;
}

test("the coordinator takes every chain at its highest generation, single-copy packs first", async () => {
  const log = [];
  const env = coordinatorEnv(log);
  const step = new Step();
  const event = { instanceId: "01JC0000000000000000000000", payload: { from: 0, to: 3, dead: true, home: null } };
  // Every group finishes as soon as it is looked at.
  const get = env.GROUP.get;
  env.GROUP.get = async (id) => {
    env.GROUP.status.get(id).status = "complete";
    return get(id);
  };
  const out = await coordinate(env, event, step);
  assert.equal(out.groups, 2);
  const started = [...env.GROUP.status.values()].map((v) => [v.params.chain, v.params.row.generation, v.params.row.site]);
  assert.deepEqual(started, [["repo:7", 2, 2], ["repo:8", 1, 1]]);
  assert.deepEqual(log, ["refill single null", "refill single 40", "createBatch 2", "refill all null", "refill all 40"]);
});

test("a coordinator replayed after its start step started nothing twice", async () => {
  const log = [];
  const env = coordinatorEnv(log);
  const event = { instanceId: "01JC0000000000000000000001", payload: { from: 0, to: 3, dead: false, home: 0 } };
  const first = new Step();
  // The first run stops at its first poll, after the groups started.
  env.GROUP.get = async () => {
    throw new Error("cut");
  };
  await assert.rejects(coordinate(env, event, first), /cut/);
  // A replay whose start step's result was lost runs createBatch again: every id answers 10405.
  first.done.delete("start 0");
  env.GROUP.get = async (id) => ({ status: async () => ({ status: "complete", id }) });
  assert.deepEqual(await coordinate(env, event, first), { groups: 2 });
  assert.equal(log.filter((l) => l.startsWith("createBatch")).length, 2);
  assert.equal(env.GROUP.status.size, 2);
  assert.ok(!log.some((l) => l.startsWith("refill")), "a restore refills nothing");
});

test("a coordinator errors when a group errors", async () => {
  const env = coordinatorEnv([]);
  env.GROUP.get = async (id) => ({
    status: async () => (id.endsWith("repo_8") ? { status: "errored", error: { message: "stuck" } } : { status: "complete" }),
  });
  const event = { instanceId: "01JC0000000000000000000002", payload: { from: 0, to: 3, dead: true, home: null } };
  await assert.rejects(coordinate(env, event, new Step()), /repo:8 failed: stuck/);
});

test("candidates keeps the highest generation whatever the order", () => {
  const r = (g) => ({ chain: "c", generation: g });
  assert.equal(candidates([r(3), r(1), r(2)])[0].generation, 3);
  assert.equal(candidates([r(1), r(3), r(2)])[0].generation, 3);
});

const P = (voters, home) => ({ voters, learners: [], home, generation: 2 });
const V = (site, gen = 1) => ({ site, kind: "full", slot: site, generation: gen, do_name: `repo:7#${gen}#f${site}` });

// A group whose row names its follower on site 1; the leader is on site 2. The lost member on site 0 throws.
test("a group follows notLeader to the leader, steps until Done, and starts its expiry", async () => {
  const before = { normal: P([V(0), V(1), V(2)], 0) };
  const after = { normal: P([V(3, 2), V(1), V(2)], 3) };
  const asked = [];
  let steps = 0;
  const env = { sites: SITES };
  for (const s of [0, 1, 2, 3]) {
    env[`MEMBER_${s}`] = ns(async (name, url, init) => {
      asked.push(`${s}${url.slice(url.lastIndexOf("/"))}`);
      if (s === 0) throw new Error("network connection lost");
      const b = JSON.parse(init.body);
      assert.deepEqual(b.replace, { lost: 0, spare: 3 });
      if (s !== 2) return ok({ error: { notLeader: { term: 4, leader: 2, config: { index: 9, config: before } } } });
      steps++;
      if (steps === 1) return ok({ step: "Swap", ok: { configIndex: 10 }, config: [10, before] });
      assert.equal(b.started, true);
      if (steps === 2) return ok({ step: "Leave", ok: { configIndex: 11 }, config: [11, after] });
      return ok({ step: "Done", ok: { step: "Done" }, config: [11, after] });
    });
  }
  const log = [];
  env.EXPIRE = workflow(log);
  const event = {
    instanceId: "01JC0000000000000000000003-repo_7",
    payload: { from: 0, to: 3, dead: true, chain: "repo:7", row: { site: 1, do_name: "repo:7#1#f1" } },
  };
  const out = await group(env, event, new Step());
  assert.deepEqual(out, { chain: "repo:7", asks: 4, removed: "repo:7#1#f0" });
  assert.deepEqual(asked, ["1/move", "2/move", "2/move", "2/move"]);
  const [x] = env.EXPIRE.status.values();
  assert.equal(x.params.member.do_name, "repo:7#1#f0");
  assert.equal(x.params.by.do_name, "repo:7#2#f3");
});

test("a group that never held the lost site is Done at once and expires no one", async () => {
  const env = { sites: SITES, EXPIRE: workflow([]) };
  env.MEMBER_1 = ns(async () => ok({ step: "Done", ok: { step: "Done" }, config: [3, { normal: P([V(1), V(2), V(3)], 1) }] }));
  const event = { instanceId: "g", payload: { from: 0, to: 3, dead: true, chain: "repo:8", row: { site: 1, do_name: "repo:7#1#f1" } } };
  assert.deepEqual(await group(env, event, new Step()), { chain: "repo:8", asks: 1, removed: null });
  assert.equal(env.EXPIRE.status.size, 0);
});

test("a group with no leader to follow asks the next live voter, never the lost one", async () => {
  const cfg = { normal: P([V(0), V(1), V(2)], 1) };
  const asked = [];
  const env = { sites: SITES, EXPIRE: workflow([]) };
  env.MEMBER_0 = ns(async () => assert.fail("the lost member was asked"));
  let n = 0;
  // Site 1 knows no leader yet, then site 2 resets, then site 1 leads.
  env.MEMBER_1 = ns(async () => {
    asked.push(1);
    if (n++ === 0) return ok({ error: { notLeader: { term: 5, leader: null, config: { index: 3, config: cfg } } } });
    return ok({ step: "Done", ok: { step: "Done" }, config: [3, cfg] });
  });
  env.MEMBER_2 = ns(async () => {
    asked.push(2);
    return new Response("member reset", { status: 503 });
  });
  const event = { instanceId: "g2", payload: { from: 0, to: 3, dead: true, chain: "repo:7", row: { site: 1, do_name: "repo:7#1#f1" } } };
  const out = await group(env, event, new Step());
  assert.equal(out.asks, 3);
  assert.deepEqual(asked, [1, 2, 1]);
});

test("an expiry asks the removed member after 35 days, naming its replacement, until it wipes", async () => {
  const asks = [];
  const env = { sites: SITES };
  let n = 0;
  env.MEMBER_0 = ns(async (name, url, init) => {
    asks.push([name, JSON.parse(init.body)]);
    return ok({ removed: true, wiped: ++n === 2 });
  });
  const step = new Step();
  const slept = [];
  step.sleep = async (name, d) => slept.push([name, d]);
  const p = { member: { site: 0, do_name: "repo:7#1#f0" }, by: { site: 3, do_name: "repo:7#2#f3", kind: "full" }, since: 5 };
  assert.deepEqual(await expire(env, { payload: p }, step), { member: "repo:7#1#f0", tries: 2 });
  assert.deepEqual(slept, [["keep", "35 days"], ["day 0", "1 day"]]);
  assert.deepEqual(asks[0], ["repo:7#1#f0", { since: 5, by: { site: 3, do_name: "repo:7#2#f3" } }]);
});

test("graft-ops' handler starts a coordinator once and reads its status", async () => {
  const env = { MOVE: workflow([]) };
  const url = "https://workflows/move/01JC0000000000000000000004";
  assert.deepEqual(await (await handle(new Request(url), env)).json(), { status: "absent" });
  const spec = { from: 0, to: 3, dead: true, home: null };
  for (let k = 0; k < 2; k++) {
    const r = await handle(new Request(url, { method: "POST", body: JSON.stringify(spec) }), env);
    assert.equal(r.status, 200);
  }
  assert.deepEqual(env.MOVE.status.get("01JC0000000000000000000004").params, spec);
  assert.deepEqual(await (await handle(new Request(url), env)).json(), { status: "running" });
  assert.equal((await handle(new Request("https://workflows/move/x"), env)).status, 404);
});
