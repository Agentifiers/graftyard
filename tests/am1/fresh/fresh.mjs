// fresh-repo's hosted check (tests/am1/results/fresh-repo.md): N fresh m3org repositories created back to back, one
// at a time, each followed by a push to `main`. Each create is sent once and never re-sent, so a slow create is
// measured, not hidden behind a 422 and a GET (`tests/am1/runs/mkrepos.mjs` did that on m23). For each repository:
//   - the create's status and wall, and every `GET /repos/m3org/<name>` sent every 3 s while it runs: each must be
//     404 (not created yet) or 200. LM-1's 503 "0 of 3 joined" was such a read; with JOINED_503_OK=1 (main before
//     the follow-up) it is counted, not judged;
//   - after the 201: `GET /repos` must answer 200, and `main` must be in ls-refs on the first read (auto_init's commit,
//     which the create acknowledged);
//   - a push of one commit to `main`, then ls-refs on the next read must name the pushed commit;
//   - the create's wall within CREATE_MAX_MS (default 60 s [guess], under one 8-try placement and over two bootstrap creates of about 15 s):
//     a create no longer waits on the near placement, which on main held a create 60 to 95 s per group placed;
//   - at the end, once the last create's placement has had PLACE_WAIT_MS (default 150 s), each group's newest committed config is a
//     `Normal` one of 3 voters (the tools edge's `/status`): the background placement finished and left no group
//     `Degraded`.
// Writes one JSON row per repository to OUT, then a summary; exits 1 on any create failure, slow create, read during it that is not 200 or 404,
// read after the 201 that is not 200, missing `main`, failed push, or group without 3 voters.
//   node tests/am1/fresh/fresh.mjs PREFIX N OUT
import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { EDGE, Git, log, row, sleep, statuses, tokens } from "../hosted/lib.mjs";

const [prefix, n, out] = [process.argv[2], Number(process.argv[3] ?? 50), process.argv[4]];
if (!prefix || !out) throw new Error("usage: fresh.mjs PREFIX N OUT");
writeFileSync(out, "");
const headers = { authorization: `token ${tokens.admin}`, "user-agent": "graftyard-am1", accept: "application/vnd.github+json" };
const call = (method, path, body, ms) => fetch(`${EDGE}${path}`, {
  method, headers: body ? { ...headers, "content-type": "application/json" } : headers,
  body: body ? JSON.stringify(body) : undefined, signal: AbortSignal.timeout(ms),
}).then(async (r) => ({ status: r.status, body: await r.text() })).catch((e) => ({ status: 0, body: String(e) }));
const CREATE_MAX_MS = Number(process.env.CREATE_MAX_MS ?? 60_000);
const PLACE_WAIT_MS = Number(process.env.PLACE_WAIT_MS ?? 150_000);
const JOINED_503_OK = process.env.JOINED_503_OK === "1";
const fails = { create: 0, slow: 0, during: 0, readAfter: 0, mainAfterCreate: 0, push: 0, mainAfterPush: 0, voters: 0 };
const made = [];
const createMs = [];
const during = {};
for (let i = 0; i < n; i++) {
  const name = `${prefix}-${i}`;
  const t0 = Date.now();
  let done = false;
  const reads = [];
  const racing = (async () => {
    while (!done) {
      await sleep(3_000);
      if (done) break;
      const r = await call("GET", `/api/v3/repos/m3org/${name}`, null, 120_000);
      reads.push({ at: Date.now() - t0, status: r.status, msg: r.status === 200 ? undefined : r.body.slice(0, 140) });
    }
  })();
  const r = await call("POST", "/api/v3/orgs/m3org/repos", { name, private: false, auto_init: true }, 400_000);
  done = true;
  await racing;
  const v = { name, at: new Date(t0).toISOString(), status: r.status, createMs: Date.now() - t0, reads };
  for (const x of reads) {
    during[x.status] = (during[x.status] ?? 0) + 1;
    if (x.status !== 200 && x.status !== 404 && !(JOINED_503_OK && /joined/.test(x.msg ?? ""))) fails.during++;
  }
  if (r.status !== 201) {
    fails.create++;
    v.body = r.body.slice(0, 300);
    row(out, v);
    log(`${name}: create ${r.status} after ${v.createMs} ms: ${v.body.slice(0, 160)}`);
    continue;
  }
  createMs.push(v.createMs);
  if (v.createMs > CREATE_MAX_MS) fails.slow++;
  v.id = JSON.parse(r.body).id;
  made.push(v);
  const after = await call("GET", `/api/v3/repos/m3org/${name}`, null, 120_000);
  v.readAfter = after.status;
  if (after.status !== 200) { fails.readAfter++; v.readAfterBody = after.body.slice(0, 200); }
  const g = new Git(`m3org/${name}`).as(tokens.admin);
  try {
    v.mainAfterCreate = g.remoteHead("refs/heads/main");
    if (!v.mainAfterCreate) fails.mainAfterCreate++;
    const c = g.clone();
    writeFileSync(join(g.wt, "fresh.txt"), `${name} ${Date.now()}\n`);
    const p = c.ok && g.run(["add", "fresh.txt"]).ok && g.run(["commit", "-q", "-m", "fresh-repo"]).ok
      && g.run(["push", "-q", "origin", "HEAD:refs/heads/main"]);
    if (!p?.ok) {
      fails.push++;
      v.push = (p?.out ?? c.out).slice(0, 300);
    } else {
      v.pushed = g.run(["rev-parse", "HEAD"]).out.trim();
      v.mainAfterPush = g.remoteHead("refs/heads/main");
      if (v.mainAfterPush !== v.pushed) fails.mainAfterPush++;
    }
  } finally {
    rmSync(g.dir, { recursive: true, force: true });
  }
  row(out, v);
  const bad = reads.filter((x) => x.status !== 200).map((x) => x.status);
  log(`${name} = ${v.id}: create ${v.createMs} ms (reads during: ${reads.length}, not 200: ${bad.join(",") || "none"}), `
    + `read after ${after.status}, main ${v.mainAfterCreate ? "yes" : "MISSING"}, `
    + `push ${v.pushed ? (v.mainAfterPush === v.pushed ? "read back" : "NOT READ BACK") : "FAILED"}`);
}
if (made.length) await sleep(Math.max(0, Date.parse(made[made.length - 1].at) + PLACE_WAIT_MS - Date.now()));
const groups = [];
for (const v of made) {
  // An idle group's lease lapses, so no member may lead now; the committed config with the highest index, from the
  // first-generation members, which a re-placed voter's peers stay, is the group's.
  const st = await statuses(`repo:${v.id}`);
  const best = st.filter((s) => s.config).sort((a, b) => b.config.index - a.config.index)[0];
  const voters = best?.config?.config?.normal?.voters ?? [];
  const g = { name: v.name, id: v.id, configIndex: best?.config?.index, voters: voters.length, generations: voters.map((m) => m.generation) };
  if (g.voters !== 3) fails.voters++;
  groups.push(g);
  row(out, { group: g });
}
log(`groups with 3 voters: ${groups.filter((g) => g.voters === 3).length} of ${groups.length}; re-placed: ${groups.filter((g) => g.generations.some((x) => x > 1)).length}`);
const ms = createMs.sort((a, b) => a - b);
const q = (p) => ms[Math.min(ms.length - 1, Math.floor(p * ms.length))];
const summary = { creates: n, fails, readsDuringCreate: during, createP50: q(0.5), createP90: q(0.9), createMax: ms[ms.length - 1] };
row(out, { summary });
log(JSON.stringify(summary));
process.exit(Object.values(fails).some((x) => x) ? 1 : 0);
