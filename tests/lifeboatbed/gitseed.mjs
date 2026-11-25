// lifeboatbed's check 8 (`run.sh`): real git history for the lifeboat's git copy (`decisions/DECISIONS.md`,
// "`lifeboat-clone`"), committed through the edge's `/_bed/push` with the seeded PAT. A local repository `<out>/src`
// makes the commits; each push is the thin pack of the new commits against the last pushed tip, as a client sends
// one, committed as a ref update with that pack inline, or stored once it is past `STORED_FROM` bytes.
//
//   node gitseed.mjs push <front> <bed token> <out> <ref> <commits> [stored]
//       commits `<commits>` new files on `<ref>` and pushes them in one entry.
//   node gitseed.mjs roll <front> <bed token> <out>
//       a roll on the history so far, as the archive alarm and the roller make one: two promotions, each a complete
//       pack of its range's objects recorded as `Promoted` at 32.25, then one `Compacted` of both at 32.25 whose
//       produced pack holds every object. Each pack is placed as a stored pack and marked at its entry. Prints the
//       consumed packs' ids, which `run.sh` deletes from every bucket, as the sweeper does after `CONSUMED_KEEP`.
// Writes <out>/git.json: the pushed tips by ref, every entry's index, and the roll.
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";

const [MODE, FRONT, BED, OUT, ...rest] = process.argv.slice(2);
const SRC = `${OUT}/src`;
const STATE = `${OUT}/git.json`;
const s = JSON.parse(readFileSync(`${OUT}/seed.json`));
const PAT = readFileSync(`${OUT}/pat`, "utf8");
const st = existsSync(STATE) ? JSON.parse(readFileSync(STATE)) : { tips: {}, entries: [], roll: null };
const git = (...a) => execFileSync("git", a, { cwd: SRC, encoding: "buffer", maxBuffer: 1 << 28 });
const out = (...a) => git(...a).toString().trim();
const FORMAT = { family: 0x20, version: 25 };

async function bed(path, body, auth) {
  const h = { "x-bed-token": BED };
  if (auth) h.authorization = `token ${PAT}`;
  const r = await fetch(`${FRONT}${path}?repo=${s.id}`, { method: "POST", headers: h, body: JSON.stringify(body) });
  const t = await r.text();
  try { return JSON.parse(t); } catch { throw new Error(`${path}: ${r.status} ${t.slice(0, 400)}`); }
}

// A pack of `revs` (rev-list arguments), thin against what the `^` revisions name when `thin`.
const pack = (revs, thin) => execFileSync("git", ["pack-objects", "--stdout", "--revs", ...(thin ? ["--thin"] : [])],
  { cwd: SRC, input: revs.join("\n") + "\n", maxBuffer: 1 << 28 });

if (!existsSync(SRC)) {
  execFileSync("git", ["init", "-q", "-b", "main", SRC]);
  git("config", "user.email", "lb@lbbed.test");
  git("config", "user.name", "lifeboatbed");
}

if (MODE === "push") {
  const [ref, n, stored] = rest;
  const branch = ref.replace("refs/heads/", "");
  const had = st.tips[ref] ?? null;
  const base = had ?? Object.values(st.tips)[0] ?? null;
  if (base) git("checkout", "-q", "-B", branch, base); else git("checkout", "-q", "--orphan", branch);
  for (let i = 0; i < Number(n); i++) {
    const f = `${branch.replace(/\//g, "_")}-${st.entries.length}-${i}.txt`;
    // Text that compresses and deltas, as source does, with enough of it that a stored pack is real.
    writeFileSync(`${SRC}/${f}`, Array.from({ length: 400 }, (_, k) => `${f} line ${k} ${k * i}\n`).join(""));
    git("add", f);
    git("commit", "-q", "-m", `${f}`);
  }
  const tip = out("rev-parse", "HEAD");
  const not = Object.values(st.tips).map((t) => `^${t}`);
  const bytes = pack([tip, ...not], true);
  const r = await bed("/_bed/push", { ref, expect: had, new: tip, pack: bytes.toString("base64"), inline: stored !== "stored" }, true);
  const o = r.outcome?.Ok ?? r.outcome;
  if (!o?.ok) throw new Error(`push ${ref}: ${JSON.stringify(r).slice(0, 400)}`);
  st.tips[ref] = tip;
  st.entries.push({ ref, tip, index: o.ok.index, stored: stored === "stored", bytes: bytes.length });
  writeFileSync(STATE, JSON.stringify(st));
  console.log(`pushed ${ref} ${tip} at ${o.ok.index} (${bytes.length} bytes, ${stored === "stored" ? "stored" : "inline"})`);
  process.exit(0);
}

if (MODE === "roll") {
  // Two promotions over the entries so far, split in the middle: each complete for its range's tips, the second's
  // objects building on the first's, as `promotion_inputs` makes them.
  const es = st.entries;
  const mid = Math.floor(es.length / 2);
  const ranges = [es.slice(0, mid), es.slice(mid)];
  const tipsThrough = (k) => {
    const t = {};
    for (const e of es.slice(0, k)) t[e.ref] = e.tip;
    return Object.values(t);
  };
  const promos = [];
  let from = 1;
  for (const [i, r] of ranges.entries()) {
    const to = r[r.length - 1].index;
    const not = i === 0 ? [] : tipsThrough(mid).map((t) => `^${t}`);
    const bytes = pack([...tipsThrough(i === 0 ? mid : es.length), ...not], false);
    const placed = await bed("/_bed/place", { pack: bytes.toString("base64") });
    if (!placed.pack) throw new Error(`place: ${JSON.stringify(placed)}`);
    const rec = await bed("/_bed/record", { events: [{ Promoted: { from, to, pack: placed.pack } }], format: FORMAT });
    if (!rec.recorded) throw new Error(`Promoted ${from}..=${to}: ${JSON.stringify(rec)}`);
    promos.push({ from, to, pack: placed.pack, at: rec.recorded });
    from = to + 1;
  }
  const all = pack(tipsThrough(es.length), false);
  const placed = await bed("/_bed/place", { pack: all.toString("base64") });
  if (!placed.pack) throw new Error(`place: ${JSON.stringify(placed)}`);
  const consumed = promos.map((p) => p.pack.stored.id);
  const c = { consumed, produced: placed.pack, from: 1, to: promos[1].to };
  const rec = await bed("/_bed/record", { events: [{ Compacted: c }], format: FORMAT });
  if (!rec.recorded) throw new Error(`Compacted: ${JSON.stringify(rec)}`);
  st.roll = { consumed, produced: placed.pack.stored.id, at: rec.recorded, promotions: promos };
  writeFileSync(STATE, JSON.stringify(st));
  console.log(consumed.join(" "));
  process.exit(0);
}

throw new Error(`mode ${MODE}`);
