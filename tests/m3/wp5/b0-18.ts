// B0-18 rerun on real members (BUILD-PLAN M3-WP5, "Learners rerun the Milestone 0 learner gate"): one repository
// group with learners placed by `place_learners` (`/learners` on the leader), each serving `--rate` read-index reads a
// second for `--seconds`, while the group takes 10 pushes a second. Pass: leader under 700 requests a second, learner
// read p99 at most 1 s (`decisions/DECISIONS.md`, "Measured latency is accepted"). As in Milestone 0's driver
// (`tests/m0/run/raft-learners.ts`) the reads are generated inside each learner (`/read_bench`), so the leader's load
// is real and the learner's own request limit is not exercised.
//
//   node tests/m3/wp5/b0-18.ts --repo 65537 [--rate 1000] [--seconds 30] [--load enam:6000,weur:5000,apac:5000]
// Env: GRAFT_TOOLS_EDGE, GRAFT_EDGE, BED_TOKEN_FILE, BF20_HOME (tokens.txt), and the M0 token in tests/m0/.m0-token.

import { readFileSync, mkdirSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  repo: { type: "string" }, rate: { type: "string", default: "1000" }, seconds: { type: "string", default: "30" },
  load: { type: "string", default: "enam:6000,weur:5000,apac:5000" }, place: { type: "boolean", default: false },
  out: { type: "string", default: "/tmp/m3h/b0-18" },
} });
const TOOLS = process.env.GRAFT_TOOLS_EDGE!, EDGE = process.env.GRAFT_EDGE!;
const TOK = readFileSync(new URL("../../m0/.m0-token", import.meta.url), "utf8").trim();
const BED = readFileSync(process.env.BED_TOKEN_FILE!, "utf8").trim();
const WRITER = readFileSync(`${process.env.BF20_HOME}/tokens.txt`, "utf8").split("\n")[1].split(" ")[0];
const chain = `repo:${opt.repo}`, rate = Number(opt.rate), seconds = Number(opt.seconds);
const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };

async function member(site: number, path: string, body?: unknown, do_name?: string, deadline = 30_000): Promise<any> {
  const r = await fetch(`${TOOLS}/member`, { method: "POST", headers: { authorization: `Bearer ${TOK}` },
    body: JSON.stringify({ chain, site, path, body, deadline, ...(do_name ? { do_name } : {}) }) });
  return r.json();
}

// A redeploy leaves an idle group with no leader until something calls it: site 0 campaigns first.
let st = await Promise.all([0, 1, 2].map((s) => member(s, "/status")));
for (let k = 0; k < 5 && !st.some((s) => s.v?.role === "leader"); k++) {
  await member(0, "/campaign", {});
  await new Promise((r) => setTimeout(r, 3000));
  st = await Promise.all([0, 1, 2].map((s) => member(s, "/status")));
}
const lead = st.findIndex((s) => s.v?.role === "leader");
if (lead < 0) throw new Error(`no leader: ${JSON.stringify(st.map((s) => s.v?.role))}`);
if (opt.place) {
  const reads = opt.load!.split(",").map((x) => { const [r, n] = x.split(":"); return [r, Number(n)]; });
  const p = await member(lead, "/learners", { load: { reads } }, undefined, 120_000);
  console.log("place", JSON.stringify(p.v ?? p).slice(0, 3000));
}
const cfg = (await member(lead, "/status")).v.config.config.normal;
const learners: { site: number; do_name: string; slot: number }[] = cfg.learners;
console.log(`leader site ${lead}, ${learners.length} learners`);
// BF-28 hosted: every member is its own object. A learner on a voter's site and two learners on one site each answer
// as themselves (their own `me` and log), and every learner has caught up before the run.
const bf28: any[] = [];
for (const v of cfg.voters) {
  const s = (await member(v.site, "/status", undefined, v.do_name)).v;
  bf28.push({ kind: "voter", site: v.site, do_name: v.do_name, role: s?.role, me: s?.doName, commit: s?.commit, base: s?.base, boot: s?.boot });
}
for (const l of learners) {
  const s = (await member(l.site, "/status", undefined, l.do_name)).v;
  const installs = (await member(l.site, "/stats", undefined, l.do_name)).v?.installs;
  bf28.push({ kind: "learner", slot: l.slot, site: l.site, do_name: l.do_name, role: s?.role, me: s?.doName, commit: s?.commit, base: s?.base, boot: s?.boot, installs });
}
const objects = new Set(bf28.map((m) => `${m.site}/${m.do_name}`)), boots = new Set(bf28.map((m) => m.boot));
const onVoterSite = learners.filter((l) => cfg.voters.some((v: any) => v.site === l.site)).length;
const shared = Math.max(0, ...[0, 1, 2].map((s) => learners.filter((l) => l.site === s).length));
const bf28Pass = objects.size === bf28.length && boots.size === bf28.length && onVoterSite > 0 && shared >= 2 &&
  bf28.every((m) => m.me === m.do_name && (m.kind === "voter" || m.role === "follower"));
