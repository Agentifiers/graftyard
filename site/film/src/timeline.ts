// The single source of truth for time. Picture (src/acts), sound (src/sync.ts -> out/cues.json)
// and QA (scripts/grid-check.ts) all import this file, so a change here moves everything together.
// Think in bars and beats, never in seconds. Keep this file free of React and CSS imports:
// Node scripts load it directly.
import { holdFor, PACE, readingWindows, revealFor, wordCount, type PaceName } from './lib/pace';
import { bar, MUSIC } from './music';

export { PACE } from './lib/pace'; // grid-check checks COPY against the film's own pace numbers
/**
 * The film-time music grid (src/music.ts). null: no licensed track (effects only, or before the
 * track is chosen), and the timeline runs on its own BPM grid below. With a track, scripts/audio/
 * track.py grid writes src/music.ts, BPM becomes the track's and b() follows the music's bars.
 */
export { MUSIC } from './music';

export const FPS = 30;
/**
 * This film's own grid tempo, used while src/music.ts carries no track. new-film.sh --bpm rewrites
 * the `const BPM = ...;` line inside, so keep that line as it is. The demo is planned for a 124 BPM
 * bed (a bar is 1.94 s, its seven bars 13.5 s): until the track is in audio/music/, the grid runs at
 * 120 BPM, the nearest tempo with whole-frame beats at 60 fps. `track.py grid` then writes
 * src/music.ts and every b() below follows the track's own bars: the cue plan moves with the music.
 */
const GRID_BPM = ((): number => {
const BPM = 120;
  return BPM;
})();
/** The tempo every helper and check uses: the track's when src/music.ts carries one, else GRID_BPM. */
export const BPM = MUSIC?.bpm ?? GRID_BPM;
export const W = 1350;
export const H = 1080;
/**
 * The film's pace (src/lib/pace.ts). calm: text arrives as short fragments that take turns with
 * the picture, held about 1-1.5 s (the default for text-led films, at 90-100 BPM). crisp: the
 * denser launch-film tempo, for briefs that ask for energy (at 120 BPM).
 */
export const PACE_NAME: PaceName = 'calm';

/**
 * Frames per beat. Without a track it must be a whole number (grid-check fails otherwise): 60 fps ->
 * 90, 100, 120, 144, 150 BPM. With a track it is fractional (124 BPM = 29.03 f) and b() rounds.
 */
export const BEAT = (60 / BPM) * FPS;
export const BAR = BEAT * 4;
/**
 * Absolute frame of a grid position: bar is 1-based, beat 0-3, sub = 16ths within the beat.
 * b(3) is the downbeat of bar 3; b(2, 1, 2) is the "and" of beat 2 in bar 2. With a track (MUSIC
 * set) it is the music's grid from its first downbeat, rounded: a whole frame within 0.5 f of the
 * music. Its fractional twin is bar() in src/music.ts, for the rare place that needs it.
 */
export const b = (n: number, beat = 0, sub = 0) =>
  MUSIC ? Math.round(bar(n, beat, sub)) : Math.round(((n - 1) * 4 + beat) * BEAT + sub * (BEAT / 4));

/**
 * A choreography length written in 60 fps frames (the unit every number in the cinetic
 * references uses), converted to this film's fps (or to `fps`, for a composition at another
 * rate). Cues come from b(); durations inside a move use f60(), so the film keeps its timing if
 * --fps changes. It rounds (copy timing, Sequence lengths); f60x() is the fractional twin, for
 * durations inside a move, where rounding would bend the curve at 24, 25 or 30 fps.
 */
export const f60 = (frames: number, fps: number = FPS) => Math.round((frames * fps) / 60);
export const f60x = (frames: number, fps: number = FPS) => (frames * fps) / 60;

/** The loop: six whole bars at 120 BPM (12 s). The film is a loop, so it has no tail: frame TOTAL would be frame 0. */
const BODY = b(7);
export const TAIL = 0;

