// The parts of a1.mjs that tests/am1/load34/load34.mjs runs too, so the hosted B0-34 client sends the same bytes: journey
// 9's plan, the pack builder (one blob, its trees, and a commit, as git writes them), pkt-lines, receive-pack bodies and
// results, and the clone probe's CloneKey digest. Node and workerd (`nodejs_compat`) both run it: `Buffer` and `node:crypto`
// only, and pako's deflate (vendor/README.md: workerd's `node:zlib` writes other bytes at level 1). load34.test.mjs checks
// that both runtimes write a1's bytes from before this file existed.
import { Buffer } from "node:buffer";
import { createHash } from "node:crypto";
import pako from "./vendor/pako_deflate.min.cjs";

const deflateSync = (b, { level }) => Buffer.from(pako.deflate(b, { level }));

/** Journey 9 at `agents` (B0-29; `cost_agents.py --a1`): pushes every 120 s (40 KB mean, 5% at 1 MiB), the A1 agent
 *  side-command mix per 500 agents, 1.25 human and 1.0 bot commands/s, 10 merges/min, 33 ref reads/s, 200 API
 *  reads/s, 500 pollers. Rates are whole-fleet per second. */
export function journey9(agents) {
  const f = agents / 500;
  return {
    agents, pushEvery: 120, packMin: 10_000, packMax: 70_000, bigShare: 0.05, bigBytes: 1 << 20,
    // cost_agents.py --a1, agent class per 500 agents, the parts the edge serves a route for. PrSync and test-merge
    // records (4.17/s each) are the host's own commands after each push; RequiredCheck and check runs (3.33/s) have
    // no edge route and are not offered (`unserved`).
    // `standIn` is task summary edits at the unserved rate, so the group sees the row's command rate; they are
    // agent-class commands of one small entry each, as a check run is.
    side: { taskEdit: 1.67 * f, comment: 1.88 * f, label: 1.87 * f, draftPr: 0.42 * f, leaseAcquire: 0.42 * f,
      leaseRelease: 0.41 * f, leaseRenew: 2.5 * f, sessionPush: 0.21 * f, standIn: 3.33 * f },
    hostMade: 8.34 * f, unserved: 3.33 * f,
    human: 1.25, bot: 1.0, mergesPerMin: 10, refReads: 33, apiReads: 200, clones: 0, pollers: 500,
  };
}

// ---- packs: one blob, its tree, and a commit, as git writes them ----

const sha1 = (b) => createHash("sha1").update(b).digest();
const obj = (type, body) => sha1(Buffer.concat([Buffer.from(`${type} ${body.length}\0`), body]));
const TYPE = { commit: 1, tree: 2, blob: 3 };
function entryHeader(type, size) {
  const out = [];
  let b = (TYPE[type] << 4) | (size & 15);
  size >>= 4;
  while (size) { out.push(b | 0x80); b = size & 0x7f; size >>= 7; }
  out.push(b);
  return Buffer.from(out);
}
/** The tree of `files` ([path parts, blob id]), its subtrees pushed to `objects`; entries sorted as git sorts them
 *  (a directory's name compares as if it ended in "/"). */
function treeOf(files, objects) {
  const groups = new Map();
  for (const [[h, ...rest], id] of files) (groups.get(h) ?? groups.set(h, []).get(h)).push([rest, id]);
  const entries = [...groups].map(([name, xs]) => (xs.length === 1 && xs[0][0].length === 0
    ? { mode: "100644", name, id: xs[0][1] } : { mode: "40000", name, id: treeOf(xs, objects) }));
  const key = (e) => Buffer.from(e.mode === "40000" ? `${e.name}/` : e.name);
  entries.sort((a, b) => Buffer.compare(key(a), key(b)));
  const body = Buffer.concat(entries.map((e) => Buffer.concat([Buffer.from(`${e.mode} ${e.name}\0`), e.id])));
  objects.push(["tree", body]);
  return obj("tree", body);
}
/** The pack and the commit id of one commit on `parent` (hex, or null) holding `bytes` at `path`, and each `extra`
 *  [path, bytes] beside it (B0-35's workflow file, so every commit and every test merge keeps it). */
