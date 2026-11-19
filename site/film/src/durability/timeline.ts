// The durability loop's timeline: the one source of its timing (src/acts/DurabilityLoop.tsx, src/durability/content.ts,
// src/durability/sync.ts and the checks import it). It shares the push loop's grid, pace and helpers (src/timeline.ts):
// 120 BPM, six bars, 12 s.
import {
  b, line, PACE, PACE_NAME, type CameraBlurRange, type ColourRange, type Copy, type Flash, type Look, type Salience, type Seam,
  type SteppedRange,
} from '../timeline';
import { readingWindows } from '../lib/pace';

export { b, BAR, BEAT, BPM, f60, f60x, FPS, H, MUSIC, PACE, PACE_NAME, W } from '../timeline';
export type { Declares } from '../timeline';

/** Six whole bars at 120 BPM (12 s), no tail: frame TOTAL would be frame 0. */
const BODY = b(7);
export const TAIL = 0;
export const TOTAL = BODY + TAIL;

// One take. Pushes land on the leader in North America and are acknowledged once Europe holds them too. Europe's site
// goes dark (blackholed); the next pushes are acknowledged once Asia-Pacific holds them, with no pause and lost still 0.
// The line lands while the product steps back. Europe comes back, catches up from the leader's log and is a follower
// again, so the last frame flows into frame 0 with all three sites healthy.
export const ACT = {
  pushes: { from: 0, dur: b(2, 2, 1) }, // pushes acknowledged on enam and weur
  dark: { from: b(2, 2, 1), dur: b(3, 2) - b(2, 2, 1) }, // weur goes dark; pushes acknowledged on enam and apac
  line: { from: b(3, 2), dur: b(5) - b(3, 2) }, // "A region goes dark, no push lost." while the product steps back
  back: { from: b(5), dur: BODY - b(5) }, // the product is relit; weur comes back and catches up
};

// A push leaves the leader on every beat, a sixteenth after it (CUE.push, then every PUSH_EVERY frames: content.ts);
// Europe holds it two beats later and Asia-Pacific a sixteenth after that, so every acknowledgement lands on the grid.
export const CUE = {
  push: b(1, 0, 1), // the first push of the loop leaves the leader
  ack: b(1, 2), // it is acknowledged, held on enam and weur
  ackMid: b(2), // a push acknowledged on enam and weur, the last bar before the fault
  dark: b(2, 2, 1), // Europe's site goes dark: every call to and from it fails (between one push's ack and the next push)
  ackApac: b(2, 3, 1), // the first push acknowledged on enam and apac, with no pause (its copy to Europe was dropped)
  lineIn: b(3, 2), // the line rises centred while the product steps back
  lineOut: b(5), // the read is over
  back: b(5, 2), // Europe's site is reachable again; it catches up from the leader's log
  caughtUp: b(6), // Europe has caught up and is a follower again
  ackWeur: b(6, 2), // the first push Europe holds again, acknowledged on enam and weur
};

export const LOOK: Look = { blur: 'crisp', texture: 'clean', filmType: 'social', oneTake: true, whiteWorld: false };
export const SEAMS: Seam[] = [
  { at: CUE.dark, kind: 'light', id: 'weur-goes-dark' },
  { at: CUE.lineIn, kind: 'light', id: 'product-steps-back' },
  { at: CUE.lineOut, kind: 'light', id: 'product-relit' },
  { at: CUE.back, kind: 'light', id: 'weur-relit' },
];
export const STEPPED: SteppedRange[] = [];
export const FLASHES: Flash[] = [];
export const CAMERA_BLUR: CameraBlurRange[] = [];
export const COLOUR: ColourRange[] = [];
export const STRUCTURAL: (keyof typeof CUE)[] = [];
export const SALIENCE: Partial<Record<keyof typeof CUE, Salience>> = {
  push: 'life', ack: 'hit', ackMid: 'hit', dark: 'hit', ackApac: 'hit', lineIn: 'copy', lineOut: 'copy', back: 'hit', caughtUp: 'hit',
  ackWeur: 'hit',
};

export const COPY: Copy[] = [
  // The one line of the script, over the product while Europe is dark. The product's own strings (the sites, their
  // states, the acknowledged pushes and the lost count) live in src/durability/content.ts.
  line('dark', 'A region goes dark, no push lost.', CUE.lineIn, CUE.lineOut),
];

export const READ = readingWindows(COPY, PACE[PACE_NAME], 30, TOTAL);

export const copy = (id: string): Copy => {
  const c = COPY.find((x) => x.id === id);
  if (!c) throw new Error(`durability timeline: no COPY entry "${id}"`);
  return c;
};
