// Whether site c's install is catching the leader or chasing it (b030r6, operator 2026-11-17: an install that extends
// can still chase the leader forever, since each extension adds archives under load). Every 60 s, from c's `/fold`
// `running` (load.rs `job_json`): the job's `next` (the next entry it replays; it stays at the first archive's `from`
// until both tiers are restored), its `through`, the gap `through - next + 1`, each tier's restored bytes, and the
// leader's commit. The gap trend is judged on the replay phase only (both tiers restored): `growing30` is true when
// every sample of the last 30 minutes of replay had a larger gap than the one before it. One JSON line a sample to
// tests/am1/raw/$B030_RUN-job.jsonl. Read-only apart from that STOP; ends when c runs no job after having run one, or on b030-run.done.
//   B030_RUN=b030r6 node tests/am1/b030split/jobwatch.mjs
import { appendFileSync, existsSync, writeFileSync } from "node:fs";
import { member, sleep, statuses } from "../hosted/lib.mjs";
import { readFileSync } from "node:fs";

const RUN = process.env.B030_RUN ?? "b030r6";
const OUT = `tests/am1/raw/${RUN}-job.jsonl`;
const REPO = JSON.parse(readFileSync("tests/am1/raw/b030-repo.json", "utf8")).repo;
const STOP = "tests/am1/raw/b030-STOP";
const C = `repo:${REPO}`, LAGGER = Number(process.env.LAGGER ?? 2);
// The replay history so far, from earlier samples, so a restart keeps the 30-minute window.
const replay = existsSync(OUT) ? readFileSync(OUT, "utf8").trim().split("\n").filter(Boolean).map((l) => JSON.parse(l))
  .filter((r) => r.restored && r.gap !== undefined).map((r) => ({ t: Date.parse(r.at), gap: r.gap })) : [];
let seen = replay.length > 0;
while (!existsSync("tests/am1/raw/b030-run.done")) {
  const f = (await member(C, LAGGER, "/fold", { audit: {} }, 30_000).catch(() => ({ v: {} }))).v ?? {};
  const j = f.running;
  const commit = (await statuses(C).catch(() => [])).filter((s) => s.role === "leader").map((s) => s.commit)[0] ?? null;
  if (!j) {
    appendFileSync(OUT, JSON.stringify({ at: new Date().toISOString(), running: null, commit }) + "\n");
    if (seen) break;
    await sleep(60_000);
    continue;
  }
  seen = true;
  const restored = (j.tiers ?? []).every((t) => t.restored);
  const gap = j.through - j.next + 1;
  if (restored) replay.push({ t: Date.now(), gap });
  const last30 = replay.filter((r) => r.t >= Date.now() - 30 * 60_000);
  const growing30 = last30.length >= 25 && last30[0].t <= Date.now() - 29 * 60_000 && last30.every((r, k) => k === 0 || r.gap > last30[k - 1].gap);
  const prev = replay.at(-2);
  appendFileSync(OUT, JSON.stringify({ at: new Date().toISOString(), next: j.next, through: j.through, gap, commit, restored,
    tiers: (j.tiers ?? []).map((t) => ({ to: t.to, offset: t.offset, size: t.size, restored: t.restored })), archives: j.archives,
    trend: restored && prev ? (gap < prev.gap ? "shrinking" : gap > prev.gap ? "growing" : "flat") : null, growing30 }) + "\n");
  // The operator's early stop (2026-11-17): a replay gap growing for 30 minutes means the install does not converge,
  // so the run stops and closes out as "did not converge".
  if (growing30 && !existsSync(STOP)) writeFileSync(STOP, "jobwatch: c's install replay gap grew every sample for 30 min\n");
  await sleep(60_000);
}
