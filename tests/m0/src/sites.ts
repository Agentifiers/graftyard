// Shared by the site scripts, the edge, and the Node runner, so it imports nothing from the Workers runtime.

export type SiteKey = "a" | "b" | "c";
export type Region = "enam" | "wnam" | "weur" | "eeur" | "apac" | "oc";

/** The `Tri` placement: A enam, B weur, C apac (design/DESIGN.md, Shape, `PlacementName::Tri`). */
export const SITES: Record<SiteKey, { script: string; hint: Region }> = {
  a: { script: "graft-site-a", hint: "enam" },
  b: { script: "graft-site-b", hint: "weur" },
  c: { script: "graft-site-c", hint: "apac" },
};
export const SITE_KEYS: SiteKey[] = ["a", "b", "c"];

/** Which Durable Object class holds a member. Each is its own namespace, so its usage reads apart in analytics:
 *  `member` for B0-12, `idle` and `control` for B0-15. */
export type MemberClass = "member" | "idle" | "control";

/** One member as the harness names it. `name` follows Shape's `do_name`: "{chain}#{generation}#{kind}{slot}". */
export interface MemberRef { site: SiteKey; name: string; hint: Region; cls?: MemberClass }

/** What a member reports about itself without counting as a served request. */
export interface MemberReport { name: string; served: number; rttChecks: number; lastMeasuredAt: number; joinedAt: number }

export interface MemberInfo extends MemberRef { colo: string; createdAt: number; fresh: boolean }

/** Shape's `Rtt` row, plus the raw samples so a cold first call stays visible. */
export interface RttRow { from: string; peer: string; peerSite: SiteKey; medianMs: number; samples: number[]; at: number }

export function median(xs: number[]): number {
  const s = [...xs].sort((x, y) => x - y);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
}
