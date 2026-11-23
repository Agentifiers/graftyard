// coldwrite's measurement (`run.sh`): on each cold edge script 1..N-1, one receive-pack POST creating a branch, then
// the same POST again on that now-warm isolate, and last a POST after an advertisement on a fresh edge when one is
// left. Prints each POST's status, its server-timing stages, and the member calls the sites logged while it ran,
// in arrival order (`cwcall` lines: path, site, object, ms), so serial round trips show. No git cache shard runs
// here, so validate fails and the POST answers 503; every stage before it is the product's.
//   node tests/coldwrite/push.mjs <front url> <bed dir> <edges>
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const [F, B, N] = process.argv.slice(2);
const seed = JSON.parse(readFileSync(`${B}/seed.json`, "utf8"));
const auth = "Basic " + Buffer.from(`x:${seed.token}`).toString("base64");
const path = `/${seed.login}/${seed.repo}.git`;

// One commit and its pack, made with git.
const g = mkdtempSync(join(tmpdir(), "cw-git-"));
const git = (...a) => execFileSync("git", ["-C", g, ...a], { env: { ...process.env, GIT_AUTHOR_NAME: "cw", GIT_AUTHOR_EMAIL: "cw@x", GIT_COMMITTER_NAME: "cw", GIT_COMMITTER_EMAIL: "cw@x" } });
git("init", "-q");
execFileSync("sh", ["-c", `echo cold > ${g}/f`]);
git("add", "f");
git("commit", "-qm", "cold");
const tip = git("rev-parse", "HEAD").toString().trim();
const pack = execFileSync("sh", ["-c", `git -C ${g} rev-list --objects HEAD | git -C ${g} pack-objects --stdout -q`]);
const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
const body = (ref) => Buffer.concat([pkt(`${"0".repeat(40)} ${tip} ${ref}\0 report-status agent=git/2.43\n`), Buffer.from("0000"), pack]);

const log = () => readFileSync(`${B}/dev.log`, "utf8").split("\n").filter((l) => l.includes("cwcall"));
const stages = (st) => Object.fromEntries((st ?? "").split(", ").map((p) => p.match(/^(\w+);dur=([\d.]+)/)).filter(Boolean).map((m) => [m[1], Number(m[2])]));
const KEEP = ["auth", "install", "resolve", "front", "gate_rows", "gate_authz", "gate", "gate_iso", "context", "total"];

async function req(edge, method, p, b, hdr = {}) {
  const before = log().length;
  const t0 = Date.now();
  const r = await fetch(F + p, { method, body: b, headers: { authorization: auth, "x-cw-edge": String(edge), "x-bed-token": "cw-bed", ...hdr } });
  await r.arrayBuffer();
  const ms = Date.now() - t0;
  await new Promise((res) => setTimeout(res, 300));
  const calls = log().slice(before).map((l) => {
    const [, at, dur, site, name, cpath, by] = l.slice(l.indexOf("cwcall")).split(" ");
    return { at: Number(at) - t0, dur: Number(dur), site, name, path: cpath, by };
  }).sort((a, b) => a.at - b.at);
  return { status: r.status, ms, st: stages(r.headers.get("server-timing")), calls };
}

function show(label, r) {
  const s = KEEP.filter((k) => k in r.st).map((k) => `${k} ${r.st[k]}`).join(", ");
  console.log(`${label}: ${r.status} in ${r.ms} ms | ${s}`);
  for (const c of r.calls) console.log(`    +${String(c.at).padStart(4)} ${String(c.dur).padStart(4)} ms  s${c.site} ${c.path.padEnd(20)} ${c.name}`);
}

const post = (edge, ref) => req(edge, "POST", `${path}/git-receive-pack`, body(ref), { "content-type": "application/x-git-receive-pack-request" });
const n = Number(N);
const out = [];
// Edge 0 pushes first, so the repository's group exists and leads, as on a hosted install: the cold edges then time
// a cold isolate, not the group's first create.
show("edge 0 setup POST", await post(0, "refs/heads/setup"));
// ADVERT=1: each cold POST follows its advertisement on edge 0, another isolate, as git sends them and as mt18's cold
// POSTs did (`cold-write`, C: only an advertisement puts the entry a POST serves from).
const advertFirst = process.env.ADVERT === "1";
for (let e = 1; e < n - 1; e++) {
  if (advertFirst) show(`edge 0 advertisement before edge ${e}`, await req(0, "GET", `${path}/info/refs?service=git-receive-pack`));
  const cold = await post(e, `refs/heads/cold${e}`);
  show(`edge ${e} cold POST`, cold);
  const warm = await post(e, `refs/heads/warm${e}`);
  show(`edge ${e} warm POST`, warm);
  out.push({ edge: e, cold: cold.st, warm: warm.st, coldCalls: cold.calls.length, warmCalls: warm.calls.length });
}
// A cold isolate whose first request is the advertisement, as git sends it, then the POST.
const adv = await req(n - 1, "GET", `${path}/info/refs?service=git-receive-pack`);
show(`edge ${n - 1} cold advertisement`, adv);
const after = await post(n - 1, `refs/heads/after`);
show(`edge ${n - 1} POST after it`, after);
out.push({ edge: n - 1, advertised: true, cold: after.st, coldCalls: after.calls.length });
console.log("JSON " + JSON.stringify(out));
