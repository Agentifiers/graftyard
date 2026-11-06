// The chaos loop's own strings and its fault plan, every figure from the record (tests/RESULTS.md, "lin24b"): the
// 24-hour run lin24b checked 2,850,523 operations under 4,176 member resets and 2,752 transfer attempts, and lost 0 of
// its 904,294 acknowledged writes (Porcupine Ok). The counter ends on that figure and the window names the run, so a
// paused frame never shows a number the run did not report. The order of the faults on screen is illustrative: the
// driver resets a member or transfers leadership every 8 to 16 s, faster here to fit 12 s.
import { CUE, RESET_DOWN, TOTAL, TRANSFER_DUR } from './timeline';

export const WINDOW = {
  title: 'Graftyard',
  run: 'lin24b, 24 h',
  acked: 'acknowledged writes',
  lost: 'lost',
};
/** The run's acknowledged writes, audited against all three members' logs, and how many were lost. */
export const ACKED = 904294;
export const LOST = 0;

export type Site = { region: string; code: string };
/** Left to right as the cards stand (the push loop's order). */
export const SITES: Site[] = [
  { region: 'North America', code: 'enam' },
  { region: 'Europe', code: 'weur' },
  { region: 'Asia-Pacific', code: 'apac' },
];

/** The leader's moves: from site `from` to site `to`, starting at `at`. The loop starts and ends led by enam. */
export const TRANSFERS = [
  { at: CUE.toWeur, from: 0, to: 1 },
  { at: CUE.toEnam, from: 1, to: 0 },
];
/** The member resets: which site, when. Each one is a follower's member at the time it hits. */
export const RESETS = [
  { at: CUE.resetApac, site: 2 },
  { at: CUE.resetEnam, site: 0 },
  { at: CUE.resetWeur, site: 1 },
];

/** The leader's site at frame f (during a transfer: the site it is leaving). */
export const leaderAt = (f: number): number => {
  const F = ((f % TOTAL) + TOTAL) % TOTAL;
  let s = 0;
  for (const t of TRANSFERS) if (F >= t.at) s = t.to;
  return s;
};
/** True while a transfer is under way. */
export const transferring = (f: number) => {
  const F = ((f % TOTAL) + TOTAL) % TOTAL;
  return TRANSFERS.some((t) => F >= t.at - 2 && F < t.at + TRANSFER_DUR);
};
/** True while site i's member is down. */
export const down = (i: number, f: number) => {
  const F = ((f % TOTAL) + TOTAL) % TOTAL;
  return RESETS.some((r) => r.site === i && F >= r.at && F < r.at + RESET_DOWN);
};

/** A write leaves the leader every WRITE_EVERY frames for the two other sites (the stream never stops). */
export const WRITE_EVERY = 10;
export const WRITE_PHASE = 3;
export const WRITE_DUR = 16;
if (TOTAL % WRITE_EVERY !== 0) throw new Error('chaos content: the write stream must repeat exactly over the loop');
for (const r of RESETS) if (leaderAt(r.at) === r.site || leaderAt(r.at + RESET_DOWN) === r.site) throw new Error(`chaos content: the reset at ${r.at} hits the leader`);
