// Load34's tests: `node --test tests/am1/load34/load34.test.mjs` from the worktree root (needs tests/m0's npm install for
// wrangler's local workerd; the workerd half is skipped with a note when it is missing).
//   - The pack builder in workerd (nodejs_compat) writes a1's bytes for one fixed input: the hashes below are a1.mjs's
//     own builder's output before wire.mjs existed (2026-11-20, a1.mjs at f413f2ac), so they do not come from the code
//     under test. Node's run of wire.mjs is checked against the same hashes.
//   - The split over N clients is a1's whole plan: every agent and poller once, every uncapped rate summed, each capped
//     kind at a1's cap, and no client holding more requests than its slots.
//   - A short run of the real client in local workerd against a fake edge: tip:move sends the staged pack's exact
//     receive-pack body after the advertisement, samples are a1's format with phases, hits and misses are apart on both
//     clocks, a slot wait never lands in a sample, and feed events join their pushes' acks.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";
import { gunzipSync } from "node:zlib";
import { cloneKeyHex, onePack, receiveBody } from "../rest/wire.mjs";
import { SLOTS, b034Plan, clientPlan, phaseOf, roles } from "./plan.mjs";
import { config } from "./config.mjs";
import { cpuLine } from "./driver.mjs";

const here = new URL(".", import.meta.url).pathname;
const sha = (b) => createHash("sha256").update(b).digest("hex");
const FIXED = Buffer.from(Array.from({ length: 40_000 }, (_, i) => (i * 31 + (i >> 7)) & 255));
// a1.mjs's builder (f413f2ac) on FIXED: [pack sha256, commit, pack bytes].
const GOLDEN = ["b5761b71012bead5f01047b4c3d2d022354e7630436e89cc619b029c17957b33", "56bc4200a9b9ceb9a2d8aafbbea43ad3a5a98179", 2779];
const GOLDEN_TREE = ["78a9455dbf264ea65dec91bcbae24a7b6578b0de398261a3d82105c7cc0011b8", "e8abd4599de19f61074731baedf5b8801740dfe5", 400];
const fixedPacks = () => [
  onePack("a1/3/7/f.txt", FIXED, "1234567890abcdef1234567890abcdef12345678", "fixed", 1_700_000_000),
  onePack("b034/4.txt", Buffer.from("x\n"), null, "root", 1_700_000_001, [["dir/sub/a.txt", Buffer.from("a")], ["dir/b", Buffer.from("b")]]),
].map((p) => [sha(p.pack), p.commit, p.pack.length]);

test("wire.mjs in node writes a1's pack bytes", () => {
  assert.deepEqual(fixedPacks(), [GOLDEN, GOLDEN_TREE]);
});

const HOT = [":second", ":hostile1", ":hostile2", ":hostile3"].map((tag) => ({ tag }));

test("the split over N clients is a1's plan", () => {
  const p = b034Plan(500);
  const rs = roles(p, HOT);
  for (const n of [rs.length + 1, 40, 96]) {
    const all = Array.from({ length: n }, (_, i) => clientPlan(i, n, p, HOT));
    const agents = all.flatMap((c) => c.agents).sort((a, b) => a - b), pollers = all.flatMap((c) => c.pollers).sort((a, b) => a - b);
    assert.deepEqual(agents, Array.from({ length: 500 }, (_, i) => i), `agents at n=${n}`);
    assert.deepEqual(pollers, Array.from({ length: 500 }, (_, i) => i), `pollers at n=${n}`);
    const rate = (k) => all.reduce((s, c) => s + (c.uncapped.find((u) => u.kind === k)?.perS ?? 0), 0);
    assert.ok(Math.abs(rate("read:api") - 200) < 1e-9 && Math.abs(rate("read:ref") - 33) < 1e-9);
    assert.ok(Math.abs(rate("side:human") - (1.25 / 2 + 1.0)) < 1e-9 && Math.abs(rate("side:lease") - 0.83) < 1e-9);
    // A role's client runs nothing else, and holds no more than its slots.
    for (const c of all.filter((x) => x.roles.length)) {
      assert.equal(c.roles.length, 1);
      assert.ok(c.roles[0].cap <= SLOTS && !c.agents.length && !c.uncapped.length);
    }
  }
  // Per kind, the caps and rates sum to a1's: probe 20 at 1/s, human 5 at 0.625/s, tip 1 at 1/6 s, hot 8 at 1 and 3.
  const sum = (kind, f) => rs.filter((r) => r.kind === kind).reduce((s, r) => s + r[f], 0);
  assert.deepEqual([sum("read:clone", "cap"), sum("push:human", "cap"), sum("tip:move", "cap"), sum("hot:push:second", "cap"), sum("hot:clone:hostile3", "cap")], [20, 5, 1, 8, 8]);
  assert.ok(Math.abs(sum("read:clone", "perS") - 1) < 1e-9 && Math.abs(sum("hot:clone:second", "perS") - 3) < 1e-9 && Math.abs(sum("tip:move", "perS") - 1 / 6) < 1e-9);
  // Each hot network's push refs are split by part, so a ref's tip stays in one client.
  const parts = rs.filter((r) => r.kind === "hot:push:second").map((r) => [r.part, r.parts]);
  assert.deepEqual(parts, [[0, 2], [1, 2]]);
  assert.equal(rs[0].kind, "tip:move");
  assert.throws(() => clientPlan(0, rs.length, p, HOT));
});

