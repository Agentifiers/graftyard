// Does a ticket from before a leader reset name a different command after the leader resumes its term?
// 1. Leader A admits put(k, lost) while its followers' acks are slow, so the command is still pending.
// 2. A is reset, the faults clear, and A resumes `led_term` on the next command.
// 3. New commands are admitted on A until one gets the old ticket's seq.
// 4. `outcome(old ticket)` must not answer with another command's result. Whether `lost` committed or not, an answer
//    for the old ticket from the reset instance is wrong: that instance never admitted it.
//
//   node run/raft-ticket.ts

import { createLedFromA, raft } from "./raft.ts";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const group = `ticket-${Date.now()}`;
await raft("/faults", { plan: { rules: [] } });
await createLedFromA(group);
// Followers' acks to A arrive late, so the command below is admitted and still pending when A resets. A resumes
// `led_term` after the reset because its followers still promise it.
await raft("/faults", { plan: { rules: [{ target: "call", mode: "slow-reply", rate: 1, match: "a>", hangMs: 900 }] } });
await sleep(1_100);
await raft("/op", { group, op: { kind: "put", key: "warm", value: "x" } }).catch(() => {}); // A rereads the plan
const old = await raft<any>("/commitRaw", { group, site: "a", op: { kind: "put", key: "k", value: "lost" } });
console.log("admitted before reset:", JSON.stringify(old));
await raft("/reset", { group, sites: ["a"] });
await raft("/faults", { plan: { rules: [] } });
await sleep(1_100);
const later: unknown[] = [];
for (let i = 0; i < 6; i++) later.push(await raft<any>("/commitRaw", { group, site: "a", op: { kind: "put", key: `other${i}`, value: "v" } }));
console.log("admitted after reset:", JSON.stringify(later));
const out = "admitted" in old ? await raft<any>("/outcomeRaw", { group, site: "a", ticket: old.admitted }) : null;
const st = await raft<any[]>("/status", { group });
const read = await raft<any>("/op", { group, op: { kind: "get", key: "k" } });
console.log(JSON.stringify({ outcomeOfOldTicket: out, kNow: read.out, leaderTerm: st.find((x) => x.role === "leader")?.term,
  verdict: out && "out" in out ? "BUG: a reset leader answered a ticket it never issued" : "ok" }));
