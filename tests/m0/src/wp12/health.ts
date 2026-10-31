// M0-WP4 `SiteHealth` (design/DESIGN.md, Shape, `SiteHealth`; "Commit and read paths", journey 5, "Automatic drain").
// One object per (observer site, target site), named "<observer>><target>" and placed on the observer's site. Members
// and edges there count their member calls to the target site and the calls that failed or missed CALL_DEADLINE. The
// site-day runner reads both other sites' objects and drains a target only when both show the burn. Counts are kept per
// second in memory: a reset loses them, which a Milestone 0 run tolerates.

import { DurableObject } from "cloudflare:workers";
import { HINT, type SiteKey } from "./types.ts";

export class SiteHealth extends DurableObject {
  private buckets = new Map<number, [number, number]>();
  async add(calls: number, missed: number): Promise<void> {
    const s = Math.floor(Date.now() / 1000);
    const b = this.buckets.get(s) ?? [0, 0];
    b[0] += calls; b[1] += missed;
    this.buckets.set(s, b);
    for (const k of this.buckets.keys()) if (k < s - 900) this.buckets.delete(k);
  }
  async read(windowMs: number): Promise<{ calls: number; missed: number }> {
    const since = Math.floor((Date.now() - windowMs) / 1000);
    let calls = 0, missed = 0;
    for (const [s, [c, m]] of this.buckets) if (s >= since) { calls += c; missed += m; }
    return { calls, missed };
  }
}

/** Per-isolate counts, flushed to the observer site's objects about once a second. */
const pending = new Map<string, [number, number]>();
let flushing: ReturnType<typeof setTimeout> | null = null;

export function healthObject(ns: DurableObjectNamespace<SiteHealth>, observer: SiteKey, target: SiteKey): DurableObjectStub<SiteHealth> {
  return ns.get(ns.idFromName(`${observer}>${target}`), { locationHint: HINT[observer] });
}

/** Counts one call from `observer` to `target`. `ns` is the observer site's `SiteHealth` namespace. */
export function recordCall(ns: DurableObjectNamespace<SiteHealth> | undefined, observer: SiteKey, target: SiteKey, ok: boolean): void {
  if (!ns || observer === target) return;
  const key = `${observer}>${target}`;
  const b = pending.get(key) ?? [0, 0];
  b[0]++; if (!ok) b[1]++;
  pending.set(key, b);
  flushing ??= setTimeout(() => {
    flushing = null;
    for (const [k, [c, m]] of pending) {
      const [o, t] = k.split(">") as [SiteKey, SiteKey];
      void healthObject(ns, o, t).add(c, m).catch(() => {});
    }
    pending.clear();
  }, 1_000);
}
