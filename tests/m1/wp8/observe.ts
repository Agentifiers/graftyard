// BM-1's observer: while a conformance or client run goes, records every 2 s each watched group's members
// (`/status`: role, term, leader) through the M1-WP7 test edge's raw `/member` call, which reaches a member even on a
// faulted site because `/status` is an observation route. Each sample is one JSON line, so a run's leader moves are
// the changes of (term, leader) and the time between the fault's start and the first sample with a leader off the
// faulted site. Each member's `now` is Cloudflare's clock, the one the fault plan's `from` is on (the orb's clock ran
// 11.06 s behind it on 2026-11-02), so moves are timed on member clocks. `--stats` at the end reads each member's `/stats` (`commitMs`: the leader's commit latency samples,
// `failedCalls`, `alarms` with the `STEP_DOWN` trips, `peers`: per peer site its calls' counts by outcome and failure
// cause, last errors, and last AppendEntries) and resets them, and prints each member's peer calls.
//
//   node tests/m1/wp8/observe.ts --out FILE [--chains repo:196608,...] [--every 2000]
//   node tests/m1/wp8/observe.ts --stats FILE [--chains ...] [--reset]
//
// The default chains are the two conformance slots and the system groups every request of theirs reads: the owner's
// `Names` shards (account 174, login 987, the slots' `RepoOwner` rows 938 and 806), `Authz(7000001)`, and the writer's
// and read-only token's `Tokens` shards (1563, 1153).
import { appendFileSync, readFileSync } from "node:fs";
import { parseArgs } from "node:util";

const { values: opt } = parseArgs({ options: {
  out: { type: "string" }, stats: { type: "string" },
  chains: { type: "string", default: "repo:196608,repo:196610,names:174,names:987,names:938,names:806,authz:7000001,tokens:1563,tokens:1153" },
  every: { type: "string", default: "2000" }, reset: { type: "boolean", default: false },
} });
// GRAFT_TOOLS_EDGE points it at another install's test edge (tests/m2/results/install-m2b.md).
const EDGE = process.env.GRAFT_TOOLS_EDGE ?? "https://graft-m1-wp7-edge.<workers-subdomain>.workers.dev";
const REPO = process.env.GRAFT_REPO ?? new URL("../../..", import.meta.url).pathname;
const TOKEN = readFileSync(`${REPO}/tests/m0/.m0-token`, "utf8").trim();
const chains = opt.chains!.split(",");
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function member(chain: string, site: number, path: string): Promise<any> {
  try {
    const r = await fetch(`${EDGE}/member`, {
      method: "POST", headers: { authorization: `Bearer ${TOKEN}` },
      body: JSON.stringify({ chain, site, path, deadline: 5_000 }), signal: AbortSignal.timeout(10_000),
    });
    const v = await r.json() as any;
    return v.v ?? { error: `status ${v.status}` };
  } catch (e) {
    return { error: String(e).slice(0, 120) };
  }
}

async function sample(): Promise<unknown> {
  const at = Date.now();
  const groups = await Promise.all(chains.map(async (chain) => {
    const ms = await Promise.all([0, 1, 2].map((s) => member(chain, s, "/status")));
    return {
      chain,
      members: ms.map((m, s) => m.error ? { site: s, error: m.error } :
        { site: s, role: m.role, term: m.term, leader: m.leader, last: m.last, commit: m.commit, applied: m.applied, now: m.now }),
    };
  }));
  return { at, groups };
}

if (opt.stats) {
  const out: any[] = [];
  for (const chain of chains) {
    for (const s of [0, 1, 2]) {
      const v = await member(chain, s, `/stats${opt.reset ? "?reset=1" : ""}`);
      out.push({ chain, site: s, leader: v.role, commitMs: v.commitMs, roundMs: v.roundMs, failedCalls: v.failedCalls,
        alarms: v.alarms, entries: v.entries, peers: v.peers, error: v.error });
    }
  }
  appendFileSync(opt.stats, JSON.stringify({ at: Date.now(), stats: out }) + "\n");
  console.log(`stats for ${chains.join(",")} -> ${opt.stats}`);
  const iso = (ms: number) => new Date(ms).toISOString().slice(11, 23);
  for (const m of out) {
    for (const [to, p] of Object.entries<any>(m.peers ?? {})) {
      const failed = Object.entries(p.failed).map(([k, n]) => `${k} ${n}`).join(", ") || "none";
      const lateFailed = Object.entries(p.lateFailed).map(([k, n]) => `${k} ${n}`).join(", ");
      console.log(`${m.chain} ${m.site}>${to}: ok ${p.ok}, failed ${failed}, late ${p.late} (max ${p.lateMsMax} ms)` +
        (lateFailed ? `, late then failed ${lateFailed}` : ""));
      for (const e of p.errors) {
        console.log(`  ${iso(e.at)} ${e.kind}${e.late ? " (late)" : ""} ${e.call} ${e.entries} entries ${e.bytes} B ${e.ms} ms: ${e.msg}`);
      }
      const a = p.appends.at(-1);
      if (a) console.log(`  last append ${iso(a.at)} round ${a.round} ${a.first}..${a.last} ${a.bytes} B ${a.ms} ms ${a.end}`);
    }
  }
} else {
  if (!opt.out) throw new Error("--out FILE or --stats FILE");
  for (;;) {
    const t0 = Date.now();
    appendFileSync(opt.out, JSON.stringify(await sample()) + "\n");
    await sleep(Math.max(0, Number(opt.every) - (Date.now() - t0)));
  }
}
