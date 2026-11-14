// L0-12's member clock line (`build/BUILD-PLAN.md`, L0-12: "measured member clock offsets stay under 500 ms"), from
// samples a1.mjs takes every 15 s: each member's `/status` `now` against the midpoint of its round trip from this orb.
// A sample is {round, site, offset, rtt}: the member's clock minus the midpoint, and the round trip, in ms.
// The line is the members' offsets from each other, not from this orb (whose clock ran about 4 s behind
// Cloudflare's in round 6). Each sample's member clock lies within half its own round trip of the midpoint. A sample
// whose round trip is more than CLOCK_RTT_OVER (200 ms) over the fastest one seen for that site is rejected, not
// widened: under load a round trip of seconds measures the queue, not the clock (rows-m19 widened every sample and
// read 3.2 s). The cap is over each site's floor because apac's idle round trip from this orb through the tools edge
// is 240 to 590 ms, so a flat 200 ms cap would reject every apac sample. Per round with two or more members kept:
// `upperMs` is the farthest two members' intervals end to end (the true spread is at most this; the gate passes on
// it), `lowerMs` the gap between them (the true spread is at least this; the gate fails on it).
const CLOCK_RTT_OVER = 200;
export const clockAccepted = (xs) => {
  const best = new Map();
  for (const c of xs) best.set(c.site, Math.min(best.get(c.site) ?? Infinity, c.rtt));
  return xs.filter((c) => c.rtt <= best.get(c.site) + CLOCK_RTT_OVER);
};
export const clockSpread = (xs) => {
  const kept = clockAccepted(xs);
  const by = new Map();
  for (const c of kept) by.set(c.round, [...(by.get(c.round) ?? []), c]);
  const rounds = [...by.values()].filter((r) => new Set(r.map((c) => c.site)).size > 1);
  if (!rounds.length) return { upperMs: null, lowerMs: null, rounds: 0, kept: kept.length, rejected: xs.length - kept.length };
  const hi = (c) => c.offset + c.rtt / 2, lo = (c) => c.offset - c.rtt / 2;
  // Over pairs of different members: one member's own interval says nothing about its spread from the others.
  const pairs = (r, f) => Math.max(...r.flatMap((a) => r.filter((b) => b.site !== a.site).map((b) => f(a, b))));
  const upper = rounds.map((r) => pairs(r, (a, b) => hi(a) - lo(b)));
  const lower = rounds.map((r) => Math.max(0, pairs(r, (a, b) => lo(a) - hi(b))));
  return { upperMs: Math.round(Math.max(...upper)), lowerMs: Math.round(Math.max(...lower)), rounds: rounds.length,
    kept: kept.length, rejected: xs.length - kept.length };
};
