// The durability loop's events (scripts/export-cues.ts --sync src/durability/sync.ts --timeline
// src/durability/timeline.ts). The loop ships muted; these events give the checks every visible action: each push
// acknowledged, Europe going dark and coming back, and its catch-up landing.
import { push, PUSH_EVERY } from './content';
import { ACT, CUE, TOTAL, type Declares } from './timeline';
import type { SoundEvent } from '../sync';

const actOf = (f: number) => Object.entries(ACT).find(([, a]) => f >= a.from && f < a.from + a.dur)?.[0] ?? 'tail';

export const declares = (): (Declares & { id?: string })[] => [];

export default function cues(): SoundEvent[] {
  const acks: SoundEvent[] = [];
  for (let k = -1; k * PUSH_EVERY + CUE.push < TOTAL; k++) {
    const f = push(k).ack;
    if (f >= 0 && f < TOTAL) acks.push({ f, kind: 'tick', weight: 0.4, id: `ack-${k}` });
  }
  const ev: SoundEvent[] = [
    ...acks,
    { f: CUE.dark, kind: 'hit', weight: 0.8, id: 'weur-dark' },
    { f: CUE.back, kind: 'tock', weight: 0.6, id: 'weur-back' },
    { f: CUE.caughtUp, kind: 'click', variant: 'confirm', weight: 0.7, id: 'weur-caught-up' },
  ];
  for (const e of ev) {
    if (!(e.f >= 0 && e.f < TOTAL)) throw new Error(`durability sync: ${e.id} at f=${e.f} is outside the film`);
    e.id = `${actOf(e.f)}:${e.id}`;
  }
  return ev;
}
