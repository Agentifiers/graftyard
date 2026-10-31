// B0-34 (build/BUILD-PLAN.md, M0-WP9): real git CPU on a real large repository, here a bare clone of torvalds/linux
// (6.4 GiB pack, 12.0 million objects, with a commit-graph), in this orb. It replaces B0-35's synthetic 3 to 4 ms test
// merge with real ones and measures the other per-operation constants `scripts/cost.py` guesses (`MERGE_VCPU_S`,
// `PUSH_VCPU_S`, `CI_MISS_VCPU_S`). CPU is the git children's user + sys seconds, read with the shell's `times`.
//
//   node run/wp9-gitcpu.ts [--repo /home/user/wp9-data/linux.git] [--n 60] [--run X]
//
// Kinds:
// - pull: `merge-tree --write-tree M^1 M^2` for recent first-parent merges M on master (real subsystem pulls).
// - pr-1d, pr-1w, pr-1m, pr-6m: an agent PR whose base is that old. For a real non-merge commit C from that long ago,
//   the head is a new commit A with C's tree and C's parent (so A is not in master, and its change is C's real change),
//   written to a scratch object directory. The test merge is `merge-tree --write-tree master A`: a real 3-way merge
//   over everything master changed since C's parent.
// - push: receive-pack's work for that agent push: a thin pack of A ^C^ built once, then timed
//   `index-pack --stdin --fix-thin` into a second scratch object dir with the repository as alternate, plus the
//   connectivity check `rev-list --objects A --not --all`.
// - clone-miss: a depth-1 clone's pack for master's tip, `pack-objects --revs --stdout` over the tip commit's objects,
//   with delta reuse as upload-pack does it (the clone-cache miss of `CI_MISS_VCPU_S`).

import { execFileSync } from "node:child_process";
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  repo: { type: "string", default: "/home/user/wp9-data/linux.git" }, n: { type: "string", default: "60" },
  run: { type: "string", default: "r1" } } });
const REPO = opt.repo!, N = Number(opt.n);
const ROOT = new URL("../", import.meta.url).pathname;
const SCRATCH = "/tmp/wp9-gitcpu";

const git = (args: string[]) => execFileSync("git", ["-C", REPO, ...args], { encoding: "utf8", maxBuffer: 1 << 28 }).trim();

/** Runs `cmd` in bash and returns the children's CPU seconds (user + sys), wall ms, exit code, and stdout's last line. */
function timed(cmd: string): { cpu: number; wallMs: number; rc: number; out: string } {
  const t = Date.now();
  const out = execFileSync("bash", ["-c", `${cmd}; rc=$?; times; echo $rc`], { encoding: "utf8", maxBuffer: 1 << 28 }).trim().split("\n");
  const rc = Number(out.pop());
  const child = out.pop()!.split(/\s+/).map((x) => { const m = /(\d+)m([\d.]+)s/.exec(x)!; return Number(m[1]) * 60 + Number(m[2]); });
  out.pop();
  return { cpu: child[0] + child[1], wallMs: Date.now() - t, rc, out: out.join("\n").slice(-200) };
}

const pct = (xs: number[], p: number) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const stats = (xs: number[]) => ({ n: xs.length, p50: pct(xs, 0.5), p90: pct(xs, 0.9), p99: pct(xs, 0.99), max: Math.max(...xs),
  mean: xs.reduce((a, b) => a + b, 0) / xs.length });

const tip = git(["rev-parse", "refs/heads/master"]);
const tipTime = Number(git(["log", "-1", "--format=%ct", tip]));
const rows: { kind: string; a: string; b: string; cpu: number; wallMs: number; rc: number; files?: number }[] = [];

// Real pulls: the newest N first-parent merges.
for (const line of git(["log", "--first-parent", "--merges", `-${N}`, "--format=%P", tip]).split("\n")) {
  const [p1, p2] = line.split(" ");
  const r = timed(`git -C ${REPO} merge-tree --write-tree ${p1} ${p2} >/dev/null 2>&1`);
  rows.push({ kind: "pull", a: p1, b: p2, cpu: r.cpu, wallMs: r.wallMs, rc: r.rc });
}

