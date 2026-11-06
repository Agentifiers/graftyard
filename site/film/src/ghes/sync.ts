// The GHES clients loop's events (scripts/export-cues.ts --sync src/ghes/sync.ts --timeline src/ghes/timeline.ts). The
// loop ships muted; these events give the checks every visible action.
import { CMD_TYPE } from '../acts/GhesLoop';
import { declares as typeDeclares } from '../fx/TypeOn';
import { TERMINAL } from './content';
import { ACT, CUE, TICKS, TOTAL, type Declares } from './timeline';
import type { SoundEvent } from '../sync';

const actOf = (f: number) => Object.entries(ACT).find(([, a]) => f >= a.from && f < a.from + a.dur)?.[0] ?? 'tail';

export const declares = (): (Declares & { id?: string })[] => [{ id: 'cmd', ...typeDeclares({ ...CMD_TYPE, text: TERMINAL.command, id: 'cmd-keys' }) }];

export default function cues(): SoundEvent[] {
  const ev: SoundEvent[] = [
    { f: CUE.enter, kind: 'key', variant: 'return', weight: 0.7, id: 'enter' },
    { f: CUE.reply, kind: 'tick', weight: 0.6, id: 'reply' },
    ...TICKS.gh.map((f, i) => ({ f, kind: 'tock' as const, weight: 0.6, id: `gh-${i}` })),
    ...TICKS.lfs.map((f, i) => ({ f, kind: 'tock' as const, weight: 0.6, id: `lfs-${i}` })),
    ...TICKS.git.map((f, i) => ({ f, kind: 'tock' as const, weight: 0.6, id: `git-${i}` })),
  ];
  for (const e of ev) {
    if (!(e.f >= 0 && e.f < TOTAL)) throw new Error(`ghes sync: ${e.id} at f=${e.f} is outside the film`);
    e.id = `${actOf(e.f)}:${e.id}`;
  }
  return ev;
}
