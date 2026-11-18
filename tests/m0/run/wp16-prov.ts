// M0-WP16 L0-10: provenance through a vault restore, on `graft-wp16-prov` (src/wp16/prov.ts) and `graft-wp16-vault`
// (src/wp16/vault.ts).
//
//   node run/wp16-prov.ts all /home/user/wp16/prov-plan.json [--out wp16-l0-10]
//
// 1. Replays wp16/provgen.py's plan (10,000 agent commits, tasks, PRs, notes, session logs, squash merges) into group
//    "mono", in batches, while the vault pulls the export behind it.
// 2. Resolves every planned commit (agent and squash) and compares with the plan's expected session or PR.
// 3. The loss: wipes group "mono" (its Durable Object storage and both its buckets), after the vault's last pull.
// 4. Whole-install restore from the vault into group "mono-r" (new object, new buckets): snapshot, tail, packs, open.
// 5. Resolves every commit again on "mono-r", and compares the fold digest with the one taken before the loss.
// Deploy: wrangler deploy -c wp16/graft-wp16-prov.jsonc and -c wp16/graft-wp16-vault.jsonc, then secrets M0_TOKEN
// (both) and EXPORT_TOKEN (both, the same random value).

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";

const ROOT = new URL("../", import.meta.url).pathname;
const TOKEN = readFileSync(`${ROOT}.m0-token`, "utf8").trim();
const PROV = `https://graft-wp16-prov.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`, VAULT = `https://graft-wp16-vault.${process.env.CF_WORKERS_SUBDOMAIN}.workers.dev`;
const arg = (name: string, d: string) => { const i = process.argv.indexOf(`--${name}`); return i > 0 ? process.argv[i + 1] : d; };

async function post<T = any>(base: string, path: string, body: unknown): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    const r = await fetch(base + path, { method: "POST", body: JSON.stringify(body), headers: { authorization: `Bearer ${TOKEN}` }, signal: AbortSignal.timeout(120_000) })
      .catch((e) => e as Error);
    if (r instanceof Error) { if (attempt < 3) continue; throw r; }
    const text = await r.text();
    if (r.ok) return JSON.parse(text) as T;
    // Only idempotent calls are retried on 5xx (pull, resolve, restore steps); /run is retried only on a network error.
    if (attempt < 3 && r.status >= 500 && path !== "/run") continue;
    throw new Error(`${path}: ${r.status} ${text.slice(0, 400)}`);
  }
}

interface Plan { events: any[]; expect: Record<string, string>; kinds: Record<string, number> }

async function resolveAll(group: string, commits: string[]): Promise<Record<string, string>> {
  const out: Record<string, string> = {};
  for (let i = 0; i < commits.length; i += 500) Object.assign(out, await post(PROV, "/resolve", { group, commits: commits.slice(i, i + 500) }));
  return out;
}
const score = (got: Record<string, string>, expect: Record<string, string>) => {
  const wrong = Object.entries(expect).filter(([c, v]) => got[c] !== v);
  return { commits: Object.keys(expect).length, resolved: Object.values(got).filter((v) => v !== "none").length, asExpected: Object.keys(expect).length - wrong.length,
    wrong: wrong.length, wrongSample: wrong.slice(0, 5).map(([c, v]) => ({ c, want: v, got: got[c] })),
    sessions: Object.values(got).filter((v) => v.startsWith("session:")).length, prs: Object.values(got).filter((v) => v.startsWith("pr:")).length };
};

