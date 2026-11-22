// Load34's pure parts (tests/am1/load34/load34.mjs runs them in each client object, tests/am1/load34/b034load.mjs on the
// orb, and load34.test.mjs checks them): how a1.mjs's B0-34 load splits over N client objects, the phase a sample
// falls in, the per-object fetch slots, and the samples' merge into a1's format.
import { journey9 } from "../rest/wire.mjs";

/** Fetches one Durable Object may have waiting for response headers at once; a seventh queues inside the runtime
 *  [measured 2026-11-20 on graft-m1-load: 30 fetches of a 2 s answer from one object took 10.1 s, and 60 from 10
 *  objects 2.5 s]. A client never has more than this many out, so no runtime queue is inside a sample. */
export const SLOTS = 6;

/** B0-34's whole-fleet plan, as a1.mjs `planFor("b0-34")` with `--b034` makes it: journey 9 at `agents`, one clone probe
 *  a second, no merges, the tip moved 10 times a minute. */
export function b034Plan(agents = 500) {
  return { ...journey9(agents), clones: 1, mergesPerMin: 0, tipMovesPerMin: 10 };
}

/** a1's capped kinds (`every(kind, perS, f, cap)`): the tip mover (cap 1), the clone probe (20), human pushes (5), and
 *  each hot network's pushes and whole clones (8 each, from the network's mark). A kind whose cap is over SLOTS is
 *  split over ceil(cap / SLOTS) clients, each with its share of the rate and of the cap, so the fleet's cap is a1's
 *  and no client holds more requests than it has slots. `part` of `parts` says which share a client has (a hot push
 *  client pushes only the refs k with k % parts == part, so each ref's tip stays in one client). */
export function roles(p, hot) {
  const kinds = [{ role: "tip", kind: "tip:move", perS: p.tipMovesPerMin / 60, cap: 1 },
    { role: "probe", kind: "read:clone", perS: p.clones, cap: 20 },
    { role: "human", kind: "push:human", perS: p.human / 2, cap: 5 },
    ...hot.flatMap((h, net) => [{ role: "hotPush", kind: `hot:push${h.tag}`, net, perS: h.pushPerS ?? 1, cap: 8 },
      { role: "hotClone", kind: `hot:clone${h.tag}`, net, perS: h.clonePerS ?? 3, cap: h.cloneCap ?? 8 }])];
  return kinds.flatMap((k) => {
    const parts = Math.ceil(k.cap / SLOTS);
    return Array.from({ length: parts }, (_, part) => ({ ...k, perS: k.perS / parts,
      cap: Math.floor(k.cap / parts) + (part < k.cap % parts ? 1 : 0), part, parts }));
  });
}

/** Client `i` of `n`. Clients 0 to R - 1 run one role each (`roles`; client 0 moves the tip); the other n - R share every uncapped kind (a1's
 *  cap-400 kinds, each at perS / (n - R)), the agents and the feed pollers, round-robin. `hot` is [{tag, ...}] for
 *  the second network and the hostile owner's three. */
export function clientPlan(i, n, p, hot) {
  const rs = roles(p, hot), w = n - rs.length;
  if (w < 1) throw new Error(`${n} clients, need more than the ${rs.length} roles`);
  if (i < rs.length) return { roles: [rs[i]], uncapped: [], agents: [], pollers: [] };
  const j = i - rs.length, side = p.side;
  const uncapped = [
    ["side:taskEdit", side.taskEdit], ["side:comment", side.comment], ["side:label", side.label],
    ["side:standIn", side.standIn], ["side:draftPr", side.draftPr], ["side:lease", side.leaseAcquire + side.leaseRelease],
    ["side:leaseRenew", side.leaseRenew], ["side:human", p.human / 2 + p.bot], ["read:ref", p.refReads], ["read:api", p.apiReads],
  ].map(([kind, perS]) => ({ kind, perS: perS / w }));
  const every = (count) => Array.from({ length: Math.max(0, Math.ceil((count - j) / w)) }, (_, k) => j + k * w);
  return { roles: [], uncapped, agents: every(p.agents), pollers: every(p.pollers) };
}

/** a1.mjs `b034Phase`: the phase of a moment, from the window's start and the phase marks in seconds. */
export function phaseOf(now, t0, { killAt, secondAt, hostileAt }) {
  const past = (at) => at !== undefined && at !== null && now >= t0 + Number(at) * 1000;
  return past(hostileAt) ? "@hostile" : past(secondAt) ? "@second" : past(killAt) ? "@kill" : "@j9";
}

/** a1.mjs's phased kinds: each one measured in B0-34 carries its phase. */
export const PHASED = /^(push:|read:clone|tip:move|hot:)/; // read:clone covers read:cloneOff too

/** At most SLOTS fetches out; a fetch that finds them full waits, and the wait is counted apart, never in a sample.
 *  `take` resolves when a slot is free and answers how long it waited; `give` frees one at the response's headers. */
export class Slots {
  constructor(n = SLOTS, now = Date.now) { this.free = n; this.queue = []; this.now = now; this.waits = []; }
  async take() {
    if (this.free > 0) { this.free--; return 0; }
    const t = this.now();
    await new Promise((r) => this.queue.push(r));
    const w = this.now() - t;
    this.waits.push(w);
    return w;
  }
  give() {
    const next = this.queue.shift();
    if (next) next(); else this.free++;
  }
}

/** Every client's sample chunks merged into a1's samples file order, by start on Cloudflare's clock. */
export function mergeSamples(chunks) {
  return chunks.flat().sort((a, b) => a[0] - b[0]);
}

/** a1.mjs's `byKind`: per kind n, rate over `seconds`, nearest-rank p50 and p99, and the codes. */
export function byKind(samples, seconds) {
  const lat = {}, codes = {};
  for (const [, k, ms, code] of samples) {
    (lat[k] ??= []).push(ms);
    const c = (codes[k] ??= {});
    c[code] = (c[code] ?? 0) + 1;
  }
  return Object.fromEntries(Object.entries(lat).map(([k, xs]) => [k, { n: xs.length, perS: +(xs.length / seconds).toFixed(2),
    p50: pct(xs, 0.5), p99: pct(xs, 0.99), codes: codes[k] }]));
}

export const pct = (xs, p) => { const s = [...(xs ?? [])].sort((a, b) => a - b); return s.length ? s[Math.min(s.length - 1, Math.floor(p * s.length))] : null; };

/** Feed latency across clients: a poll that saw an agent push's commit, less that push's acknowledgment on the client
 *  that pushed it, both on Cloudflare's clock (a1 kept both in one process). */
export function feedLatency(acks, seen) {
  const at = new Map(acks);
  return seen.filter(([head]) => at.has(head)).map(([head, seenAt]) => seenAt - at.get(head));
}
