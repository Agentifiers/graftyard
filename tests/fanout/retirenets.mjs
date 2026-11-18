// Retires every member of finished test networks by their current names, then runs purge.py on them (R25, operator
// 2026-11-17). Old retire lists named members by the generation they had when listed, and a later voter generation
// stayed joined (131336, 131346), so the names come from the install now:
//   - each site's registry row for `repo:N` (the voter that site's lookups reach);
//   - the voters and learners of the newest committed config any of those voters answers;
//   - each site's first-generation voter (`repo:N#1#f<site>`).
// Each is sent `/retire` (test route: storage deleted, object reset), then `/status` again; a network purges only when
// purge.py's own gate finds every voter "not joined". `repo-meta:N` is handled the same way when a registry row names it.
//   node tests/fanout/retirenets.mjs FIRST-LAST|N ...      (NO_PURGE=1: retire only)
import { appendFileSync } from "node:fs";
import { spawn } from "node:child_process";
import { member, tools } from "../am1/hosted/lib.mjs";

const LOG = process.env.RETIRE_LOG ?? "tests/fanout/raw/retirenets-m2b.log";
const log = (s) => { const l = `${new Date().toISOString()} ${s}`; console.log(l); appendFileSync(LOG, l + "\n"); };
const nets = process.argv.slice(2).flatMap((a) => {
  const [x, y] = a.split("-").map(Number);
  return y ? Array.from({ length: y - x + 1 }, (_, i) => x + i) : [x];
});
const status = (c, s, n) => member(c, s, "/status", undefined, 15_000, n).then((r) => r.v, (e) => ({ error: String(e).slice(0, 80) }));
const brief = (v) => v?.joined === false ? "not joined" : v?.error ? `error ${v.error}` : `${v?.role} t${v?.term}`;

async function members(chain) {
  const names = new Map(); // do_name -> site
  let cfg = null;
  for (const s of [0, 1, 2]) {
    names.set(`${chain}#1#f${s}`, s);
    const row = await tools("/registry", { chain, site: s }).catch(() => null);
    if (row?.do_name) names.set(row.do_name, s);
  }
  for (const [n, s] of [...names]) {
    const st = await status(chain, s, n);
    const c = st?.config;
    if (c?.config?.normal && (!cfg || (c.index ?? 0) > (cfg.index ?? 0))) cfg = c;
  }
  for (const m of [...(cfg?.config?.normal?.voters ?? []), ...(cfg?.config?.normal?.learners ?? [])]) names.set(m.do_name, m.site);
  return names;
}

const done = [];
for (const net of nets) {
  for (const chain of [`repo:${net}`, `repo-meta:${net}`]) {
    if (chain.startsWith("repo-meta:")) {
      const rows = await Promise.all([0, 1, 2].map((s) => tools("/registry", { chain, site: s }).catch(() => null)));
      if (!rows.some((r) => r?.do_name)) continue;
    }
    const names = await members(chain);
    let joined = 0;
    for (const [n, s] of names) {
      const before = await status(chain, s, n);
      if (before?.joined === false) continue;
      joined++;
      // The member aborts as it answers, so the reply is often empty: the `/status` after is the check.
      await member(chain, s, "/retire", {}, 60_000, n).catch(() => null);
    }
    await new Promise((r) => setTimeout(r, 2_000));
    const left = [];
    for (const [n, s] of names) { const b = brief(await status(chain, s, n)); if (b !== "not joined") left.push(`${n} ${b}`); }
    log(`${chain}: ${names.size} names, ${joined} were joined, ${left.length ? `STILL JOINED ${left.join("; ")}` : "all not joined"}`);
  }
  done.push(net);
}
// purge.py runs in its own process group, which this process kills if it is stopped itself: run under `timeout`, the
// old `execFileSync` child outlived it, and an orphaned purge.py deleted a later run's objects with no log line
// (131357's bucket, B0-29 run 5, `tests/m0/results/merge-tail.md`).
if (!process.env.NO_PURGE && done.length) {
  const child = spawn("uv", ["run", "-q", "--with", "boto3", "python3", "tests/fanout/purge.py", ...done.map(String)],
    { detached: true, stdio: ["ignore", "pipe", "inherit"],
      env: { ...process.env, PURGE_LOG: process.env.PURGE_LOG ?? "tests/fanout/raw/purge-m2b.log" } });
  const stop = (sig) => { try { process.kill(-child.pid, "SIGTERM"); } catch {} process.exit(sig === "SIGINT" ? 130 : 143); };
  for (const sig of ["SIGTERM", "SIGINT", "SIGHUP"]) process.on(sig, () => stop(sig));
  let out = "";
  child.stdout.on("data", (d) => { out += d; });
  const code = await new Promise((r) => child.on("close", r));
  for (const l of out.trim().split("\n").filter(Boolean)) log(`purge: ${l.slice(21)}`);
  if (code !== 0) { log(`purge: exit ${code}`); process.exitCode = 1; }
}
