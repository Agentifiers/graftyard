// The agents loop's events (scripts/export-cues.ts --sync src/agents/sync.ts --timeline src/agents/timeline.ts). The
// loop ships muted; these events give the checks every visible action.
import { MERGE_TYPE, PUSH_TYPE } from '../acts/AgentsLoop';
import { declares as typeDeclares } from '../fx/TypeOn';
import { TERMINAL } from './content';
import { ACT, CUE, TOTAL, type Declares } from './timeline';
import type { SoundEvent } from '../sync';

const actOf = (f: number) => Object.entries(ACT).find(([, a]) => f >= a.from && f < a.from + a.dur)?.[0] ?? 'tail';

export const declares = (): (Declares & { id?: string })[] => [
  { id: 'push', ...typeDeclares({ ...PUSH_TYPE, text: TERMINAL.push, id: 'push-keys' }) },
  { id: 'merge', ...typeDeclares({ ...MERGE_TYPE, text: TERMINAL.merge, id: 'merge-keys' }) },
];

export default function cues(): SoundEvent[] {
  const ev: SoundEvent[] = [
    { f: CUE.enter, kind: 'key', variant: 'return', weight: 0.7, id: 'enter' },
    { f: CUE.reply, kind: 'tick', weight: 0.6, id: 'reply' },
    { f: CUE.mergeEnter, kind: 'key', variant: 'return', weight: 0.7, id: 'merge-enter' },
    { f: CUE.refuse, kind: 'tick', weight: 0.7, id: 'refused' },
    { f: CUE.approve, kind: 'tock', weight: 0.6, id: 'approved' },
    { f: CUE.gate, kind: 'tock', weight: 0.6, id: 'gate' },
    { f: CUE.merged, kind: 'click', variant: 'confirm', weight: 0.8, id: 'merged' },
  ];
  for (const e of ev) {
    if (!(e.f >= 0 && e.f < TOTAL)) throw new Error(`agents sync: ${e.id} at f=${e.f} is outside the film`);
    e.id = `${actOf(e.f)}:${e.id}`;
  }
  return ev;
}