async function all(file: string): Promise<void> {
  const plan: Plan = JSON.parse(readFileSync(file, "utf8")), out = arg("out", "wp16-l0-10"), t0 = Date.now(), log: Record<string, unknown> = {};
  // 1. replay, in batches of commands, with a vault pull after every batch.
  // A clean start: an empty install group and an empty vault.
  log.reset = { mono: await post(PROV, "/wipe", { group: "mono" }), monoR: await post(PROV, "/wipe", { group: "mono-r" }), vault: await post(VAULT, "/reset", {}) };
  // The vault pulls continuously behind the install, as the vault Worker does; about 80 entries a pull keeps one pull's
  // pack copies well inside a Worker invocation. Lag samples are entries behind the install's last entry.
  let vaultCursor = 0, replaying = true;
  const lag: number[] = [];
  const vaultLoop = (async () => {
    while (replaying) {
      const pl = await post(VAULT, "/pull", { max: 150 }).catch(() => null);
      if (pl) vaultCursor = pl.cursor;
      const head = await post(PROV, "/digest", { group: "mono" }).catch(() => null);
      if (head) lag.push(head.last - vaultCursor);
      if (pl && pl.entries === 0) await new Promise((r) => setTimeout(r, 2000));
    }
  })();
  let refused = 0, refusedAsExpected = 0, unexpected = 0;
  const refusals: unknown[] = [];
  for (let i = 0; i < plan.events.length;) {
    const batch: any[] = [];
    let bytes = 0;
    while (i < plan.events.length && batch.length < 40 && bytes < 2_000_000) { const e = plan.events[i++]; bytes += JSON.stringify(e).length; batch.push(e); }
    const res = await post<{ ok: boolean; why?: string }[]>(PROV, "/run", { group: "mono", cmds: batch });
    // A command marked `refuse` (an agent's `refs/notes/ai` update) must be refused; every other must commit.
    res.forEach((r, x) => {
      if (!r.ok) refused++;
      if (batch[x].refuse && !r.ok) refusedAsExpected++;
      if (!!batch[x].refuse === r.ok && refusals.length < 20) refusals.push({ cmd: { ...batch[x], objects: undefined }, ok: r.ok, why: r.why });
      if (!!batch[x].refuse === r.ok) { unexpected++; if (unexpected <= 5) console.log("unexpected", JSON.stringify({ k: batch[x].k, ref: batch[x].ref, actor: batch[x].actor, refuse: batch[x].refuse, r })); }
    });
    if (i % 400 < 40) console.log(`${i}/${plan.events.length} refused ${refused} vault ${vaultCursor} ${((Date.now() - t0) / 1000).toFixed(0)} s`);
  }
  replaying = false; await vaultLoop;
  const ls = [...lag].sort((x, y) => x - y);
  log.replay = { events: plan.events.length, refused, refusedAsExpected, unexpected, refusals, s: (Date.now() - t0) / 1000,
    vaultLagEntries: { samples: ls.length, p50: ls[Math.floor(ls.length / 2)], max: ls.at(-1) } };
  // Vault catches up to the install's last entry.
  const before = await post(PROV, "/digest", { group: "mono" });
  for (;;) { const p = await post(VAULT, "/pull", { max: 150 }); if (p.cursor >= before.last) break; }
  log.before = before;
  // 2. resolve before the loss.
  const commits = Object.keys(plan.expect);
  log.resolveBefore = score(await resolveAll("mono", commits), plan.expect);
  console.log("before", JSON.stringify(log.resolveBefore));
  // 3. the loss.
  log.wipe = await post(PROV, "/wipe", { group: "mono" });
  log.afterWipe = { digest: await post(PROV, "/digest", { group: "mono" }), resolve: score(await resolveAll("mono", commits.slice(0, 500)), Object.fromEntries(commits.slice(0, 500).map((c) => [c, plan.expect[c]]))) };
  // 4. restore from the vault into "mono-r", to the cutoff = the vault cursor (the last entry before the loss).
  const tr = Date.now(), cutoff = (await post(VAULT, "/cursor", {})).cursor;
  const snap = await post(VAULT, "/restore", { step: "snapshot", cutoff });
  let from = (snap.upto ?? 0) + 1, tail = 0;
  while (from <= cutoff) {
    const r = await post(VAULT, "/restore", { step: "tail", cutoff, from, max: 300 });
    if (!r.ok) throw new Error(`tail: ${JSON.stringify(r)}`);
    tail += r.n; from = r.last + 1;
  }
  let packs = 0, packsOk = 0;
  const bad: string[] = [];
  for (let offset = 0; ; offset += 40) {
    const r = await post(VAULT, "/restore", { step: "packs", cutoff, offset, max: 40 });
    packs += r.n; packsOk += r.ok; bad.push(...r.bad);
    if (r.n < 40) break;
  }
  const open = await post(VAULT, "/restore", { step: "open", cutoff });
  log.restore = { cutoff, snapshot: snap, tailEntries: tail, packs, packsOk, bad, open, s: (Date.now() - tr) / 1000 };
  console.log("restore", JSON.stringify(log.restore));
  // 5. resolve after, and compare folds.
  const after = await post(PROV, "/digest", { group: "mono-r" });
  log.after = after;
  log.resolveAfter = score(await resolveAll("mono-r", commits), plan.expect);
  log.foldEqual = after.digest === before.digest && after.lastDigest === before.lastDigest;
  log.kinds = plan.kinds;
  log.totalS = (Date.now() - t0) / 1000;
  mkdirSync(`${ROOT}results/raw`, { recursive: true });
  writeFileSync(`${ROOT}results/raw/${out}.json`, JSON.stringify(log, null, 1));
  console.log(JSON.stringify({ before: log.resolveBefore, after: log.resolveAfter, foldEqual: log.foldEqual, restore: log.restore }, null, 1));
}

if (import.meta.url === `file://${process.argv[1]}`) {
  if (process.argv[2] === "all") await all(process.argv[3]);
  else throw new Error("usage: node run/wp16-prov.ts all <prov-plan.json> [--out name]");
}