// Agent PRs: for a real commit C of each age, head A = C's tree on C's parent, in a scratch object directory with the
// repository as alternate, test-merged onto today's tip.
const ages: [string, number][] = [["pr-1d", 86_400], ["pr-1w", 7 * 86_400], ["pr-1m", 30 * 86_400], ["pr-6m", 182 * 86_400]];
rmSync(SCRATCH, { recursive: true, force: true });
mkdirSync(`${SCRATCH}/heads/pack`, { recursive: true });
const HEADS = `GIT_DIR=${REPO} GIT_OBJECT_DIRECTORY=${SCRATCH}/heads GIT_ALTERNATE_OBJECT_DIRECTORIES=${REPO}/objects`;
const sh = (cmd: string) => execFileSync("bash", ["-c", cmd], { encoding: "utf8", maxBuffer: 1 << 28 }).trim();
const prHeads: Record<string, [string, string][]> = {};
for (const [kind, age] of ages) {
  const before = tipTime - age;
  const cs = git(["rev-list", "--no-merges", `--max-count=${N}`, `--before=${before}`, `--after=${before - 86_400}`, tip]).split("\n").filter(Boolean);
  prHeads[kind] = [];
  for (const c of cs) {
    const files = Number(git(["diff-tree", "--no-commit-id", "-r", "--name-only", c]).split("\n").filter(Boolean).length);
    const a = sh(`${HEADS} git commit-tree ${c}^{tree} -p ${c}^ -m 'wp9 agent head for ${c}'`);
    prHeads[kind].push([c, a]);
    const r = timed(`${HEADS} git merge-tree --write-tree ${tip} ${a} >/dev/null 2>&1`);
    rows.push({ kind, a: tip, b: a, cpu: r.cpu, wallMs: r.wallMs, rc: r.rc, files });
  }
}

// Agent pushes: receive-pack's index-pack plus the connectivity check, for the pr-1d heads.
mkdirSync(`${SCRATCH}/objects/pack`, { recursive: true });
const pushBytes: number[] = [];
for (const [c, a] of prHeads["pr-1d"]) {
  sh(`printf '%s\\n^%s^\\n' ${a} ${c} | ${HEADS} git pack-objects --revs --thin --stdout > ${SCRATCH}/in.pack 2>/dev/null`);
  pushBytes.push(Number(sh(`stat -c %s ${SCRATCH}/in.pack`)));
  const env = `GIT_DIR=${REPO} GIT_OBJECT_DIRECTORY=${SCRATCH}/objects GIT_ALTERNATE_OBJECT_DIRECTORIES=${REPO}/objects`;
  const r = timed(`${env} git index-pack --stdin --fix-thin < ${SCRATCH}/in.pack >/dev/null 2>&1 && ${env} git rev-list --objects ${a} --not --all >/dev/null 2>&1`);
  rows.push({ kind: "push", a, b: "", cpu: r.cpu, wallMs: r.wallMs, rc: r.rc });
  rmSync(`${SCRATCH}/objects`, { recursive: true, force: true });
  mkdirSync(`${SCRATCH}/objects/pack`, { recursive: true });
}

// Clone-cache misses: a depth-1 pack of the tip, five times (the page cache is warm after the first).
let cloneBytes = 0;
for (let i = 0; i < 5; i++) {
  const r = timed(`printf '%s\\n' ${tip} | git -C ${REPO} rev-list --objects --no-walk --stdin | git -C ${REPO} pack-objects --stdout --delta-base-offset 2>/dev/null | wc -c`);
  cloneBytes = Number(r.out.trim().split("\n").pop());
  rows.push({ kind: "clone-miss", a: tip, b: "", cpu: r.cpu, wallMs: r.wallMs, rc: r.rc });
}
rmSync(SCRATCH, { recursive: true, force: true });

const summary: Record<string, object> = {};
for (const kind of new Set(rows.map((r) => r.kind))) {
  const rs = rows.filter((r) => r.kind === kind);
  summary[kind] = { cpuS: stats(rs.map((r) => r.cpu)), wallMs: stats(rs.map((r) => r.wallMs)), nonzeroRc: rs.filter((r) => r.rc !== 0).length,
    ...(rs[0].files !== undefined ? { filesP50: pct(rs.map((r) => r.files!), 0.5) } : {}) };
}
const out = { repo: REPO, tip, packBytes: Number(execFileSync("bash", ["-c", `du -sb ${REPO}/objects/pack | cut -f1`], { encoding: "utf8" })),
  gitVersion: execFileSync("git", ["--version"], { encoding: "utf8" }).trim(), host: "orb, 8 vCPU", summary,
  pushBytes: stats(pushBytes), cloneBytes, rows };
writeFileSync(`${ROOT}results/raw/wp9-b0-34-gitcpu-${opt.run}.json`, JSON.stringify(out, null, 1));
console.log(JSON.stringify({ summary, pushBytes: out.pushBytes, cloneBytes }, null, 1));