test("phases are a1's b034Phase, each mark inclusive", () => {
  const m = { killAt: 300, secondAt: 420, hostileAt: 600 };
  assert.deepEqual([0, 299_999, 300_000, 419_999, 420_000, 600_000].map((t) => phaseOf(t, 0, m)),
    ["@j9", "@j9", "@kill", "@kill", "@second", "@hostile"]);
  assert.equal(phaseOf(900_000, 0, { secondAt: 420, hostileAt: 600 }), "@hostile");
  assert.equal(phaseOf(350_000, 0, { killAt: null, secondAt: 420 }), "@j9");
});

test("the CPU line divides each client's CPU by its own operations", () => {
  const rows = [
    { sum: { cpuTime: 2_000_000, activeTime: 600e6 }, dimensions: { name: "r1-0" } },
    { sum: { cpuTime: 1_000_000, activeTime: 600e6 }, dimensions: { name: "r1-0" } },
    { sum: { cpuTime: 500_000, activeTime: 1200e6 }, dimensions: { name: "r1-1" } },
    { sum: { cpuTime: 9e9, activeTime: 1 }, dimensions: { name: "r10-1" } },
  ];
  const c = cpuLine("r1", rows, { 0: { api: 1000, git: 500 }, 1: { member: 100 } });
  assert.deepEqual(c.clients.map((x) => [x.i, x.cpuMs, x.cpuMsPerOp]), [[0, 3000, 2], [1, 500, 5]]);
  assert.equal(c.cpuMsPerOp, +(3500 / 1600).toFixed(3));
  assert.equal(c.gbS, Math.round(2400 * 0.128));
});

// ---- the client in local workerd ----

// tests/m0's wrangler, or WRANGLER_CLI (a worktree without its own npm install points it at the main checkout's).
const WRANGLER = process.env.WRANGLER_CLI ?? resolve(here, "../../m0/node_modules/wrangler/wrangler-dist/cli.js");

