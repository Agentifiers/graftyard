// The chaos loop's events (scripts/export-cues.ts --sync src/chaos/sync.ts --timeline src/chaos/timeline.ts). The loop
// ships muted; these events give the checks every visible action.
import { RESETS, TRANSFERS } from './content';
import { ACT, CUE, TOTAL, type Declares } from './timeline';
import type { SoundEvent } from '../sync';

const actOf = (f: number) => Object.entries(ACT).find(([, a]) => f >= a.from && f < a.from + a.dur)?.[0] ?? 'tail';

export const declares = (): (Declares & { id?: string })[] => [];

export default function cues(): SoundEvent[] {
  const ev: SoundEvent[] = [
    ...RESETS.map((r, i) => ({ f: r.at, kind: 'hit' as const, weight: 0.6, id: `reset-${i}` })),
    ...TRANSFERS.map((t, i) => ({ f: t.at, kind: 'whoosh' as const, dur: 30, weight: 0.5, id: `transfer-${i}` })),
    { f: CUE.counted, kind: 'click', variant: 'confirm', weight: 0.8, id: 'counted' },
  ];
  for (const e of ev) {
    if (!(e.f >= 0 && e.f < TOTAL)) throw new Error(`chaos sync: ${e.id} at f=${e.f} is outside the film`);
    e.id = `${actOf(e.f)}:${e.id}`;
  }
  return ev;
}