// One continuous take on a still camera, held as four acts on the bar lines. The entry is the element carried
// throughout: the typed command becomes the pill, the pill becomes three copies, two copies become "held" ticks, their
// acks converge into the terminal's check, and the third copy catches up. The last bar returns to the empty prompt.
export const ACT = {
  push: { from: 0, dur: b(2) }, // the prompt types `git push`; Enter turns it into the entry, which travels down
  hold: { from: b(2), dur: b(4) - b(2) }, // the entry splits to the three sites; two hold it; the line lands
  ack: { from: b(4), dur: b(6) - b(4) }, // the two acks converge back; the terminal acknowledges; the third catches up
  reset: { from: b(6), dur: BODY - b(6) }, // the terminal scrolls to a fresh prompt and the sites clear: frame 0 again
};

export const TOTAL = BODY + TAIL;

// Named sync points (absolute frames), all on the 16th grid.
export const CUE = {
  type: b(1, 0, 1), // the first key of `git push` lands just after frame 0: the action is under way at once
  enter: b(1, 2), // Enter: the caret swells into the entry, a leaf-green pill
  split: b(2, 2), // the pill reaches the sites' row and becomes three copies, one per site
  heldWeur: b(3), // Europe holds the entry (its copy docked, the tick)
  heldEnam: b(3, 1), // North America holds it: two sites in two regions
  lineIn: b(3, 1, 2), // "Held on two continents" rises centred while the product steps back
  lineOut: b(4, 2), // the read is over: the line lifts away
  ackGo: b(4, 2, 2), // the two acks leave their sites for the terminal
  ack: b(5), // the acks converge into the check: the terminal prints the acknowledged line
  prompt: b(5, 1), // the shell's next prompt
  heldApac: b(5, 2), // Asia-Pacific catches up: its copy, still on its way, docks and ticks
  scroll: b(6), // the terminal scrolls the push away to the fresh prompt
  clear: b(6, 1), // the three sites clear for the next entry, left to right
};

// ---- Declarations: what the picture does on purpose -------------------------------------------
// Every export from here to COPY is optional: a timeline without them keeps the 1.1 behaviour.
// With them, grid-check validates them, scripts/export-cues.ts writes them into out/cues.json
// (`look`, `declared`, `salience`), and the blur sampling, forensics and the speed checks treat a
// declared step, flash, seam or camera-blur moment as craft while still failing undeclared pops.
// Frames are absolute film frames, like CUE.

/** How fast moves render: 'crisp' (house default, flat graphics stay sharp) or 'camera' (180-degree shutter on fast moves). */
export type BlurPolicy = 'crisp' | 'camera';
/** The film's type sets its shot-length band and whether a grid-locked edit is allowed (reel, social). */
export type FilmType = 'launch' | 'update' | 'explainer' | 'reel' | 'social';
/** One surface material per film, or none. */
export type Texture = 'clean' | 'paper' | 'grid' | 'halftone' | 'dither';
export type Look = { blur: BlurPolicy; texture: Texture; filmType: FilmType; oneTake: boolean; whiteWorld: boolean; palette?: 'colour' | 'mono' };
/** How one shot hands over to the next. `hard` is a designed cut; `fadeEnd` only closes a film. */
export type SeamKind =
  | 'velocity' | 'morph' | 'hard' | 'mask' | 'whip' | 'defocus' | 'light' | 'snapZoom' | 'breath'
  | 'zoomThrough' | 'stepped' | 'colour' | 'dissolveUi' | 'fadeEnd' | 'wipe';
/** A seam at frame `at`: what carries across it (`carry`), its direction, and whether it is the film's signature move. */
export type Seam = { at: number; kind: SeamKind; carry?: 'x' | 'y' | 'scale' | 'rot' | 'shape' | 'colour'; dir?: 1 | -1; signature?: boolean; id?: string };
/** Frames [from, to) animated on a lower cadence (`fps`) or held `on` 2s / 3s: a designed step, not a dropped frame. */
export type SteppedRange = { from: number; to: number; fps?: 12 | 16 | 20 | 30; on?: 2 | 3; id: string };
/** A designed flash: 1 or 2 frames from `f`. */
export type Flash = { f: number; frames: 1 | 2; id: string };
/** Frames [from, to) rendered with camera blur inside a crisp film (a flick, a whip). */
export type CameraBlurRange = { from: number; to: number; id: string };
/** The film's one end lock: frames [from, to), 1-5 s. */
export type EndLock = { from: number; to: number };
/** A colour event (a deep field, an accent flood, a colour cut): look-audit pauses its wash and accent ceilings inside it. */
export type ColourRange = { from: number; to: number; id: string };
/**
 * What one primitive instance does on purpose, from its own props: every fx exports
 * `declares(props, film): Declares`. src/sync.ts `declares()` lists them for the instances the acts
 * use; export-cues writes them under `declared.fx`, and grid-check fails when the timeline's own
 * declarations below do not cover them.
 */
