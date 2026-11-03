// The member-side ack check (`tests/m1/results/m1-wp2.md`, "Member-side ack evidence"). Every acknowledged write in a
// lin history carries the answering member's evidence (`workers/site/src/host.rs`, `ack_json`), all on that member's
// clock. Without trusting any client clock, this checks that each acknowledgment left only after its entry
// committed on a quorum of durable appends:
//
//   entry       the write's entry is at or below the commit the proof is for (`index <= commitIndex`)
//   sent        the reply was built at or after the commit (`sentAt >= commitAt`)
//   quorum      at least `quorum` distinct sites are counted, the answering site among them
//   durable     each counted site's durable index reaches the commit (`durable >= commitIndex`)
//   order       each ack was learned no earlier than the entry was built and no later than the commit
//   selfSynced  the answering host's own storage confirm, its record and not the crate's, covered the commit at or
//               before it (`selfSyncedAt <= commitAt`)
//   evidence    an acknowledged write with no evidence is a violation
//
// It also reports, per (leader, follower) pair, how long after the entry was built the follower's durable ack
// arrived: a single-clock interval that no ack can beat by less than the round trip between the two sites.
//
//   node tests/m1/lin/ackcheck.ts <history.jsonl> [--quorum 2]

import { createReadStream } from "node:fs";
import { createInterface } from "node:readline";

export type Ack = {
  site: number; index: number; commitIndex: number; entryAt: number; commitAt: number; sentAt: number;
  selfSyncedAt: number | null; acks: { site: number; durable: number; at: number }[];
};

/** The rules an ack breaks, empty when it proves its acknowledgment. */
export function violations(a: Ack, quorum: number): string[] {
  const bad: string[] = [];
  if (a.index > a.commitIndex) bad.push("entry");
  if (a.sentAt < a.commitAt) bad.push("sent");
  const sites = new Set(a.acks.map((x) => x.site));
  if (sites.size < quorum || sites.size !== a.acks.length || !sites.has(a.site)) bad.push("quorum");
  if (a.acks.some((x) => x.durable < a.commitIndex)) bad.push("durable");
  if (a.acks.some((x) => x.at > a.commitAt || x.at < a.entryAt)) bad.push("order");
  if (a.selfSyncedAt === null || a.selfSyncedAt === undefined || a.selfSyncedAt > a.commitAt) bad.push("selfSynced");
  return bad;
}

const pct = (xs: number[], p: number) => (xs.length ? xs[Math.min(xs.length - 1, Math.floor(xs.length * p))] : null);

export async function check(path: string, quorum = 2) {
  let acked = 0, withEvidence = 0, violated = 0;
  const byRule: Record<string, number> = {};
  const examples: object[] = [];
  const followerMs = new Map<string, number[]>();
  const sendMs: number[] = [];
  for await (const l of createInterface({ input: createReadStream(path) })) {
    if (!l) continue;
    const r = JSON.parse(l);
    if (r.op?.kind === "get" || !r.ok || r.out?.ok !== true) continue;
    acked++;
    const a = r.ack as Ack | undefined;
    const bad = a ? violations(a, quorum) : ["evidence"];
    if (a) {
      withEvidence++;
      sendMs.push(a.sentAt - a.commitAt);
      for (const x of a.acks) {
        if (x.site === a.site) continue;
        const k = `${a.site}>${x.site}`;
        (followerMs.get(k) ?? followerMs.set(k, []).get(k)!).push(x.at - a.entryAt);
      }
    }
    if (bad.length) violated++;
    for (const b of bad) byRule[b] = (byRule[b] ?? 0) + 1;
    if (bad.length && examples.length < 20) examples.push({ bad, rid: r.rid, client: r.client, colo: r.colo, ack: a ?? null });
  }
  const pairs = Object.fromEntries([...followerMs].sort().map(([k, v]) => {
    v.sort((x, y) => x - y);
    return [k, { n: v.length, min: v[0], p01: pct(v, 0.01), median: pct(v, 0.5) }];
  }));
  sendMs.sort((x, y) => x - y);
  return {
    result: violated ? "Violations" : "Ok",
    acked, withEvidence, byRule, violated, examples,
    sentAfterCommitMs: { min: sendMs[0] ?? null, median: pct(sendMs, 0.5), max: sendMs.at(-1) ?? null },
    followerAckAfterEntryMs: pairs,
  };
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const path = process.argv[2];
  if (!path) throw new Error("usage: node tests/m1/lin/ackcheck.ts <history.jsonl> [--quorum 2]");
  const q = process.argv.indexOf("--quorum");
  console.log(JSON.stringify(await check(path, q > 0 ? Number(process.argv[q + 1]) : 2), null, 1));
}