export function onePack(path, bytes, parent, message, at = 1_700_000_000, extra = []) {
  const files = [[path, bytes], ...extra];
  const blobs = files.map(([, b]) => ["blob", b]);
  const objects = [];
  const root = treeOf(files.map(([p, b]) => [p.split("/"), obj("blob", b)]), objects);
  const who = `a1 <a1@graftyard.invalid> ${at} +0000`;
  const commit = Buffer.from(`tree ${root.toString("hex")}\n${parent ? `parent ${parent}\n` : ""}author ${who}\ncommitter ${who}\n\n${message}\n`);
  const all = [...blobs, ...objects, ["commit", commit]];
  const head = Buffer.alloc(12);
  head.write("PACK"); head.writeUInt32BE(2, 4); head.writeUInt32BE(all.length, 8);
  const body = Buffer.concat([head, ...all.flatMap(([t, b]) => [entryHeader(t, b.length), deflateSync(b, { level: 1 })])]);
  return { pack: Buffer.concat([body, sha1(body)]), commit: obj("commit", commit).toString("hex") };
}
export const pkt = (s) => { const b = Buffer.from(s); return Buffer.concat([Buffer.from((b.length + 4).toString(16).padStart(4, "0")), b]); };
export const ZERO = "0".repeat(40);
/** The receive-pack request body for one ref update, with report-status and no side-band. */
export const receiveBody = (ref, old, nu, pack) =>
  Buffer.concat([pkt(`${old ?? ZERO} ${nu} ${ref}\0report-status agent=graftyard-a1\n`), Buffer.from("0000"), pack]);
/** The payloads of a pkt-line stream (flushes dropped); stops at the first malformed length. */
function pktLines(s) {
  const out = [];
  for (let i = 0; i + 4 <= s.length;) {
    const n = parseInt(s.slice(i, i + 4), 16);
    if (!/^[0-9a-f]{4}$/.test(s.slice(i, i + 4)) || (n > 0 && n < 4)) break;
    if (n === 0) { i += 4; continue; }
    out.push(s.slice(i + 4, i + n));
    i += n;
  }
  return out;
}
/** `ok`, or the `ng` reason, from a report-status reply, side-band framed or not. A status line is read by its
 * pkt-line length, not by the text before it, since a length can end in a hex letter (`002cok refs/...`). */
export function pushResult(text) {
  const raw = pktLines(text);
  const band1 = raw.filter((l) => l[0] === "\x01").map((l) => l.slice(1)).join("");
  const lines = raw.some((l) => l[0] === "\x01") ? pktLines(band1) : raw.filter((l) => l[0] !== "\x02" && l[0] !== "\x03");
  const ng = lines.map((l) => l.match(/^ng (\S+) ([^\n]*)/)).find(Boolean);
  // Any other refusal keeps its reason, ids and numbers masked, so a row says why (B0-31's 1,484 were unexplained).
  const mask = (x) => x.replace(/[0-9a-f]{12,}/g, "H").replace(/\d+/g, "N").slice(0, 80).trim();
  // `unpacker error` names its cause on the `unpack <why>` line (`Refusal::unpack_failed`).
  const unpack = lines.map((l) => l.match(/^unpack (?!ok\n)([^\n]*)/)).find(Boolean);
  if (ng) return ng[2].includes("admission full") ? "full" : /stale|fetch first|non-fast-forward/.test(ng[2]) ? "stale"
    : `refused ${mask(ng[2])}${unpack ? `: ${mask(unpack[1])}` : ""}`;
  return lines.some((l) => l.startsWith("ok refs/")) ? "ok" : "error";
}

/** The edge's CloneKey digest (crates/git/src/clone.rs `CloneKey::digest`) for the probe's fetch: one want, depth 1,
 *  no filter, caps `no-progress` and `ofs-delta`, no `include-tag` (so no tags digest). */
export function cloneKeyHex(net, want) {
  const wants = createHash("sha256").update(Buffer.from(want, "hex")).digest();
  const be64 = (n) => { const b = Buffer.alloc(8); b.writeBigUInt64BE(BigInt(n)); return b; };
  const h = createHash("sha256").update("graftyard/clone/1\0").update(be64(net)).update(wants).update(be64(2))
    .update("\0none").update("\0");
  for (const c of ["no-progress", "ofs-delta"]) h.update(c).update("\0");
  return h.update(Buffer.alloc(32)).digest("hex");
}
