// The sound events of the film, computed from the picture code. scripts/export-cues.ts calls the
// default export and writes out/cues.json; scripts/audio/score.py voices each event with the
// film's sound kit (audio/kit.json: the recorded take scripts/audio/sfx.mjs placed for its kind, the earshot take as the fallback) on its exact frame, levelled over
// the film's licensed music track (audio/score.json "music"; schema, roles and levels: references/sound.md).
//
// The rules this file follows:
// - Every sound has a visible cause, but not every picture event gets a sound: sound discrete
//   state changes (a press, an insert, a lock, a landing, the logo assembling), leave continuous
//   motion silent, and let the music carry most cuts. A class of action, once sounded, is
//   sounded every time.
// - Impacts go on the contact frame, pops on the 50% frame, settles on the 97% frame, and a
//   whoosh's apex on the velocity peak of the move it belongs to (a launch peaks on its cut).
// - Import the constants the acts animate with (they export them for this file) and compute the
//   frame; a frame typed by hand drifts the first time an act is retimed.
// - Text gets one soft `text` sound per fragment, on the frame its last word visibly comes to rest
//   (chainResolves(), at the size and role the act sets it at), never one per word;
//   what was just said is read in near-silence. The lockup is a stop-down and ONE clean element:
//   score.py's stack guard warns when anything else lands within 3 f of it or leads into it.
import { ACK_TYPE, CMD_TYPE } from './acts/PushLoop';
import { TERMINAL } from './content';
import { declares as typeDeclares } from './fx/TypeOn';
import { ACT, CUE, type Declares, type Salience, TOTAL } from './timeline';

export type Kind =
  | 'tick' | 'tock' | 'hit' | 'land' | 'pop' | 'whoosh' | 'riser'
  | 'drop' | 'key' | 'click' | 'snap' | 'swell' | 'bell' | 'suck' | 'text';

export type SoundEvent = {
  f: number; // absolute frame the sound belongs on (fractional is fine)
  kind: Kind;
  weight?: number; // 0..2, 1 = normal; scales level (and size for whoosh and land)
  pan?: number; // -1 left .. +1 right from screen x; score.py keeps effects within +-0.15, a whoosh travels the full amount
  pitch?: number | string; // MIDI number or "Eb6"; omit and score.py picks a tone in key
  apexFrac?: number; // whoosh / suck: where in its length the loudest point sits (lands on f)
  dur?: number; // frames: whoosh, riser, swell and suck length
  variant?: string; // key: 'word' | 'space' | 'return' | 'back'; tick: 'crisp'; click: 'confirm'; land: 'light'; bell: 'motif'; pop: 'down'
  action?: string; // what the picture does (scripts/audio/actions.py: 'card-land', 'click', 'toggle' ...): kit.py cast voices each action with its own sound
  id?: string; // a name for QA reports
  structural?: true; // an act or section change, the reveal or the ending: it must land on a music event (av-audit music_sync, grid-check)
  force?: true; // sound it even when a track accent already carries the moment
  offset_ms?: number; // -500..500: overrides the kit role's offset for this event only (rare)
  salience?: Salience; // what the moment asks of the eye: copy | hit | cut | move | life (see timeline.ts Salience)
};

/** The act a frame belongs to, so QA reports read "hold:held-weur" rather than a bare frame. */
const actOf = (f: number) => Object.entries(ACT).find(([, a]) => f >= a.from && f < a.from + a.dur)?.[0] ?? 'tail';

/** What the primitives do on purpose: the typed command and the printed acknowledgement land on whole frames. */
export const declares = (): (Declares & { id?: string })[] => [
  { id: 'cmd', ...typeDeclares({ ...CMD_TYPE, text: TERMINAL.command, id: 'cmd-keys' }) },
  { id: 'ack', ...typeDeclares({ ...ACK_TYPE, text: TERMINAL.ack, id: 'ack-print' }) },
];

// The loop ships muted (render.sh --loop --no-audio); these events still give the checks every visible action.
export default function cues(): SoundEvent[] {
  const ev: SoundEvent[] = [
    { f: CUE.enter, kind: 'pop', weight: 0.7, id: 'entry' },
    { f: CUE.split, kind: 'tick', weight: 0.6, id: 'split' },
    { f: CUE.heldWeur, kind: 'tock', weight: 0.7, id: 'held-weur' },
    { f: CUE.heldEnam, kind: 'tock', weight: 0.7, id: 'held-enam' },
    { f: CUE.ack, kind: 'click', variant: 'confirm', weight: 0.9, id: 'ack' },
    { f: CUE.heldApac, kind: 'tock', weight: 0.5, id: 'held-apac' },
  ];
  for (const e of ev) {
    if (!(e.f >= 0 && e.f < TOTAL)) throw new Error(`sync: ${e.id} at f=${e.f} is outside the film (0..${TOTAL - 1})`);
    e.id = `${actOf(e.f)}:${e.id}`;
  }
  return ev;
}
