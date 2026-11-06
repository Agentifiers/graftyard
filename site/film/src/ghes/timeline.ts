// The GHES clients loop's timeline: the one source of its timing (src/acts/GhesLoop.tsx, src/ghes/sync.ts and the
// checks import it). It shares the push loop's grid, pace and helpers (src/timeline.ts): 120 BPM, six bars, 12 s.
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

// One take. `gh api meta` against git.example answers installed_version 3.22.0; then the three clients run their checks
// against the same host and tick in; the line lands while the product steps back; the terminal scrolls to a fresh
// prompt and the checks return to pending, so the last frame flows into frame 0. The camera pushes in slowly through
// the first two acts and steps back under the line (src/acts/parts.tsx stage()).
export const ACT = {
  meta: { from: 0, dur: b(2, 3) }, // `gh api meta --hostname git.example` is typed; the reply prints 3.22.0
  clients: { from: b(2, 3), dur: b(4, 1, 2) - b(2, 3) }, // gh, git-lfs and git tick through their checks
  line: { from: b(4, 1, 2), dur: b(6) - b(4, 1, 2) }, // "Seen as GHES 3.22" lands while the product steps back
  reset: { from: b(6), dur: BODY - b(6) }, // the terminal scrolls to a fresh prompt; the checks return to pending
};

export const CUE = {
  type: b(1, 0, 1), // the first key lands just after frame 0
  enter: b(1, 3), // Enter
  reply: b(1, 3, 1), // the reply prints
  gh: b(2, 3), // gh 2.102.1: issue, pr, release (late enough for the version's dot to travel round the reply)
  lfs: b(3), // git-lfs 3.8.0: push, clone, pull, lock
  git: b(3, 1), // git 2.39.5, 2.51.0, 2.56.0 count their 29 checks
  lineIn: b(4, 1, 2), // the line rises centred
  lineOut: b(5, 3), // the read is over
  scroll: b(6), // the terminal scrolls the reply away
  clear: b(6, 1), // the checks return to pending, left to right
};

/** The frame each row's check lands on, per client card (phrased: each card a little faster than an even beat). */
export const TICKS = {
  gh: [CUE.gh, CUE.gh + 6, CUE.gh + 11],
  lfs: [CUE.lfs, CUE.lfs + 7, CUE.lfs + 13, CUE.lfs + 18],
  git: [CUE.git + 34, CUE.git + 45, CUE.git + 54], // the rows count 0 -> 29 and land here
};
/** Each git row starts counting this many frames before its check. */
export const GIT_COUNT = 34;
/** Where each card's rows return to pending (left to right) and how long that takes. */
export const CLEAR_AT = [CUE.clear, CUE.clear + 8, CUE.clear + 15];
export const CLEAR_DUR = 26;

export const LOOK: Look = { blur: 'crisp', texture: 'clean', filmType: 'social', oneTake: true, whiteWorld: false };
export const SEAMS: Seam[] = [
  { at: CUE.reply, kind: 'mask', carry: 'y', dir: 1, id: 'reply-prints' }, // Enter: the host's answer is revealed row by row under the command
  // the version's dots land on each card's status light and become it, and that card's checks start
  { at: CUE.gh - 3, kind: 'morph', carry: 'shape', id: 'version-lights-gh' },
  { at: CUE.lfs - 3, kind: 'morph', carry: 'shape', id: 'version-lights-lfs' },
  { at: CUE.git - 3, kind: 'morph', carry: 'shape', id: 'version-lights-git' },
  { at: CUE.lineIn, kind: 'light', id: 'product-steps-back' },
  { at: CUE.lineOut, kind: 'light', id: 'product-relit' },
  { at: CUE.scroll, kind: 'mask', carry: 'y', dir: -1, id: 'terminal-scrolls' },
];
/** Keys land on whole frames: the typed command and the printed reply (fx/TypeOn declares them in src/ghes/sync.ts). */
export const STEPPED: SteppedRange[] = [
  { from: CUE.type, to: CUE.enter, id: 'cmd-keys' },
  { from: CUE.reply, to: CUE.gh, id: 'reply-print' },
];
export const FLASHES: Flash[] = [];
export const CAMERA_BLUR: CameraBlurRange[] = [];
export const COLOUR: ColourRange[] = [];
export const STRUCTURAL: (keyof typeof CUE)[] = [];
export const SALIENCE: Partial<Record<keyof typeof CUE, Salience>> = {
  type: 'move', enter: 'hit', reply: 'hit', gh: 'hit', lfs: 'hit', git: 'life', lineIn: 'copy', lineOut: 'copy', scroll: 'move', clear: 'move',
};

export const COPY: Copy[] = [
  // The one line of the script, over the product once every client's checks are in. The product's own strings (the
  // command, the reply, the clients and their checks) live in src/ghes/content.ts.
  line('seen', 'Seen as GHES 3.22', CUE.lineIn, CUE.lineOut),
];

export const READ = readingWindows(COPY, PACE[PACE_NAME], 30, TOTAL);

export const copy = (id: string): Copy => {
  const c = COPY.find((x) => x.id === id);
  if (!c) throw new Error(`ghes timeline: no COPY entry "${id}"`);
  return c;
};
