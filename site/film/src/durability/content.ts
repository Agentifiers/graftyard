// The durability loop's own strings and its push plan. What it shows, and where each part comes from:
//   - A push is acknowledged only after its entry is on two sites in two regions (design/DESIGN.md, Summary). Every
//     push here is acknowledged on the leader's site plus the first other site that holds it: Europe while it is up
//     (the nearer follower), Asia-Pacific while Europe is dark. Each acknowledged row names its two sites.
//   - A whole site lost loses no acknowledged write: the other two sites hold every acknowledged entry and form a
//     quorum (design/DESIGN.md, "RPO and RTO per durability class": 0 for loss of any one site). B0-03 measured it with
//     one site's member class failing or blackholed: 26 runs plus 6 reruns, about 100 ops/s, 0 failed operations
//     (tests/RESULTS.md and tests/m0/results/m0-wp2.md, "B0-03"). The window's "lost" row is that 0; it is the only
//     figure on screen. B0-03 failed the leader's site, so the film does not name it: here a follower's site goes
//     dark, the simpler case, and the leader stays on enam with no election to show.
//   - A follower that comes back catches up from the leader's log, and only then counts toward a quorum again.
// Time is compressed, as in the chaos loop: the push rate, the 6 s outage and the catch-up's length are illustrative,
// not measured (B0-03 ran about 100 ops/s; the loop shows one push every two thirds of a second so each can be seen).
import { CUE, TOTAL } from './timeline';

export const WINDOW = {
  title: 'Graftyard',
  acked: 'acknowledged',
  heldOn: 'held on',
  lost: 'lost',
};
/** The acknowledged writes lost: 0, as B0-03's runs and the design's per-class RPO give it. */
export const LOST = 0;

export type Site = { region: string; code: string };
/** Left to right as the cards stand (the push and chaos loops' order). */
export const SITES: Site[] = [
  { region: 'North America', code: 'enam' },
  { region: 'Europe', code: 'weur' },
  { region: 'Asia-Pacific', code: 'apac' },
];
/** The leader's site, all loop long, and the follower whose site goes dark. */
export const LEADER = 0;
export const DARK = 1;
export const STATUS = { leader: 'leader', follower: 'follower', dark: 'offline', catchingUp: 'catching up' };

/** A push leaves the leader every PUSH_EVERY frames (one a beat), from CUE.push; the stream repeats exactly over the loop. */
export const PUSH_EVERY = 15;
/**
 * Frames from a push leaving the leader to each follower's site holding it: Europe, the nearer (B0-01's rounds), lands
 * on the beat after next, Asia-Pacific a sixteenth later. Each flight is longer than the gap between pushes, so the
 * stream never stops moving (as the chaos loop's writes).
 */
export const LAND: Record<number, number> = { 1: 26, 2: 30 };
/** The catch-up once Europe is back: entries from the leader's log, a quicker run of `count` dots `every` frames apart. */
export const CATCH = { count: 5, every: 3, fly: 14, lead: 4 };

const wrap = (f: number) => ((f % TOTAL) + TOTAL) % TOTAL;
/** True while Europe's site is reachable and caught up, so the leader's new entries reach it. */
export const isCurrent = (f: number) => wrap(f) < CUE.dark || wrap(f) >= CUE.caughtUp;

/**
 * The push that leaves the leader at absolute frame t0. Its entry goes to both followers. Europe holds it only if Europe
 * is current when it is sent and still up when it lands; a copy in flight to Europe as it goes dark is dropped. The
 * push is acknowledged when its first other site holds it: Europe, else Asia-Pacific.
 */
export const pushAt = (t0: number) => {
  const sentWeur = isCurrent(t0);
  const heldWeur = sentWeur && (wrap(t0) >= CUE.dark || wrap(t0) + LAND[1] < CUE.dark);
  const quorum = heldWeur ? 1 : 2;
  return { t0, sentWeur, heldWeur, quorum, ack: t0 + LAND[quorum] };
};
/** The k-th push (any integer k; absolute frames, so the stream runs on across the loop's seam). */
export const push = (k: number) => pushAt(CUE.push + k * PUSH_EVERY);
/** The catch-up's entries, in loop frames: when each leaves the leader and when Europe holds it. */
export const catchUp = () => Array.from({ length: CATCH.count }, (_, n) => {
  const from = CUE.back + CATCH.lead + n * CATCH.every;
  return { from, land: from + CATCH.fly };
});

if (TOTAL % PUSH_EVERY !== 0) throw new Error('durability content: the push stream must repeat exactly over the loop');
// the named acknowledgements in the timeline are pushes of this stream
const ackOf = (f: number) => [-1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23].map(push).find((p) => p.ack === f);
for (const [name, quorum] of [['ack', 1], ['ackMid', 1], ['ackApac', 2], ['ackWeur', 1]] as const) {
  const p = ackOf(CUE[name]);
  if (!p || p.quorum !== quorum) throw new Error(`durability content: CUE.${name} (f${CUE[name]}) is not a push acknowledged with ${SITES[quorum].code}`);
}
if (catchUp()[CATCH.count - 1].land > CUE.caughtUp) throw new Error('durability content: the catch-up must land before Europe counts again');