console.table(bf28);
console.log(JSON.stringify({ row: "BF-28 hosted", members: bf28.length, objects: objects.size, boots: boots.size, onVoterSite, mostOnOneSite: shared, verdict: bf28Pass ? "Pass" : "Fail" }));
// Warm-up: a learner evicted while idle knows no leader until the leader's next append reaches it, and refuses reads
// until then. Pushes drive appends until every learner names the leader (at most 30 s).
for (let k = 0; k < 30; k++) {
  const known = await Promise.all(learners.map((l) => member(l.site, "/status", undefined, l.do_name).then((r) => r.v?.leader)));
  if (known.every((x) => x === lead)) { console.log(`warm after ${k} s`); break; }
  await fetch(`${EDGE}/_bed/push?repo=${opt.repo}`, { method: "POST", headers: { "x-bed-token": BED, authorization: `token ${WRITER}` },
    body: JSON.stringify({ ref: `refs/heads/b018/warm/${k}`, expect: null, new: randomBytes(20).toString("hex") }) }).catch(() => {});
  await new Promise((r) => setTimeout(r, 1000));
}
await member(lead, "/stats?reset=1");
const t0 = Date.now();
let pushes = 0, pushOk = 0;
const writes = (async () => {
  while (Date.now() - t0 < seconds * 1000) {
    const due = t0 + pushes * 100;
    if (due > Date.now()) await new Promise((r) => setTimeout(r, due - Date.now()));
    const k = pushes++;
    fetch(`${EDGE}/_bed/push?repo=${opt.repo}`, { method: "POST", headers: { "x-bed-token": BED, authorization: `token ${WRITER}` },
      body: JSON.stringify({ ref: `refs/heads/b018/${t0}/${k}`, expect: null, new: randomBytes(20).toString("hex") }) })
      .then((r) => r.json()).then((v: any) => { if (v.ok || v.outcome || v.done) pushOk++; }).catch(() => {});
  }
})();
const runs = await Promise.all(learners.map((l) =>
  member(l.site, "/read_bench", { rate, secs: seconds }, l.do_name, seconds * 1000 + 60_000).then((r) => ({ l, r }))));
await writes;
const stats = (await member(lead, "/stats")).v;
const inbound: Record<string, number> = stats.inbound ?? {};
const leaderReqs = Object.values(inbound).reduce((a, b) => a + b, 0);
const ok = runs.filter((x) => x.r?.v?.ok);
const summary = {
  row: "B0-18 rerun", at: new Date().toISOString(), chain, learners: learners.length, ratePerLearner: rate, seconds,
  reads: ok.reduce((a, x) => a + x.r.v.ok, 0), readsPerS: Math.round(ok.reduce((a, x) => a + x.r.v.ok, 0) / seconds),
  failed: ok.reduce((a, x) => a + x.r.v.failed + x.r.v.refused, 0), errors: runs.filter((x) => !x.r?.v?.ok).map((x) => ({ l: x.l.do_name, r: x.r })),
  learnerP99Max: Math.max(...ok.map((x) => x.r.v.p99)), learnerP50: pct(ok.map((x) => x.r.v.p50), 0.5),
  perLearner: ok.map((x) => ({ slot: x.l.slot, site: x.l.site, n: x.r.v.n, ok: x.r.v.ok, failed: x.r.v.failed, p50: x.r.v.p50, p99: x.r.v.p99, max: x.r.v.max })),
  leader: { site: lead, inbound, reqsPerS: Math.round(leaderReqs / (stats.inboundMs / 1000)), windowMs: stats.inboundMs },
  pushes: { sent: pushes, ok: pushOk },
  verdict: leaderReqs / (stats.inboundMs / 1000) < 700 && Math.max(...ok.map((x) => x.r.v.p99)) <= 1000 && ok.length === learners.length ? "Pass" : "Fail",
};
mkdirSync(opt.out!, { recursive: true });
writeFileSync(`${opt.out}/b0-18-${t0}.json`, JSON.stringify(summary, null, 2));
console.log(JSON.stringify({ ...summary, perLearner: undefined }, null, 2));
console.table(summary.perLearner);