export type Declares = { stepped?: SteppedRange[]; flashes?: Flash[]; cameraBlur?: CameraBlurRange[]; seams?: Seam[]; colour?: ColourRange[] };
/**
 * What a CUE asks of the eye. read-quiet guards `copy`, `hit` and `cut` (and fast moves over the
 * text being read); `life` (creep, light, drift, a carried decay) and camera moves away from the
 * text may run during a read and count toward the picture's density. A CUE without one is `hit`.
 */
export type Salience = 'copy' | 'hit' | 'cut' | 'move' | 'life';

export const LOOK: Look = { blur: 'crisp', texture: 'clean', filmType: 'social', oneTake: true, whiteWorld: false };
/** The scene changes of the one take. */
export const SEAMS: Seam[] = [
  { at: CUE.enter, kind: 'morph', carry: 'shape', id: 'command-becomes-entry' },
  { at: CUE.split, kind: 'morph', carry: 'shape', id: 'entry-becomes-three' },
  { at: CUE.lineIn, kind: 'light', id: 'product-steps-back' },
  { at: CUE.ackGo, kind: 'light', id: 'product-relit' },
  { at: CUE.ack, kind: 'morph', carry: 'shape', id: 'acks-become-check' },
  { at: CUE.scroll, kind: 'mask', carry: 'y', dir: -1, id: 'terminal-scrolls' },
];
/** Keys land on whole frames: the typed command and the printed acknowledgement (fx/TypeOn declares them). */
export const STEPPED: SteppedRange[] = [
  { from: CUE.type, to: CUE.enter, id: 'cmd-keys' },
  { from: CUE.ack, to: CUE.prompt, id: 'ack-print' },
];
export const FLASHES: Flash[] = [];
export const CAMERA_BLUR: CameraBlurRange[] = [];
export const COLOUR: ColourRange[] = [];
/** A loop has no end card or end lock. */
export const STRUCTURAL: (keyof typeof CUE)[] = [];
/** Each CUE's salience (missing = 'hit'). */
export const SALIENCE: Partial<Record<keyof typeof CUE, Salience>> = {
  type: 'move', enter: 'hit', split: 'hit', heldWeur: 'hit', heldEnam: 'hit', lineIn: 'copy', lineOut: 'copy', ackGo: 'move',
  ack: 'hit', prompt: 'hit', heldApac: 'hit', scroll: 'move', clear: 'move',
};

export type Copy = {
  id: string;
  text: string; // exactly what is on screen (\n separates lines)
  in: number; // first frame any of it is visible
  resolved: number; // first frame it is fully readable
  out: number; // first frame it starts to leave (TOTAL if it stays)
  kind?: 'copy' | 'ui' | 'name'; // 'ui': a string the product shows (a typed command, code, a URL, an app label), not budgeted; 'name': the product's name or version on the end card, not a line of the script
  chain?: string; // the thought this fragment belongs to (fx/Chain renders a chain; grid-check checks its gaps)
  mode?: 'build' | 'replace'; // build: fragments append to one held line; replace: each takes the last one's place
  reveal?: 'focus'; // the phrase comes into focus whole (fx/FocusWord): grid-check reads its hold, not a per-word rate
  swap?: string | boolean; // a deliberate swap: this line takes the named line's place in one move (an erode, a redaction, a wheel), the one time two lines share the screen
};

const pace = PACE[PACE_NAME];

/** How the words of a line or fragment arrive, as fx/Chain draws them with the same `ink`. */
export type Ink = 'reveal' | 'pop' | 'partial';
/**
 * Frames from a fragment's first frame to every word in: the pace's reveal, or for a line cut on ('pop')
 * the frame after its last word's start, or part-inked ('partial') that start plus the 10 f ink, so a
 * caption the act cuts on is not held as long as a calm reveal.
 */
const revealSpan = (n: number, ink: Ink = 'reveal'): number => {
  const last = (Math.max(0, n - 1) * pace.stagger * FPS) / 60;
  return ink === 'pop' ? Math.floor(last) + 1 : ink === 'partial' ? Math.ceil(last + (10 * FPS) / 60) : f60(revealFor(n, pace));
};

