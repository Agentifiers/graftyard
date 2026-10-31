// Prototype of `Placement::new` (design/DESIGN.md, Shape, "Sites and placement"). It refuses two voters in one
// region or one site, a voter pair under MIN_PAIR_RTT, fewer than two Full voters, and, when the install separates
// accounts, two voters in one account. It reads only the joining members' `Rtt` rows.

import type { Region, SiteKey } from "./sites.ts";

export const MIN_PAIR_RTT_MS = 30; // [guess] in the design; B0-12 measures it

export interface Voter { site: SiteKey; name: string; region: Region; account: string; kind: "Full" | "Witness" }
export interface RttEntry { from: string; peer: string; medianMs: number }

export type Refusal =
  | { kind: "SameSite"; site: SiteKey }
  | { kind: "SameRegion"; region: Region }
  | { kind: "SameAccount"; account: string }
  | { kind: "TooFewFull"; full: number }
  | { kind: "PairTooClose"; from: string; peer: string; medianMs: number }
  | { kind: "PairUnmeasured"; from: string; peer: string };

export function placementNew(voters: Voter[], rtt: RttEntry[], accountsSeparated: boolean): Refusal[] {
  const refusals: Refusal[] = [];
  const seen = <K>(key: (v: Voter) => K, refuse: (k: K) => Refusal) => {
    const s = new Set<K>();
    for (const v of voters) { const k = key(v); if (s.has(k)) refusals.push(refuse(k)); s.add(k); }
  };
  seen((v) => v.site, (site) => ({ kind: "SameSite", site }));
  seen((v) => v.region, (region) => ({ kind: "SameRegion", region }));
  if (accountsSeparated) seen((v) => v.account, (account) => ({ kind: "SameAccount", account }));
  const full = voters.filter((v) => v.kind === "Full").length;
  if (full < 2) refusals.push({ kind: "TooFewFull", full });
  for (const a of voters) for (const b of voters) {
    if (a === b) continue;
    const row = rtt.find((r) => r.from === a.name && r.peer === b.name);
    if (!row) refusals.push({ kind: "PairUnmeasured", from: a.name, peer: b.name });
    else if (row.medianMs < MIN_PAIR_RTT_MS) refusals.push({ kind: "PairTooClose", from: a.name, peer: b.name, medianMs: row.medianMs });
  }
  return refusals;
}
