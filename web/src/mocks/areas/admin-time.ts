// Long-running mock state as a timeline, read at the current time instead of driven by
// timers: a process is its start time plus the phases it walks through. A paused clock
// banks the time it ran, so pause and resume move the same timeline.

export type Phase<S> = readonly [state: S, ms: number];

// The phase a timeline is in `elapsed` ms after it started, and how far through it.
// The last phase is terminal and never ends.
export function phaseAt<S>(phases: readonly Phase<S>[], elapsed: number) {
  let t = elapsed;
  for (let i = 0; i < phases.length; i++) {
    const [state, ms] = phases[i]!;
    if (i === phases.length - 1 || t < ms) {
      return { state, index: i, fraction: i === phases.length - 1 ? 1 : t / ms };
    }
    t -= ms;
  }
  throw new Error("empty timeline");
}

export const total = <S>(phases: readonly Phase<S>[]) =>
  phases.slice(0, -1).reduce((sum, [, ms]) => sum + ms, 0);

// A clock that can stop and restart.
export type Clock = { banked: number; runningSince: number | null };

export const startedClock = (at = Date.now()): Clock => ({ banked: 0, runningSince: at });
export const elapsed = (c: Clock, now = Date.now()) =>
  c.banked + (c.runningSince === null ? 0 : now - c.runningSince);
export function pause(c: Clock, now = Date.now()) {
  c.banked = elapsed(c, now);
  c.runningSince = null;
}
export function resume(c: Clock, now = Date.now()) {
  if (c.runningSince === null) c.runningSince = now;
}

export const iso = (ms: number) => new Date(ms).toISOString();
