// B0-30's create-time config check (b030r6, operator 2026-11-17: "no Degraded entry before a try is chosen at the
// create", G22). No member route returns an entry's body, so for each `config` entry in the first N entries of a
// chain's log this rebuilds the canonical bytes (`crates/raft/src/encode.rs`, `encode_entry`) of every `Normal` and
// `Degraded { lost }` candidate over the group's three voters, one site at generation 1 to GEN and the others at 1, and
// names the one whose SHA-256 is the stored digest, as `digest208160.py` did for b030r2 and the b030r5 evidence did by
// hand. Prints one line per config entry, then a verdict: a `Degraded` entry must be followed by the `Normal` naming
// its new member at the next index (G22's back-to-back pair), so a Degraded held across other entries fails.
//   node tests/am1/b030split/configs.mjs CHAIN [SITE N]   (reads the leader's log through SITE's current voter)
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const [chain, site = "0", n = "60"] = process.argv.slice(2);
const GEN = 12;
// LOG_FILE: a saved `/log` answer instead of a call (also how the matcher was checked against b030r5's entries 3 and 5).
const log = process.env.LOG_FILE ? JSON.parse(readFileSync(process.env.LOG_FILE, "utf8")) : await (async () => {
  const { member, voterNames } = await import("../hosted/lib.mjs");
  const names = await voterNames(chain);
  return (await member(chain, Number(site), `/log?from=1&n=${n}`, undefined, 30_000, names[site])).v ?? [];
})();
const u = (bytes, v) => { const b = Buffer.alloc(bytes); bytes === 8 ? b.writeBigUInt64BE(BigInt(v)) : bytes === 4 ? b.writeUInt32BE(v) : bytes === 2 ? b.writeUInt16BE(v) : b.writeUInt8(v); return b; };
const str = (s) => Buffer.concat([u(4, Buffer.byteLength(s)), Buffer.from(s)]);
const mem = (s, g) => Buffer.concat([u(1, s), u(1, 0), u(1, s), u(4, g), str(`${chain}#${g}#f${s}`)]);
const placement = (gens, gen) => Buffer.concat([u(4, 3), ...gens.map((g, s) => mem(s, g)), u(4, 0), u(1, 0), u(4, gen)]);
const entry = (e, prev, cfg) => Buffer.concat([Buffer.from("graftyard/log/1"), u(8, e.term), u(8, e.index), prev,
  u(2, 0), u(2, 1), u(1, 0), u(8, e.at), u(4, 0), u(1, 0), u(1, 1), cfg]);
const candidates = function* () {
  for (let s = 0; s < 3; s++) for (let g = s === 0 ? 1 : 2; g <= GEN; g++) {
    const gens = [1, 1, 1]; gens[s] = g;
    for (let pg = 1; pg <= GEN; pg++) {
      yield [`Normal gen ${pg} [${gens}]`, Buffer.concat([u(1, 0), placement(gens, pg)])];
      for (const l of [0, 1, 2]) yield [`Degraded lost ${l} gen ${pg} [${gens}]`, Buffer.concat([u(1, 1), placement(gens, pg), u(1, l)])];
    }
  }
};
const out = [];
for (let i = 1; i < log.length; i++) {
  const e = log[i];
  if (e.kind !== "config") continue;
  const prev = Buffer.from(log[i - 1].digest, "hex");
  const hit = [...candidates()].filter(([, cfg]) => createHash("sha256").update(entry(e, prev, cfg)).digest("hex") === e.digest).map(([k]) => k);
  out.push({ index: e.index, term: e.term, at: new Date(e.at).toISOString(), committed: e.committed, body: hit[0] ?? "unmatched", matches: hit.length });
}
for (const o of out) console.log(JSON.stringify(o));
// Pass: every Degraded entry is followed at the next index by a Normal entry.
const bad = out.filter((o, k) => o.body.startsWith("Degraded") && !(out[k + 1]?.index === o.index + 1 && out[k + 1].body.startsWith("Normal")));
const unmatched = out.filter((o) => o.body === "unmatched");
console.log(JSON.stringify({ chain, configs: out.length, degradedHeld: bad.map((o) => o.index), unmatched: unmatched.map((o) => o.index),
  pass: bad.length === 0 && unmatched.length === 0 }));
