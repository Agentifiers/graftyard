// The agents loop's timeline: the one source of its timing (src/acts/AgentsLoop.tsx, src/agents/sync.ts and the
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

// One take. An agent's terminal pushes its branch and the host reports it; the agent tries `gh pr merge 412` and the
// host refuses. Under the terminal, PR #412's graftyard/agent-gate reads 0 of 1 approvals from human writers; mara
// approves, the gate reads 1 of 1 and ticks, and the merge queue merges it. The line lands while the product steps
// back; the terminal scrolls to a fresh prompt and the cards return to pending, so the last frame flows into frame 0.
export const ACT = {
  push: { from: 0, dur: b(2, 1) }, // `git push origin HEAD:graftyard/7f3c/fix-session` is typed; the reply shows it landed
  merge: { from: b(2, 1), dur: b(3, 2) - b(2, 1) }, // `gh pr merge 412` is typed; the host refuses
  decide: { from: b(3, 2), dur: b(4, 2) - b(3, 2) }, // mara approves, the gate passes, the merge queue merges
  line: { from: b(4, 2), dur: b(6) - b(4, 2) }, // "Agents propose, people decide." lands while the product steps back
  reset: { from: b(6), dur: BODY - b(6) }, // the terminal scrolls to a fresh prompt; the cards return to pending
};

export const CUE = {
  type: b(1, 0, 1), // the first key of the push lands just after frame 0
  enter: b(2), // Enter
  reply: b(2, 0, 1), // the push's reply prints
  mergeType: b(2, 1, 1), // the merge is typed at the next prompt
  mergeEnter: b(2, 2, 2), // Enter
  refuse: b(2, 3), // the host's refusal prints
  approve: b(3, 2), // mara approves
  gate: b(3, 3), // graftyard/agent-gate reads 1 of 1 and ticks
  merged: b(4, 1), // the merge queue merges #412
  lineIn: b(4, 2), // the line rises centred
  lineOut: b(5, 3), // the read is over
  scroll: b(6), // the terminal scrolls the session away
  clear: b(6, 1), // the cards return to pending, the merge queue first
};

/**
 * Where each card returns to pending and how long that takes: in the reverse of the take, so no card contradicts
 * another (the merge goes first, then mara's approval, then the gate's count drops to 0).
 */
export const CLEAR_AT = { queue: CUE.clear, review: CUE.clear + 8, pr: CUE.clear + 18 };
export const CLEAR_DUR = 26;

export const LOOK: Look = { blur: 'crisp', texture: 'clean', filmType: 'social', oneTake: true, whiteWorld: false };
export const SEAMS: Seam[] = [
  { at: CUE.reply, kind: 'mask', carry: 'y', dir: 1, id: 'push-reply-prints' }, // the host's report is revealed under the command
  { at: CUE.refuse, kind: 'mask', carry: 'y', dir: 1, id: 'refusal-prints' }, // the refusal is revealed under the merge
  { at: CUE.lineIn, kind: 'light', id: 'product-steps-back' },
  { at: CUE.lineOut, kind: 'light', id: 'product-relit' },
  { at: CUE.scroll, kind: 'mask', carry: 'y', dir: -1, id: 'terminal-scrolls' },
];
/** Keys land on whole frames: the two typed commands and the printed replies (fx/TypeOn declares the keys in src/agents/sync.ts). */
export const STEPPED: SteppedRange[] = [
  { from: CUE.type, to: CUE.enter, id: 'push-keys' },
  { from: CUE.reply, to: ACT.merge.from, id: 'push-reply-print' },
  { from: CUE.mergeType, to: CUE.mergeEnter, id: 'merge-keys' },
  { from: CUE.refuse, to: CUE.approve, id: 'refusal-print' },
];
export const FLASHES: Flash[] = [];
export const CAMERA_BLUR: CameraBlurRange[] = [];
export const COLOUR: ColourRange[] = [];
export const STRUCTURAL: (keyof typeof CUE)[] = [];
export const SALIENCE: Partial<Record<keyof typeof CUE, Salience>> = {
  type: 'move', enter: 'hit', reply: 'hit', mergeType: 'move', mergeEnter: 'hit', refuse: 'hit', approve: 'hit', gate: 'hit',
  merged: 'hit', lineIn: 'copy', lineOut: 'copy', scroll: 'move', clear: 'move',
};

export const COPY: Copy[] = [
  // The one line of the script, over the product once the merge has landed. The page's heading writes it as two
  // sentences; copy-check holds a film line to one claim, so the film says it as one sentence. The product's own
  // strings (the commands, the refusal, the gate, the cards) live in src/agents/content.ts.
  line('decide', 'Agents propose, people decide.', CUE.lineIn, CUE.lineOut),
];

export const READ = readingWindows(COPY, PACE[PACE_NAME], 30, TOTAL);

export const copy = (id: string): Copy => {
  const c = COPY.find((x) => x.id === id);
  if (!c) throw new Error(`agents timeline: no COPY entry "${id}"`);
  return c;
};