/**
 * One line of copy revealed word by word with this film's pace (the numbers <Words> uses), so
 * `resolved` is the frame its last word is fully in. `out` defaults to the pace's hold. `ink` as the act
 * draws it: 'pop' (cut on) or 'partial' resolve as soon as the last word is in.
 */
export const line = (id: string, text: string, inF: number, out?: number, o: { ink?: Ink } = {}): Copy => {
  const n = wordCount(text);
  const resolved = inF + revealSpan(n, o.ink);
  return { id, text, in: inF, resolved, out: out ?? resolved + f60(holdFor(n, pace)) };
};

/**
 * One line built in beats, one COPY entry per fragment (`<id>.1`, `<id>.2`, ...), rendered by fx/Chain:
 * the script's sentence (2-7 words in all) arrives in two or three parts with a picture beat between
 * them (the product does what the first part set up). It is still one line: grid-check's script rule
 * reads a build chain as one sentence. Give each fragment its first frame; `resolved` follows from the pace.
 *   build    fragments append to one held line, which re-centres; the line clears as a whole, at
 *            `out` (default: the last fragment's resolve plus the hold of the full line)
 *   replace  each fragment leaves (as a whole, over pace.exitDur: 24 f when calm) and has gone
 *            6 f before the next one rises in its place, so two fragments never share the spot;
 *            the last one leaves at `out` (default: its resolve plus its hold)
 * grid-check checks the result: every fragment is read before the next arrives, the picture beat
 * between them, the words on screen at once and the copy rate. `ink`: as <Chain ink> draws them.
 */
export const chain = (id: string, mode: 'build' | 'replace', fragments: [text: string, inF: number][], o: { out?: number; ink?: Ink } = {}): Copy[] => {
  const base = fragments.map(([text, inF], i) => ({ id: `${id}.${i + 1}`, text, in: inF, resolved: inF + revealSpan(wordCount(text), o.ink), chain: id, mode }));
  const last = base[base.length - 1];
  if (!last) throw new Error(`timeline: chain "${id}" has no fragments`);
  const words = (list: { text: string }[]) => list.reduce((n, c) => n + wordCount(c.text), 0);
  if (mode === 'build') {
    const out = o.out ?? last.resolved + f60(holdFor(words(base), pace));
    return base.map((c) => ({ ...c, out }));
  }
  // replace: the outgoing fragment's exit (exitDur) is over 6 f before the next one starts to rise
  const leave = f60(pace.exitDur) + f60(6);
  return base.map((c, i) => ({ ...c, out: base[i + 1] ? base[i + 1].in - leave : (o.out ?? c.resolved + f60(holdFor(wordCount(c.text), pace))) }));
};

// Every on-screen word: the word budget (kind: 'ui' entries are counted apart), the hold check,
// and the text-flow checks (fragments, words on screen at once, copy rate, text share) read this.
export const COPY: Copy[] = [
  // The one line of the script, set over the product once two sites hold the entry. The product's own strings (the
  // command, the acknowledgement, the site names) live in src/content.ts.
  line('held', 'Held on two continents', CUE.lineIn, CUE.lineOut),
];

/**
 * When each COPY entry is being read: [from, to) frame ranges by id (src/lib/pace.ts
 * readingWindows(): from `resolved` for at most readMax, an earlier fragment of a build chain only
 * for its own hold, then the whole line again). The one rule every check uses: export-cues.ts
 * writes it into out/cues.json, and grid-check, layout-audit, measure-speed and forensics read it.
 * Nothing new should start inside a window: the read happens in the quiet.
 */
export const READ = readingWindows(COPY, pace, FPS, TOTAL);

/** One COPY entry by id; acts read their text and timing from here so the budget stays true. */
export const copy = (id: string): Copy => {
  const c = COPY.find((x) => x.id === id);
  if (!c) throw new Error(`timeline: no COPY entry "${id}"`);
  return c;
};

/** The fragments of one chain, in order of arrival. */
export const chainOf = (id: string): Copy[] => {
  const list = COPY.filter((x) => x.chain === id).sort((p, q) => p.in - q.in);
  if (!list.length) throw new Error(`timeline: no COPY chain "${id}"`);
  return list;
};
