// The chaos loop's timeline: the one source of its timing (src/acts/ChaosLoop.tsx, src/chaos/sync.ts and the checks
// import it). It shares the push loop's grid, pace and helpers (src/timeline.ts): 120 BPM, six bars, 12 s.
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

// One take. Writes leave the leader for the two other sites all the time while the chaos driver works: a member is
// reset (its card flashes, goes down and comes back), leadership moves to another site and back, and the run's counter
// climbs to lin24b's 904,294 acknowledged writes with 0 lost. The line lands while the product steps back; the
// window scrolls to a fresh counter, so the last frame flows into frame 0.
export const ACT = {
  faults: { from: 0, dur: b(2, 2) }, // writes flow from enam; Asia-Pacific's member is reset and comes back
  transfer: { from: b(2, 2), dur: b(3, 3) - b(2, 2) }, // leadership moves to Europe; North America's member is reset
  line: { from: b(3, 3), dur: b(5, 1) - b(3, 3) }, // "0 acknowledged writes lost" lands while the writes go on
  back: { from: b(5, 1), dur: BODY - b(5, 1) }, // leadership returns to enam; Europe's member is reset; a fresh counter
};

export const CUE = {
  count: 8, // the counter starts
  resetApac: b(1, 3), // Asia-Pacific's member is reset
  toWeur: b(2, 2), // leadership transfers from North America to Europe
  resetEnam: b(3), // North America's member is reset
  counted: b(3, 2), // the counter lands on 904,294
  lineIn: b(3, 3), // the line rises centred while the product steps back
  lineOut: b(5), // the read is over
  toEnam: b(5, 1), // leadership transfers back to North America
  scroll: b(6), // the window scrolls to a fresh counter
  resetWeur: b(6, 1), // Europe's member is reset, once the scroll is under way
};

/** Each reset: the frame it hits, the frames the member is down, then it is back. */
export const RESET_DOWN = 32;
/** A transfer's travel, frames. */
export const TRANSFER_DUR = 30;

export const LOOK: Look = { blur: 'crisp', texture: 'clean', filmType: 'social', oneTake: true, whiteWorld: false };
export const SEAMS: Seam[] = [
  { at: CUE.toWeur, kind: 'morph', carry: 'x', dir: 1, id: 'leader-to-weur' },
  { at: CUE.lineIn, kind: 'light', id: 'product-steps-back' },
  { at: CUE.lineOut, kind: 'light', id: 'product-relit' },
  { at: CUE.toEnam, kind: 'morph', carry: 'x', dir: -1, id: 'leader-to-enam' },
  { at: CUE.scroll, kind: 'mask', carry: 'y', dir: -1, id: 'counter-scrolls' },
];
export const STEPPED: SteppedRange[] = [];
export const FLASHES: Flash[] = [];
export const CAMERA_BLUR: CameraBlurRange[] = [];
export const COLOUR: ColourRange[] = [];
export const STRUCTURAL: (keyof typeof CUE)[] = [];
export const SALIENCE: Partial<Record<keyof typeof CUE, Salience>> = {
  count: 'life', resetApac: 'hit', toWeur: 'move', resetEnam: 'hit', counted: 'hit', lineIn: 'copy', lineOut: 'copy', toEnam: 'move',
  scroll: 'move', resetWeur: 'hit',
};

export const COPY: Copy[] = [
  // The one line of the script. The product's own strings (the sites, the run's name, its counters) live in
  // src/chaos/content.ts.
  line('lost', '0 acknowledged writes lost', CUE.lineIn, CUE.lineOut),
];

export const READ = readingWindows(COPY, PACE[PACE_NAME], 30, TOTAL);

export const copy = (id: string): Copy => {
  const c = COPY.find((x) => x.id === id);
  if (!c) throw new Error(`chaos timeline: no COPY entry "${id}"`);
  return c;
};
