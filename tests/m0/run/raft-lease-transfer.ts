// Lease after an abandoned transfer (B0-11 finding, 2026-10-28). Leader A transfers to B; A's TimeoutNow reply
// arrives after its 1 s deadline, so A abandons while B wins the next term with C's transfer vote. A transfer-round
// ack from C that was in flight across the abandonment must not give A a lease: the design serves `fresh` again only
// after one full round sent after the transfer ended (design/DESIGN.md, `transfer`). The run then writes v2 on B and
// polls A's own read; any `v1` from A after v2 committed is a stale read.
//
//   node run/raft-lease-transfer.ts [--runs 5]

import { parseArgs } from "node:util";
import { createLedFromA, raft } from "./raft.ts";

const { values: opt } = parseArgs({ options: { runs: { type: "string", default: "5" } } });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let stale = 0;
for (let run = 0; run < Number(opt.runs); run++) {
  const group = `lease-xfer-${Date.now()}-${run}`;
  await raft("/faults", { plan: { rules: [] } });
  await createLedFromA(group);
  await raft("/op", { group, op: { kind: "put", key: "k", value: "v1" } });
  await sleep(1_500); // a lease round with promises before the transfer
  await raft("/faults", { plan: { rules: [
    { target: "call", mode: "slow-reply", rate: 1, match: "a>b:timeoutNow", hangMs: 2_000 }, // A gives up on the transfer
    { target: "call", mode: "error", rate: 1, match: "b>a:" },                                // B's term never reaches A
    { target: "call", mode: "slow-reply", rate: 1, match: "a>c:append", hangMs: 1_500 },      // C's acks land after A abandons
  ] } });
  // A member's isolate rereads the plan only when it rolls a call, at most once a second: make A call out, then wait.
  await sleep(1_100);
  await raft("/op", { group, op: { kind: "put", key: "warm", value: "2" } });
  await sleep(300);
  const t0 = Date.now();
  const xfer = await raft<{ result: string }>("/transfer", { group, to: "b" }).catch((e) => ({ result: String(e) }));
  const w = await raft<Record<string, unknown>>("/commitAt", { group, site: "b", op: { kind: "put", key: "k", value: "v2" } }).catch((e) => ({ error: String(e) }));
  const committedAt = Date.now();
  const reads: string[] = [];
  while (Date.now() - t0 < 5_000) {
    const r = await raft<Record<string, any>>("/readAt", { group, site: "a", key: "k" }).catch((e) => ({ error: String(e) }));
    const v = r.out?.value;
    reads.push(`${Date.now() - t0}ms:${v !== undefined ? v : JSON.stringify(r).slice(0, 60)}`);
    if (v === "v1" && "out" in w) stale++;
    await sleep(50);
  }
  console.log(JSON.stringify({ run, transfer: xfer.result, writeOnB: w, committedMs: committedAt - t0, readsServedByA: reads }));
}
await raft("/faults", { plan: { rules: [] } });
console.log(JSON.stringify({ staleReadsFromA: stale }));