test("the client in workerd", { skip: existsSync(WRANGLER) ? false : "tests/m0 has no wrangler (npm install there)", timeout: 240_000 }, async (t) => {
  const { unstable_dev } = await import(WRANGLER);
  const dir = mkdtempSync(join(tmpdir(), "load34-"));
  const cfg = config({ name: "load34-test", main: resolve(here, "fake.mjs"), members: [null, null, null], memberClass: "FakeMember", bucket: "load34-test", flags: ["nodejs_compat"] });
  cfg.durable_objects.bindings.push({ name: "EDGE", class_name: "FakeEdge" });
  cfg.migrations[0].new_sqlite_classes.push("FakeEdge", "FakeMember");
  // 120 ms per edge answer: a client offered 60 API reads a second needs about 7 at once, past its 6 slots.
  cfg.vars = { TOKEN: "t0k3n-for-the-test", EDGE_DELAY_MS: "120" };
  writeFileSync(join(dir, "wrangler.json"), JSON.stringify(cfg));
  const w = await unstable_dev(cfg.main, { config: join(dir, "wrangler.json"), experimental: { disableExperimentalWarning: true }, logLevel: "error", persist: false });
  t.after(() => w.stop());
  const H = { authorization: "Bearer t0k3n-for-the-test" };
  const post = async (path, body) => { const r = await w.fetch(path, { method: "POST", headers: H, body: JSON.stringify(body) }); assert.equal(r.status, 200, `${path}: ${await r.clone().text()}`); return r.json(); };

  // In workerd: the same bytes as a1's builder.
  const wp = await (await w.fetch("/_fake/pack", { headers: H })).json();
  assert.deepEqual(wp, [GOLDEN, GOLDEN_TREE]);

  // Staged tip:move packs and a main the first of them follows.
  // The fake edge answers a clone-cache miss for a want ending in 0, so the probes see both.
  const future = ["1".repeat(40), `${"2".repeat(39)}0`, "3".repeat(40), `${"4".repeat(39)}0`, "5".repeat(40)];
  const prefix = "tip/test";
  const staged = future.map((c, i) => Buffer.from(`PACK staged ${i} ${c}`));
  for (const [i, b] of staged.entries()) {
    const r = await w.fetch(`/stage?key=${prefix}/${i}`, { method: "PUT", headers: H, body: b });
    assert.equal(r.status, 200);
  }
  assert.equal((await w.fetch(`/stage?key=runs/x`, { method: "PUT", headers: H, body: "x" })).status, 400);
  // The repository keeps main between rounds, so the run starts mid-history (round 26's 60 refused moves from index 0).
  await w.fetch(`/edge/_fake/main?sha=${future[1]}`);

  // The pre-row smoke clone goes through the client's own fetch, so it fails as the row's clones would (mt8's 403s).
  const sm = await post("/smoke", { spec: { edge: `http://${w.address}:${w.port}/edge`, repo: "o/mono", chain: "repo:77", want: future[2],
    tokens: { admin: "adm1nPAT" }, b034: {} } });
  assert.deepEqual([sm.code, sm.status, sm.cache, sm.bytes], ["200", 200, "hit", 4096], JSON.stringify(sm));

  const plan = { ...b034Plan(12), pushEvery: 2, apiReads: 120, refReads: 4, pollers: 4, human: 4, tipMovesPerMin: 120, clones: 4,
    side: { taskEdit: 1, comment: 1, label: 1, draftPr: 0.5, leaseAcquire: 1, leaseRelease: 1, leaseRenew: 1, sessionPush: 0, standIn: 1 } };
  const hot = [{ repo: "o/second", chain: "repo:2", who: "member" }];
  const n = roles(plan, [{ tag: ":second" }]).length + 2;
  const t0 = Date.now() + 3_000, seconds = 12;
  await post("/start", { spec: { run: "rt", n, t0, seconds, edge: `http://${w.address}:${w.port}/edge`,
    repo: "o/mono", chain: "repo:77", repoId: "R", target: { id: "I", number: 1 }, main: future[1], tasks: [[0, "T0"], [1, "T1"]], plan,
    tokens: { admin: "adm1nPAT", app: "appT0KEN", member: "memb3r" },
    b034: { future, packs: prefix, second: hot[0], hostile: [], hotPushPerS: 2, hotClonePerS: 2, hotReadMBps: 0 },
    marks: { killAt: 4, secondAt: 8, hostileAt: null }, members: [{ site: 0, name: "l0", learner: true }] }, hints: [] });

  let ps;
  for (let k = 0; k < 120; k++) {
    await new Promise((r) => setTimeout(r, 1_000));
    ps = await post("/progress", { run: "rt", n, members: [] });
    if (ps.every((p) => p.state === "done")) break;
  }
  assert.ok(ps.every((p) => p.state === "done"), JSON.stringify(ps.map((p) => p.state)));

  // Every client's chunks, as the orb collects them.
  const { keys } = await (await w.fetch(`/list?prefix=runs/rt/`, { headers: H })).json();
  const chunks = [];
  for (const k of keys) chunks.push(JSON.parse(gunzipSync(Buffer.from(await (await w.fetch(`/get?key=${encodeURIComponent(k)}`, { headers: H })).arrayBuffer())).toString()));
  assert.equal(chunks.filter((c) => c.done).length, n, "one last chunk per client");
  const samples = chunks.flatMap((c) => c.samples), summaries = chunks.filter((c) => c.done).map((c) => c.summary);
  const edge = await (await w.fetch("/edge/_fake/log")).json();

  // a1's sample shape, on Cloudflare's clock inside the window, phased where a1 phases.
  for (const s of samples) {
    assert.ok(Array.isArray(s) && s.length === 4 && typeof s[0] === "number" && typeof s[1] === "string" && typeof s[2] === "number" && typeof s[3] === "string", JSON.stringify(s));
    if (/^(push:|read:clone|tip:move|hot:)/.test(s[1])) assert.match(s[1], /@(j9|kill|second|hostile)$/);
  }
  const kinds = new Set(samples.map((s) => s[1].split("@")[0]));
  for (const k of ["push:agent", "push:human", "push:human:tip", "push:human:edge", "read:api", "read:ref", "feed", "tip:move", "read:clone",
    "read:cloneTtfb", "read:cloneTtfb:hit", "read:cloneTtfb:miss", "read:cloneTtfb:hit:edge", "read:cloneTtfb:miss:edge", "read:cloneTtfb:front", "read:cloneTtfb:hit:front",
    "read:cloneTtfb:miss:front", "read:cloneTtfb:chain_open", "read:cloneTtfb:hit:chain_open", "hot:push:second", "hot:cloneTtfb:second"])
    assert.ok(kinds.has(k), `no ${k} sample (${[...kinds].join(" ")})`);
  assert.ok(samples.some((s) => s[1].endsWith("@kill")) && samples.some((s) => s[1].endsWith("@second")));
  // The edge's stages carry their own durations, not the clone's: the fake edge sends front 4 and chain_open 6 ms.
  for (const [k, v] of [["front", 4], ["chain_open", 6]])
    assert.deepEqual([...new Set(samples.filter((s) => s[1].split("@")[0] === `read:cloneTtfb:${k}`).map((s) => s[2]))], [v], k);
  assert.ok(!kinds.has("read:cloneTtfb:miss:chain_open"), "a miss with no chain_open stage records none");
  assert.ok(!samples.some((s) => s[1].startsWith("hot:") && s[1].endsWith("@j9")), "a hot network before its mark");

  // tip:move: the staged pack's exact receive-pack body, old = the advertised main, after an advertisement GET.
  const moves = edge.log.filter((e) => e.ref === "refs/heads/main");
  assert.ok(moves.length >= 2, `tip moves ${moves.length}: ${JSON.stringify(samples.filter((x) => x[1].startsWith("tip:move")).map((x) => x[3]))}`);
  assert.equal(moves[0].sha256, sha(receiveBody("refs/heads/main", future[1], future[2], staged[2])));
  assert.equal(moves[1].sha256, sha(receiveBody("refs/heads/main", future[2], future[3], staged[3])));
  const firstMove = edge.log.indexOf(moves[0]);
  assert.ok(edge.log.slice(0, firstMove).some((e) => e.path.includes("/info/refs?service=git-receive-pack") && e.auth.startsWith("Basic ")));

  // Probes ask depth 1 for the tip client's main; a miss carries the CloneKey digest a1 computes.
  const probes = edge.log.filter((e) => e.path.startsWith("/o/mono.git/git-upload-pack") && e.want);
  assert.ok(probes.length && probes.every((e) => e.deepen && future.includes(e.want)));
  const misses = chunks.flatMap((c) => c.misses ?? []);
  assert.ok(misses.length && misses.every((m) => m.key === cloneKeyHex(77, m.want)));

  // Slots: a client queued for one (the API client is offered more than 6 at once), and no read:api sample holds the
  // wait (each is the edge's 120 ms plus little; a sample with a 120 ms wait in it would be 240 and more).
  assert.ok(summaries.some((s) => s.slotWaits.n > 0), "no client ever waited for a slot");
  const api = samples.filter((s) => s[1] === "read:api" && s[3] === "200").map((s) => s[2]).sort((a, b) => a - b);
  assert.ok(api.length > 100 && api[Math.floor(api.length * 0.95)] < 235, `read:api p95 ${api[Math.floor(api.length * 0.95)]} ms`);

  // Feed events join their pushes' acks across clients.
  const acks = new Map(chunks.flatMap((c) => c.acks)), seen = chunks.flatMap((c) => c.seen);
  assert.ok(seen.some(([head]) => acks.has(head)), "no feed event matched a push");
  // Credentials: git under Basic, the API under `token`, and nothing of a token in a sample.
  assert.ok(edge.log.filter((e) => e.path.includes(".git/")).every((e) => e.auth.startsWith("Basic ")));
  assert.ok(edge.log.filter((e) => e.path.startsWith("/api/")).every((e) => /^token (adm1nPAT|appT0KEN)$/.test(e.auth)));
  assert.ok(!JSON.stringify(chunks).includes("adm1nPAT") && !JSON.stringify(chunks).includes("appT0KEN"));

  // Once collected, the objects keep nothing (the spec held the tokens).
  await post("/forget", { run: "rt", n });
  assert.ok((await post("/progress", { run: "rt", n, members: [] })).every((p) => p.state === "idle"));
});
